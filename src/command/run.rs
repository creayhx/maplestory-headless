use crate::command::{
    ApCmd, Command, HuntFilterCmd, HuntMode, LoginCmd, PartyCmd, ReactorCmd, TaskCmd, TradeCmd,
    ap_stat_name, ap_stat_value, equip_slot, equip_slot_name, reported_damage,
};
use crate::command::view;
    use crate::runtime_config::{GroupDef, Rule};

/// Reconnect count display: 0 = infinite.
fn reconnect_max_str(n: u32) -> String {
    if n == 0 {
        "inf".to_string()
    } else {
        n.to_string()
    }
}
use crate::packets::chat as pchat;
use crate::packets::cashshop as pcash;
use crate::packets::combat as pcombat;
use crate::packets::item as pitem;
use crate::packets::login as plogin;
use crate::packets::map as pmap;
use crate::packets::movement::{move_player, Movement};
use crate::packets::npc as pnpc;
use crate::packets::party as pparty;
use crate::packets::stats as pstats;
use crate::packets::trade as ptrade;
use crate::session::Session;
use crate::state::{BotState, Phase};

/// rule status output lines (shared by local stdout and the #rule status whisper reply).
/// Lists only top-level rules (numbered, with when/then/cooldown on separate lines);
/// group rules belong to group status and don't appear here (same for rule open/close).
pub fn rule_status_lines(state: &BotState, filter: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let rules: Vec<&Rule> = state
        .cfg
        .rules
        .iter()
        .filter(|r| filter.map_or(true, |f| r.id == f))
        .collect();
    if rules.is_empty() {
        out.push("[rule] no top-level rules defined (config.json \"rules\")".to_string());
        return out;
    }
    for (i, r) in rules.iter().enumerate() {
        out.push(format!(
            "[rule] {}. {} ({})",
            i + 1,
            r.id,
            if r.enabled { "on" } else { "off" }
        ));
        out.push(format!("       when: {}", r.when));
        out.push(format!("       then: {}", r.then.join(" / ")));
        out.push(format!("       cooldown: {}ms", r.cooldown));
    }
    out
}

/// group status output lines (shared by local stdout and the #group status whisper reply).
/// One block per group: switch state/exclusivity + full details of its rules and tasks (when/then/steps).
pub fn group_status_lines(state: &BotState, filter: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let groups: Vec<&GroupDef> = state
        .cfg
        .groups
        .iter()
        .filter(|g| filter.map_or(true, |f| g.id == f))
        .collect();
    if groups.is_empty() {
        out.push("[group] no groups defined (config.json \"groups\")".to_string());
        return out;
    }
    for (i, g) in groups.iter().enumerate() {
        let mark = if g.enabled { "on" } else { "off" };
        let excl = if g.exclusive { ", exclusive" } else { "" };
        out.push(format!(
            "[group] {}. {} ({}{}) rules={} tasks={}",
            i + 1,
            g.id,
            mark,
            excl,
            g.rules.len(),
            g.tasks.len()
        ));
        for (ri, r) in g.rules.iter().enumerate() {
            out.push(format!("       rule {}: {}", ri + 1, r.id));
            out.push(format!("         when: {}", r.when));
            out.push(format!("         then: {}", r.then.join(" / ")));
            if r.cooldown > 0 {
                out.push(format!("         cooldown: {}ms", r.cooldown));
            }
        }
        for (ti, t) in g.tasks.iter().enumerate() {
            out.push(format!(
                "       task {}: {} priority={} steps={}",
                ti + 1,
                t.id,
                t.priority,
                t.steps.len()
            ));
            for (si, st) in t.steps.iter().enumerate() {
                let wait = st
                    .wait
                    .as_deref()
                    .map(|w| format!(" (wait: {w})"))
                    .unwrap_or_default();
                out.push(format!("         step {}: {}{}", si + 1, st.cmd, wait));
            }
        }
    }
    out
}

/// task status output lines (shared by local stdout and the #task status whisper reply).
/// Active task stack first (-> marks the stack-top lock holder), then the top-level task
/// definitions (numbered, with step details); group tasks belong to group status, not here.
pub fn task_status_lines(state: &BotState) -> Vec<String> {
    let mut out = Vec::new();
    if state.task_stack.is_empty() {
        out.push("[task] no active task".to_string());
    } else {
        for (i, rt) in state.task_stack.iter().enumerate().rev() {
            let marker = if i + 1 == state.task_stack.len() { "->" } else { "  " };
            out.push(format!(
                "[task] {marker} '{}' step {}/{}",
                rt.def_id,
                rt.step_idx + 1,
                rt.steps.len()
            ));
        }
    }
    if state.cfg.tasks.is_empty() {
        out.push("[task] no top-level tasks defined (config.json \"tasks\")".to_string());
        return out;
    }
    for (i, t) in state.cfg.tasks.iter().enumerate() {
        out.push(format!(
            "[task] {}. {} priority={} steps={}",
            i + 1,
            t.id,
            t.priority,
            t.steps.len()
        ));
        for (si, st) in t.steps.iter().enumerate() {
            let wait = st
                .wait
                .as_deref()
                .map(|w| format!(" (wait: {w})"))
                .unwrap_or_default();
            out.push(format!("       step {}: {}{}", si + 1, st.cmd, wait));
        }
    }
    out
}

/// Guard for commands that require an active field session.
fn in_game(state: &BotState) -> bool {
    state.phase == Phase::InGame
}

/// Ammo-type items: arrows 206xxxx / throwing stars 207xxxx / bullets 233xxxx.
fn is_ammo_item(itemid: i32) -> bool {
    matches!(itemid / 10000, 206 | 207 | 233)
}

/// At startup (after the runtime loads the config) validate the attack mode ("attack"/"skill";
/// later manual `hunt attack|skill` choices persist permanently —
/// no map change, task round-trip, or reenter resets the attack mode.
pub(crate) fn apply_default_attack_mode(state: &mut BotState) {
    if !matches!(state.cfg.hunt.attack_mode.as_str(), "attack" | "skill") {
        state.cfg.hunt.attack_mode = "attack".into();
    }
}

/// Open a top-level rule. Group rules return Err with the owning group (they follow the group switch).
/// When a task step controls a top-level rule via `rule open <id>`, an unknown id fails the step —
/// that is expected behavior (config errors should surface).
pub fn rule_open(state: &mut BotState, id: &str) -> Result<(), String> {
    let Some(rule) = state.cfg.rules.iter_mut().find(|r| r.id == id) else {
        if let Some(g) = state.cfg.groups.iter().find(|g| g.rules.iter().any(|r| r.id == id)) {
            return Err(format!(
                "rule '{id}' belongs to group '{gid}' — group rules follow the group switch, use `group open {gid}` (or `rule status` to see group rules)",
                gid = g.id
            ));
        }
        return Err(format!("no rule '{id}'"));
    };
    rule.enabled = true;
    crate::emit_f!([crate::emit::Field::Name => id] => "[rule] open {id}");
    Ok(())
}

/// Close a top-level rule. Group rules return Err with the owning group.
pub fn rule_close(state: &mut BotState, id: &str) -> Result<(), String> {
    let Some(rule) = state.cfg.rules.iter_mut().find(|r| r.id == id) else {
        if let Some(g) = state.cfg.groups.iter().find(|g| g.rules.iter().any(|r| r.id == id)) {
            return Err(format!(
                "rule '{id}' belongs to group '{gid}' — group rules follow the group switch, use `group close {gid}`",
                gid = g.id
            ));
        }
        return Err(format!("no rule '{id}'"));
    };
    rule.enabled = false;
    crate::emit_f!([crate::emit::Field::Name => id] => "[rule] close {id}");
    // A top-level rule that inlined a `wait`/`while`/`task run` has a runtime on
    // the stack with group:None (no owning group). Stop those so closing the rule
    // actually halts its loop — group rules are handled by `group stop` instead.
    let mut removed: Vec<crate::runtime_config::TaskRuntime> = Vec::new();
    let mut i = 0;
    while i < state.task_stack.len() {
        if state.task_stack[i].group.is_none() {
            removed.push(state.task_stack.remove(i));
        } else {
            i += 1;
        }
    }
    if !removed.is_empty() {
        for rt in &removed {
            crate::emit_f!([crate::emit::Field::Name => rt.def_id] => "[task] '{}' stopped (rule '{id}' closed)", rt.def_id);
        }
        if state.task_stack.is_empty() {
            state.hunt = removed.first().map(|rt| rt.hunt_before).unwrap_or(state.hunt);
        }
    }
    Ok(())
}

/// Open a feature group (master switch). Opening an exclusive group auto-closes other open
/// exclusive groups and stops their running tasks (lock-stack cleanup, see
/// `tasks::stop_tasks_in_group`).

pub fn group_open(state: &mut BotState, id: &str) -> Result<(), String> {
    let Some(idx) = state.cfg.groups.iter().position(|g| g.id == id) else {
        return Err(format!("no group '{id}'"));
    };
    // Exclusive: opening an exclusive group auto-closes other open exclusive groups (and stops their group tasks).
    if state.cfg.groups[idx].exclusive {
        for i in 0..state.cfg.groups.len() {
            if i != idx && state.cfg.groups[i].exclusive && state.cfg.groups[i].enabled {
                let closed_id = state.cfg.groups[i].id.clone();
                state.cfg.groups[i].enabled = false;
                crate::emit_f!([crate::emit::Field::Name => closed_id.as_str()] => "[group] exclusive: closed '{closed_id}'");
                crate::command::tasks::stop_tasks_in_group(state, &closed_id);
            }
        }
    }
    state.cfg.groups[idx].enabled = true;
    crate::emit_f!([crate::emit::Field::Name => id] => "[group] open {id}");
    Ok(())
}

/// Close a feature group (master switch) and stop its running tasks.
pub fn group_close(state: &mut BotState, id: &str) -> Result<(), String> {
    let Some(group) = state.cfg.group_mut(id) else {
        return Err(format!("no group '{id}'"));
    };
    group.enabled = false;
    crate::emit_f!([crate::emit::Field::Name => id] => "[group] close {id}");
    crate::command::tasks::stop_tasks_in_group(state, id);
    Ok(())
}

/// Learned level of a skill (0/None when not learned). The server validates
/// the reported level against its own copy, so callers must use this.
fn skill_level(state: &BotState, skillid: i32) -> Option<u8> {
    state.skills.get(&skillid).map(|s| s.level)
}

/// Put an item from the inventory into the open trade. `qty <= 0` puts the
/// full stack. Returns Err only on send failure; prints diagnostics itself.
pub(crate) async fn send_trade_put(
    state: &mut BotState,
    session: &mut Session,
    itemid: i32,
    qty: i16,
) -> Result<(), String> {
    // the inventory key's first byte IS the MapleInventoryType (1=EQUIP..5=CASH)
    let found = state
        .inventory
        .iter()
        .find(|((_, _), it)| it.itemid == itemid)
        .map(|((tab, slot), it)| (*tab, *slot, it.qty));
    match found {
        Some((tab, slot, have)) => {
            let n = if qty <= 0 { have } else { qty.min(have) };
            if n <= 0 {
                crate::emit_f!([crate::emit::Field::ItemId => itemid] => "[trade] nothing to put for {itemid}");
            } else {
                session.send_packet(ptrade::put_item(tab, slot, n, 0xFF)).await?;
                crate::emit_f!([crate::emit::Field::ItemId => itemid, crate::emit::Field::Qty => n, crate::emit::Field::Slot => slot] => 
                    "[trade] put {itemid} x{n} (tab={tab} slot={slot}) — waiting for 'trade confirm'");
            }
        }
        None => crate::emit_f!([crate::emit::Field::ItemId => itemid] => "[trade] item {itemid} not found in inventory"),
    }
    Ok(())
}

/// Called after sending an NPC dialog advance command: optimistically closes the dialog. The
/// next NPC_TALK(0x145) from the server sets `dialog` back to true — so `wait: "dialog==1"`
/// means waiting for the next dialog content (not "kept open forever").
pub fn npc_dialog_advance(state: &mut BotState) {
    state.dialog_open = false;
}

/// `hunt filter` executor: mutate the pickup filter mode/lists, prune drops
/// that the new filter rejects (meso always kept), and return the new state
/// line. Exposed for tests (no session needed).
pub fn run_hunt_filter(state: &mut BotState, cmd: &HuntFilterCmd) -> Result<String, String> {
    let h = &mut state.cfg.hunt;
    let dedup = |v: &mut Vec<i32>| {
        let mut seen = std::collections::HashSet::new();
        v.retain(|id| seen.insert(*id));
    };
    let changed = !matches!(cmd, HuntFilterCmd::Status);
    match cmd {
        HuntFilterCmd::Status => {}
        HuntFilterCmd::Off => {
            h.pickup_filter_mode = "off".into();
        }
        HuntFilterCmd::SetAllow(ids) => {
            h.pickup_filter_mode = "allow".into();
            if !ids.is_empty() {
                h.pickup_allow = ids.clone();
                dedup(&mut h.pickup_allow);
            }
        }
        HuntFilterCmd::SetDeny(ids) => {
            h.pickup_filter_mode = "deny".into();
            if !ids.is_empty() {
                h.pickup_deny = ids.clone();
                dedup(&mut h.pickup_deny);
            }
        }
        HuntFilterCmd::Add(ids) => match h.pickup_filter_mode.as_str() {
            "allow" => {
                h.pickup_allow.extend(ids.iter().copied());
                dedup(&mut h.pickup_allow);
            }
            "deny" => {
                h.pickup_deny.extend(ids.iter().copied());
                dedup(&mut h.pickup_deny);
            }
            _ => {
                return Err(
                    "hunt filter: no mode active — start with 'hunt filter allow <ids>' or 'hunt filter deny <ids>'"
                        .into(),
                )
            }
        },
        HuntFilterCmd::Del(ids) => match h.pickup_filter_mode.as_str() {
            "allow" => {
                h.pickup_allow.retain(|id| !ids.contains(id));
            }
            "deny" => {
                h.pickup_deny.retain(|id| !ids.contains(id));
            }
            _ => {
                return Err("hunt filter: no mode active — nothing to remove".into())
            }
        },
    }
    // Re-evaluate drops already on the ground against the (possibly changed)
    // filter; meso always survives. A pure status query skips the prune.
    if changed {
        state.prune_filtered_drops();
    }
    let allow: Vec<String> = state.cfg.hunt.pickup_allow.iter().map(i32::to_string).collect();
    let deny: Vec<String> = state.cfg.hunt.pickup_deny.iter().map(i32::to_string).collect();
    let mode = crate::runtime_config::pickup_filter_mode(&state.cfg.hunt);
    Ok(format!(
        "[hunt] filter={mode} allow=[{}] deny=[{}]",
        allow.join(","),
        deny.join(",")
    ))
}

pub async fn run(
    cmd: Command,
    state: &mut BotState,
    session: &mut Session,
) -> Result<bool, String> {
    match cmd {
        Command::Move(x, requested_y) => {
            if !in_game(state) {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "not in game yet");
                return Ok(false);
            }
            let y = if requested_y != 0 {
                requested_y
            } else if state.ground_y != 0 {
                state.ground_y
            } else {
                crate::emit!("no ground reference yet (waiting for mobs)");
                return Ok(false);
            };
            let m = Movement::absolute(x, y, 2);
            session.send_packet(move_player(&m)).await?;
            state.position = (x, y);
            crate::emit_f!([crate::emit::Field::Pos => format!("({x},{y})"), crate::emit::Field::Ground => state.ground_y] => "move -> ({x}, {y}) [ground={}]", state.ground_y);
        }
        Command::MovePortal(portal_name) => {
            if !in_game(state) {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "not in game yet");
                return Ok(false);
            }
            match session.names.find_portal(state.mapid, &portal_name) {
                Some(portal) => {
                    let x = portal.x as i16;
                    let y = portal.y as i16;
                    let m = Movement::absolute(x, y, 2);
                    session.send_packet(move_player(&m)).await?;
                    state.position = (x, y);
                    crate::emit_f!([crate::emit::Field::Pos => format!("({x},{y})"), crate::emit::Field::Portal => portal_name] => "move -> portal \"{portal_name}\" ({x}, {y})");
                }
                None => {
                    crate::emit_f!([crate::emit::Field::Portal => portal_name] => "[move] portal \"{portal_name}\" not found on map {}", state.mapid);
                }
            }
        }
        Command::MoveWarp(portal_name) => {
            if !in_game(state) {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "not in game yet");
                return Ok(false);
            }
            match session.names.find_portal(state.mapid, &portal_name) {
                Some(portal) => {
                    let x = portal.x as i16;
                    let y = portal.y as i16;
                    // Step 1: move to portal position
                    let m = Movement::absolute(x, y, 2);
                    session.send_packet(move_player(&m)).await?;
                    state.position = (x, y);
                    crate::emit_f!([crate::emit::Field::Pos => format!("({x},{y})")] => "[movewarp] move -> ({x}, {y})");
                    // Step 2: warp through portal
                    state.hunt = false;
                    state.hunt_target = None;
                    session.send_packet(pmap::change_map_special(&portal_name, x, y)).await?;
                    state.phase = Phase::EnteringMap;
                    state.map_enter_time = std::time::Instant::now();
                    crate::emit_f!([crate::emit::Field::Portal => portal_name] => "[movewarp] entering portal \"{portal_name}\"");
                }
                None => {
                    crate::emit_f!([crate::emit::Field::Portal => portal_name] => "[movewarp] portal \"{portal_name}\" not found on map {}", state.mapid);
                }
            }
        }
        Command::Chat(text) => {
            session
                .send_packet(pchat::general_chat(&text, false)).await?;
            crate::emit_f!([crate::emit::Field::Text => text] => "chat: {text}");
        }
        Command::View(view) => view::run_view(view, state, session).await?,
        Command::PickupAll => {
            let drops: Vec<(i32, i16, i16)> = state
                .drops
                .values()
                .map(|d| (d.oid, d.x, d.y))
                .collect();
            for (oid, x, y) in &drops {
                session.send_packet(pitem::pickup(*oid, *x, *y)).await?;
            }
            crate::emit_f!([crate::emit::Field::Count => drops.len()] => "[pickup] sent {} pickup(s)", drops.len());
        }
        Command::Login(lc) => match lc {
            LoginCmd::World(idx, chan) => {
                if state.phase != Phase::WorldSelect {
                    return Err(format!(
                        "world pick only valid in WorldSelect phase (now {:?})",
                        state.phase
                    ));
                }
                let world = state
                    .worlds
                    .get(idx)
                    .ok_or_else(|| format!("world index {idx} out of range"))?;
                let wid = world.wid;
                state.selected_world = wid;
                // user-facing channel is 1-based; the wire byte is 0-based (chan - 1)
                state.selected_channel = chan as i32;
                state.channel = chan as i32;
                session.send_packet(plogin::server_status_request()).await?;
                session.send_packet(plogin::charlist_request(wid as u8, chan - 1)).await?;
                state.phase = Phase::CharSelect;
                crate::emit_f!([crate::emit::Field::World => wid, crate::emit::Field::Channel => chan] => "[login] selecting world={wid} channel={chan}");
            }
            LoginCmd::Char(idx) => {
                if state.phase != Phase::CharSelect {
                    return Err(format!(
                        "char pick only valid in CharSelect phase (now {:?})",
                        state.phase
                    ));
                }
                // The server may repeat CHARLIST; dedup by id so indexes stay stable.
                let mut unique: Vec<crate::state::CharEntry> = Vec::new();
                for c in &state.characters {
                    if !unique.iter().any(|u| u.id == c.id) {
                        unique.push(c.clone());
                    }
                }
                let chosen = unique
                    .get(idx)
                    .ok_or_else(|| format!("char index {idx} out of range"))?;
                state.my_cid = chosen.id;
                state.mapid = chosen.stats.mapid;
                state.level = chosen.stats.level;
                state.exp = chosen.stats.exp;
                state.ap = chosen.stats.ap;
                state.hp = chosen.stats.hp;
                state.maxhp = chosen.stats.maxhp;
                state.mp = chosen.stats.mp;
                state.maxmp = chosen.stats.maxmp;
                state.str = chosen.stats.str;
                state.dex = chosen.stats.dex;
                state.int = chosen.stats.int;
                state.luk = chosen.stats.luk;
                state.phase = Phase::EnteringMap;
                crate::emit_f!([crate::emit::Field::Cid => chosen.id, crate::emit::Field::Name => chosen.stats.name, crate::emit::Field::MapId => chosen.stats.mapid] => 
                    "[login] selecting char cid={} ({}) map={}",
                    chosen.id,
                    chosen.stats.name,
                    chosen.stats.mapid);
                session.send_packet(plogin::select_char(chosen.id)).await?;
            }
        },
        Command::Attack(oid) => {
            let (x, y) = state.position;
            let dmg = reported_damage(state);
            if let Some(e) = state.entities.get(&oid) {
                state.facing = if e.x < x { 1 } else { 0 };
            }
            session
                .send_packet(pcombat::close_attack(oid, x, y, dmg, state.facing)).await?;
            crate::emit_f!([crate::emit::Field::Oid => oid, crate::emit::Field::Damage => dmg] => "[attack] oid={oid} dmg={dmg}");
        }
        Command::Hunt(on) => {
            state.hunt = on;
            if on {
                // Restarting hunt: mark the hunt clock fresh so post-(re)start mobs
                // aren't treated as stale (last_kill only updates on actual kills).
                state.last_kill = std::time::Instant::now();
                crate::emit_f!([crate::emit::Field::Toggle => if on { "on" } else { "off" }] => "[hunt] on - autonomous mob hunting + loot");
            } else {
                state.hunt_target = None;
                crate::emit_f!([crate::emit::Field::Toggle => if on { "on" } else { "off" }] => "[hunt] off");
            }
        }
        Command::HuntRange(n) => {
            // Attack range (send attack packets only for mobs within this distance)
            state.cfg.hunt.attack_range = n;
            crate::emit_f!([crate::emit::Field::Range => n] => "[hunt] attack range = {n}px");
        }
        Command::HuntDamage(n) => {            // 0 = internal formula (level^2); >0 = fixed damage (min 20).
            // Sole source is cfg.damage: 0 = auto formula, non-zero = fixed value.
            state.cfg.hunt.damage = n;
            if n == 0 {
                crate::emit_f!([crate::emit::Field::Damage => 0] => "[hunt] damage = auto (lvl^2)");
            } else {
                crate::emit_f!([crate::emit::Field::Damage => n] => "[hunt] damage = {n}");
            }
        }
        Command::HuntHits(n) => {
            state.cfg.hunt.attack_skill_hits = n;
            crate::emit_f!([crate::emit::Field::Text => format!("{n}")] => "[hunt] skill hits = {n} (per target)");
        }
        Command::HuntAtkCd(ms) => {
            // Attack cooldown: minimum interval between attack packets (tick.rs gates on it).
            state.cfg.hunt.attack_cooldown = ms;
            crate::emit_f!([crate::emit::Field::Duration => ms] => "[hunt] atkcd = {ms}ms ({})", if ms == 0 { "every tick" } else { "per attack" });
        }
        Command::Gather(on) => {
            state.gather = on;
            if !on {
                // Clear the moveid table when disabled (restart counting from scratch;
                // no rollback for the same mob after restart)
                state.gather_moveids.clear();
            }
            crate::emit_f!([crate::emit::Field::Toggle => if on { "on" } else { "off" }] => "[gather] = {}", if on { "on" } else { "off" });
        }
        Command::GatherStep(n) => {
            state.cfg.hunt.gather_step = n;
            crate::emit_f!([crate::emit::Field::Range => n] => "[gather] step = {n}px");
        }
        Command::GatherInterval(ms) => {
            state.cfg.hunt.gather_interval = ms;
            crate::emit_f!([crate::emit::Field::Duration => ms] => "[gather] interval = {ms}ms");
        }
        Command::GatherMax(n) => {
            state.cfg.hunt.gather_max = n;
            crate::emit_f!([crate::emit::Field::Count => n] => "[gather] max = {n} mobs per round");
        }
        Command::GatherController(on) => {
            state.gather_controller_only = on;
            crate::emit_f!([crate::emit::Field::Toggle => if on { "on" } else { "off" }] => "[gather] controller-only = {}", if on { "on" } else { "off" });
        }
        Command::HuntStand(on) => {
            state.hunt_stand = on;
            crate::emit_f!([crate::emit::Field::Toggle => if on { "on" } else { "off" }] => "[hunt] stand = {}", if on { "on" } else { "off" });
        }
        Command::HuntController(on) => {
            state.cfg.hunt.attack_controller_only = on;
            crate::emit_f!([crate::emit::Field::Toggle => if on { "on" } else { "off" }] => "[hunt] controller-only = {}", if on { "on" } else { "off" });
        }
        Command::HuntStatus => {
            // Hunt config overview (multi-line). The CLI outputs English; the TUI translates
            // each line to Chinese via zh_body. Field names are fixed (status= / mode= / skill= /
            // range= / teleport_delay= /
            // attack_max= / skill_range= / max_targets= / mp_cost= /
            // damage= / until=).
            let h = &state.cfg.hunt;
            let mode = if state.hunt_reactor {
                "reactor".to_string()
            } else {
                h.attack_mode.clone()
            };
            let skill = if h.attack_skill > 0 {
                format!("{}", h.attack_skill)
            } else {
                "0".to_string()
            };
            let pickup = if h.pickup_enabled {
                format!("{} enabled", h.pickup_range)
            } else {
                "disabled".to_string()
            };            let damage = match state.cfg.hunt.damage {
                d if d > 0 => format!("override({d})"),
                _ => "auto(lvl^2/2)".to_string(),
            };
            let until = h.until.clone().unwrap_or_else(|| "none".to_string());
            let run = if state.hunt { "on" } else { "off" };
            crate::emit_f!([crate::emit::Field::Toggle => run] => "[hunt] status={run} mode={mode} skill={skill}");
            crate::emit_f!([crate::emit::Field::Text => pickup.as_str()] => "[hunt] pickup={pickup} cooldown={}ms", h.attack_cooldown);
            crate::emit_f!([crate::emit::Field::Range => h.attack_range] => "[hunt] range={} teleport_delay={}ms", h.attack_range, h.teleport_delay);
            crate::emit_f!([crate::emit::Field::SkillId => h.attack_skill] => "[hunt] attack_max={} skill_range={} max_targets={} mp_cost={} hits={}", h.attack_max_targets, h.attack_skill_range, h.attack_skill_max_targets, h.attack_skill_mp_cost, h.attack_skill_hits);
            let filter = crate::runtime_config::pickup_filter_mode(&state.cfg.hunt);
            let items: Vec<String> = (if filter == "allow" {
                &h.pickup_allow
            } else {
                &h.pickup_deny
            })
            .iter()
            .map(i32::to_string)
            .collect();
            crate::emit_f!([crate::emit::Field::Text => filter] =>
                "[hunt] pickup_filter={filter} items=[{}]", items.join(","));
            crate::emit_f!([crate::emit::Field::Damage => state.cfg.hunt.damage] => "[hunt] damage={damage} until={until}");
        }
        Command::HuntFilter(cmd) => {
            let line = run_hunt_filter(state, &cmd)?;
            crate::emit_f!([crate::emit::Field::Text => line.as_str()] => "{line}");
        }
        Command::HuntOnce => {
            crate::command::tick::hunt_once(state, session).await?;
        }
        Command::SetVar(name, value) => {
            state.vars.insert(name.clone(), value.clone());
            crate::emit_f!([crate::emit::Field::Text => name.as_str()] =>
                "[var] set {name}={value}");
        }
        Command::ClearVar(name) => {
            if state.vars.remove(name.as_str()).is_some() {
                crate::emit_f!([crate::emit::Field::Text => name.as_str()] =>
                    "[var] cleared {name}");
            } else {
                crate::emit_f!([crate::emit::Field::Text => name.as_str()] =>
                    "[var] {name} not set");
            }
        }
        Command::HuntSave => {
            // Persist only the hunt section into THIS session's active config
            // file (profiles/<x>.json or config.json) so the current parameters
            // survive a restart (no reload: other runtime toggles like
            // rule open/close must stay as-is).
            //
            // Session-scoped on purpose: with N accounts in one process, a
            // process-global write target would save account A's hunt settings
            // into account B's profile.
            let target_path = session.write_path();
            match state.cfg.save_hunt_to(&target_path) {
                Ok(()) => {
                    let target = target_path.display().to_string();
                    crate::emit_f!([crate::emit::Field::Text => target] =>
                        "[hunt] saved to {target} (reused on next launch)")
                }
                Err(e) => crate::emit_f!([crate::emit::Field::Text => e] => "[hunt] save failed: {e}"),
            }
        }
        Command::HuntMode(mode) => match mode {
            HuntMode::Attack(n) => {
                state.cfg.hunt.attack_mode = "attack".into();
                if let Some(n) = n {
                    state.cfg.hunt.attack_max_targets = n.max(1).min(15);
                }
                crate::emit_f!([crate::emit::Field::AttackMode => "attack"] => 
                    "[hunt] attack mode: normal (one packet per mob, max {} packets)",
                    state.cfg.hunt.attack_max_targets);
            }
            HuntMode::Skill(id, n, range, mp) => {
                state.cfg.hunt.attack_mode = "skill".into();
                if let Some(id) = id {
                    state.cfg.hunt.attack_skill = id;
                }
                if let Some(n) = n {
                    state.cfg.hunt.attack_skill_max_targets = n;
                }
                if let Some(r) = range {
                    state.cfg.hunt.attack_skill_range = r;
                }
                if let Some(m) = mp {
                    state.cfg.hunt.attack_skill_mp_cost = m;
                }
                if state.cfg.hunt.attack_skill == 0 {
                    crate::emit_f!([crate::emit::Field::AttackMode => "skill", crate::emit::Field::SkillId => 0, crate::emit::Field::Targets => state.cfg.hunt.attack_skill_max_targets] => 
                        "[hunt] attack mode: skill 0 (no-skill group attack, max {} targets)",
                        state.cfg.hunt.attack_skill_max_targets);
                } else {
                    crate::emit_f!([crate::emit::Field::AttackMode => "skill", crate::emit::Field::SkillId => state.cfg.hunt.attack_skill, crate::emit::Field::Range => state.cfg.hunt.attack_skill_range, crate::emit::Field::Targets => state.cfg.hunt.attack_skill_max_targets, crate::emit::Field::Mp => state.cfg.hunt.attack_skill_mp_cost] => 
                        "[hunt] attack mode: skill {} (range {}px, max {} targets, mp {})",
                        state.cfg.hunt.attack_skill,
                        state.cfg.hunt.attack_skill_range,
                        state.cfg.hunt.attack_skill_max_targets,
                        state.cfg.hunt.attack_skill_mp_cost);
                }
            }
        },
        Command::HuntPickup(on) => {
            state.cfg.hunt.pickup_enabled = on;
            crate::emit_f!([crate::emit::Field::Toggle => if on { "on" } else { "off" }] => "[hunt] pickup = {}", if on { "on" } else { "off" });
        }
        Command::HuntPickupRange(range) => {
            state.cfg.hunt.pickup_range = range;
            crate::emit_f!([crate::emit::Field::Range => range] => "[hunt] pickup range = {range}px");
        }
        Command::Task(cmd) => match cmd {
            TaskCmd::Start(id) => {
                if let Err(e) = crate::command::tasks::start_task(state, &id) {
                    crate::emit_f!([crate::emit::Field::Text => e] => "[task] {e}");
                }
            }
            TaskCmd::Stop => {
                crate::command::tasks::stop_task(state);
            }
            TaskCmd::Status => {
                // Merge the whole block into one event: continuation lines inherit the first line's
                // [task] category (keeps TUI filters from scattering)
                crate::emit!("{}", task_status_lines(state).join("\n"));
            }
        },
        Command::Reload => match state.cfg.reload_from(&session.config_paths()) {
            Ok(()) => {
                // Pickup filter may have changed: drop items already on the
                // ground that the new rules reject (meso always survives).
                let filter = crate::runtime_config::pickup_filter_mode(&state.cfg.hunt);
                state.prune_filtered_drops();
                let target = session.write_path().display().to_string();
                crate::emit_f!([crate::emit::Field::Text => target.as_str()] =>
                    "[config] reloaded {target} (tasks/rules/potion updated; drops pruned, filter={filter})");
            }
            Err(e) => crate::emit_f!([crate::emit::Field::Text => e.as_str()] => "[config] reload failed: {e}"),
        },
        Command::Rule(rcmd) => match rcmd {
            crate::command::RuleCmd::Open(id) => rule_open(state, &id)?,
            crate::command::RuleCmd::Close(id) => rule_close(state, &id)?,
            crate::command::RuleCmd::Status(filter) => {
                // Merge the whole block into one event: continuation lines inherit the first line's [rule] category
                crate::emit!("{}", rule_status_lines(state, filter.as_deref()).join("\n"));
            }
        },
Command::Group(gcmd) => match gcmd {
            crate::command::GroupCmd::Open(id) => group_open(state, &id)?,
            crate::command::GroupCmd::Close(id) => group_close(state, &id)?,
            crate::command::GroupCmd::Status(filter) => {
                for l in group_status_lines(state, filter.as_deref()) {
                    crate::emit!("{l}");
                }
            }
        },
        Command::Reconnect(cmd) => match cmd {
            crate::command::ReconnectCmd::On => {
                state.cfg.reconnect = true;
                crate::emit!("[reconnect] on (every {}s, max {})", state.cfg.reconnect_delay, reconnect_max_str(state.cfg.reconnect_max));
            }
            crate::command::ReconnectCmd::Off => {
                state.cfg.reconnect = false;
                crate::emit!("[reconnect] off");
            }
            crate::command::ReconnectCmd::Delay(secs) => {
                state.cfg.reconnect_delay = secs;
                crate::emit!("[reconnect] delay = {secs}s");
            }
            crate::command::ReconnectCmd::Max(n) => {
                state.cfg.reconnect_max = n as u32;
                crate::emit!("[reconnect] max = {}", reconnect_max_str(n as u32));
            }
            crate::command::ReconnectCmd::Status => {
                crate::emit!(
                    "[reconnect] {} (every {}s, max {})",
                    if state.cfg.reconnect { "on" } else { "off" },
                    state.cfg.reconnect_delay,
                    reconnect_max_str(state.cfg.reconnect_max)
                );
            }
        },
        Command::Use(itemid) => {
            let found = state
                .inventory
                .iter()
                .find(|((tab, _), it)| *tab == 2 && it.itemid == itemid)
                .map(|((_, slot), it)| (*slot, it.qty));
            match found {
                Some((slot, qty)) => {
                    session
                        .send_packet(pitem::use_item(slot, itemid)).await?;
                    crate::emit_f!([crate::emit::Field::ItemId => itemid, crate::emit::Field::Slot => slot, crate::emit::Field::Qty => qty] => "[use] {itemid} slot={slot} qty={qty}");
                }
                None => crate::emit_f!([crate::emit::Field::ItemId => itemid] => "[use] item {itemid} not in USE tab"),
            }
        }
        Command::UseReward(itemid) => {
            let found = state
                .inventory
                .iter()
                .find(|((tab, _), it)| *tab == 2 && it.itemid == itemid)
                .map(|((_, slot), it)| (*slot, it.qty));
            match found {
                Some((slot, qty)) => {
                    session
                        .send_packet(pitem::use_reward_item(slot, itemid)).await?;
                    crate::emit_f!([crate::emit::Field::ItemId => itemid, crate::emit::Field::Slot => slot, crate::emit::Field::Qty => qty] => "[reward] {itemid} slot={slot} qty={qty}");
                }
                None => crate::emit_f!([crate::emit::Field::ItemId => itemid] => "[reward] item {itemid} not in USE tab"),
            }
        }
        Command::Drop(itemid, qty, slot) => {
            // When a slot is given, only drop from that slot (bag UI D key): equips/consumables/others all droppable.
            if let Some(s) = slot {
                let found = state
                    .inventory
                    .iter()
                    .find(|((_, sl), it)| *sl == s && it.itemid == itemid)
                    .map(|((tab, sl), it)| (*tab, *sl, it.qty));
                match found {
                    Some((tab, sl, owned)) => {
                        let qty = qty.clamp(1, owned.max(1));
                        session.send_packet(pitem::drop_item(tab, sl, qty)).await?;
                        crate::emit_f!([crate::emit::Field::ItemId => itemid, crate::emit::Field::Slot => sl, crate::emit::Field::Qty => qty] => "[drop] {itemid} tab={tab} slot={sl} qty={qty}");
                    }
                    None => crate::emit_f!([crate::emit::Field::ItemId => itemid, crate::emit::Field::Slot => s] => "[drop] slot {s} has no {itemid}"),
                }
                return Ok(false);
            }
            let found = state
                .inventory
                .iter()
                .find(|((_, _), it)| it.itemid == itemid)
                .map(|((tab, slot), it)| (*tab, *slot, it.qty));
            match found {
                Some((tab, slot, owned)) => {
                    let qty = qty.clamp(1, owned.max(1));
                    session
                        .send_packet(pitem::drop_item(tab, slot, qty)).await?;
                    crate::emit_f!([crate::emit::Field::ItemId => itemid, crate::emit::Field::Slot => slot, crate::emit::Field::Qty => qty] => "[drop] {itemid} tab={tab} slot={slot} qty={qty}");
                }
                None => crate::emit_f!([crate::emit::Field::ItemId => itemid] => "[drop] item {itemid} not in inventory"),
            }
        }
        Command::DropMeso(amount) => {
            let amount = amount.clamp(10, 50000);
            session
                .send_packet(pitem::drop_meso(amount)).await?;
            crate::emit_f!([crate::emit::Field::Meso => amount] => "[dropmeso] {amount}");
        }
        Command::UseScroll(scroll_id, equip_id, bless) => {
            // Scroll slot: itemid match in the USE tab (tab=2)
            let slot = state
                .inventory
                .iter()
                .find(|((tab, _), it)| *tab == 2 && it.itemid == scroll_id)
                .map(|((_, s), _)| *s);
            let Some(slot) = slot else {
                crate::emit_f!([crate::emit::Field::ItemId => scroll_id] => "[scroll] scroll {scroll_id} not in USE tab");
                return Ok(false);
            };
            // Target equip: prefer worn gear (equipped, dst<0), else the backpack equip tab (dst>0)
            let dst = state
                .equipped
                .iter()
                .find(|(_, it)| it.itemid == equip_id)
                .map(|(s, _)| *s)
                .or_else(|| {
                    state
                        .inventory
                        .iter()
                        .find(|((tab, _), it)| *tab == 1 && it.itemid == equip_id)
                        .map(|((_, s), _)| *s)
                });
            let Some(dst) = dst else {
                crate::emit_f!([crate::emit::Field::ItemId => equip_id] => "[scroll] equip {equip_id} not worn nor in backpack (equip first)");
                return Ok(false);
            };
            // ws: bless = 2 (blessed scroll protection, server deducts a white scroll), default = 1
            let ws: i16 = if bless { 2 } else { 1 };
            session
                .send_packet(pitem::use_upgrade_scroll(slot, dst, ws))
                .await?;
            crate::emit_f!([crate::emit::Field::ItemId => scroll_id, crate::emit::Field::Slot => slot, crate::emit::Field::Text => format!("dst={dst} ws={ws}")] => 
                "[scroll] {scroll_id} slot={slot} -> equip {equip_id} dst={dst} ws={ws} sent");
        }
        Command::Party(pcmd) => {
            match pcmd {
                PartyCmd::Create => {
                    // Client sends 78 00 01 to create; partyid is assigned by the server reply (3B 00 08)
                    session.send_packet(pparty::create()).await?;
                    crate::emit!("[party] create sent (waiting for partyid)");
                }
                PartyCmd::Invite(name) => {
                    // Client sends 78 00 04 + name (GBK, no partyid)
                    session.send_packet(pparty::invite(&name)).await?;
                    crate::emit_f!([crate::emit::Field::Name => name] => "[party] invite '{name}' sent");
                }
                PartyCmd::InviteCid(cid) => {
                    // Resolve the player's name on this map by cid, then invite by name (the protocol only supports name invites)
                    let name = state
                        .entities
                        .get(&cid)
                        .filter(|e| e.kind == crate::state::EntityKind::Player)
                        .map(|e| e.charname.clone());
                    match name {
                        Some(name) => {
                            session.send_packet(pparty::invite(&name)).await?;
                            crate::emit_f!([crate::emit::Field::Name => name, crate::emit::Field::Cid => cid] => "[party] invite cid={cid} -> '{name}' sent");
                        }
                        None => {
                            crate::emit_f!([crate::emit::Field::Cid => cid] => "[party] cid {cid} not on this map");
                        }
                    }
                }
                PartyCmd::Leave => {
                    session.send_packet(pparty::leave()).await?;
                    state.party = None;
                    crate::emit!("[party] leave sent");
                }
                PartyCmd::Kick(name) => {
                    // Find cid by name: party member table first, then map players
                    let cid = state
                        .party
                        .as_ref()
                        .and_then(|p| {
                            p.members
                                .values()
                                .find(|m| m.name == name)
                                .map(|m| m.id)
                        })
                        .or_else(|| {
                            state
                                .entities
                                .iter()
                                .find(|(_, e)| {
                                    e.kind == crate::state::EntityKind::Player
                                        && e.charname == name
                                })
                                .map(|(cid, _)| *cid)
                        });
                    match cid {
                        Some(cid) => {
                            // Client sends 78 00 05 + cid (captured from real traffic, party2.txt)
                            session.send_packet(pparty::expel(cid)).await?;
                            crate::emit_f!([crate::emit::Field::Name => name, crate::emit::Field::Cid => cid] => "[party] kick '{name}' (cid={cid}) sent");
                        }
                        None => {
                            crate::emit_f!([crate::emit::Field::Name => name] => "[party] '{name}' not in party members nor on this map");
                        }
                    }
                }
                PartyCmd::KickCid(cid) => {
                    // Send the packet directly (78 00 05 + cid)
                    session.send_packet(pparty::expel(cid)).await?;
                    crate::emit_f!([crate::emit::Field::Cid => cid] => "[party] kick cid={cid} sent");
                }
            }
        }
        Command::Ap(apcmd) => match apcmd {
            ApCmd::Add(stat, n) => {
                if !in_game(state) {
                    crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "not in game yet");
                    return Ok(false);
                }
                let v = ap_stat_value(&stat);
                let name = ap_stat_name(v);
                let mut sent = 0;
                for _ in 0..n {
                    if state.ap <= 0 {
                        crate::emit_f!([crate::emit::Field::Ap => state.ap, crate::emit::Field::Count => sent, crate::emit::Field::Name => name] => "[ap] only {} AP left, spent {sent} on {name}", state.ap);
                        break;
                    }
                    session.send_packet(pstats::distribute_ap(v)).await?;
                    state.ap -= 1;
                    sent += 1;
                }
                state.last_ap = std::time::Instant::now();
                crate::emit_f!([crate::emit::Field::Count => sent, crate::emit::Field::Name => name, crate::emit::Field::Ap => state.ap] => "[ap] +{sent} {name} ({} AP left)", state.ap);
            }
            ApCmd::Status => {
                crate::emit_f!([crate::emit::Field::Ap => state.ap] => 
                    "[ap status] available={} (use `ap <str|dex|int|luk|maxhp|maxmp> [n]`)",
                    state.ap);
            }
        },
        Command::Equip(ecmd) => {
            use crate::command::EquipCmd;
            match ecmd {
                EquipCmd::Equip(itemid) => {
                    let slot = match equip_slot(itemid) {
                        Some(s) => s,
                        None => {
                            crate::emit_f!([crate::emit::Field::ItemId => itemid] => "[equip] item {itemid} has no supported body slot");
                            return Ok(false);
                        }
                    };
                    let found = state
                        .inventory
                        .iter()
                        .find(|((tab, _), it)| *tab == 1 && it.itemid == itemid)
                        .map(|((_, s), it)| (*s, it.qty));
                    match found {
                        Some((src, _)) => {
                            // rings may land on -12..-15: pick the first free slot
                            let dst = if slot == -12 {
                                (-15..=-12)
                                    .find(|s| !state.equipped.contains_key(s))
                                    .unwrap_or(-12)
                            } else {
                                slot
                            };
                            if state.equipped.contains_key(&dst) {
                                crate::emit_f!([crate::emit::Field::ItemId => itemid, crate::emit::Field::Slot => dst] => 
                                    "[equip] {itemid}: {} occupied (already worn)",
                                    equip_slot_name(dst));
                                return Ok(false);
                            }
                            session.send_packet(pitem::move_item(src, dst)).await?;
                            crate::emit_f!([crate::emit::Field::ItemId => itemid, crate::emit::Field::Slot => src] => 
                                "[equip] {itemid} slot={src} -> {} ({dst})",
                                equip_slot_name(dst));
                        }
                        None => crate::emit_f!([crate::emit::Field::ItemId => itemid] => "[equip] item {itemid} not in EQUIP tab"),
                    }
                }
                EquipCmd::Unequip(itemid) => {
                    let found = state
                        .equipped
                        .iter()
                        .find(|(_, it)| it.itemid == itemid)
                        .map(|(slot, _)| *slot);
                    match found {
                        Some(src) => {
                            let free = (1..=96)
                                .find(|s| !state.inventory.contains_key(&(1, *s)))
                                .unwrap_or(1);
                            session.send_packet(pitem::move_item(src, free)).await?;
                            crate::emit_f!([crate::emit::Field::ItemId => itemid, crate::emit::Field::Slot => free] => 
                                "[equip] unequip {itemid} from {} ({src}) -> slot {free}",
                                equip_slot_name(src));
                        }
                        None => crate::emit_f!([crate::emit::Field::ItemId => itemid] => "[equip] item {itemid} not worn"),
                    }
                }
            }
        }
        Command::Skill(scmd) => {
            use crate::command::SkillCmd;
            use crate::packets::stats as pstats;
            match scmd {
                SkillCmd::Learn(skillid) => {
                    if !in_game(state) {
                        crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "not in game yet");
                        return Ok(false);
                    }
                    if state.sp <= 0 {
                        crate::emit_f!([crate::emit::Field::Sp => state.sp] => "[sp] no skill points left");
                        return Ok(false);
                    }
                    if let Some(sk) = state.skills.get(&skillid) {
                        crate::emit_f!([crate::emit::Field::SkillId => skillid, crate::emit::Field::Level => sk.level] => 
                            "[sp] {skillid} currently L{} (sending +1)",
                            sk.level);
                    } else {
                        crate::emit_f!([crate::emit::Field::SkillId => skillid] => "[sp] {skillid} not learned yet (sending +1)");
                    }
                    session.send_packet(pstats::distribute_sp(skillid)).await?;
                    crate::emit_f!([crate::emit::Field::SkillId => skillid] => "[sp] +1 sent for {skillid}");
                }
                SkillCmd::Cast(skillid, oid) => {
                    if !in_game(state) {
                        crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "not in game yet");
                        return Ok(false);
                    }
                    let Some(level) = skill_level(state, skillid) else {
                        crate::emit_f!([crate::emit::Field::SkillId => skillid] => "[skill] {skillid} not learned");
                        return Ok(false);
                    };
                    let (x, y) = state.position;
                    let dmg = reported_damage(state);
                    if let Some(e) = state.entities.get(&oid) {
                        state.facing = if e.x < x { 1 } else { 0 };
                    }
                    session
                        .send_packet(pcombat::skill_attack(skillid, 1, &[(oid, vec![dmg])], x, y, state.facing))
                        .await?;
                    crate::emit_f!([crate::emit::Field::SkillId => skillid, crate::emit::Field::Level => level, crate::emit::Field::Oid => oid, crate::emit::Field::Damage => dmg] => "[skill] cast {skillid} L{level} -> oid={oid} dmg={dmg}");
                }
                SkillCmd::Info(skillid) => {
                    match state.skills.get(&skillid) {
                        Some(sk) => crate::emit_f!([crate::emit::Field::SkillId => skillid, crate::emit::Field::Level => sk.level] => 
                            "[skill] {skillid} L{}{}",
                            sk.level,
                            if sk.masterlevel > 0 {
                                format!("/{}", sk.masterlevel)
                            } else {
                                String::new()
                            }),
                        None => crate::emit_f!([crate::emit::Field::SkillId => skillid] => "[skill] {skillid} not learned"),
                    }
                }
            }
        }
        Command::ChangeMap(_mapid, portal) => {
            if !in_game(state) {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "not in game yet");
                return Ok(false);
            }
            state.hunt = false;
            state.hunt_target = None;
            session.send_packet(pmap::change_map(&portal, state.position.0, state.position.1)).await?;
            state.phase = Phase::EnteringMap;
            // Fresh EnteringMap timeout window (same race as reconnect: the stale
            // map_enter_time would fire the 10s watchdog right after the warp).
            state.map_enter_time = std::time::Instant::now();
            crate::emit_f!([crate::emit::Field::Portal => portal] => "[town] entering portal \"{portal}\"");
        }
        Command::ChangeMapSpecial(portal) => {
            if !in_game(state) {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "not in game yet");
                return Ok(false);
            }
            match session.names.find_portal(state.mapid, &portal) {
                Some(_portal) => {
                    state.hunt = false;
                    state.hunt_target = None;
                    session.send_packet(pmap::change_map_special(&portal, state.position.0, state.position.1)).await?;
                    state.phase = Phase::EnteringMap;
                    state.map_enter_time = std::time::Instant::now();
                    crate::emit_f!([crate::emit::Field::Portal => portal] => "[warp] entering portal \"{portal}\"");
                }
                None => {
                    crate::emit_f!([crate::emit::Field::Portal => portal] => "[warp] portal \"{portal}\" not found on map {}", state.mapid);
                }
            }
        }
        Command::PortalList(map_id) => {
            let mid = map_id.unwrap_or(state.mapid);
            let portals = session.names.map_portals(mid);
            if portals.is_empty() {
                crate::emit_f!([crate::emit::Field::MapId => mid] => "[view] no portals for map {mid}");
            } else {
                crate::emit_f!([crate::emit::Field::Count => portals.len()] => "[view] map {mid} — {} portal(s):", portals.len());
                for (i, p) in portals.iter().enumerate() {
                    let target_name = session.names.map_name_text(p.target_map);
                    crate::emit!("[view] {:>3}  {:<10} → {:<7}{}", i + 1, p.name, p.target_map, target_name);
                }
            }
        }
        Command::NpcTalk(id) => {
            if !in_game(state) {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "not in game yet");
                return Ok(false);
            }
            // Match by oid first; fall back to npcid (config tasks may write a fixed
            // npcid like `npc 1011100`; the oid is resolved from the local NPC table).
            let oid = match state.npcs.get(&id) {
                Some(_) => id,
                None => match state.npcs.values().find(|n| n.npcid == id) {
                    Some(n) => n.oid,
                    None => {
                        crate::emit_f!([crate::emit::Field::NpcId => id] => "[npc] id {id} not on this map (neither oid nor npcid)");
                        // Return Err: an NPC missing from the map in a task step = step failure -> clean
                        // task abort (no stray sell/spend packets). Manual calls just print one extra error line.
                        return Err(format!("npc {id} not on this map"));
                    }
                },
            };
            session.send_packet(pnpc::npc_talk(oid)).await?;
            // Optimistically set false: the next NPC_TALK(0x145) from the server sets dialog
            // back to true — so wait: "dialog==1" means waiting for the next dialog content.
            npc_dialog_advance(state);
            crate::emit_f!([crate::emit::Field::Oid => oid, crate::emit::Field::NpcId => id] => "[npc] talk oid={oid} (id={id})");
        }
        Command::NpcReply(selection) => {
            session.send_packet(pnpc::npc_talk_more(4, selection)).await?;
            npc_dialog_advance(state);
            crate::emit_f!([crate::emit::Field::Selection => selection] => "[npc] reply selection={selection}");
        }
        Command::NpcNext => {
            // For type 0 (sendSay) and type 1 (sendNextPrev), the server
            // expects lastMsg matching the type. Use 0 as a safe default.
            let last = if state.last_npc_msg_type == 1 { 1 } else { 0 };
            session.send_packet(pnpc::npc_talk_ack(last)).await?;
            npc_dialog_advance(state);
            crate::emit!("[npc] next (last_msg={last})");
        }
        Command::NpcPrev => {
            let last = state.last_npc_msg_type;
            session.send_packet(pnpc::npc_talk_prev(last)).await?;
            npc_dialog_advance(state);
            crate::emit!("[npc] prev (last_msg={last})");
        }
        Command::NpcCancel => {
            session.send_packet(pnpc::npc_talk_cancel()).await?;
            npc_dialog_advance(state);
            crate::emit!("[npc] cancel dialog");
        }
        Command::NpcYesNo(yes) => {
            let last = state.last_npc_msg_type;
            if yes {
                session.send_packet(pnpc::npc_talk_more(last, 1)).await?;
                crate::emit_f!([crate::emit::Field::Toggle => if yes { "on" } else { "off" }] => "[npc] yes (last_msg={last})");
            } else {
                if last == 1 {
                    session.send_packet(pnpc::npc_talk_prev(last)).await?;
                } else {
                    session.send_packet(pnpc::npc_talk_more(last, 0)).await?;
                }
                crate::emit_f!([crate::emit::Field::Toggle => if yes { "on" } else { "off" }] => "[npc] no (last_msg={last})");
            }
            npc_dialog_advance(state);
        }
        Command::NpcNumber(value) => {
            let last = state.last_npc_msg_type;
            session.send_packet(pnpc::npc_talk_more(last, value)).await?;
            npc_dialog_advance(state);
            crate::emit_f!([crate::emit::Field::Value => value] => "[npc] number {value} (last_msg={last})");
        }
        Command::NpcText(text) => {
            session.send_packet(pnpc::npc_talk_text(&text)).await?;
            npc_dialog_advance(state);
            crate::emit_f!([crate::emit::Field::Text => text] => "[npc] text \"{text}\"");
        }
        Command::NpcBuy(itemid, qty) => {
            session.send_packet(pnpc::npc_shop_buy(itemid, qty)).await?;
            crate::emit_f!([crate::emit::Field::ItemId => itemid, crate::emit::Field::Qty => qty] => "[shop] buy {itemid} x{qty}");
        }
        Command::NpcSell(itemid, qty, slot) => {
            // Cumulative selling across slots: qty<=0 or >= total stock = sell all slots of that itemid;
            // otherwise fill qty slot by slot (one packet per slot). The total uses i32 to avoid
            // overflow (rechargeable ammo: 68 slots x 800 = 54400 > i16 max 32767).
            // When a slot is given, sell only that slot (bag UI "sell current"): qty<=0 = sell it all.
            if let Some(s) = slot {
                let found = state
                    .inventory
                    .iter()
                    .find(|((_, sl), it)| *sl == s && it.itemid == itemid);
                match found {
                    Some((_, it)) => {
                        let take = if qty <= 0 { it.qty.max(1) } else { qty.min(it.qty) };
                        session.send_packet(pnpc::npc_shop_sell(s, itemid, take)).await?;
                        crate::emit_f!([crate::emit::Field::ItemId => itemid, crate::emit::Field::Slot => s, crate::emit::Field::Qty => take] => "[shop] sell {itemid} slot={s} qty={take}");
                    }
                    None => {
                        crate::emit_f!([crate::emit::Field::ItemId => itemid, crate::emit::Field::Slot => s] => "[shop] slot {s} has no {itemid}");
                    }
                }
                return Ok(false);
            }
            let matches: Vec<(i16, i32, i16)> = state
                .inventory
                .iter()
                .filter(|(_, it)| it.itemid == itemid && it.qty > 0)
                .map(|((_, slot), it)| (*slot, it.itemid, it.qty))
                .collect();
            if matches.is_empty() {
                crate::emit_f!([crate::emit::Field::ItemId => itemid] => "[shop] item {itemid} not in inventory");
                return Ok(false);
            }
            let total: i32 = matches.iter().map(|(_, _, q)| *q as i32).sum();
            let want = if qty <= 0 { total } else { (qty as i32).min(total) };
            let mut remaining = want;
            for (slot, id, q) in matches {
                if remaining <= 0 {
                    break;
                }
                let take = remaining.min(q as i32) as i16;
                session.send_packet(pnpc::npc_shop_sell(slot, id, take)).await?;
                crate::emit_f!([crate::emit::Field::ItemId => itemid, crate::emit::Field::Slot => slot, crate::emit::Field::Qty => take] => "[shop] sell {itemid} slot={slot} qty={take}");
                remaining -= take as i32;
            }
        }
        Command::SellAmmo => {
            // Sell all ammo items (USE tab): arrows 206xxxx / throwing stars 207xxxx / bullets 233xxxx
            let items: Vec<(i16, i32, i16)> = state
                .inventory
                .iter()
                .filter(|((tab, _), it)| *tab == 2 && is_ammo_item(it.itemid))
                .map(|((_, slot), it)| (*slot, it.itemid, it.qty.max(1)))
                .collect();
            if items.is_empty() {
                crate::emit!("[shop] nothing to sell (no ammo)");
                return Ok(false);
            }
            for (slot, itemid, qty) in &items {
                session.send_packet(pnpc::npc_shop_sell(*slot, *itemid, *qty)).await?;
            }
            crate::emit_f!([crate::emit::Field::Count => items.len()] => "[shop] sellammo {} items sent", items.len());
        }
        Command::SellType(kind) => {
            let items: Vec<(i16, i32, i16)> = state
                .inventory
                .iter()
                .filter(|((tab, _), _)| *tab == kind.tab())
                .map(|((_, slot), it)| (*slot, it.itemid, it.qty.max(1)))
                .collect();
            if items.is_empty() {
                crate::emit_f!([crate::emit::Field::Text => kind.name()] => "[shop] no {} items to sell", kind.name());
                return Ok(false);
            }
            for (slot, itemid, qty) in &items {
                session.send_packet(pnpc::npc_shop_sell(*slot, *itemid, *qty)).await?;
            }
            crate::emit_f!([crate::emit::Field::Count => items.len(), crate::emit::Field::Text => kind.name()] => "[shop] sell tab {}: {} items sent", kind.name(), items.len());
        }
        Command::CashShop => {
            if !in_game(state) {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "[cashshop] must be in game (phase={:?})", state.phase);
                return Ok(false);
            }
            session.send_packet(pcash::enter()).await?;
            crate::emit!("[cashshop] requesting entrance...");
        }
        Command::AuctionOpen => {
            // Open the auction (0x8D ENTER_MTS empty packet) — the client's
            // "auction" button behavior. Used when reward (0x70) is disabled;
            // once the dialog arrives, proceed with `npc reply <n>`.
            if !in_game(state) {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "[auction] must be in game (phase={:?})", state.phase);
                return Ok(false);
            }
            session.send_packet(pcash::enter_mts()).await?;
            crate::emit!("[auction] opening auction...");
        }
        Command::Reenter => {
            if !in_game(state) {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "[reenter] must be in game (phase={:?})", state.phase);
                return Ok(false);
            }
            if state.reenter.is_some() {
                crate::emit!("[reenter] already in progress");
                return Ok(false);
            }
            // Snapshot the hunt switch: hunt yields during the cash shop trip/return; once
            // SET_FIELD lands back on the origin map, restore from the snapshot (same pattern as
            // hunt_before in the task lock stack).
            let restore_hunt = state.hunt;
            state.reenter = Some(crate::state::ReenterCtx {
                restore_hunt,
                mapid: state.mapid,
                leave_sent: false,
                started: std::time::Instant::now(),
            });
            state.hunt = false;
            state.hunt_target = None;
            // Reset the EnteringMap timeout window (reuses main.rs's 10s fallback, covering both
            // 0x13 reconnect phases; the previous map_enter_time may have long expired).
            state.map_enter_time = std::time::Instant::now();
            session.send_packet(pcash::enter()).await?;
            crate::emit_f!([crate::emit::Field::MapId => state.mapid, crate::emit::Field::Toggle => if restore_hunt { "on" } else { "off" }] => "[reenter] entering cash shop (map={}, restore_hunt={})", state.mapid, restore_hunt);
        }
        Command::CsBuy(sn, points) => {
            if state.phase != Phase::CashShop {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "[cashshop] not in cash shop (phase={:?})", state.phase);
                return Ok(false);
            }
            session.send_packet(pcash::buy(sn, points)).await?;
            crate::emit_f!([crate::emit::Field::Value => sn] => 
                "[cashshop] buy sn={sn} ({})",
                if points { "points" } else { "nx" });
        }
        Command::CsTakeOut(uid) => {
            if state.phase != Phase::CashShop {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "[cashshop] not in cash shop (phase={:?})", state.phase);
                return Ok(false);
            }
            let uid = match uid {
                Some(u) => u,
                None => match state.cs_inventory.last() {
                    Some(it) => it.unique_id,
                    None => {
                        crate::emit!("[cashshop] cash inventory empty");
                        return Ok(false);
                    }
                },
            };
            session.send_packet(pcash::take_out(uid)).await?;
            crate::emit_f!([crate::emit::Field::Value => uid] => "[cashshop] take out unique={uid}");
        }
        Command::CsOut => {
            if state.phase != Phase::CashShop {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "[cashshop] not in cash shop (phase={:?})", state.phase);
                return Ok(false);
            }
            session.send_packet(pcash::leave()).await?;
            // Clear right after sending: the sale list (thousands of entries) and cash inventory
            // cache are no longer needed, release immediately
            state.clear_cash_shop_cache();
            crate::emit!("[cashshop] leaving...");
        }
        Command::CsList => {
            if state.phase != Phase::CashShop {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "[cashshop] not in cash shop (phase={:?})", state.phase);
                return Ok(false);
            }
            crate::emit!("{}", cashshop_list_lines(state).join("\n"));
        }
        Command::CsStore(uid) => {
            if state.phase != Phase::CashShop {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "[cashshop] not in cash shop (phase={:?})", state.phase);
                return Ok(false);
            }
            // Find the tab holding this unique id in the backpack/worn gear (1=equip, 2=use, 3=setup, 4=etc)
            let mut found: Option<(u8, i32)> = None;
            for ((tab, _), it) in &state.inventory {
                if it.unique_id == uid && *tab != 5 {
                    found = Some((*tab, it.itemid));
                    break;
                }
            }
            if found.is_none() {
                for (_, it) in &state.equipped {
                    if it.unique_id == uid {
                        found = Some((1, it.itemid));
                        break;
                    }
                }
            }
            let Some((tab, itemid)) = found else {
                crate::emit_f!([crate::emit::Field::Value => uid] =>
                    "[cashshop] no item with unique={uid} in the backpack (only cash/point items can be stored)");
                return Ok(false);
            };
            session.send_packet(pcash::store(uid, tab)).await?;
            crate::emit_f!([crate::emit::Field::Value => uid, crate::emit::Field::ItemId => itemid, crate::emit::Field::Slot => tab as i16] =>
                "[cashshop] store unique={uid} item={itemid} tab={tab}");
        }
        Command::ShopLeave => {
            session.send_packet(pnpc::npc_shop_leave()).await?;
            state.shop_open = false;
            crate::handlers::field::clear_shop(state);
            crate::emit!("[shop] leave");
        }
        Command::BuffSkill(skillid) => {
            if !in_game(state) {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "not in game yet");
                return Ok(false);
            }
            let Some(level) = skill_level(state, skillid) else {
                crate::emit_f!([crate::emit::Field::SkillId => skillid] => "[buff] {skillid} not learned");
                return Ok(false);
            };
            let (x, y) = state.position;
            session
                .send_packet(pcombat::special_move(skillid, level, x, y, state.facing))
                .await?;
            crate::emit_f!([crate::emit::Field::SkillId => skillid, crate::emit::Field::Level => level] => "[buff] {skillid} L{level} cast");
        }
        Command::BuffCancel(skillid) => {
            if !in_game(state) {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "not in game yet");
                return Ok(false);
            }
            session.send_packet(pcombat::cancel_buff(skillid)).await?;
            crate::emit_f!([crate::emit::Field::SkillId => skillid] => "[buff] cancel {skillid}");
        }
        Command::KeymapSet(key, ty, action) => {
            if !in_game(state) {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "not in game yet");
                return Ok(false);
            }
            session
                .send_packet(pcombat::change_keymap(&[(key, ty, action)]))
                .await?;
            if let Some(e) = state.keymap.get_mut(key as usize) {
                *e = (ty, action);
            } else if key >= 0 && key < 90 {
                state.keymap.resize(90, (0, 0));
                state.keymap[key as usize] = (ty, action);
            }
            crate::emit!("[keymap] key={key} type={ty} action={action}");
        }
        Command::ChangeChannel(n) => {
            if !in_game(state) {
                crate::emit_f!([crate::emit::Field::Phase => format!("{:?}", state.phase)] => "not in game yet");
                return Ok(false);
            }
            session.send_packet(plogin::change_channel(n - 1)).await?;
            state.hunt = false;
            state.hunt_target = None;
            // user-facing channel is 1-based; state keeps the same (wire is n - 1)
            state.channel = n as i32;
            crate::emit_f!([crate::emit::Field::Channel => n] => "[channel] switching to channel {} (1-based)", n);
        }
        Command::Reactor(cmd) => match cmd {
            ReactorCmd::On(on) => {
                state.hunt_reactor = on;
                if on {
                    crate::emit_f!([crate::emit::Field::Toggle => "on"] => "[rhunt] on - hit all reactors + insta-pickup drops");
                } else {
                    crate::emit_f!([crate::emit::Field::Toggle => "off"] => "[rhunt] off");
                }
            }
            ReactorCmd::HitAll => {
                let oids: Vec<i32> = state.reactors.keys().copied().collect();
                for oid in oids {
                    session.send_packet(pcombat::damage_reactor(oid, 0, 2, state.position.0)).await?;
                }
                crate::emit_f!([crate::emit::Field::Count => state.reactors.len()] => "[reactor] hit {} reactor(s)", state.reactors.len());
            }
            ReactorCmd::Hit(oid) => {
                if !state.reactors.contains_key(&oid) {
                    crate::emit_f!([crate::emit::Field::Oid => oid] => "[reactor] oid={oid} not tracked on this map");
                    return Ok(false);
                }
                session.send_packet(pcombat::damage_reactor(oid, 0, 2, state.position.0)).await?;
                crate::emit_f!([crate::emit::Field::Oid => oid] => "[reactor] hit oid={oid}");
            }
            ReactorCmd::Cooldown(ms) => {
                state.cfg.hunt.reactor_cooldown = ms;
                crate::emit_f!([crate::emit::Field::Duration => ms] => "[reactor] cooldown = {ms}ms ({})", if ms == 0 { "every tick" } else { "per hit round" });
            }
        },
        Command::Trade(tcmd) => match tcmd {
            TradeCmd::Invite(name) => {
                if state.trade.active || state.trade.invite.is_some() {
                    crate::emit!("[trade] already in a trade — quit first");
                    return Ok(false);
                }
                // The client actually invites by name (02 03 + string + int 0), no cid needed
                session.send_packet(ptrade::start()).await?;
                session.send_packet(ptrade::invite(&name)).await?;
                crate::emit_f!([crate::emit::Field::Name => name] => "[trade] invited '{name}'");
            }
            TradeCmd::Accept => {
                if state.trade.active {
                    if state.trade.locked {
                        crate::emit!("[trade] already confirmed");
                    } else {
                        session.send_packet(ptrade::confirm()).await?;
                        state.trade.locked = true;
                        crate::emit!("[trade] confirmed");
                    }
                } else if let Some(inv) = state.trade.invite.take() {
                    session.send_packet(ptrade::accept()).await?;
                    crate::emit_f!([crate::emit::Field::Name => inv.name.as_str()] => "[trade] accepted invite from '{}'", inv.name);
                } else {
                    crate::emit!("[trade] no pending invite or open window");
                }
            }
            TradeCmd::Confirm => {
                if !state.trade.active {
                    crate::emit!("[trade] no open trade window");
                } else if state.trade.locked {
                    crate::emit!("[trade] already confirmed");
                } else {
                    session.send_packet(ptrade::confirm()).await?;
                    state.trade.locked = true;
                    crate::emit!("[trade] confirmed");
                }
            }
            TradeCmd::Decline => {
                if state.trade.invite.is_some() {
                    session.send_packet(ptrade::decline()).await?;
                    crate::emit!("[trade] invite declined");
                } else {
                    crate::emit!("[trade] no pending invite");
                }
                state.trade.invite = None;
            }
            TradeCmd::Quit => {
                if state.trade.active || state.trade.invite.is_some() {
                    session.send_packet(ptrade::quit()).await?;
                    crate::emit!("[trade] left the trade");
                } else {
                    crate::emit!("[trade] not in a trade");
                }
                state.trade.active = false;
                state.trade.invite = None;
                state.trade.items_partner.clear();
                state.trade.meso_partner = 0;
                state.trade.partner_locked = false;
                state.trade.locked = false;
            }
            TradeCmd::Put(itemid, qty) => {
                if state.trade.active {
                    if state.trade.locked {
                        crate::emit!("[trade] already confirmed — cannot put more items");
                    } else {
                        send_trade_put(state, session, itemid, qty).await?;
                    }
                } else {
                    // No more queueing: putting items/meso is flow behavior, orchestrated by task
                    // steps (open the window first, then put; can wait trade_active==1).
                    crate::emit!("[trade] no window open — start a trade first (use tasks to sequence put/meso/confirm)");
                }
            }
            TradeCmd::Meso(n) => {
                if state.trade.active {
                    if state.trade.locked {
                        crate::emit!("[trade] already confirmed — cannot put more meso");
                    } else if n <= 0 {
                        crate::emit!("[trade] amount must be positive");
                    } else if n > state.meso {
                        crate::emit_f!([crate::emit::Field::Meso => state.meso] => "[trade] only have {} meso", state.meso);
                    } else {
                        session.send_packet(ptrade::put_meso(n)).await?;
                        crate::emit_f!([crate::emit::Field::Meso => n] => "[trade] put {n} meso — waiting for 'trade confirm'");
                    }
                } else {
                    // No more queueing: putting items/meso is flow behavior, orchestrated by task steps.
                    crate::emit!("[trade] no window open — start a trade first (use tasks to sequence put/meso/confirm)");
                }
            }
        },
        Command::Help => {
            crate::emit!(
                "view commands:
  view                  full status (position/hp/party)
  view attr             player stats (hp/mp/exp/level/str/dex/int/luk/ap/sp/meso)
  view mobs             map mobs
  view players          nearby players
  view npcs             map NPCs
  view nearby           nearby players (same as view players)
  view drops            map drops (id·name; meso = gold amount)
  view inventory        inventory + meso
  view skills           learned skills + SP
  view equiped          worn equipment
  view eqpinfo <id>     equip info: every match in backpack and worn, tagged
  view party            party members
  view trade            trade window state
  view reactor          reactors on the map
  view keymap           key bindings
  view cashshop         cash inventory / nx / points
movement commands:
  move <x> [y]          walk (y optional, uses ground level)
  town <mapid> [portal] warp to a map (default portal: sp)
  warp <portal>         warp through a portal by name on current map
  reenter               re-enter the current map (forced cash shop round trip)
  channel <n>           switch channel (1-based)
  sleep <sec>           wait (handy in task steps / --auto)
hunt commands:
  hunt on|off           autonomous mob hunting + loot
  hunt attack [n]       normal attack mode (one packet per mob in range, max n)
  hunt skill <id> [n] [range] [mp]  skill mode (id=0 = skill-less group attack; n targets / range px / mp check)
  hunt hits <n>         hits per target per cast (1..15, e.g. 2 for a two-hit AoE)
  hunt atkcd <ms>       attack cooldown interval (0 = every tick)
  hunt range <px>       attack range: mob filter radius after teleport
  hunt damage <n>       reported attack damage (0 = auto lvl^2 formula)
  hunt pickup on|off|<px>  pickup toggle or radius (0 = whole map)
  hunt controller on|off  attack only mobs we hold control of (default on)
  hunt stand on|off     stand still and attack from the spot
  hunt filter           pickup filter state (allow/deny lists)
    hunt filter status      show the filter state (alias: hunt filter)
    hunt filter off         pause filtering (both lists kept)
    hunt filter allow [ids] switch to whitelist mode (ids = replace the list; no ids = keep it)
    hunt filter deny [ids]  switch to blacklist mode (same list semantics)
    hunt filter add <ids>   append to the active list
    hunt filter del <ids>   remove from the active list
  hunt status           print the current hunt config
  hunt save             persist hunt config into the config file (reused next launch)
  hunt once             one manual attack round: locate mobs in range + fire once
                        (no pickup/gather; works during tasks; rule-friendly)
  gather on|off         pull out-of-range mobs toward the player (independent of hunt)
  gather step <px>      pull distance per round (0 = straight to the player)
  gather interval <ms>  round interval (0 = every tick)
  gather max <n>        max mobs pulled per round (0 = unlimited)
  gather controller on|off  only pull mobs we hold control of
  pickup all            no-range pick up every drop
  reactor on|off        reactor farming: hit all reactors + insta-pickup drops
  reactor hit all|<oid> manual reactor hit
  reactor cooldown <ms> reactor hit round interval (0 = every tick)
  attack <oid>          one-shot normal attack
item commands:
  use <itemid>          use a USE-tab item (e.g. potion)
  reward <itemid>       open a reward box
  drop <itemid> [qty] [slot <n>]  drop an item (slot = drop only that slot)
  dropmeso <amount>     drop meso (10..50000)
  scroll <scrollid> <equipid> [bless]  apply an upgrade scroll (bless = white scroll protection, ws=2)
  equip <itemid>        wear an equip from the backpack
  unequip <itemid>      take an equip off
  ap status             show ability points
  ap <stat> [n]         spend n ability points (str|dex|int|luk|maxhp|maxmp)
  keymap set <key> <type> <action>  rebind a key (view via 'view keymap')
  skill learn <skillid> spend 1 skill point
  skill cast <skillid> <oid>  cast an attack skill on a target (alias: cast)
  skill info <skillid>  show a skill's level
  skill status          show learned skills (alias: view skills)
  buff <skillid>        cast a buff/assist skill
  buff cancel <skillid> cancel a buff
  sell <itemid> [qty]   sell an item (qty<=0 = all stacks of it, cross-slot)
  sell <itemid> <qty> slot <n>  sell only the item in slot n
  sell tab <equip|consume|etc>  sell a whole inventory tab
  sellammo              sell all ammo (arrows/stars/bullets)
  buy <itemid> [qty]    buy from the open shop
  shop leave            close the shop window
  npc <oid>             talk to an NPC (dialog)
    npc yes|no              answer a yes/no dialog
    npc next|prev           advance / go back in a dialog
    npc reply <n>           select dialog option n
    npc num <n>             answer a number prompt
    npc text <...>          answer a text prompt
    npc cancel              close the dialog
trade/party commands:
  trade invite <name>   invite a player
  trade accept          accept a pending invite
  trade confirm         lock the trade window
  trade decline         decline a pending invite
  trade quit            leave the trade
  trade put <itemid> [qty]  put items (trade window must be open)
  trade meso <amount>   put meso (trade window must be open)
  party create          create a party
  party invite <name>   invite a player by name
  party invitecid <cid> invite a player by character id
  party leave           leave the party
  party kick <name>     kick a member by name
  party kickcid <cid>   kick a member by character id
cash shop commands:
  cashshop              enter the cash shop
  cashshop buy <sn> [nx|points]  buy a sale item (sn from 'cashshop list'; default nx)
  cashshop get [uniqueid]  take an item out of the cash inventory (default: last)
  cashshop list         list the sale items
  cashshop store <uniqueid>  store a backpack cash item back into the shop
  cashshop out          leave the cash shop
  auction               open the auction (continue the dialog with 'npc reply')
rule/task/group commands:
  rule open|close <id>  toggle a config rule at runtime (rules default off)
  rule status [id]      show rule definitions and enabled state
  group run|stop <id>   run/stop a feature group (start/stop its rules+tasks)
  group status          show groups and their state
  task run <id>         push a task onto the lock stack
  task stop             clear the stack (release locks, reset cursors)
  task status           show stack + defined tasks
  setvar <name> <value> set a flow variable (rule gate: var==<name>:<value>)
  clearvar <name>       remove a flow variable (manual lifecycle only)
  reload                hot-reload the config file (rules reset to file values)
  reconnect on|off      toggle auto-reconnect
  reconnect delay <secs>  reconnect interval in seconds
  reconnect max <count>   max consecutive reconnects (0 = forever)
  reconnect status      show reconnect settings
system commands:
  chat <text>           speak in the map channel
  help                  show this help
  quit                  graceful logout and exit
in-game chat control: #rule/#task/#group <...> — same commands via whisper (chat_admins whitelist)"
            );
        }

        Command::Sleep(secs) => {
            tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
        }
        Command::Wait(pred, _) => {
            return Err(format!("wait is not standalone: using '{pred}' inside a rule action auto-inlines the wait; or use the wait field in a task step"));
        }
        Command::While(pred, cmd) => {
            return Err(format!("while is not standalone: using 'while {pred} do {cmd}' inside a rule action auto-inlines as a loop step; or use the loop field in a task step"));
        }
        Command::Quit => {
            crate::emit!("bye");
            return Ok(true);
        }
    }
    Ok(false)
}

fn cashshop_list_lines(state: &BotState) -> Vec<String> {
    if state.cs_items.is_empty() {
        return vec!["[cashshop] sale list empty".to_string()];
    }
    // First line carries the [cashshop] tag (for TUI extraction); continuation lines don't (in a
    // multi-line emit, continuation lines inherit the first line's tag, so repeating [cashshop]
    // at line start would duplicate the category)
    let mut lines = vec![format!("[cashshop] {} sale entries:", state.cs_items.len())];
    // Only list entries with a real itemid (flags&1); the rest are server-reserved
    let mut shown = 0;
    for it in &state.cs_items {
        if it.itemid == 0 {
            continue;
        }
        shown += 1;
        lines.push(format!(
            "  sn={} item={} qty={} price={} meso={}",
            it.sn, it.itemid, it.count, it.price, it.meso
        ));
    }
    if shown == 0 {
        lines.push("  (no item entries with an itemid)".to_string());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::tasks;
    use crate::runtime_config::{GroupDef, Rule, TaskDef};
    use crate::state::BotState;

    #[test]
    fn hunt_filter_crud_and_mode_switch() {
        let mut s = BotState::default();
        // whitelist replace + mode switch
        run_hunt_filter(&mut s, &HuntFilterCmd::SetAllow(vec![2000000, 2000000, 2000001])).unwrap();
        assert_eq!(s.cfg.hunt.pickup_allow, vec![2000000, 2000001], "dedup");
        assert_eq!(s.cfg.hunt.pickup_filter_mode, "allow");
        assert_eq!(run_hunt_filter(&mut s, &HuntFilterCmd::Status).unwrap(),
            "[hunt] filter=allow allow=[2000000,2000001] deny=[]");
        // append to the allow list (mode allow)
        run_hunt_filter(&mut s, &HuntFilterCmd::Add(vec![2000002, 2000000])).unwrap();
        assert_eq!(s.cfg.hunt.pickup_allow, vec![2000000, 2000001, 2000002]);
        // remove from allow
        run_hunt_filter(&mut s, &HuntFilterCmd::Del(vec![2000001])).unwrap();
        assert_eq!(s.cfg.hunt.pickup_allow, vec![2000000, 2000002]);
        // blacklist replace: mode switches to deny; + and - target deny now
        run_hunt_filter(&mut s, &HuntFilterCmd::SetDeny(vec![4000000])).unwrap();
        assert_eq!(s.cfg.hunt.pickup_filter_mode, "deny");
        run_hunt_filter(&mut s, &HuntFilterCmd::Add(vec![4000006])).unwrap();
        assert_eq!(s.cfg.hunt.pickup_deny, vec![4000000, 4000006], "add targets the deny list in deny mode");
        // switch back to allow: both lists kept, mode controls which is active
        run_hunt_filter(&mut s, &HuntFilterCmd::SetAllow(vec![2000000])).unwrap();
        assert_eq!(s.cfg.hunt.pickup_deny, vec![4000000, 4000006], "deny list kept for later switch");
        // off pauses filtering but keeps the lists for later
        run_hunt_filter(&mut s, &HuntFilterCmd::Off).unwrap();
        assert_eq!(crate::runtime_config::pickup_filter_mode(&s.cfg.hunt), "off");
        assert!(!s.cfg.hunt.pickup_allow.is_empty(), "lists survive off");
        assert_eq!(run_hunt_filter(&mut s, &HuntFilterCmd::Status).unwrap(),
            "[hunt] filter=off allow=[2000000] deny=[4000000,4000006]");
        // bare mode switch keeps the lists untouched
        run_hunt_filter(&mut s, &HuntFilterCmd::SetAllow(vec![])).unwrap();
        assert_eq!(s.cfg.hunt.pickup_allow, vec![2000000], "empty SetAllow keeps the list");
        assert_eq!(crate::runtime_config::pickup_filter_mode(&s.cfg.hunt), "allow");
        run_hunt_filter(&mut s, &HuntFilterCmd::SetDeny(vec![])).unwrap();
        assert_eq!(s.cfg.hunt.pickup_deny, vec![4000000, 4000006], "empty SetDeny keeps the list");
        assert_eq!(crate::runtime_config::pickup_filter_mode(&s.cfg.hunt), "deny");
    }

    #[test]
    fn hunt_filter_add_without_mode_errors() {
        let mut s = BotState::default();
        assert!(run_hunt_filter(&mut s, &HuntFilterCmd::Add(vec![2000000])).is_err());
        assert!(run_hunt_filter(&mut s, &HuntFilterCmd::Del(vec![2000000])).is_err());
    }

    #[test]
    fn hunt_filter_prunes_ground_drops() {
        let mut s = BotState::default();
        s.drops.insert(
            1,
            crate::state::Drop { oid: 1, itemid: 4000000, is_meso: false, ..Default::default() },
        );
        s.drops.insert(
            2,
            crate::state::Drop { oid: 2, itemid: 500, is_meso: true, ..Default::default() },
        );
        run_hunt_filter(&mut s, &HuntFilterCmd::SetDeny(vec![4000000])).unwrap();
        assert!(!s.drops.contains_key(&1), "pruned by the new deny list");
        assert!(s.drops.contains_key(&2), "meso always survives");
    }

    fn state_with_groups() -> BotState {
        let mut s = BotState::default();
        s.cfg.groups.push(GroupDef {
            id: "autosell".into(),
            enabled: false,
            exclusive: true,
            rules: vec![Rule {
                id: "sell_loop".into(),
                enabled: false,
                when: "always".into(),
                then: vec![],
                cooldown: 0,
            }],
            tasks: vec![TaskDef {
                id: "open_shop".into(),
                priority: 10,
                timeout: None,
                steps: vec![],
                vars: Default::default(),
            recurring: false,
            }],
        });
        s.cfg.groups.push(GroupDef {
            id: "autopotion".into(),
            enabled: false,
            exclusive: true,
            rules: vec![],
            tasks: vec![],
        });
        s
    }

    #[test]
    fn group_open_close_toggles_and_unknown_err() {
        let mut s = state_with_groups();
        assert!(group_open(&mut s, "nope").is_err());
        assert!(group_close(&mut s, "nope").is_err());
        group_open(&mut s, "autosell").unwrap();
        assert!(s.cfg.group("autosell").unwrap().enabled);
        group_close(&mut s, "autosell").unwrap();
        assert!(!s.cfg.group("autosell").unwrap().enabled);
    }

    #[test]
    fn exclusive_cascade_closes_other_exclusive_groups() {
        let mut s = state_with_groups();
        group_open(&mut s, "autosell").unwrap();
        assert!(s.cfg.group("autosell").unwrap().enabled);
        // Opening another exclusive group -> autosell is auto-closed
        group_open(&mut s, "autopotion").unwrap();
        assert!(s.cfg.group("autopotion").unwrap().enabled);
        assert!(!s.cfg.group("autosell").unwrap().enabled);
        // Non-exclusive groups are unaffected by the cascade
        s.cfg.groups.push(GroupDef {
            id: "watchdog".into(),
            enabled: false,
            exclusive: false,
            rules: vec![],
            tasks: vec![],
        });
        group_open(&mut s, "watchdog").unwrap();
        group_open(&mut s, "autopotion").unwrap();
        assert!(s.cfg.group("watchdog").unwrap().enabled);
    }

    #[test]
    fn exclusive_cascade_stops_running_tasks_of_closed_group() {
        let mut s = state_with_groups();
        group_open(&mut s, "autosell").unwrap();
        // Start a group task (the group is open)
        tasks::start_task(&mut s, "open_shop").unwrap();
        assert_eq!(s.task_stack.len(), 1);
        assert_eq!(s.task_stack.last().unwrap().group.as_deref(), Some("autosell"));
        // Opening exclusive group autopotion -> autosell is closed, its tasks stopped
        group_open(&mut s, "autopotion").unwrap();
        assert!(s.task_stack.is_empty(), "closing a group must stop its tasks");
        // Explicit group close also stops group tasks
        group_open(&mut s, "autosell").unwrap();
        tasks::start_task(&mut s, "open_shop").unwrap();
        group_close(&mut s, "autosell").unwrap();
        assert!(s.task_stack.is_empty());
    }

    #[test]
    fn rule_open_close_on_group_rule_errors() {
        let mut s = state_with_groups();
        let err = rule_open(&mut s, "sell_loop").unwrap_err();
        assert!(err.contains("group 'autosell'"), "err={err}");
        let err = rule_close(&mut s, "sell_loop").unwrap_err();
        assert!(err.contains("group 'autosell'"), "err={err}");
        assert!(rule_open(&mut s, "nope").is_err());
    }

    #[test]
    fn default_attack_mode_validated() {
        let mut s = BotState::default();
        s.cfg.hunt.attack_mode = "area".into();
        apply_default_attack_mode(&mut s);
        assert_eq!(s.cfg.hunt.attack_mode, "attack");
        s.cfg.hunt.attack_mode = "skill".into();
        apply_default_attack_mode(&mut s);
        assert_eq!(s.cfg.hunt.attack_mode, "skill");
    }

    #[test]
    fn status_lines_separate_top_level_and_group() {
        let mut s = state_with_groups();
        s.cfg.rules.push(Rule {
            id: "trade_accept".into(),
            enabled: false,
            when: "always".into(),
            then: vec![],
            cooldown: 0,
        });
        // rule status: only top-level rules, no group rules; numbered with multi-line fields
        let rule_lines = rule_status_lines(&s, None);
        assert!(rule_lines.iter().any(|l| l.contains("1. trade_accept")));
        assert!(rule_lines.iter().any(|l| l.contains("when:")));
        assert!(rule_lines.iter().any(|l| l.contains("then:")));
        assert!(
            !rule_lines.iter().any(|l| l.contains("sell_loop")),
            "group rules must not appear in rule status"
        );
        // group status: full details of group rules/tasks
        let group_lines = group_status_lines(&s, None);
        assert!(group_lines.iter().any(|l| l.contains("autosell") && l.contains("off, exclusive")));
        assert!(group_lines.iter().any(|l| l.contains("rule 1:") && l.contains("sell_loop")));
        assert!(group_lines.iter().any(|l| l.contains("when:") && l.contains("always")));
        assert!(group_lines.iter().any(|l| l.contains("task 1:") && l.contains("open_shop")));
        // Step details are output by group status (test data has no steps, so no assertion here)
        // task status: group tasks do not appear
        let task_lines = task_status_lines(&s);
        assert!(
            !task_lines.iter().any(|l| l.contains("open_shop")),
            "group tasks must not appear in task status"
        );
    }

    #[test]
    fn npc_dialog_advance_closes_dialog_until_server_returns() {
        // Server returns a dialog page -> dialog set to true (handle_npc_talk behavior)
        let mut s = BotState::default();
        s.dialog_open = true;
        // Sending an advance command (reply/next/yes/no/number/text/cancel share this entry)
        // -> optimistically set false: wait: "dialog==1" awaits the next dialog content
        npc_dialog_advance(&mut s);
        assert!(!s.dialog_open);
        // The next server NPC_TALK sets it back to true
        s.dialog_open = true;
        assert!(crate::runtime_config::eval_predicate(&s, "dialog==1"));
        npc_dialog_advance(&mut s);
        assert!(crate::runtime_config::eval_predicate(&s, "dialog==0"));
    }

    #[test]
    fn cashshop_list_lines_filters_and_formats() {
        let mut s = BotState::default();
        // Empty list
        let lines = cashshop_list_lines(&s);
        assert!(lines[0].contains("sale list empty"));
        // One real item + one reserved entry without an itemid
        s.cs_items.push(crate::state::CashShopItem { sn: 5550000, itemid: 5050000, count: 2, price: 99, ..Default::default() });
        s.cs_items.push(Default::default()); // sn=0 itemid=0 (server-reserved)
        let lines = cashshop_list_lines(&s);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("2 sale entries"));
        assert!(lines[1].contains("sn=5550000 item=5050000 qty=2 price=99"));
    }
    #[test]
    fn reload_syncs_toggles_to_file_values() {
        // Rule/group enabled lives in the config file - the file is the source of
        // truth - runtime toggles reset on reload. Tasks carry no switch and
        // only run when invoked.
        let dir = std::env::temp_dir().join(format!("osbot_rst_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("cfg.json");
        let body = r#"{"rules":[{"id":"r1","enabled":false,"when":"always","then":[],"cooldown":0}],
            "groups":[{"id":"g1","enabled":false,"exclusive":false,"rules":[],"tasks":[]}]}"#;
        std::fs::write(&file, body).unwrap();
        let paths = vec![file.clone()];
        let mut s = BotState {
            cfg: crate::runtime_config::RuntimeConfig::load_from(&paths),
            ..Default::default()
        };
        group_open(&mut s, "g1").unwrap();
        rule_open(&mut s, "r1").unwrap();
        assert!(s.cfg.group("g1").unwrap().enabled);
        assert!(s.cfg.rules[0].enabled);
        // Same file still says false -> reload must reset both toggles.
        s.cfg.reload_from(&paths).unwrap();
        assert!(!s.cfg.group("g1").unwrap().enabled, "group switch syncs to file");
        assert!(!s.cfg.rules[0].enabled, "rule toggle syncs to file");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rule_close_stops_untagged_runtimes_but_keeps_grouped() {
        // 顶层规则内联的 wait/while/task run 在栈上是 group:None; rule close 必须杀掉它们,
        // 但不能误杀属于某个 group 的 runtime(那些由 group stop 管理)。
        let mut s = BotState::default();
        s.cfg.rules.push(Rule {
            id: "top_rule".into(),
            enabled: true,
            when: "always".into(),
            then: vec![],
            cooldown: 0,
        });
        s.task_stack.push(crate::runtime_config::TaskRuntime {
            def_id: "top_rule".into(),
            steps: vec![],
            step_idx: 0,
            step_since: std::time::Instant::now(),
            group: None,
            hunt_before: false,
            recurring: false,
        });
        s.task_stack.push(crate::runtime_config::TaskRuntime {
            def_id: "in_group".into(),
            steps: vec![],
            step_idx: 0,
            step_since: std::time::Instant::now(),
            group: Some("paplatus".into()),
            hunt_before: false,
            recurring: false,
        });
        rule_close(&mut s, "top_rule").unwrap();
        assert_eq!(s.task_stack.len(), 1);
        assert_eq!(s.task_stack[0].def_id, "in_group");
    }

    #[test]
    fn group_close_stops_group_runtimes() {
        // group stop(close) 杀掉该 group 名下所有 runtime, 不动其它 group / 顶层 runtime。
        let mut s = BotState::default();
        s.cfg.groups.push(GroupDef {
            id: "paplatus".into(),
            enabled: true,
            exclusive: false,
            rules: vec![],
            tasks: vec![],
        });
        s.task_stack.push(crate::runtime_config::TaskRuntime {
            def_id: "boss_fight".into(),
            steps: vec![],
            step_idx: 0,
            step_since: std::time::Instant::now(),
            group: Some("paplatus".into()),
            hunt_before: false,
            recurring: false,
        });
        s.task_stack.push(crate::runtime_config::TaskRuntime {
            def_id: "top".into(),
            steps: vec![],
            step_idx: 0,
            step_since: std::time::Instant::now(),
            group: None,
            hunt_before: false,
            recurring: false,
        });
        group_close(&mut s, "paplatus").unwrap();
        assert!(!s.cfg.group("paplatus").unwrap().enabled);
        assert_eq!(s.task_stack.len(), 1);
        assert_eq!(s.task_stack[0].def_id, "top");
    }

    #[test]
    fn group_stop_unknown_group_errors() {
        let mut s = BotState::default();
        assert!(group_close(&mut s, "nope").is_err());
    }
}
