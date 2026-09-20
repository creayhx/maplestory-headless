//! Field/entity packet handlers: players, mobs, drops, stats, inventory.

use crate::packet::Cursor;
use crate::parsers;
use crate::session::Session;
use crate::state::{BotState, Drop, Entity, EntityKind, Item, Npc, Reactor, ShopItem};

/// SET_FIELD (0x81):
/// - getCharInfo variant: `int channel; byte 0; byte 1; ...` (mode2 == 1)
/// - getWarpToMap variant: `int channel; byte 0; byte 3; byte 0; short 0;
///   int mapid; byte portal; short hp; long time` (mode2 == 3)
pub(super) fn handle_set_field(
    recv: &mut Cursor<'_>,
    state: &mut BotState,
    names: &crate::names::NameTable,
) {
    let channel = recv.read_i32();
    let _mode1 = recv.read_i8();
    let mode2 = recv.read_i8();
    // The server's authoritative channel (wire int is 0-based, getChannel()-1):
    // calibrate both the display value and the reconnect target (relogin reuses
    // selected_channel), so a reconnect always returns to the actual channel.
    // Stored 1-based (user-facing convention).
    state.channel = channel + 1;
    state.selected_channel = channel + 1;

    if mode2 == 3 {
        recv.skip(3);
        let mapid = recv.read_i32();
        let portalid = recv.read_u8();
        state.mapid = mapid;
        // Map change = leaving the cash shop (0x21/0x61 landing): clear the cash shop cache (sale list / cash inventory)
        state.clear_cash_shop_cache();
        state.phase = crate::state::Phase::InGame;
        state.discovery_sent = false;
        state.position = (0, 0); // reset so SPAWN_PLAYER updates it
        state.map_enter_time = std::time::Instant::now();
        state.npcs.clear();
        state.reactors.clear();
        state.drops.clear();
        // Clear leftovers from the previous map: other player entities (keep our own player
        // entry; SPAWN_PLAYER will re-add it), platform layer, ground y, locked target —
        // otherwise hunt would chase ghost mobs from the old map after re-entry
        // (channel change / town / reenter all benefit).
        state
            .entities
            .retain(|_, e| e.kind == EntityKind::Player && e.oid == state.my_cid);
        state.ground_y = 0;
        state.hunt_target = None;
        // On a map change the server always closes shop/dialog windows: reset state so stale
        // shop_open/dialog can't misjudge rule wait predicates.
        state.shop_open = false;
        state.dialog_open = false;
        clear_npc_menu(state);
        clear_shop(state);
        // reenter landing: SET_FIELD returns to the origin map -> round trip done, restore hunt from the snapshot.
        complete_reenter(state, mapid, names);
        crate::emit_f!([crate::emit::Field::MapId => mapid, crate::emit::Field::Portal => portalid] => "[set_field] in game! map={mapid} portal={portalid}");
    } else if mode2 == 1 {
        if let Some(ci) = parsers::parse_charinfo(recv) {
            state.level = ci.level;
            state.hp = ci.hp;
            state.maxhp = ci.maxhp;
            state.mp = ci.mp;
            state.maxmp = ci.maxmp;
            state.str = ci.str;
            state.dex = ci.dex;
            state.int = ci.int;
            state.luk = ci.luk;
            state.ap = ci.ap;
            state.sp = ci.sp;
            state.exp = ci.exp;
            state.meso = ci.meso;
            state.inventory = ci.inventory;
            state.equipped = ci.equipped;
            state.skills = ci.skills;
            state.name = ci.name.clone();
            crate::emit_f!([
                    crate::emit::Field::Name => ci.name,
                    crate::emit::Field::Level => ci.level,
                    crate::emit::Field::Job => ci.job,
                    crate::emit::Field::Hp => ci.hp,
                    crate::emit::Field::MaxHp => ci.maxhp,
                    crate::emit::Field::Mp => ci.mp,
                    crate::emit::Field::MaxMp => ci.maxmp,
                    crate::emit::Field::Meso => ci.meso,
                    crate::emit::Field::Items => state.inventory.len(),
                    crate::emit::Field::Equips => state.equipped.len(),
                    crate::emit::Field::Sp => state.sp,
                    crate::emit::Field::Skills => state.skills.len(),
                ] => 
                "[charinfo] {} L{} job={} hp={}/{} mp={}/{} meso={} items={} equips={} sp={} skills={}",
                ci.name,
                ci.level,
                ci.job,
                ci.hp,
                ci.maxhp,
                ci.mp,
                ci.maxmp,
                ci.meso,
                state.inventory.len(),
                state.equipped.len(),
                state.sp,
                state.skills.len());
        }
        // Map change = leaving the cash shop (charinfo landing path): clear the cash shop cache
        state.clear_cash_shop_cache();
        state.phase = crate::state::Phase::InGame;
        state.discovery_sent = false;
        state.map_enter_time = std::time::Instant::now();
        // Channel change (0x13 reconnect) / reenter / fresh login all land via
        // this getCharInfo variant: clear the previous channel's map objects —
        // otherwise stale mobs from the old channel accumulate with the new
        // spawns (54 -> 100+ ghosts, some forever unreachable). Same cleanup as
        // the mode2==3 warpToMap branch.
        state.position = (0, 0);
        state.npcs.clear();
        state.reactors.clear();
        state.drops.clear();
        state
            .entities
            .retain(|_, e| e.kind == EntityKind::Player && e.oid == state.my_cid);
        state.ground_y = 0;
        state.hunt_target = None;
        state.shop_open = false;
        state.dialog_open = false;
        clear_npc_menu(state);
        clear_shop(state);
        // The reenter return path goes through the charinfo branch (CharacterTransfer landing =
        // getCharInfo): also completes the round trip and restores the hunt snapshot.
        complete_reenter(state, state.mapid, names);
        crate::emit_f!([crate::emit::Field::MapId => state.mapid] => 
            "[set_field] in game! (charinfo) map={}",
            state.mapid);
    } else {
        crate::emit_f!([crate::emit::Field::Id => mode2] => "[set_field] unknown variant (mode2={mode2})");
    }
}

/// reenter round trip complete (SET_FIELD back to the origin map): clear leftover map state,
/// restore hunt from the snapshot. Both the warp and charinfo landing paths reach this.
fn complete_reenter(state: &mut BotState, mapid: i32, names: &crate::names::NameTable) {
    let Some(ctx) = state.reenter.take() else {
        return;
    };
    // Back on the origin map: clear stale entities/drops left over from the cash shop round trip;
    // wait for SPAWN to rebuild them
    state
        .entities
        .retain(|_, e| e.kind == EntityKind::Player && e.oid == state.my_cid);
    state.drops.clear();
    state.npcs.clear();
    state.reactors.clear();
        state.ground_y = 0;
        // Try to get ground level from the "sp" (spawn point) portal data first,
        // so we have a reliable y even before any mob spawns.
        if let Some(sp) = names.find_portal(mapid, "sp") {
            state.ground_y = sp.y as i16;
        }
        state.hunt_target = None;
    state.shop_open = false;
    state.dialog_open = false;
    clear_npc_menu(state);
        clear_shop(state);
    if mapid == ctx.mapid && ctx.leave_sent {
        state.hunt = ctx.restore_hunt;
        if ctx.restore_hunt {
            // Restoring hunt on return: mark the hunt clock fresh so post-trip
            // mobs aren't treated as stale.
            state.last_kill = std::time::Instant::now();
        }
        crate::emit_f!([
                crate::emit::Field::MapId => mapid,
                crate::emit::Field::Toggle => if state.hunt { "on" } else { "off" },
            ] => 
            "[reenter] back on map={mapid} (hunt restored to {})",
            state.hunt);
    } else {
        crate::emit_f!([
                crate::emit::Field::MapId => mapid,
                crate::emit::Field::MapId => ctx.mapid,
            ] => 
            "[reenter] landed on map={mapid} (origin {}) — hunt stays off",
            ctx.mapid);
    }
}

/// Track a spawned mob and remember the ground level it stands on.
pub(super) fn handle_spawn_mob(recv: &mut Cursor<'_>, state: &mut BotState, controlled: bool) {
    let parsed = if controlled {
        parsers::parse_spawn_monster_control(recv)
    } else {
        parsers::parse_spawn_monster(recv)
    };

    match parsed {
        Some(mob) => {
            if mob.mobid == 0 {
                // 0xF0 aggro=0: the server takes back control (mob died/transferred), remove locally
                state.entities.remove(&mob.oid);
                return;
            }
            if mob.y > state.ground_y {
                state.ground_y = mob.y;
            }
            let oid = mob.oid;
            if controlled {
                // 0xF0 (control granted): overwrite-insert (controlled=true)
                state.entities.insert(oid, mob);
            } else {
                // 0xEE (broadcast): if 0xF0 arrived first (controlled=true), the broadcast must not
                // overwrite the control flag — otherwise gather would think mobs lack control and pull none.
                state.entities.entry(oid).or_insert(mob);
            }
        }
        None => {
            crate::emit_err_f!([] => "[mob] parse skipped (statuses or unknown layout)");
        }
    }
}

/// Track a player that entered the map. For our own cid, also learn the
/// authoritative spawn position from the full packet.
pub(super) fn handle_spawn_player(recv: &mut Cursor<'_>, state: &mut BotState) {
    let (charid, name) = parsers::parse_spawn_player_head(recv);

    if charid == state.my_cid {
        if let Some((x, y, stance)) = parsers::parse_spawn_player_self(recv) {
            // Abnormal y (|y|<10 is an airborne point; with buffs the packet grows and the fixed
            // offset parse misaligns, observed as y=1) must not overwrite position — otherwise
            // gather pull targets and attack filtering all use the wrong y, and mobs pulled to
            // an airborne point can't be hit.
            // Threshold 10: blocks the misaligned y=1 while normal spawns (e.g. y=92/214) still apply.
            if y.abs() >= 10 {
                state.position = (x, y);
            }
            crate::emit_f!([crate::emit::Field::Pos => format!("({x},{y})"), crate::emit::Field::Id => stance] => "[self] spawn pos = ({x},{y}) stance={stance}");
        }
    }

    crate::emit_f!([crate::emit::Field::Cid => charid, crate::emit::Field::Name => name] => "[spawn_player] cid={charid} name={name}");
    state.entities.insert(
        charid,
        Entity {
            oid: charid,
            kind: EntityKind::Player,
            charname: name,
            ..Entity::default()
        },
    );
}

/// Update a player's position/stance from their MOVE_PLAYER broadcast (0xBB).
/// For our own cid this is the server's authoritative position.
pub(super) fn handle_move_player(recv: &mut Cursor<'_>, state: &mut BotState) {
    let (cid, pos, stance) = parsers::parse_move_player(recv);
    if cid == state.my_cid {
        if let Some((x, y)) = pos {
            state.position = (x, y);
        }
    }
    if let Some(e) = state.entities.get_mut(&cid) {
        if let Some((x, y)) = pos {
            e.x = x;
            e.y = y;
        }
        e.stance = stance;
    }
}

/// Record our own attacks from the 0xBC broadcast (for logging).
pub(super) fn handle_attack_broadcast(recv: &mut Cursor<'_>, state: &mut BotState) {
    let atk = parsers::parse_attack_broadcast(recv);
    if atk.cid == state.my_cid {
        let targets: Vec<String> = atk.targets.iter().map(|t| t.oid.to_string()).collect();
        crate::emit_f!([
                crate::emit::Field::SkillId => atk.skill,
                crate::emit::Field::Targets => atk.targets.len(),
            ] => 
            "[bc-own] skill={} targets=[{}]",
            atk.skill,
            targets.join(","));
    }
}

/// Keep mob positions live so hunt targets track real coordinates
/// instead of stale spawn points (mobs jump/roam around).
pub(super) fn handle_move_life(recv: &mut Cursor<'_>, state: &mut BotState) {
    let (oid, pos) = parsers::parse_mob_move(recv);
    if let Some(e) = state.entities.get_mut(&oid) {
        if let Some((x, y)) = pos {
            e.x = x;
            e.y = y;
        }
    }
}

/// 0xF8 `int oid; byte 0; int damage` goes to the mob's controller —
/// informational only. NOT used as a combat-progress signal for the stuck
/// guard: it is only sent to the mob's controller, so other players holding
/// the mob would make it never arrive despite us dealing damage. The guard
/// relies on EXP gain (0x22) alone.
pub(super) fn handle_damage_monster(recv: &mut Cursor<'_>, _state: &mut BotState) {
    let oid = recv.read_i32();
    crate::emit_f!([crate::emit::Field::Oid => oid] => "[dmg] oid={oid}");
}

/// 0xFC `int oid; byte remhppercentage` — the mob HP bar sent to the
/// attacker. Confirms a mob took damage and shows its remaining HP.
/// HP at 0 (0%) = death confirmation fallback: 0xEF may lag or be lost, so remove
/// immediately and stop attacking the dead mob.
pub(super) fn handle_player_hp(recv: &mut Cursor<'_>, state: &mut BotState) {
    let oid = recv.read_i32();
    let percent = recv.read_u8();
    let mobid = state.entities.get(&oid).map(|e| e.mobid).unwrap_or(0);
    crate::emit_f!([crate::emit::Field::Oid => oid, crate::emit::Field::Percent => percent, crate::emit::Field::MobId => mobid] => "[mobhp] oid={oid} mobid={mobid} hp={percent}%");
    if let Some(e) = state.entities.get_mut(&oid) {
        e.hp_pct = Some(percent);
    }
    if percent == 0 {
        state.entities.remove(&oid);
        if state.attack_target == Some(oid) {
            state.attack_target = None;
        }
        if state.hunt_target == Some(oid) {
            state.hunt_target = None;
        }
    }
}

pub(super) fn handle_kill_monster(recv: &mut Cursor<'_>, state: &mut BotState) {
    let oid = recv.read_i32();
    // 0xEF is a map-wide death broadcast: any mob dying on the map triggers it (including kills
    // by other players or out of view). Only handle **tracked mobs** (spawned, possibly killed by the bot):
    // - untracked mobs aren't printed (avoids mobid=0 spam)
    // - untracked mobs don't refresh last_kill (only the bot's own kills advance the hunt clock)
    let mobid = state.entities.get(&oid).map(|e| e.mobid);
    match mobid {
        Some(mobid) => {
            state.entities.remove(&oid);
            if state.attack_target == Some(oid) {
                state.attack_target = None;
            }
            state.last_kill = std::time::Instant::now();
            if state.hunt_target == Some(oid) {
                state.hunt_target = None;
            }
            crate::emit_f!([crate::emit::Field::Oid => oid, crate::emit::Field::MobId => mobid] => "[kill] oid={oid} mobid={mobid}");
        }
        None => {
            // Untracked mob death: ignore silently
        }
    }
}

/// Track a dropped item (0x110). Items rejected by the pickup filter
/// (pickup_allow/pickup_deny) never enter `state.drops` — every pickup path
/// (per-tick radius pick, far-drop teleport, reactor farm, `pickup all`) is
/// thereby exempt in one place, and the per-tick loops stay small. Meso
/// always passes. Filtered drops are dropped silently (no log).
pub(super) fn handle_drop(recv: &mut Cursor<'_>, state: &mut BotState) {
    if let Some((oid, itemid, is_meso, x, y)) = parsers::parse_drop(recv) {
        if !is_meso && itemid == 0 {
            return;
        }
        if !is_meso && !crate::runtime_config::pickup_allowed(&state.cfg.hunt, itemid) {
            return;
        }
        state.drops.insert(
            oid,
            Drop {
                oid,
                itemid,
                is_meso,
                x,
                y,
                ..Default::default()
            },
        );
    }
}

/// Remove a picked-up/expired drop from the map.
pub(super) fn handle_drop_removal(recv: &mut Cursor<'_>, state: &mut BotState) {
    let oid = parsers::parse_remove_drop(recv);
    state.drops.remove(&oid);
}

/// Track HP/MP/EXP/level from UPDATE_STATS (0x22).
pub(super) fn handle_update_stats(recv: &mut Cursor<'_>, state: &mut BotState) {
    let item_reaction = recv.read_u8(); // 1 = enableActions (denied action)
    // If we were waiting for a map change and got denied, reset phase
    if item_reaction == 1 && state.phase == crate::state::Phase::EnteringMap {
        state.phase = crate::state::Phase::InGame;
        crate::emit_f!([crate::emit::Field::MapId => state.mapid] => "[set_field] map change denied �?staying on map={}", state.mapid);
    }
    let u = parsers::parse_stats_update_after_reaction(recv);
    if let Some(v) = u.hp {
        state.hp = v;
    }
    if let Some(v) = u.maxhp {
        state.maxhp = v;
    }
    if let Some(v) = u.mp {
        state.mp = v;
    }
    if let Some(v) = u.maxmp {
        state.maxmp = v;
    }
    if let Some(v) = u.str {
        state.str = v;
    }
    if let Some(v) = u.dex {
        state.dex = v;
    }
    if let Some(v) = u.int {
        state.int = v;
    }
    if let Some(v) = u.luk {
        state.luk = v;
    }
    if let Some(v) = u.exp {
        if state.exp != 0 && v > state.exp {
            let rem = crate::exptable::exp_remain(state.level as i32, v as i64);
            crate::emit_f!([crate::emit::Field::Exp => v] => "[exp] +{} (total {} rem {})", v - state.exp, v, rem);
            state.last_kill = std::time::Instant::now();
        }
        state.exp = v;
    }
    if let Some(v) = u.level {
        if state.level != 0 && v > state.level {
            crate::emit_f!([crate::emit::Field::Level => v] => "[level] up! now level {v}");
        }
        state.level = v;
    }
    if let Some(v) = u.ap {
        if state.ap != 0 && v > state.ap {
            crate::emit_f!([crate::emit::Field::Ap => v] => "[ap] +{} -> {v} available", v - state.ap);
        }
        state.ap = v;
    }
    if let Some(v) = u.sp {
        if state.sp != 0 && v != state.sp {
            crate::emit_f!([crate::emit::Field::Sp => v] => "[sp] {} -> {v} available", state.sp);
        }
        state.sp = v;
    }
    if let Some(v) = u.meso {
        state.meso = v;
    }
}

/// UPDATE_SKILLS (0x27) — a skill level change (from `MaplePacketCreator.
/// updateSkill`): `byte 1; short 1; int skillid; int level; int masterlevel;
/// long expiration; byte 4`. Update the local skill table.
pub(super) fn handle_skill_update(recv: &mut Cursor<'_>, state: &mut BotState) {
    if recv.read_u8() != 1 {
        return;
    }
    let count = recv.read_i16();
    for _ in 0..count {
        if recv.failed() {
            return;
        }
        let skillid = recv.read_i32();
        let level = recv.read_i32();
        let masterlevel = recv.read_i32();
        recv.skip(8); // expiration
        recv.skip(1); // byte 4
        if recv.failed() {
            return;
        }
        let prev = state.skills.get(&skillid).copied();
        state.skills.insert(
            skillid,
            crate::state::SkillEntry {
                level: level as u8,
                masterlevel: masterlevel as u8,
            },
        );
        if let Some(p) = prev {
            crate::emit_f!([crate::emit::Field::SkillId => skillid, crate::emit::Field::Level => level] => "[skill] {skillid} {} -> {}", p.level, level);
        } else {
            crate::emit_f!([crate::emit::Field::SkillId => skillid, crate::emit::Field::Level => level] => "[skill] {skillid} learned L{level}");
        }
    }
}

/// CHATTEXT (0xA4) — a chat message broadcast on the current map.
pub(super) fn handle_chattext(recv: &mut Cursor<'_>, state: &mut BotState) {
    if let Some(chat) = crate::parsers::chat::parse_chattext(recv, &state.entities) {
        crate::emit_f!([crate::emit::Field::Name => chat.sender, crate::emit::Field::Text => chat.text] => "[chat] {}: {}", chat.sender, chat.text);
    }
}

/// MULTICHAT (0x8A) — party/guild/buddy/alliance chat.
pub(super) fn handle_multichat(recv: &mut Cursor<'_>) {
    if let Some(m) = crate::parsers::chat::parse_multichat(recv) {
        let channel = match m.mode {
            0 => "buddy",
            1 => "party",
            2 => "guild",
            3 => "alliance",
            _ => "multichat",
        };
        crate::emit_f!([crate::emit::Field::Name => m.name, crate::emit::Field::Text => m.text] => "[{}] {}: {}", channel, m.name, m.text);
    }
}

/// WHISPER (0x8B) — incoming whisper. Chat-control: if the sender is on the
/// chat-admin whitelist (config.json `chat_admins` / `--chat-admin`) and the
/// text starts with `#`, run a query and reply line-by-line as whispers
/// (shell-style report).
pub(super) async fn handle_whisper(
    recv: &mut Cursor<'_>,
    state: &mut BotState,
    session: &mut Session,
) {
    let Some(w) = crate::parsers::chat::parse_whisper(recv) else {
        return;
    };
    crate::emit_f!([crate::emit::Field::Name => w.sender, crate::emit::Field::Channel => w.channel + 1, crate::emit::Field::Text => w.text] => "[whisper] {} (ch{}): {}", w.sender, w.channel + 1, w.text);

    if state.cfg.chat_admins.is_empty()
        || !state.cfg.chat_admins.iter().any(|n| n == &w.sender)
    {
        return;
    }
    let Some(cmd) = w.text.strip_prefix('#') else {
        return;
    };
    let words: Vec<&str> = cmd.split_whitespace().collect();
    if words.is_empty() {
        return;
    }
    let lines = chat_command(state, session, words[0], &words[1..]).await;
    for line in lines {
        if let Err(e) = session
            .send_packet(crate::packets::chat::whisper(&w.sender, &line))
            .await
        {
            crate::emit_err_f!([crate::emit::Field::Text => e] => "[chatctl] reply error: {e}");
            break;
        }
    }
}

/// Build the line-by-line reply for a `#<query> [arg]` whisper command.
/// Read-only queries report state; drop/dropmeso/pickup execute real actions
/// (whitelisted caller only, results are echoed back).
async fn chat_command(
    state: &mut BotState,
    session: &mut Session,
    cmd: &str,
    args: &[&str],
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    match cmd {
        "help" => {
            out.push("#help #status #player #skills #reactor #where #inv <tab> #drop <id> [qty] #dropmeso <n> #pickup #hunt <on|off|normal|skill|walk|teleport|status> #pose <0-255> #rule <open|close|status> <id> #task <start|stop|status> #town <mapid> <portal> #warp <portal> #say <text>".to_string());
        }
        "status" => {
            let (x, y) = state.position;
            out.push(format!(
                "map={} pos=({x},{y}) hunt={} rhunt={} hp={}/{} mp={}/{}",
                state.mapid,
                state.hunt,
                state.hunt_reactor,
                state.hp,
                state.maxhp,
                state.mp,
                state.maxmp
            ));
        }
        "player" => {
            out.push(format!(
                "L{} {}hp/{}mp exp={} ap={} sp={} meso={}",
                state.level,
                state.hp,
                state.mp,
                state.exp,
                state.ap,
                state.sp,
                state.meso
            ));
        }
        "where" => {
            let (x, y) = state.position;
            out.push(format!("map={} pos=({x},{y}) ground={}", state.mapid, state.ground_y));
        }
        "reactor" => {
            if state.reactors.is_empty() {
                out.push("reactors: none".to_string());
            } else {
                out.push(format!("reactors: {} alive", state.reactors.len()));
                for r in state.reactors.values().take(10) {
                    out.push(format!("  oid={} rid={} st={} ({},{})", r.oid, r.rid, r.state, r.x, r.y));
                }
            }
        }
        "skills" => {
            out.push(format!("sp={} skills={}", state.sp, state.skills.len()));
            for (id, sk) in &state.skills {
                out.push(format!(
                    "  {id}·{} L{}",
                    session.names.skill_name_text(*id),
                    sk.level
                ));
            }
        }
        "inv" => {
            let only = match args.first() {
                Some(&"equip") => Some(1),
                Some(&"use") | Some(&"consume") => Some(2),
                Some(&"setup") => Some(3),
                Some(&"etc") => Some(4),
                Some(&"cash") => Some(5),
                Some(other) => {
                    out.push(format!("bad tab: {other} (equip|use|setup|etc|cash)"));
                    return out;
                }
                None => None,
            };
            let mut entries: Vec<_> = state
                .inventory
                .iter()
                .filter(|((t, _), _)| only.map_or(true, |f| *t == f))
                .map(|((t, s), it)| (*t, *s, it.itemid, it.qty))
                .collect();
            entries.sort_by_key(|(t, s, _, _)| (*t, *s));
            if only.is_none() {
                out.push(format!(
                    "items={} equip={} use={} etc={} meso={}",
                    entries.len(),
                    state.inventory.iter().filter(|((t, _), _)| *t == 1).count(),
                    state.inventory.iter().filter(|((t, _), _)| *t == 2).count(),
                    state.inventory.iter().filter(|((t, _), _)| *t == 4).count(),
                    state.meso
                ));
            } else {
                out.push(format!("tab={} {} items", only.unwrap(), entries.len()));
            }
            let shown = entries.len().min(40);
            for (t, s, itemid, qty) in entries.iter().take(shown) {
                let tabname = match t {
                    1 => "EQUIP",
                    2 => "USE",
                    3 => "SETUP",
                    4 => "ETC",
                    5 => "CASH",
                    _ => "?",
                };
                out.push(format!("  {tabname} {s} {itemid} x{qty}"));
            }
            if entries.len() > shown {
                out.push(format!("  ... {} more", entries.len() - shown));
            }
        }
        "drop" => {
            let itemid: i32 = match args.first().and_then(|s| s.parse().ok()) {
                Some(v) => v,
                None => {
                    out.push("#drop <itemid> [qty]".to_string());
                    return out;
                }
            };
            let qty: i16 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
            let _ = crate::command::run(crate::command::Command::Drop(itemid, qty, None), state, session)
                .await;
            out.push(format!("drop {itemid} x{qty} sent"));
        }
        "dropmeso" => {
            let n: i32 = match args.first().and_then(|s| s.parse().ok()) {
                Some(v) if v > 0 => v,
                _ => {
                    out.push("#dropmeso <amount>".to_string());
                    return out;
                }
            };
            let _ = crate::command::run(crate::command::Command::DropMeso(n), state, session)
                .await;
            out.push(format!("dropmeso {n} sent"));
        }
        "pickup" => {
            let before = state.drops.len();
            let _ = crate::command::run(crate::command::Command::PickupAll, state, session).await;
            out.push(format!("pickup sent for {before} drop(s)"));
        }
        "say" => {
            let text = args.join(" ");
            if text.is_empty() {
                out.push("#say <text>".to_string());
                return out;
            }
            let _ =
                crate::command::run(crate::command::Command::Chat(text.clone()), state, session)
                    .await;
            out.push(format!("said: {text}"));
        }
        "town" => {
            // 0x21 targetid=-1 + portal name: server-side enterPortal (must be within 150px of the portal).
            // If the portal's target map == current map: same-map re-entry, the server replays the whole map.
            let portal = match args.get(1) {
                Some(p) if !p.is_empty() => *p,
                _ => {
                    out.push("#town <mapid> <portal>".to_string());
                    return out;
                }
            };
            let mapid: i32 = args.first().and_then(|s| s.parse().ok()).unwrap_or(0);
            match crate::command::run(
                crate::command::Command::ChangeMap(mapid, portal.to_string()),
                state,
                session,
            )
            .await
            {
                Ok(_) => out.push(format!("town {mapid} portal {portal} sent")),
                Err(e) => out.push(format!("err: {e}")),
            }
        }
        "warp" => {
            // #warp <portal>: enter a portal by name on current map (0x61 CHANGE_MAP_SPECIAL).
            let portal = match args.first() {
                Some(p) if !p.is_empty() => *p,
                _ => {
                    out.push("#warp <portal>".to_string());
                    return out;
                }
            };
            match crate::command::run(
                crate::command::Command::ChangeMapSpecial(portal.to_string()),
                state,
                session,
            )
            .await
            {
                Ok(_) => out.push(format!("warp {portal} sent")),
                Err(e) => out.push(format!("err: {e}")),
            }
        }
        "hunt" => {
            // #hunt on|off|normal|skill|walk|teleport|status: manually control mob hunting.
            // The skill id is hardcoded from config.json hunt.attack_skill (AoE parameters).
            // In-game changes aren't allowed yet; #hunt skill <id> opens once skill data is complete.
            match args.first() {
                None | Some(&"status") => {
                    out.push(format!(
                        "hunt={} mode={} skill={} max_targets={} pickup={} pickuprange={} until={}",
                        state.hunt,
                        if state.cfg.hunt.attack_skill > 0 { "skill" } else { "normal" },
                        state.cfg.hunt.attack_skill,
                        state.cfg.hunt.attack_skill_max_targets,
                        state.cfg.hunt.pickup_enabled,
                        state.cfg.hunt.pickup_range,
                        state.cfg.hunt.until.as_deref().unwrap_or("none")
                    ));
                }
                Some(&"skill") if args.len() > 1 => {
                    out.push(
                        "err: skill id is hard-coded via config.json hunt.attack_skill; not editable in-game"
                            .to_string(),
                    );
                }
                Some(&"on") | Some(&"off") | Some(&"normal") | Some(&"skill") | Some(&"area")
                | Some(&"all") | Some(&"map")
                | Some(&"pickup") | Some(&"pickuprange") | Some(&"atkcd") | Some(&"save") => {
                    let line = format!("hunt {}", args.join(" "));
                    match crate::command::parse(&line) {
                        Ok(Some(c)) => match crate::command::run(c, state, session).await {
                            Ok(_) => out.push(format!("ok: {line}")),
                            Err(e) => out.push(format!("err: {e}")),
                        },
                        Ok(None) => out.push(format!("ok: {line} (no-op)")),
                        Err(e) => out.push(format!("parse error: {e}")),
                    }
                }
                Some(other) => {
                    out.push(format!(
                        "bad: #hunt {other} (on|off|normal|skill|walk|teleport|status)"
                    ));
                }
            }
        }
        "rule" | "task" | "group" => {
            // Pass through the command pipeline (same as local stdin commands): #rule open sellauto,
            // #rule status, #task start sell, #task stop, #task status,
            // #group open autosell, #group status, etc.
            // status/list queries return content lines directly (visible line-by-line in the
            // whisper reply) without running a command.
            let line = format!("{cmd} {}", args.join(" "));
            match crate::command::parse(&line) {
                Ok(Some(crate::command::Command::Rule(crate::command::RuleCmd::Status(filter)))) => {
                    out.extend(crate::command::run::rule_status_lines(state, filter.as_deref()));
                }
                Ok(Some(crate::command::Command::Task(crate::command::TaskCmd::Status))) => {
                    out.extend(crate::command::run::task_status_lines(state));
                }
                Ok(Some(crate::command::Command::Group(crate::command::GroupCmd::Status(filter)))) => {
                    out.extend(crate::command::run::group_status_lines(state, filter.as_deref()));
                }
                Ok(Some(c)) => {
                    // After an action (open/close/start/stop) succeeds, append the current status
                    // lines to the whisper reply — the top-level vs group distinction and results
                    // (an exclusive group was closed, a rule actually opened) are directly visible.
                    let is_group = matches!(&c, crate::command::Command::Group(_));
                    let is_task_start = matches!(
                        &c,
                        crate::command::Command::Task(crate::command::TaskCmd::Start(_))
                    );
                    let id = match &c {
                        crate::command::Command::Rule(
                            crate::command::RuleCmd::Open(id) | crate::command::RuleCmd::Close(id),
                        )
                        | crate::command::Command::Group(
                            crate::command::GroupCmd::Open(id) | crate::command::GroupCmd::Close(id),
                        ) => Some(id.clone()),
                        _ => None,
                    };
                    match crate::command::run(c, state, session).await {
                        Ok(_) => {
                            out.push(format!("ok: {line}"));
                            if let Some(id) = id {
                                // Group switch: list all groups (auto-closed exclusive groups are visible);
                                // rule switch: focus on that single rule.
                                if is_group {
                                    out.extend(crate::command::run::group_status_lines(state, None));
                                } else {
                                    out.extend(crate::command::run::rule_status_lines(state, Some(&id)));
                                }
                            }
                            if is_task_start {
                                if let Some(line) = crate::command::run::task_status_lines(state).first() {
                                    out.push(line.clone());
                                }
                            }
                        }
                        Err(e) => out.push(format!("err: {e}")),
                    }
                }
                Ok(None) => out.push(format!("ok: {line} (no-op)")),
                Err(e) => out.push(format!("parse error: {e}")),
            }
        }
        other => {
            out.push(format!("unknown: #{other} (try #help)"));
        }
    }
    out
}

/// SET_WEEK_EVENT_MESSAGE (0x4E) — yellow GM chat broadcast.
pub(super) fn handle_yellow_chat(recv: &mut Cursor<'_>) {
    if let Some(text) = crate::parsers::chat::parse_yellow_chat(recv) {
        crate::emit_f!([crate::emit::Field::Text => text] => "[notice] {}", text);
    }
}

/// SERVERMESSAGE (0x41) — server notices. Kind mapping follows
/// `MaplePacketCreator.serverNotice` usage: 5/6 = player-facing info popups,
/// 0-4 = scrolling notices, anything else is logged raw.
pub(super) fn handle_server_message(recv: &mut Cursor<'_>) {
    if let Some(m) = crate::parsers::chat::parse_server_message(recv) {
        let tag = match m.kind {
            0..=4 => "notice",
            5 => "info",
            6 => "notice",
            9 => "notice",
            _ => "message",
        };
        if !m.text.is_empty() {
            crate::emit_f!([crate::emit::Field::Id => m.kind, crate::emit::Field::Text => m.text] => "[{} type={}] {}", tag, m.kind, m.text);
        }
    }
}

/// Keep the inventory index in sync from MODIFY_INVENTORY_ITEM (0x20).
/// Negative positions are worn-equip slots (equipped tab); the trailing
/// `addMovement` byte is ignored.
pub(super) fn handle_inventory_update(recv: &mut Cursor<'_>, state: &mut BotState) {
    recv.skip(1);
    let count = recv.read_u8();
    for _ in 0..count {
        if recv.failed() {
            return;
        }
        let mode = recv.read_u8();
        let inv_type = recv.read_u8();
        let pos = recv.read_i16();
        match mode {
            0 => {
                if let Some(info) = parsers::parse_item_info_add(recv) {
                    let item = Item {
                        itemid: info.itemid,
                        qty: info.qty,
                        item_type: info.item_type,
                        stats: info.stats,
                        unique_id: info.unique_id,
                    };
                    if pos < 0 {
                        state.equipped.insert(pos, item);
                    } else {
                        state.inventory.insert((inv_type, pos), item);
                    }
                } else {
                    return;
                }
            }
            1 => {
                let qty = recv.read_i16();
                if pos < 0 {
                    if let Some(it) = state.equipped.get_mut(&pos) {
                        it.qty = qty;
                    }
                } else if let Some(it) = state.inventory.get_mut(&(inv_type, pos)) {
                    it.qty = qty;
                }
            }
            2 => {
                let new_pos = recv.read_i16();
                let src_equipped = pos < 0;
                let dst_equipped = new_pos < 0;
                if src_equipped && dst_equipped {
                    if let Some(it) = state.equipped.remove(&pos) {
                        state.equipped.insert(new_pos, it);
                    }
                } else if src_equipped {
                    // unequip: worn slot -> backpack
                    if let Some(it) = state.equipped.remove(&pos) {
                        state.inventory.insert((inv_type, new_pos), it);
                    }
                } else if dst_equipped {
                    // equip: backpack -> worn slot
                    if let Some(it) = state.inventory.remove(&(inv_type, pos)) {
                        state.equipped.insert(new_pos, it);
                    }
                } else if let Some(it) = state.inventory.remove(&(inv_type, pos)) {
                    state.inventory.insert((inv_type, new_pos), it);
                }
            }
            3 => {
                if pos < 0 {
                    state.equipped.remove(&pos);
                } else {
                    state.inventory.remove(&(inv_type, pos));
                }
            }
            _ => {}
        }
    }
}

/// Track spawned NPCs (0x104).
pub(super) fn handle_spawn_npc(recv: &mut Cursor<'_>, state: &mut BotState) {
    let oid = recv.read_i32();
    let npcid = recv.read_i32();
    if recv.failed() {
        return;
    }
    recv.skip(4); // x, y
    let x = recv.read_i16();
    let y = recv.read_i16();
    state.npcs.insert(oid, Npc { oid, npcid, x, y });
    crate::emit_f!([crate::emit::Field::Oid => oid, crate::emit::Field::NpcId => npcid, crate::emit::Field::Pos => format!("({x},{y})")] => "[npc] oid={oid} npcid={npcid} pos=({x},{y})");
}

/// Track a spawned reactor (0x11E REACTOR_SPAWN).
/// Layout: `int oid + int rid + byte state + pos + byte facing + string name`.
pub(super) fn handle_spawn_reactor(recv: &mut Cursor<'_>, state: &mut BotState) {
    let oid = recv.read_i32();
    let rid = recv.read_i32();
    let st = recv.read_u8();
    let (x, y) = recv.read_point();
    if recv.failed() {
        return;
    }
    recv.skip(1); // facing
    let _name = recv.read_string_gb();
    state.reactors.insert(oid, Reactor { oid, rid, state: st, x, y });
}

/// Update a reactor's state (0x11C REACTOR_HIT).
/// Layout: `int oid + byte state + pos + short stance + byte 0 + byte 4`.
pub(super) fn handle_reactor_hit(recv: &mut Cursor<'_>, state: &mut BotState) {
    let oid = recv.read_i32();
    let st = recv.read_u8();
    if recv.failed() {
        return;
    }
    if let Some(r) = state.reactors.get_mut(&oid) {
        r.state = st;
        r.x = recv.read_i16();
        r.y = recv.read_i16();
    }
}

/// Remove a destroyed reactor (0x11F REACTOR_DESTROY).
/// Layout: `int oid + byte state + pos`.
pub(super) fn handle_reactor_destroy(recv: &mut Cursor<'_>, state: &mut BotState) {
    let oid = recv.read_i32();
    if recv.failed() {
        return;
    }
    state.reactors.remove(&oid);
}

/// Parse the keymap (0x16F KEYMAP, sent on login).
/// Layout: `byte 0 + 90 × (byte type + int action)`. Key indices are implicit
/// (0-89); empty bindings are (0, 0).
pub(super) fn handle_keymap(recv: &mut Cursor<'_>, state: &mut BotState) {
    recv.skip(1); // version byte
    let mut keymap = Vec::with_capacity(90);
    for _ in 0..90 {
        let ty = recv.read_u8();
        let action = recv.read_i32();
        if recv.failed() {
            return;
        }
        keymap.push((ty, action));
    }
    state.keymap = keymap;
    let bound = state.keymap.iter().filter(|(t, _)| *t != 0).count();
    crate::emit_f!([crate::emit::Field::Count => bound] => "[keymap] {} keys bound", bound);
}

/// Remove a despawned NPC (0x105).
pub(super) fn handle_remove_npc(recv: &mut Cursor<'_>, state: &mut BotState) {
    let oid = recv.read_i32();
    state.npcs.remove(&oid);
}

/// Handle NPC_TALK response (0x145) — server sends dialog content.
/// Format: `byte 4 + int npcId + byte msgType + byte speaker + string talk + [endBytes]`
pub(super) fn handle_npc_talk(recv: &mut Cursor<'_>, state: &mut BotState) {
    recv.skip(1); // header byte (always 4 for sendSimple)
    let npcid = recv.read_i32();
    let msg_type = recv.read_u8();
    state.last_npc_msg_type = msg_type;
    state.dialog_open = true; // Server sent a new dialog page -> the dialog is open
    let _speaker = recv.read_u8();
    let raw_msg = recv.read_string_gb();
    // Strip format tags (#fEffect/...# #e#r#k#n etc.) and truncate, so overly long tagged text
    // doesn't wreck the log/TUI layout; menu options #L<id># are kept.
    let msg = parsers::clean_npc_text(&raw_msg, 400);
    crate::emit_f!([crate::emit::Field::NpcId => npcid, crate::emit::Field::Id => msg_type, crate::emit::Field::Text => msg] => "[npc_talk] npcid={npcid} type={msg_type} msg=\"{msg}\"");
    // Refresh the option picker state for the TUI: a menu (type 4) fills the
    // options; any other dialog page clears them. The seq bump marks "new
    // dialog content" so the TUI re-evaluates its popup (menu -> show, no
    // menu -> keep hidden) without polling the raw options.
    state.npc_options_seq = state.npc_options_seq.wrapping_add(1);
    state.npc_options.clear();
    if msg_type == 4 {
        // sendSimple: option format `#L<id>#<text>#l` — the id is the argument to `npc reply <id>`.
        // Parse from the RAW message: clean_npc_text truncates at 400 chars for
        // display, and long menus (e.g. the auction hub with 20+ items) would
        // silently lose every option past the cutoff.
        let options = parsers::parse_simple_options(&raw_msg);
        // Clean once, store for the TUI picker and emit the same rows (keeps the
        // log order: count line first, then one line per option).
        state.npc_options = options
            .into_iter()
            .map(|(id, label)| (id, parsers::clean_npc_text(label.trim(), 24)))
            .collect();
        crate::emit_f!([crate::emit::Field::Count => state.npc_options.len()] => "[npc_talk] {} options", state.npc_options.len());
        for (id, label) in &state.npc_options {
            crate::emit_f!([crate::emit::Field::Selection => id, crate::emit::Field::Text => label] => "  id={id} label={label}");
        }
    }
}

/// Clear the NPC menu state: no options left, and bump the seq so consumers
/// (TUI option picker) see a fresh "no menu" state instead of stale options.
/// Called wherever the server closes dialogs (map change / reenter / reset).
pub(crate) fn clear_npc_menu(state: &mut BotState) {
    state.npc_options.clear();
    state.npc_options_seq = state.npc_options_seq.wrapping_add(1);
}

/// Clear the NPC shop state: no items left, and bump the seq so the TUI shop
/// picker drops its popup. Called wherever the shop closes (leave command /
/// map change / reenter / reset) — every picker refresh is driven by a fresh
/// 0x146 (handle_open_npc_shop bumps seq again).
pub(crate) fn clear_shop(state: &mut BotState) {
    state.shop_items.clear();
    state.shop_seq = state.shop_seq.wrapping_add(1);
}

/// Handle OPEN_NPC_SHOP (0x146) — server opens the shop interface.
pub(super) fn handle_open_npc_shop(recv: &mut Cursor<'_>, state: &mut BotState) {
    let npcid = recv.read_i32();
    let count = recv.read_u16();
    let mut items = Vec::new();
    for _ in 0..count {
        let itemid = recv.read_i32();
        let price = recv.read_i32();
        // After price, regular items have short(1) + short(buyable).
        // Rechargeable items (throwing stars, bullets) have byte[6] zeros +
        // short(rechargePrice) + short(slotMax). Detect by reading the first
        // short: 1 = regular, 0 = rechargeable.
        let tag = recv.read_i16();
        if tag == 1 {
            let qty = recv.read_i16();
            items.push(ShopItem { itemid, price, qty });
        } else {
            // tag was 0, skip remaining 4 zero bytes + recharge price
            recv.skip(4);
            recv.skip(2); // recharge price
            let qty = recv.read_i16(); // slot max
            items.push(ShopItem { itemid, price, qty });
        }
        if recv.failed() {
            break;
        }
    }
    state.shop_open = true;
    state.shop_items = items.clone();
    state.shop_seq = state.shop_seq.wrapping_add(1);
    // Opening a shop ends the NPC conversation: clear dialog + stale menu options so
    // consumers of `dialog`/`npc_options` (task wait predicates, TUI option picker)
    // don't act on a conversation the server has already replaced with the shop.
    state.dialog_open = false;
    clear_npc_menu(state);
    crate::emit_f!([crate::emit::Field::NpcId => npcid, crate::emit::Field::Count => items.len()] => "[shop] open npcid={npcid} items={}", items.len());
    for it in items.iter() {
        // Keep the [shop] tag: untagged lines would be categorized as Cmd and the TUI
        // couldn't localize them
        crate::emit_f!([crate::emit::Field::ItemId => it.itemid, crate::emit::Field::Price => it.price, crate::emit::Field::Qty => it.qty] => "[shop]   itemid={} price={} qty={}", it.itemid, it.price, it.qty);
    }
}

/// Handle CONFIRM_SHOP_TRANSACTION (0x147).
pub(super) fn handle_confirm_shop(recv: &mut Cursor<'_>, _state: &mut BotState) {
    if recv.remaining() >= 1 {
        let result = recv.read_u8();
        // result=0 = success: the reply carries itemid/qty (MapleShop.buy success layout:
        // byte mode + int itemid + short qty + int price).
        if result == 0 && recv.remaining() >= 10 {
            let itemid = recv.read_i32();
            let qty = recv.read_i16();
            let price = recv.read_i32();
            crate::emit_f!([crate::emit::Field::ItemId => itemid, crate::emit::Field::Qty => qty, crate::emit::Field::Meso => price] => "[shop] ok buy {itemid} x{qty} ({price} meso)");
        } else {
            crate::emit_f!([crate::emit::Field::Status => result] => "[shop] txn result={result}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Entity, ReenterCtx};

    /// SET_FIELD (0x81) warpToMap variant body:
    /// `int channel + byte 0 + byte 3 + skip(3) + int mapid + byte portal`
    fn set_field_body(mapid: i32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0i32.to_le_bytes()); // channel
        b.push(0); // mode1
        b.push(3); // mode2 = warpToMap
        b.extend_from_slice(&[0, 0, 0]); // skipped
        b.extend_from_slice(&mapid.to_le_bytes());
        b.push(0); // portalid
        b
    }

    /// Build a drop packet body: mod, oid, isMeso, itemId, owner, dropType,
    /// x, y, extra.
    fn drop_body(oid: i32, is_meso: bool, itemid: i32) -> Vec<u8> {
        let mut b = Vec::new();
        b.push(0); // mod
        b.extend_from_slice(&oid.to_le_bytes());
        b.push(u8::from(is_meso));
        b.extend_from_slice(&itemid.to_le_bytes());
        b.extend_from_slice(&0i32.to_le_bytes()); // owner
        b.push(1); // dropType
        b.extend_from_slice(&100i16.to_le_bytes()); // x
        b.extend_from_slice(&200i16.to_le_bytes()); // y
        b.extend_from_slice(&0i32.to_le_bytes()); // extra
        b
    }

    #[test]
    fn drop_filter_deny_keeps_item_out_of_state() {
        let mut state = BotState::default();
        state.cfg.hunt.pickup_filter_mode = "deny".into();
        state.cfg.hunt.pickup_deny = vec![4000000];
        let body = drop_body(1, false, 4000000);
        let mut c = Cursor::new(&body);
        handle_drop(&mut c, &mut state);
        assert!(state.drops.is_empty(), "denied item must not be tracked");
        // non-denied item still tracked
        let body = drop_body(2, false, 2000000);
        let mut c = Cursor::new(&body);
        handle_drop(&mut c, &mut state);
        assert_eq!(state.drops.len(), 1);
        assert_eq!(state.drops[&2].itemid, 2000000);
    }

    #[test]
    fn drop_filter_allow_keeps_only_whitelisted() {
        let mut state = BotState::default();
        state.cfg.hunt.pickup_filter_mode = "allow".into();
        state.cfg.hunt.pickup_allow = vec![4000006];
        let body = drop_body(1, false, 4000000);
        let mut c = Cursor::new(&body);
        handle_drop(&mut c, &mut state);
        assert!(state.drops.is_empty());
        let body = drop_body(2, false, 4000006);
        let mut c = Cursor::new(&body);
        handle_drop(&mut c, &mut state);
        assert_eq!(state.drops.len(), 1);
    }

    #[test]
    fn meso_always_bypasses_pickup_filter() {
        let mut state = BotState::default();
        state.cfg.hunt.pickup_allow = vec![4000006]; // strict whitelist
        let body = drop_body(1, true, 500); // 500 meso
        let mut c = Cursor::new(&body);
        handle_drop(&mut c, &mut state);
        assert_eq!(state.drops.len(), 1, "meso must pass any filter");
        assert!(state.drops[&1].is_meso);
        assert_eq!(state.drops[&1].itemid, 500);
    }

    #[test]
    fn set_field_warp_clears_old_map_entities() {
        let mut state = BotState::default();
        state.my_cid = 7;
        state.mapid = 100000000;
        state.ground_y = 214;
        state.hunt_target = Some(123);
        state.drops.insert(1, Drop::default());
        state.npcs.insert(2, Npc { oid: 2, npcid: 1011100, x: 0, y: 0 });
        state.reactors.insert(3, Reactor { oid: 3, rid: 1102000, state: 0, x: 0, y: 0 });
        // Leftovers from the old map: a mob + another player + ourselves
        state.entities.insert(
            100,
            Entity {
                oid: 100,
                kind: EntityKind::Mob,
                ..Entity::default()
            },
        );
        state.entities.insert(
            200,
            Entity {
                oid: 200,
                kind: EntityKind::Player,
                charname: "someone".into(),
                ..Entity::default()
            },
        );
        state.entities.insert(
            7,
            Entity {
                oid: 7,
                kind: EntityKind::Player,
                charname: "me".into(),
                ..Entity::default()
            },
        );

        let body = set_field_body(104040000);
        let mut recv = Cursor::new(&body);
        handle_set_field(&mut recv, &mut state, &crate::names::default_table());

        assert!(!recv.failed());
        assert_eq!(state.mapid, 104040000);
        assert_eq!(state.phase, crate::state::Phase::InGame);
        // Only our own player entity remains
        assert_eq!(state.entities.len(), 1);
        assert!(state.entities.contains_key(&7));
        assert_eq!(state.ground_y, 0);
        assert_eq!(state.hunt_target, None);
        assert!(state.npcs.is_empty());
        assert!(state.reactors.is_empty());
        assert!(state.drops.is_empty());
    }

    #[test]
    fn set_field_warp_completes_reenter_and_restores_hunt() {
        let mut state = BotState::default();
        state.my_cid = 7;
        state.hunt = false;
        state.reenter = Some(ReenterCtx {
            restore_hunt: true,
            mapid: 104040000,
            leave_sent: true,
            started: std::time::Instant::now(),
        });

        let body = set_field_body(104040000);
        let mut recv = Cursor::new(&body);
        handle_set_field(&mut recv, &mut state, &crate::names::default_table());

        assert!(state.reenter.is_none());
        assert!(state.hunt);
    }

    #[test]
    fn set_field_warp_on_wrong_map_drops_reenter_without_restoring_hunt() {
        let mut state = BotState::default();
        state.my_cid = 7;
        state.hunt = false;
        state.reenter = Some(ReenterCtx {
            restore_hunt: true,
            mapid: 104040000,
            leave_sent: true,
            started: std::time::Instant::now(),
        });

        let body = set_field_body(100000000);
        let mut recv = Cursor::new(&body);
        handle_set_field(&mut recv, &mut state, &crate::names::default_table());

        assert!(state.reenter.is_none());
        assert!(!state.hunt);
    }

    /// OPEN_NPC_SHOP (0x146) body with one regular item:
    /// `int npcid + short count + int itemid + int price + short 1 + short qty`
    fn open_shop_body() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&1011100i32.to_le_bytes()); // npcid
        b.extend_from_slice(&1u16.to_le_bytes()); // count
        b.extend_from_slice(&2000000i32.to_le_bytes()); // itemid
        b.extend_from_slice(&50i32.to_le_bytes()); // price
        b.extend_from_slice(&1i16.to_le_bytes()); // tag = regular
        b.extend_from_slice(&100i16.to_le_bytes()); // buyable qty
        b
    }

    #[test]
    fn shop_open_closes_dialog_and_clears_stale_menu() {
        // 开店前对话还开着、菜单选项残留 (开店流程的最后一页 sendSimple):
        // 0x146 到达 = 服务端用商店替换了对话 → dialog 关 + 菜单清空,
        // TUI 选项弹框/任务谓词不会基于已结束的对话误动作。
        let mut state = BotState::default();
        state.dialog_open = true;
        state.npc_options = vec![(3, "商店".into())];

        let body = open_shop_body();
        let mut recv = Cursor::new(&body);
        handle_open_npc_shop(&mut recv, &mut state);

        assert!(!recv.failed());
        assert!(state.shop_open);
        assert_eq!(state.shop_items.len(), 1);
        assert_eq!(state.shop_items[0].itemid, 2000000);
        assert!(!state.dialog_open, "开店应关闭对话状态");
        assert!(state.npc_options.is_empty(), "开店应清空残留菜单选项");
    }
}




