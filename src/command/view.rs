//! `view <object>` — all read-only inspection. Never sends packets.
//!
//! All output is collected into one multi-line block and emitted as a single
//! event, so UIs render the table as one unit (never interleaved).

use crate::command::{View, equip_slot_name, equip_stats_line, reported_damage};
use crate::names::NameTable;
use crate::session::Session;
use crate::state::{BotState, EntityKind};

/// Name suffix: returns " (name)" when known, empty otherwise.
///
/// Takes the session's own name table so two sessions on different `data_dir`
/// values render their own names (never a shared process-global table).
fn name_suffix(id: i32, names: &NameTable, f: fn(&NameTable, i32) -> String) -> String {
    let n = f(names, id);
    if n == "未知" {
        String::new()
    } else {
        format!(" ({n})")
    }
}

/// Field accessors so `name_suffix` can stay a plain `fn` pointer.
fn map_text(n: &NameTable, id: i32) -> String {
    n.map_name_text(id)
}
fn mob_text(n: &NameTable, id: i32) -> String {
    n.mob_name_text(id)
}
fn npc_text(n: &NameTable, id: i32) -> String {
    n.npc_name_text(id)
}
fn item_text(n: &NameTable, id: i32) -> String {
    n.item_name_text(id)
}
fn skill_text(n: &NameTable, id: i32) -> String {
    n.skill_name_text(id)
}

pub(super) async fn run_view(
    view: View,
    state: &mut BotState,
    session: &mut Session,
) -> Result<(), String> {
    let names = session.names.clone();
    let names = names.as_ref();
    let mut lines: Vec<String> = Vec::new();
    match view {
        View::Status => {
            let world = state.selected_world;
            let map = state.mapid;
            let (x, y) = state.position;
            let chars = state.characters.len();
            let phase = format!("{:?}", state.phase);
            lines.push(format!(
                "[view] phase={phase} world={world} map={map}{} pos=({x},{y}) ground={} mobs={} chars={} my_cid={}",
                name_suffix(map, names, map_text),
                state.ground_y,
                state.entities.len(),
                chars,
                state.my_cid
            ));
            lines.push(format!(
                "[view] hp={}/{} mp={}/{} party={} target={:?}",
                state.hp,
                state.maxhp,
                state.mp,
                state.maxmp,
                state.party.as_ref().map(|p| p.partyid).is_some(),
                state.attack_target
            ));
            lines.push(format!(
                "[view] ap={} hunt={} skill={} rhunt={} dmg={}",
                state.ap,
                state.hunt,
                state.cfg.hunt.attack_skill,
                state.hunt_reactor,
                reported_damage(state)
            ));
        }
        View::Mobs => {
            let mobs: Vec<_> = state
                .entities
                .values()
                .filter(|e| e.kind == EntityKind::Mob)
                .collect();
            lines.push(format!("[view mobs] {} tracked", mobs.len()));
            for m in mobs.iter().take(20) {
                lines.push(format!(
                    "  oid={} mobid={}{} pos=({},{}) fh={}",
                    m.oid,
                    m.mobid,
                    name_suffix(m.mobid, names, mob_text),
                    m.x,
                    m.y,
                    m.fh
                ));
            }
        }
        View::Players => {
            let players: Vec<_> = state
                .entities
                .values()
                .filter(|e| e.kind == EntityKind::Player)
                .collect();
            lines.push(format!("[view nearby] {} players tracked", players.len()));
            for p in players.iter().take(20) {
                lines.push(format!("  cid={} name={} pos=({},{})", p.oid, p.charname, p.x, p.y));
            }
        }
        View::Npcs => {
            lines.push(format!("[view npcs] {} tracked", state.npcs.len()));
            for n in state.npcs.values().take(20) {
                lines.push(format!(
                    "  oid={} npcid={}{} pos=({},{})",
                    n.oid,
                    n.npcid,
                    name_suffix(n.npcid, names, npc_text),
                    n.x,
                    n.y
                ));
            }
        }
        View::Drops => {
            lines.push(format!("[view drops] {} tracked", state.drops.len()));
            for d in state.drops.values().take(20) {
                // itemid stays visible (`id·名称`); meso shows the amount.
                let what = if d.is_meso {
                    format!("meso {}", d.itemid)
                } else {
                    names.item_name(d.itemid)
                };
                lines.push(format!(
                    "  oid={} {} pos=({},{})",
                    d.oid,
                    what,
                    d.x,
                    d.y
                ));
            }
        }
        View::Inventory => push_view_inventory(&mut lines, state, None, names),
        View::InventoryTab(tab) => push_view_inventory(&mut lines, state, Some(tab), names),
        View::Skills => {
            lines.push(format!("[view skills] sp available: {}", state.sp));
            if state.skills.is_empty() {
                lines.push("[view skills] no skills learned".to_string());
            } else {
                lines.push(format!("[view skills] learned ({}):", state.skills.len()));
                for (id, sk) in &state.skills {
                    let ml = if sk.masterlevel > 0 {
                        format!("/{}", sk.masterlevel)
                    } else {
                        String::new()
                    };
                    lines.push(format!(
                        "  {id}{} L{}{}",
                        name_suffix(*id, names, skill_text),
                        sk.level,
                        ml
                    ));
                }
            }
        }
        View::Equips => {
            // Worn equipment list: summary line + per-item (slot/item/stats)
            if state.equipped.is_empty() {
                lines.push("[view equiped] nothing worn".to_string());
            } else {
                lines.push(format!(
                    "[view equiped] {} worn",
                    state.equipped.len()
                ));
                for (slot, it) in &state.equipped {
                    lines.push(format!(
                        "  {} itemid={}{} {}",
                        equip_slot_name(*slot),
                        it.itemid,
                        name_suffix(it.itemid, names, item_text),
                        match &it.stats {
                            Some(st) => equip_stats_line(it.itemid, st),
                            None => "(no stats)".to_string(),
                        }
                    ));
                }
            }
        }
        View::EqpInfo(itemid) => {
            // Equip info query: list every item matching itemid in the backpack and worn gear,
            // tagging each entry's source (backpack slot / worn body slot).
            let backpacks: Vec<(i16, i32)> = state
                .inventory
                .iter()
                .filter(|((tab, _), it)| *tab == 1 && it.itemid == itemid)
                .map(|((_, slot), it)| (*slot, it.itemid))
                .collect();
            let worn: Vec<(i16, i32)> = state
                .equipped
                .iter()
                .filter(|(_, it)| it.itemid == itemid)
                .map(|(slot, it)| (*slot, it.itemid))
                .collect();
            if backpacks.is_empty() && worn.is_empty() {
                lines.push(format!("[view eqpinfo] item {itemid} not found"));
            } else {
            lines.push(format!(
                "[view eqpinfo] item {itemid}: {} found ({} in backpack, {} worn)",
                backpacks.len() + worn.len(),
                backpacks.len(),
                worn.len()
            ));
            let stats_of = |itemid: i32| match &state.inventory.values().find(|it| it.itemid == itemid).and_then(|it| it.stats.as_ref()) {
                Some(st) => equip_stats_line(itemid, st),
                None => {
                    match &state.equipped.values().find(|it| it.itemid == itemid).and_then(|it| it.stats.as_ref()) {
                        Some(st) => equip_stats_line(itemid, st),
                        None => "(no stats)".to_string(),
                    }
                }
            };
            for (slot, id) in &backpacks {
                lines.push(format!(
                    "  backpack slot={slot} itemid={id}{} {}",
                    name_suffix(*id, names, item_text),
                    stats_of(*id)
                ));
            }
            for (slot, id) in &worn {
                lines.push(format!(
                    "  worn {} itemid={id}{} {}",
                    equip_slot_name(*slot),
                    name_suffix(*id, names, item_text),
                    stats_of(*id)
                ));
            }
            }
        }
        View::Party => match &state.party {
            Some(p) => {
                lines.push(format!(
                    "[view party] id={} leader={} members={}",
                    p.partyid,
                    p.leader_id,
                    p.members.len()
                ));
                for m in p.members.values() {
                    lines.push(format!(
                        "  id={} name={} job={} lvl={} chan={} map={}{}",
                        m.id,
                        m.name,
                        m.job,
                        m.level,
                        m.channel,
                        m.mapid,
                        name_suffix(m.mapid, names, map_text)
                    ));
                }
            }
            None => lines.push("[view party] not in a party".to_string()),
        },
        View::Trade => {
            let t = &state.trade;
            if t.active {
                lines.push(format!(
                    "[view trade] open with '{}' slot={} partner_meso={} partner_items={} locked={}/{}",
                    t.partner,
                    t.my_slot,
                    t.meso_partner,
                    t.items_partner.len(),
                    if t.locked { "yes" } else { "no" },
                    if t.partner_locked { "yes" } else { "no" }
                ));
                for (itemid, qty) in &t.items_partner {
                    lines.push(format!("  item {itemid} x{}", (*qty).max(1)));
                }
            } else if let Some(inv) = &t.invite {
                lines.push(format!(
                    "[view trade] pending invite from '{}' ({}s ago)",
                    inv.name,
                    inv.time.elapsed().as_secs()
                ));
            } else {
                lines.push("[view trade] no trade active".to_string());
            }
        }
        View::Attr => {
            lines.push(format!(
                "[view attr] hp:{}/{} mp:{}/{} exp:{} rem:{} level:{}",
                state.hp,
                state.maxhp,
                state.mp,
                state.maxmp,
                state.exp,
                crate::exptable::exp_remain(state.level as i32, state.exp as i64),
                state.level
            ));
            lines.push(format!(
                "[view attr] str:{} dex:{} int:{} luk:{} ap:{} sp:{} meso:{}",
                state.str, state.dex, state.int, state.luk, state.ap, state.sp, state.meso
            ));
        }
        View::Reactor => {
            if state.reactors.is_empty() {
                lines.push("[view reactor] none on map".to_string());
            }
            for r in state.reactors.values() {
                lines.push(format!(
                    "[view reactor] oid={} rid={} state={} pos=({},{})",
                    r.oid, r.rid, r.state, r.x, r.y
                ));
            }
        }
        View::Keymap => {
            if state.keymap.is_empty() {
                lines.push("[view keymap] not received yet".to_string());
            } else {
                for (i, (ty, action)) in state.keymap.iter().enumerate() {
                    if *ty != 0 {
                        let kind = match ty {
                            1 => "skill",
                            2 => "item",
                            3 => "chat",
                            4 => "ui",
                            5 => "emote",
                            6 => "macro",
                            _ => "?",
                        };
                        lines.push(format!("[view keymap] key={i} type={ty}({kind}) action={action}"));
                    }
                }
            }
        }
        View::CashShop => {
            if state.phase != crate::state::Phase::CashShop {
                lines.push(format!("[view cashshop] not in cash shop (phase={:?})", state.phase));
            } else {
                lines.push(format!(
                    "[view cashshop] nx={} points={} items={} sale={}",
                    state.cs_nx,
                    state.cs_points,
                    state.cs_inventory.len(),
                    state.cs_items.len()
                ));
                for it in &state.cs_inventory {
                    lines.push(format!(
                        "[view cashshop]   unique={} item={}{} sn={} qty={}",
                        it.unique_id,
                        it.itemid,
                        name_suffix(it.itemid, names, item_text),
                        it.sn,
                        it.qty
                    ));
                }
            }
        }
        View::Portals => {
            let mid = state.mapid;
            let portals = names.map_portals(mid);
            if portals.is_empty() {
                lines.push(format!("[view portals] no portals for map {mid}"));
            } else {
                let map_name = names.map_name_text(mid);
                lines.push(format!("[view portals] map {mid}{map_name} — {} portal(s):", portals.len()));
                for (i, p) in portals.iter().enumerate() {
                    let target_name = names.map_name_text(p.target_map);
                    lines.push(format!(
                        "  {:>3}  {:<10} → {:<7}{}",
                        i + 1,
                        p.name,
                        p.target_map,
                        target_name,
                    ));
                }
            }
        }
    }
    if !lines.is_empty() {
        crate::emit!("{}", lines.join("\n"));
    }
    Ok(())
}

/// List the inventory, optionally filtered to one tab (1=EQUIP 2=USE 3=SETUP
/// 4=ETC 5=CASH).
fn push_view_inventory(
    lines: &mut Vec<String>,
    state: &BotState,
    only_tab: Option<u8>,
    names: &NameTable,
) {
    let mut entries: Vec<_> = state
        .inventory
        .iter()
        .filter(|((tab, _), _)| only_tab.map_or(true, |t| *tab == t))
        .map(|((tab, slot), it)| (*tab, *slot, *it))
        .collect();
    entries.sort_by_key(|(tab, slot, _)| (*tab, *slot));
    if only_tab.is_none() {
        lines.push(format!(
            "[view inventory] {} items (equip={} use={} etc={} meso={})",
            entries.len(),
            state
                .inventory
                .iter()
                .filter(|((t, _), _)| *t == 1)
                .count(),
            state
                .inventory
                .iter()
                .filter(|((t, _), _)| *t == 2)
                .count(),
            state
                .inventory
                .iter()
                .filter(|((t, _), _)| *t == 4)
                .count(),
            state.meso
        ));
    } else {
        lines.push(format!("[view inventory] tab={} {} items", only_tab.unwrap(), entries.len()));
    }
    for (tab, slot, it) in entries.iter().take(40) {
        let tabname = match tab {
            1 => "EQUIP",
            2 => "USE",
            3 => "SETUP",
            4 => "ETC",
            5 => "CASH",
            _ => "?",
        };
        lines.push(format!(
            "  {tabname} slot={slot} itemid={}{} qty={}",
            it.itemid,
            name_suffix(it.itemid, names, item_text),
            it.qty
        ));
    }
}
