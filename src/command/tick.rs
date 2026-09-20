use crate::command::reported_damage;
use crate::packets::combat as pcombat;
use crate::packets::item as pitem;
use crate::packets::movement::{move_player, Movement};
use crate::session::Session;
use crate::state::{BotState, Drop as DropItem, EntityKind};

/// Whether the teleport cooldown has elapsed (`hunt.teleport_delay` since
/// the last teleport). During the window, ticks keep running so drops within
/// range still get picked up.
fn can_teleport(state: &BotState) -> bool {
    state.last_teleport.elapsed()
        >= std::time::Duration::from_millis(state.cfg.hunt.teleport_delay)
}

/// Pickup radius (px): 0 = whole map.
fn pickup_radius(state: &BotState) -> i32 {
    let n = state.cfg.hunt.pickup_range;
    if n <= 0 {
        i32::MAX
    } else {
        n
    }
}

/// Max pickup distance squared per pickup_range (see pickup_radius).
fn pickup_far_max(state: &BotState) -> i32 {
    let r = pickup_radius(state);
    r.saturating_mul(r)
}

fn skill_mp_cost(state: &BotState) -> i16 {
    state.cfg.hunt.attack_skill_mp_cost
}

/// Pending attack packet. `skill` semantics:
/// - `-1` = multi basic attack: one basic attack packet per mob in `targets` (several in the same tick)
/// - `0` = skill-less group attack: one skill=0 packet for ANY target count
///   (single target included — close-attack has no hit segments and would
///   lose damage in single-mob fights)
/// - `>0` = skill group packet
struct PendingAttack {
    skill: i32,
    /// Hit segments per target (skill packet): always 1 for basic/skill-less group packets
    hits: u8,
    /// Damage per segment per target (each segment rolls independently within the damage range)
    targets: Vec<(i32, Vec<i32>)>,
}

/// Teleport directly to a target position (absolute move, one packet).
/// Doesn't sleep: the caller gates further teleports with `can_teleport`,
/// so ticks keep running (pickup etc.) during the cooldown window.
/// No x clamping — teleport is absolute movement and can land anywhere on the map.
///
/// Attack timing is fully gated by attack_cooldown (teleport itself is risk-free, no waiting).
async fn teleport_to(
    state: &mut BotState,
    session: &mut Session,
    tx: i16,
    ty: i16,
) -> Result<bool, String> {
    let (mx, my) = state.position;
    let ny = if ty != 0 {
        ty
    } else if state.ground_y != 0 {
        state.ground_y
    } else {
        my
    };
    if tx == mx && ny == my {
        return Ok(false);
    }
    let facing = if tx < mx { 1 } else { 0 };
    let m = Movement::absolute(tx, ny, 2 | facing);
    session.send_packet(move_player(&m)).await?;
    state.position = (tx, ny);
    state.last_sent_pos = (tx, ny);
    state.facing = facing;
    state.last_teleport = std::time::Instant::now();
    Ok(true)
}

/// Independent pickup: active whenever `hunt pickup on` is set, regardless of hunting.
/// Picks up drops within pickup_radius without moving; farther drops are teleported to by
/// do_hunt while hunting. Silently rejected drops (others' drops / unique items) retry after
/// RETRY_COOLDOWN and are abandoned once tries run out (a 0x111 remove packet is the success signal).
/// Range = circle centered on the player (dx²+dy² ≤ r²).
async fn tick_pickup(state: &mut BotState, session: &mut Session) -> Result<(), String> {
    if !state.cfg.hunt.pickup_enabled {
        return Ok(());
    }
    let (mx, my) = state.position;
    let r2 = pickup_radius(state).saturating_mul(pickup_radius(state));
    let near_oids: Vec<i32> = state
        .drops
        .values()
        .filter(|d| {
            let dx = (d.x - mx) as i32;
            let dy = (d.y - my) as i32;
            dx * dx + dy * dy <= r2
        })
        .map(|d| d.oid)
        .collect();
    for oid in near_oids {
        let d = match state.drops.get_mut(&oid) {
            Some(d) => d,
            None => continue,
        };
        if let Some(t) = d.last_try {
            if t.elapsed() < DropItem::RETRY_COOLDOWN {
                continue;
            }
        }
        d.tries += 1;
        d.last_try = Some(std::time::Instant::now());
        if d.tries >= DropItem::MAX_DROP_TRIES {
            let d = state.drops.remove(&oid).unwrap();
            crate::emit_f!([crate::emit::Field::Oid => d.oid, crate::emit::Field::ItemId => d.itemid] => "[drop] giving up on oid={} item={}", d.oid, d.itemid);
            continue;
        }
        let (x, y) = (d.x, d.y);
        session.send_packet(pitem::pickup(oid, x, y)).await?;
    }
    Ok(())
}

/// Unified hunt flow: find mob → teleport → filter by attack range → send packets by attack mode.
/// The attack mode (hunt attack / hunt skill) only decides which packets to send:
/// - attack: one basic attack packet per mob in range (up to attack_max_targets)
/// - skill + attack_skill=0: skill-less group attack (one skill=0 packet for any
///   target count — single target included, so configured hits always apply;
///   the server processes each as a basic attack, ignoring skill stats: no MP, no cooldown, any hits/damage)
/// - skill + attack_skill>0: one skill group packet (≤ attack_skill_max_targets); skipped when MP is low
/// Filter radius: skill mode uses attack_skill_range (0 = fall back to the attack range),
/// basic mode uses the attack range (hunt range).
/// Independent gathering (`gather on`, independent of the hunt switch): each tick pulls mobs outside
/// the attack range one step closer to the player. Step size gather_step (0 = pull straight to the
/// player's position); default 150px to avoid MOB_VAC detection (reduce_x > 200 || reduce_y > 150 flagged).
/// Stops naturally once a mob enters the attack range (no pulling inside the circle). Standalone
/// commands: `gather on|off` / `gather step <px>`.
async fn tick_gather(state: &mut BotState, session: &mut Session) -> Result<(), String> {
    if !state.gather {
        return Ok(());
    }
    // Round interval (gather interval <ms>, default 0 = every tick):
    // sustained high-frequency move packets get detected; an interval + per-round cap lower the signature.
    let interval = std::time::Duration::from_millis(state.cfg.hunt.gather_interval);
    if state.last_gather.elapsed() < interval {
        return Ok(());
    }
    let (mx, my) = state.position;
    let gather_step = state.cfg.hunt.gather_step;
    // Gather target = the player's current position (same center as the attack filter, so pulled mobs
    // can be hit). No longer falls back to ground height: a mismatched center offsets the gather and
    // hunt circles (a mob inside gather's circle sits outside hunt's), only 1 mob is pulled.
    let target_y = my;
    let atk_range = if state.cfg.hunt.attack_mode == "skill"
        && state.cfg.hunt.attack_skill_range > 0
    {
        state.cfg.hunt.attack_skill_range
    } else {
        state.cfg.hunt.attack_range
    };
    let atk2 = (atk_range as i32).saturating_mul(atk_range as i32);
    let mut gather_n = 0;
    let mut gather_targets: Vec<(i32, i16, i16)> = state
        .entities
        .values()
        // By default only pull mobs the server hands control over (0xF0 aggro=1): pulling without
        // control is flagged as abnormal by the server. `gather controller off` ignores the limit
        // and force-pulls everything (special cases, e.g. when the server never sends control).
        .filter(|e| {
            e.kind == EntityKind::Mob && (!state.gather_controller_only || e.controlled)
        })
        .filter_map(|e| {
            let dx = e.x as i32 - mx as i32;
            let dy = e.y as i32 - target_y as i32;
            if dx * dx + dy * dy > atk2 {
                Some((e.oid, e.x, e.y))
            } else {
                None
            }
        })
        .collect();
    // Per-round cap (gather max <n>, 0 = unlimited): pull in batches to avoid batch-move detection.
    let max = state.cfg.hunt.gather_max;
    if max > 0 {
        gather_targets.truncate(max);
    }
    for (oid, ex, ey) in gather_targets {
        let (tx, ty) = if gather_step <= 0 {
            (mx, target_y) // pull straight to the player's position
        } else {
            let dx = mx as i32 - ex as i32;
            let dy = target_y as i32 - ey as i32;
            let dist = ((dx * dx + dy * dy) as f64).sqrt().max(1.0);
            let step = (gather_step as f64).min(dist);
            (
                (ex as f64 + dx as f64 * step / dist).round() as i16,
                (ey as f64 + dy as f64 * step / dist).round() as i16,
            )
        };
        // moveid: increments independently per mob (continuous for the same mob),
        // avoiding a global counter going negative/wrapping in the u16→i16 cast and being rejected.
        let mid = state.gather_moveids.entry(oid).or_insert(0);
        *mid = mid.wrapping_add(1).max(1);
        session
            .send_packet(crate::packets::movement::move_monster(
                oid,
                *mid,
                ex,
                ey,
                tx,
                ty,
            ))
            .await?;
        // The server broadcast excludes the sender, so the bot never receives the 0xF1 — sync
        // coordinates locally, otherwise the attack filter never finds the mobs that were pulled in.
        if let Some(e) = state.entities.get_mut(&oid) {
            e.x = tx;
            e.y = ty;
        }
        gather_n += 1;
    }
    if gather_n > 0 {
        crate::emit_f!([crate::emit::Field::Count => gather_n] => "[gather] pulled {gather_n} mob(s) toward player");
    }    state.last_gather = std::time::Instant::now();
    Ok(())
}

/// Unified hunt flow: find mob → teleport → filter by attack range → send packets by attack mode.
async fn hunt_loop(
    state: &mut BotState,
    session: &mut Session,
    moved: &mut bool,
    attack: &mut Option<PendingAttack>,
) -> Result<(), String> {
    let (mx, my) = state.position;

    // Far-drop teleport target (teleport to drops outside the circle; picked up next tick as near drops).
    let mut far_drop: Option<(i32, i32)> = None; // (dist2, oid)
    if state.cfg.hunt.pickup_enabled {
        let r2 = pickup_radius(state).saturating_mul(pickup_radius(state));
        let far_max = pickup_far_max(state);
        for d in state.drops.values() {
            // Drops in retry cooldown are excluded from target selection (avoids pointless trips after silent rejections).
            if let Some(t) = d.last_try {
                if t.elapsed() < DropItem::RETRY_COOLDOWN {
                    continue;
                }
            }
            let dx = (d.x - mx) as i32;
            let dy = (d.y - my) as i32;
            let d2 = dx * dx + dy * dy;
            if d2 > r2 && d2 <= far_max && far_drop.map(|(f, _)| d2 < f).unwrap_or(true) {
                far_drop = Some((d2, d.oid));
            }
        }
    }

    // Mob pool: every mob on the map (teleport ignores distance and flies straight in).
    // By default only attack mobs the server hands control over (0xF0 aggro=1) — attacking without
    // control is flagged by detecting servers. `hunt.attack_controller_only=false` attacks all mobs
    // (for undetected servers, or when other players share control on the map).
    let mob_pool: Vec<(i32, i16, i16)> = state
        .entities
        .values()
        .filter(|e| {
            e.kind == EntityKind::Mob && (!state.cfg.hunt.attack_controller_only || e.controlled)
        })
        .map(|e| (e.oid, e.x, e.y))
        .collect();

    // Attack range (filter radius): mobs within this distance of the player get attacked; chase farther ones.
    // Skill mode uses the skill range (attack_skill_range, 0 = fall back to the attack range),
    // basic mode uses attack_range (default 70) — 300px out-of-range attacks trigger detection in practice.
    let range = if state.cfg.hunt.attack_mode == "skill"
        && state.cfg.hunt.attack_skill_range > 0
    {
        state.cfg.hunt.attack_skill_range
    } else {
        state.cfg.hunt.attack_range
    };
    let range2 = range.saturating_mul(range);

    let in_range: Vec<(i32, i16, i16)> = mob_pool
        .iter()
        .filter(|(_, ex, ey)| {
            let dx = *ex as i32 - mx as i32;
            let dy = *ey as i32 - my as i32;
            dx * dx + dy * dy <= range2
        })
        .copied()
        .collect();
    if in_range.is_empty() {
        if state.hunt_stand {
            // stand mode: stay put, don't chase — pair with gather to fight mobs pulled close.
            return Ok(());
        }
        // No mob in range: jump straight to the nearest mob on the map.
        // Movement has no distance limit — the server never validates player teleports (no check in source);
        // single goal: find a mob and teleport in front of it. Throttled only by
        // teleport_delay (can_teleport); with teleport_delay=0 it seeks every idle tick.
        let far = mob_pool
            .iter()
            .min_by_key(|(_, ex, ey)| {
                let dx = *ex as i32 - mx as i32;
                let dy = *ey as i32 - my as i32;
                dx * dx + dy * dy
            })
            .copied();
        if let Some((_, ex, ey)) = far {
            if can_teleport(state) {
                let dx = ex as i32 - mx as i32;
                let dy = ey as i32 - my as i32;
                let dist = ((dx * dx + dy * dy) as f64).sqrt().round() as i32;
                crate::emit_f!([crate::emit::Field::Pos => format!("({ex},{ey})")] =>
                    "[teleport] far seek -> ({ex},{ey}) dist={dist}px");
                if teleport_to(state, session, ex, ey).await? {
                    *moved = true;
                }
            }
            return Ok(());
        }
        // No mob to chase: teleport to the far drop and pick it up.
        if let Some((_, oid)) = far_drop {
            let d = state.drops.get(&oid).cloned().unwrap();
            if can_teleport(state) {
                if teleport_to(state, session, d.x, d.y).await? {
                    *moved = true;
                }
            }
            return Ok(());
        }
        // No mobs, no drops: return (rescan the mob pool next tick).
        return Ok(());
    }

    // Mobs in range: send packets per attack mode (the attack rhythm is gated by
    // attack_cooldown at the end of tick()).
    // Each segment rolls damage independently within the range (e.g. a 2-segment group attack
    // capped at 1200 = each segment 1080~1200; the segment count only adds damage values, no splitting).
    let hits: u8 = match state.cfg.hunt.attack_mode.as_str() {
        // Skill mode (incl. skill=0 group attack) uses the configured hits; basic attack is always 1
        "skill" => state.cfg.hunt.attack_skill_hits.clamp(1, 15),
        _ => 1,
    };
    let targets: Vec<(i32, Vec<i32>)> = in_range
        .iter()
        .map(|(oid, _, _)| {
            let dmgs: Vec<i32> = (0..hits).map(|_| reported_damage(state)).collect();
            (*oid, dmgs)
        })
        .collect();
    match state.cfg.hunt.attack_mode.as_str() {
        "skill" if state.cfg.hunt.attack_skill == 0 => {
            // skill=0: skill-less group attack (one packet, any target count)
            let n = state.cfg.hunt.attack_skill_max_targets.max(1).min(15);
            *attack = Some(PendingAttack {
                skill: 0,
                hits,
                targets: targets.into_iter().take(n).collect(),
            });
        }
        "skill" => {
            // Skill: do nothing when MP is low (no fallback to basic attack)
            if state.mp >= skill_mp_cost(state) {
                let n = state.cfg.hunt.attack_skill_max_targets.max(1).min(15);
                *attack = Some(PendingAttack {
                    skill: state.cfg.hunt.attack_skill,
                    hits,
                    targets: targets.into_iter().take(n).collect(),
                });
            }
        }
        _ => {
            // Basic attack: one packet per mob, up to attack_max_targets (skill=-1 sentinel)
            let n = state.cfg.hunt.attack_max_targets.clamp(1, 15);
            *attack = Some(PendingAttack {
                skill: -1,
                hits: 1,
                targets: targets.into_iter().take(n).collect(),
            });
        }
    }
    Ok(())
}

/// `hunt once` target selection (pure, testable): mobs within attack range,
/// controller filter applied, nearest-first, capped per attack mode
/// (skill mode → attack_skill_max_targets, basic → attack_max_targets).
fn hunt_once_targets(state: &BotState) -> Vec<(i32, i16, i16)> {
    let (mx, my) = state.position;
    let range = if state.cfg.hunt.attack_mode == "skill"
        && state.cfg.hunt.attack_skill_range > 0
    {
        state.cfg.hunt.attack_skill_range
    } else {
        state.cfg.hunt.attack_range
    };
    let range2 = range.saturating_mul(range);
    let mut in_range: Vec<(i32, i16, i16)> = state
        .entities
        .values()
        .filter(|e| {
            e.kind == EntityKind::Mob
                && (!state.cfg.hunt.attack_controller_only || e.controlled)
        })
        .filter(|e| {
            let dx = e.x as i32 - mx as i32;
            let dy = e.y as i32 - my as i32;
            dx * dx + dy * dy <= range2
        })
        .map(|e| (e.oid, e.x, e.y))
        .collect();
    // Nearest-first so the per-mode cap keeps the closest targets.
    in_range.sort_by_key(|(_, ex, ey)| {
        let dx = *ex as i64 - mx as i64;
        let dy = *ey as i64 - my as i64;
        dx * dx + dy * dy
    });
    let cap = match state.cfg.hunt.attack_mode.as_str() {
        "skill" => state.cfg.hunt.attack_skill_max_targets.clamp(1, 15),
        _ => state.cfg.hunt.attack_max_targets.clamp(1, 15),
    };
    in_range.truncate(cap);
    in_range
}

/// One manual attack round (`hunt once`): locate mobs within attack range and
/// fire a single attack volley — nothing else. Unlike the full hunt loop there
/// is no pickup / gather / far-drop logic, and unlike `hunt on` it runs even
/// while a task holds the lock (rules execute every tick regardless), enabling
/// rule-gated boss flows: `when: "mobs>0"` → `["hunt once"]` with cooldown 1.
///
/// Packet rhythm stays server-safe: every call is gated by attack_cooldown and
/// teleport_delay (after any move/seek), matching the loop's send gating.
pub async fn hunt_once(state: &mut BotState, session: &mut Session) -> Result<(), String> {
    let cd_ok = state.last_attack.elapsed()
        >= std::time::Duration::from_millis(state.cfg.hunt.attack_cooldown);
    if !cd_ok || !can_teleport(state) {
        return Ok(()); // silent: rule callers fire this every tick
    }
    let targets = hunt_once_targets(state);
    if targets.is_empty() {
        // Mobs exist but none in range → one far-seek teleport to the nearest
        // (same gates as the loop's seek); the next call attacks.
        let (mx, my) = state.position;
        let far: Option<(i16, i16)> = state
            .entities
            .values()
            .filter(|e| {
                e.kind == EntityKind::Mob
                    && (!state.cfg.hunt.attack_controller_only || e.controlled)
            })
            .min_by_key(|e| {
                let dx = e.x as i64 - mx as i64;
                let dy = e.y as i64 - my as i64;
                dx * dx + dy * dy
            })
            .map(|e| (e.x, e.y));
        if let Some((ex, ey)) = far {
            // Entry gate (can_teleport / teleport_delay) already passed above, so
            // a far-seek teleport is always allowed here when the target exists.
            teleport_to(state, session, ex, ey).await?;
        }
        return Ok(());
    }
    // Stand first when currently moving: attacking mid-move gets detected.
    if state.last_stance == 2 {
        let (sx, sy) = state.position;
        let m = Movement::stand(sx, sy, state.facing);
        session.send_packet(move_player(&m)).await?;
        state.last_stance = 4;
    }
    let mode = state.cfg.hunt.attack_mode.as_str();
    let skill_id = state.cfg.hunt.attack_skill;
    if mode == "skill" && skill_id > 0 && state.mp < skill_mp_cost(state) {
        return Ok(()); // low MP: skip, no fallback (same as the loop)
    }
    let hits: u8 = if mode == "skill" {
        state.cfg.hunt.attack_skill_hits.clamp(1, 15)
    } else {
        1
    };
    let (x, y) = state.position;
    let mut hit = 0usize;
    if mode != "skill" {
        // Basic attack: one close-attack packet per mob.
        for &(oid, _, _) in &targets {
            let dmg = reported_damage(state);
            session
                .send_packet(pcombat::close_attack(oid, x, y, dmg, state.facing))
                .await?;
            hit += 1;
        }
    } else {
        let volley: Vec<(i32, Vec<i32>)> = targets
            .iter()
            .map(|&(oid, _, _)| (oid, (0..hits).map(|_| reported_damage(state)).collect()))
            .collect();
        hit = volley.len();
        // Skill mode ALWAYS sends the skill packet (incl. skill=0 group
        // attack, single target included): the close-attack packet has no
        // hit segments, so a boss fight would silently lose hits/damage.
        session
            .send_packet(pcombat::skill_attack(skill_id, hits, &volley, x, y, state.facing))
            .await?;
    }
    crate::emit_f!([crate::emit::Field::Count => hit] =>
        "[attack] {} mob(s) hit", hit);
    state.last_attack = std::time::Instant::now();
    Ok(())
}

/// Auto-sell is fully config-driven now: the `sellauto` rule fires
/// `task start sell` when the inventory is full, and the sell task's
/// steps live in config.json `tasks`.

/// Reactor farming loop: hit every alive reactor and pick up every drop every
/// tick. No range checks anywhere — the server accepts hits from anywhere and
/// only warns (never blocks) on >800px pickups, so the bot can stay planted
/// in one spot while the whole map cycles.
async fn tick_reactor_hunt(state: &mut BotState, session: &mut Session) -> Result<(), String> {
    // Pick up every drop each tick, decoupled from attack timing (pickup packets are light; the server only logs notices).
    let drops: Vec<(i32, i16, i16)> = state
        .drops
        .values()
        .map(|d| (d.oid, d.x, d.y))
        .collect();
    let mut picked = 0usize;
    for (oid, x, y) in drops {
        session.send_packet(pitem::pickup(oid, x, y)).await?;
        picked += 1;
    }
    // Attack round gate: reactor_cooldown ms (0 = hit everything every tick).
    // No cooldown by default; raise it when the server is rate-sensitive (e.g. 1000 = one round per second).
    let cooldown = std::time::Duration::from_millis(state.cfg.hunt.reactor_cooldown);
    if state.last_rhunt_attack.elapsed() < cooldown {
        return Ok(());
    }
    state.last_rhunt_attack = std::time::Instant::now();
    let oids: Vec<i32> = state.reactors.keys().copied().collect();
    let mut hit = 0usize;
    for oid in &oids {
        session
            .send_packet(pcombat::damage_reactor(*oid, 0, 2, state.position.0))
            .await?;
        hit += 1;
    }
    // Summary log throttle: packets go out every tick; logging everything would spam, so at most one per second.
    // Format matches hunting: `[attack] N mob(s) hit [...]` → `[rhunt] N reactor(s) hit [...]`
    // (the CLI outputs English verbatim; the TUI translates to Chinese via zh.rs).
    if (hit > 0 || picked > 0) && state.last_rhunt_log.elapsed() >= std::time::Duration::from_secs(1)
    {
        state.last_rhunt_log = std::time::Instant::now();
        let detail: Vec<String> = oids
            .iter()
            .filter_map(|oid| state.reactors.get(oid).map(|r| format!("{}@{}", r.rid, r.oid)))
            .collect();
        crate::emit_f!([crate::emit::Field::Count => hit] => 
            "[rhunt] {} reactor(s) hit [{}]",
            hit,
            detail.join(", "));
        if picked > 0 {
            crate::emit_f!([crate::emit::Field::Count => picked] => "[rhunt] {picked} drop(s) picked");
        }
    }
    Ok(())
}

pub async fn tick(
    state: &mut BotState,
    session: &mut Session,
) -> Result<(), String> {
    let mut moved = false;
    let mut attack: Option<PendingAttack> = None;

    // Position sync: resend a move packet when position differs from the last one sent to the server.
    // Cases: [self] spawn overwrites local coords / after a discover wake — otherwise the server-side
    // character stays at the old spot (e.g. airborne (0,1)), gets no mob control, and gathering/attacking
    // all follow local coords while the server is out of sync (stuck in the air doing nothing).
    if state.discovery_sent && state.position != state.last_sent_pos {
        let (x, y) = state.position;
        if x != 0 || y != 0 {
            let m = Movement::stand(x, y, state.facing);
            session.send_packet(move_player(&m)).await?;
            state.last_sent_pos = state.position;
            state.last_stance = 4;
        }
    }

    // Auto-sell is config-driven (rule `sellauto` → task `sell`); nothing to do here.

    // Rule engine (side-channel, every tick): potion rules + configured rules
    // (only enabled rules run; see rules_to_fire).
    crate::command::rules::run_rules(state, session).await?;

    // Task engine: a running task owns the tick until it completes.
    if crate::command::tasks::tick_tasks(state, session).await? {
        return Ok(());
    }

    // Hunt stop condition (config `hunt.until`): predicate met → stop hunting.
    if state.hunt {
        if let Some(until) = state.cfg.hunt.until.clone() {
            if crate::runtime_config::eval_predicate(state, &until) {
                state.hunt = false;
                state.hunt_target = None;
                crate::emit_f!([crate::emit::Field::Until => until] => "[hunt] until '{until}' met — stopped");
            }
        }
    }

    if !state.discovery_sent {
        let (x, y) = state.position;
        // Wait for SPAWN_PLAYER to give a valid position before moving; but some servers (e.g. variant
        // servers) never send their own SPAWN_PLAYER after entering — the position stays 0,0, and the
        // server also waits for the client's first move packet before pushing map data (mobs/NPCs/join
        // notices), so if the position is still unknown 5 seconds after entering, send one move packet
        // to "wake" the server.
        let pos_known = x != 0 || y != 0;
        let wake = !pos_known && state.map_enter_time.elapsed() > std::time::Duration::from_secs(5);

        // If the map has an "sp" (spawn point) portal, teleport there directly
        // to correct coordinates. Only when position is known (SPAWN_PLAYER arrived).
        let sp_portal = if pos_known {
            session.names.find_portal(state.mapid, "sp")
        } else {
            None
        };

        if let Some(sp) = sp_portal {
            let sp_x = sp.x as i16;
            let sp_y = sp.y as i16;
            let facing = if sp_x < x { 1 } else { 0 };
            let m = Movement::absolute(sp_x, sp_y, 2 | facing);
            session.send_packet(move_player(&m)).await?;
            state.position = (sp_x, sp_y);
            state.last_sent_pos = (sp_x, sp_y);
            state.facing = facing;
            state.ground_y = sp_y;
            state.discovery_sent = true;
            state.last_stance = 2;
            crate::emit_f!([crate::emit::Field::Pos => format!("({sp_x},{sp_y})"), crate::emit::Field::Portal => "sp"] =>
                "[discover] tp -> sp ({sp_x},{sp_y})");
        } else if pos_known || wake {
            let target_y = if y != 0 { y } else { state.ground_y };
            let m = Movement::absolute(x, target_y.max(1), 2);
            session.send_packet(move_player(&m)).await?;
            state.position = (x, target_y);
            state.last_sent_pos = (x, target_y);
            state.discovery_sent = true;
            state.last_stance = 2;
            crate::emit_f!([crate::emit::Field::Pos => format!("({x},{target_y})")] => "[discover] move -> ({x},{target_y}) to reveal map players");
        }
    }

    // Periodic client report packet (0x15 STRANGE_DATA): ~3-4 times/sec,
    // payload `01 + random + 0`. 0x15 is CS_USE (cash shop) on standard servers and gets ignored, harmless.
    // The server may rely on it to confirm client liveness/controller state; missing it is a missed step.
    if state.cfg.hunt.strange_report {
        let last = state.last_strange;
        if last.elapsed() >= std::time::Duration::from_millis(300) {
            session
                .send_packet(crate::packets::movement::strange_report())
                .await?;
            state.last_strange = std::time::Instant::now();
        }
    }

    // Independent gathering (gather on, independent of the hunt switch)
    tick_gather(state, session).await?;
    // Hunting: unified flow (find mob → teleport → filter by attack range → send packets by attack mode);
    // reactor mode is separate (hunt reactor).
    if state.hunt {
        hunt_loop(state, session, &mut moved, &mut attack).await?;
    } else if state.hunt_reactor {
        tick_reactor_hunt(state, session).await?;
    }
    // Independent pickup: pick up nearby drops whether hunting is on or not
    tick_pickup(state, session).await?;

    if let Some(atk) = attack {
        let attack_cooldown = std::time::Duration::from_millis(state.cfg.hunt.attack_cooldown);
        // Wait teleport_delay after moving before attacking: real clients stop first, then hit; attacking right after moving is a script signature.
        if state.last_attack.elapsed() >= attack_cooldown
            && state.last_teleport.elapsed()
                >= std::time::Duration::from_millis(state.cfg.hunt.teleport_delay)
        {
            // Ensure standing (stance=4) before attacking: attacking right after moving (stance=2) gets detected; clients stop first
            if state.last_stance == 2 {
                let (x, y) = state.position;
                let m = Movement::stand(x, y, state.facing);
                session.send_packet(move_player(&m)).await?;
                state.last_stance = 4;
            }
            let (x, y) = state.position;
            if atk.skill == -1 {
                // Multi basic attack: one basic attack packet per mob (several in the same tick)
                for &(oid, ref dmgs) in &atk.targets {
                    session
                        .send_packet(pcombat::close_attack(oid, x, y, dmgs[0], state.facing))
                        .await?;
                }
            } else {
                // Skill mode ALWAYS sends the skill packet (incl. skill=0
                // group attack with a single target): close-attack has no hit
                // segments, so single-mob fights would silently lose damage.
                session
                    .send_packet(pcombat::skill_attack(atk.skill, atk.hits, &atk.targets, x, y, state.facing)).await?;
            }
            // Unified attack log: one line per attack, count only; each mob's remaining HP is echoed
            // by the server 0xFC ([mobhp]). EXP is logged via [exp]. Verbose:
            // floods the TUI at 600ms attack cadence, but headless still prints it.
            crate::emit_f!([crate::emit::Field::Count => atk.targets.len()] => 
                "[attack] {} mob(s) hit",
                atk.targets.len());
            state.last_attack = std::time::Instant::now();
        }
    }

    if moved {
        state.last_stance = 2;
    } else if state.last_stance == 2
        && state.discovery_sent
        && state.last_attack.elapsed() >= std::time::Duration::from_millis(500)
    {
        let (x, y) = state.position;
        let m = Movement::stand(x, y, state.facing);
        session.send_packet(move_player(&m)).await?;
        state.last_stance = 4;
        crate::emit_f!([crate::emit::Field::Pos => format!("({x},{y})")] => "[stand] -> ({x},{y}) facing={}", state.facing);
    }

    Ok(())
}





#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Entity;

    fn mob(oid: i32, x: i16, y: i16, controlled: bool) -> Entity {
        Entity {
            oid,
            kind: EntityKind::Mob,
            mobid: 8800000,
            x,
            y,
            controlled,
            ..Default::default()
        }
    }

    #[test]
    fn targets_respect_range_controller_and_cap() {
        let mut s = BotState::default();
        s.cfg.hunt.attack_mode = "attack".into();
        s.cfg.hunt.attack_controller_only = true;
        s.cfg.hunt.attack_range = 300;
        s.cfg.hunt.attack_max_targets = 2;
        // in range: 100px & 200px; out of range: 400px
        s.entities.insert(1, mob(1, 100, 0, true));
        s.entities.insert(2, mob(2, 200, 0, true));
        s.entities.insert(3, mob(3, 400, 0, true));
        // nearest-first + capped at max_targets=2 -> oids 1,2 (not 3)
        let picked: Vec<i32> = hunt_once_targets(&s).iter().map(|t| t.0).collect();
        assert_eq!(picked, vec![1, 2]);
        // controller_only on -> uncontrolled mobs excluded entirely
        s.entities.clear();
        s.entities.insert(5, mob(5, 10, 0, false));
        assert!(hunt_once_targets(&s).is_empty());
        s.cfg.hunt.attack_controller_only = false;
        assert_eq!(hunt_once_targets(&s).len(), 1);
    }

    #[test]
    fn targets_skill_mode_uses_skill_range_and_cap() {
        let mut s = BotState::default();
        s.cfg.hunt.attack_mode = "skill".into();
        s.cfg.hunt.attack_controller_only = true;
        s.cfg.hunt.attack_skill = 0;
        s.cfg.hunt.attack_range = 70;
        s.cfg.hunt.attack_skill_range = 320;
        s.cfg.hunt.attack_skill_max_targets = 15;
        // inside the 320px skill range but outside the 70px basic range
        s.entities.insert(9, mob(9, 300, 0, true));
        assert_eq!(hunt_once_targets(&s).len(), 1);
        // cap to 1 target keeps the NEAREST (oid 8 at 100px < oid 9 at 300px)
        s.entities.insert(8, mob(8, 100, 0, true));
        s.cfg.hunt.attack_skill_max_targets = 1;
        assert_eq!(hunt_once_targets(&s)[0].0, 8);
    }

    #[test]
    fn targets_empty_without_mobs() {
        let s = BotState::default();
        assert!(hunt_once_targets(&s).is_empty());
    }
}
