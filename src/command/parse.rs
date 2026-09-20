use crate::command::{ApCmd, ApStat, Command, EquipCmd, GroupCmd, HuntFilterCmd, HuntMode, LoginCmd, PartyCmd, ReactorCmd, ReconnectCmd, RuleCmd, SellTab, SkillCmd, TaskCmd, TradeCmd, View};

fn parse_bool(s: &str) -> Result<bool, String> {
    match s.to_ascii_lowercase().as_str() {
        "on" | "1" | "true" => Ok(true),
        "off" | "0" | "false" => Ok(false),
        other => Err(format!("bad value: {other} (use on/off)")),
    }
}

/// Parse `view <object>`: `view` (status), `view attr|mobs|players|drops|
/// inventory|skills|equiped|eqpinfo`, `view eqpinfo <itemid>`.
fn parse_view(rest: Vec<&str>) -> Result<Option<Command>, String> {
    let obj = rest.first().map(|s| s.to_ascii_lowercase());
    let view = match obj.as_deref() {
        None | Some("status") => View::Status,
        // view player merged into attr (identical content; player kept as an alias)
        Some("player") => View::Attr,
        Some("mobs") => View::Mobs,
        Some("players" | "nearby") => View::Players,
        Some("npcs") => View::Npcs,
        Some("drops") => View::Drops,
        Some("inventory" | "inv") => {
            if rest.len() >= 2 {
                let tab = match rest[1] {
                    "equip" => 1u8,
                    "use" | "consume" => 2,
                    "setup" => 3,
                    "etc" => 4,
                    "cash" => 5,
                    other => {
                        return Err(format!(
                            "bad inventory tab: {other} (equip|use|setup|etc|cash)"
                        ));
                    }
                };
                return Ok(Some(Command::View(View::InventoryTab(tab))));
            }
            View::Inventory
        }
        Some("skills") => View::Skills,
        Some("equiped" | "equipped") => View::Equips,
        Some("eqpinfo") => {
            let id: i32 = rest
                .get(1)
                .ok_or("usage: view eqpinfo <itemid>")?
                .parse()
                .map_err(|_| "bad itemid (usage: view eqpinfo <itemid>)")?;
            return Ok(Some(Command::View(View::EqpInfo(id))));
        }
        Some("party") => View::Party,
        Some("trade") => View::Trade,
        Some("attr" | "attributes") => View::Attr,
        Some("reactor") => View::Reactor,
        Some("keymap") => View::Keymap,
        Some("cs" | "cashshop") => View::CashShop,
        Some("portals") => {
            let map_id = rest.get(1).and_then(|s| s.parse::<i32>().ok());
            return Ok(Some(Command::PortalList(map_id)));
        }
        Some(other) => {
            return Err(format!(
                "bad view object: {other} (status|attr|mobs|players|npcs|drops|inventory|skills|equiped|eqpinfo <itemid>|party|trade|reactor|keymap|cashshop|portals [mapid])"
            ));
        }
    };
    Ok(Some(Command::View(view)))
}

pub fn parse(line: &str) -> Result<Option<Command>, String> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }

    let mut parts = line.split_whitespace();
    let cmd = parts.next().unwrap_or("").to_ascii_lowercase();
    let rest: Vec<&str> = parts.collect();

    match cmd.as_str() {
    "move" => {
        if rest.is_empty() {
            return Err("usage: move <x> [y] | move <portal_name>".into());
        }
        // Try parsing as x coordinate first
        if let Ok(x) = rest[0].parse::<i16>() {
            let y = match rest.get(1) {
                Some(s) => s.parse().map_err(|_| format!("bad y: {s}"))?,
                None => 0,
            };
            Ok(Some(Command::Move(x, y)))
        } else {
            // Not a number — treat as portal name
            Ok(Some(Command::MovePortal(rest[0].to_string())))
        }
    }
        "chat" | "say" | "c" => Ok(Some(Command::Chat(rest.join(" ")))),
        "gather" => match rest.get(0) {
            Some(s) if *s == "on" => Ok(Some(Command::Gather(true))),
            Some(s) if *s == "off" => Ok(Some(Command::Gather(false))),
            Some(s) if *s == "step" => {
                let n: i32 = rest
                    .get(1)
                    .ok_or("usage: gather step <px> (0 = pull to the player)")?
                    .parse()
                    .map_err(|_| "bad gather step")?;
                if n < 0 {
                    return Err("gather step must be >= 0".into());
                }
                Ok(Some(Command::GatherStep(n)))
            }
            Some(s) if *s == "interval" => {
                let n: u64 = rest
                    .get(1)
                    .ok_or("usage: gather interval <ms> (0 = every tick)")?
                    .parse()
                    .map_err(|_| "bad gather interval")?;
                Ok(Some(Command::GatherInterval(n)))
            }
            Some(s) if *s == "max" => {
                let n: usize = rest
                    .get(1)
                    .ok_or("usage: gather max <n> (0 = unlimited)")?
                    .parse()
                    .map_err(|_| "bad gather max")?;
                Ok(Some(Command::GatherMax(n)))
            }
            Some(s) if *s == "controller" => {
                let on = match rest.get(1).map(|s| &**s) {
                    Some("on") => true,
                    Some("off") => false,
                    _ => return Err("usage: gather controller on|off (on = only pull mobs we hold control of)".into()),
                };
                Ok(Some(Command::GatherController(on)))
            }            _ => Err("usage: gather on|off|step <px>|interval <ms>|max <n>|controller <on|off> (pull off-range mobs to the player; independent of hunt)".into()),
        },
        "view" | "v" => parse_view(rest),
        // `bag` = the composite backpack UI command (TUI-local popup with
        // sell/equip actions); NOT a pure `view` duplicate, keep it. In the
        // headless CLI it falls back to showing the inventory.
        "bag" => Ok(Some(Command::View(View::Inventory))),
        "pickup" => {
            if rest.is_empty() {
                return Err("usage: pickup all".into());
            }
            match rest[0] {
                "all" => Ok(Some(Command::PickupAll)),
                _ => Err("usage: pickup all".into()),
            }
        }
        "sleep" => {
            let s: u64 = rest
                .first()
                .ok_or("usage: sleep <seconds>")?
                .parse()
                .map_err(|_| "bad seconds")?;
            Ok(Some(Command::Sleep(s)))
        }
        "wait" => {
            if rest.is_empty() {
                return Err("usage: wait <predicate> [timeout_ms] (only inlined as a task step inside a rule)".into());
            }
            let ms = match rest.last().and_then(|t| t.parse::<u64>().ok()) {
                Some(n) if rest.len() > 1 => Some(n),
                _ => None,
            };
            let pred = if ms.is_some() && rest.len() > 1 {
                rest[..rest.len() - 1].join(" ")
            } else {
                rest.join(" ")
            };
            Ok(Some(Command::Wait(pred, ms)))
        }
        "while" => {
            if rest.is_empty() {
                return Err("usage: while <predicate> do <command> (only inlined inside a rule / as a task step)".into());
            }
            // 用第一个 " do " 作分隔:前面是 pred(可含空格,如 mob>0 or drop>0),
            // 后面是单条命令(可含空格,如 task run hunt_cycle)。
            let joined = rest.join(" ");
            match joined.find(" do ") {
                Some(i) => {
                    let pred = joined[..i].trim().to_string();
                    let cmd = joined[i + 4..].trim().to_string();
                    if pred.is_empty() || cmd.is_empty() {
                        return Err("usage: while <predicate> do <command>".into());
                    }
                    Ok(Some(Command::While(pred, cmd)))
                }
                None => Err("usage: while <predicate> do <command> (missing 'do' separator)".into()),
            }
        }
        "attack" | "atk" => {
            if rest.is_empty() {
                return Err("usage: attack <oid>".into());
            }
            let oid: i32 = rest[0].parse().map_err(|_| format!("bad oid: {}", rest[0]))?;
            Ok(Some(Command::Attack(oid)))
        }
        "hunt" => {
            if rest.is_empty() {
                return Ok(Some(Command::Hunt(true)));
            }
            match rest[0] {
                "on" | "1" | "true" => Ok(Some(Command::Hunt(true))),
                "off" | "0" | "false" => Ok(Some(Command::Hunt(false))),
                "attack" => {
                    let n: Option<usize> = match rest.get(1) {
                        Some(s) => Some(s.parse().map_err(|_| "bad attack count")?),
                        None => None,
                    };
                    Ok(Some(Command::HuntMode(HuntMode::Attack(n))))
                }
                "skill" => {
                    let id: Option<i32> = match rest.get(1) {
                        Some(s) => Some(s.parse().map_err(|_| "bad skillid")?),
                        None => None,
                    };
                    let n: Option<usize> = match rest.get(2) {
                        Some(s) => Some(s.parse().map_err(|_| "bad max targets")?),
                        None => None,
                    };
                    let range: Option<i32> = match rest.get(3) {
                        Some(s) => Some(s.parse().map_err(|_| "bad skill range")?),
                        None => None,
                    };
                    let mp: Option<i16> = match rest.get(4) {
                        Some(s) => Some(s.parse().map_err(|_| "bad mp cost")?),
                        None => None,
                    };
                    Ok(Some(Command::HuntMode(HuntMode::Skill(
                        id, n, range, mp,
                    ))))
                }
                "range" => {
                    let n: i32 = rest
                        .get(1)
                        .ok_or("usage: hunt range <px> (attack range, default 300)")?
                        .parse()
                        .map_err(|_| "bad range")?;
                    if n < 0 {
                        return Err("range must be >= 0".into());
                    }
                    Ok(Some(Command::HuntRange(n)))
                }
                "pickup" => match rest.get(1) {
                    // on|off = toggle; a number = pickup radius in px (0 = whole map)
                    Some(s) if *s == "on" => Ok(Some(Command::HuntPickup(true))),
                    Some(s) if *s == "off" => Ok(Some(Command::HuntPickup(false))),
                    Some(s) => {
                        let n: i32 = s
                            .parse()
                            .map_err(|_| format!("bad pickup: {s} (on|off or px value, 0 = whole map)"))?;
                        if n < 0 {
                            return Err("pickup must be >= 0 (0 = whole map)".into());
                        }
                        Ok(Some(Command::HuntPickupRange(n)))
                    }
                    None => Err("usage: hunt pickup on|off|<px> (0 = whole map)".into()),
                },
                "damage" => match rest.get(1) {
                    Some(s) => {
                        let n: i32 = s.parse().map_err(|_| "bad damage")?;
                        if n < 0 {
                            return Err("damage must be >= 0 (0 = auto formula)".into());
                        }
                        Ok(Some(Command::HuntDamage(n)))
                    }
                    None => Err("usage: hunt damage <n> (0 = auto lvl^2)".into()),
                },
                "hits" => {
                    let n: u8 = rest
                        .get(1)
                        .ok_or("usage: hunt hits <n> (hits per target per cast 1..15, e.g. 2 for a two-hit AoE)")?
                        .parse()
                        .map_err(|_| "bad hits")?;
                    if !(1..=15).contains(&n) {
                        return Err("hits must be 1..=15".into());
                    }
                    Ok(Some(Command::HuntHits(n)))
                },
                "atkcd" | "atkcd_ms" => {
                    let ms: u64 = rest
                        .get(1)
                        .ok_or("usage: hunt atkcd <ms> (attack cooldown interval, 0 = every tick)")?
                        .parse()
                        .map_err(|_| "bad atkcd (milliseconds)")?;
                    Ok(Some(Command::HuntAtkCd(ms)))
                },
                "stand" => match rest.get(1) {
                    Some(s) if *s == "on" => Ok(Some(Command::HuntStand(true))),
                    Some(s) if *s == "off" => Ok(Some(Command::HuntStand(false))),
                    _ => Err("usage: hunt stand on|off (stand still, attack from the spot)".into()),
                },
                "controller" => match rest.get(1) {
                    Some(s) if *s == "on" => Ok(Some(Command::HuntController(true))),
                    Some(s) if *s == "off" => Ok(Some(Command::HuntController(false))),
                    _ => Err("usage: hunt controller on|off (only attack mobs we hold control of, default on)".into()),
                },
                "filter" => {
                    // hunt filter [status] | off | allow <id...> | deny <id...> | + <id...> | - <id...>
                    let sub = rest.get(1).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
                    let ids_of = |rest: &[&str], required: bool| -> Result<Vec<i32>, String> {
                        let mut ids = Vec::new();
                        for s in rest.iter().skip(2) {
                            match s.parse::<i32>() {
                                Ok(id) if id > 0 => ids.push(id),
                                _ => return Err(format!("bad itemid: {s}")),
                            }
                        }
                        if required && ids.is_empty() {
                            Err(format!("usage: hunt filter {sub} <itemid> [itemid...]"))
                        } else {
                            Ok(ids)
                        }
                    };
                    match sub.as_str() {
                        "" | "status" | "?" => Ok(Some(Command::HuntFilter(HuntFilterCmd::Status))),
                        "off" | "clear" => Ok(Some(Command::HuntFilter(HuntFilterCmd::Off))),
                        // 无 id 的 allow/deny = 只切换模式, 保留现有清单
                        "allow" | "only" => Ok(Some(Command::HuntFilter(HuntFilterCmd::SetAllow(ids_of(&rest, false)?)))),
                        "deny" => Ok(Some(Command::HuntFilter(HuntFilterCmd::SetDeny(ids_of(&rest, false)?)))),
                        "add" => Ok(Some(Command::HuntFilter(HuntFilterCmd::Add(ids_of(&rest, true)?)))),
                        "del" | "rm" => Ok(Some(Command::HuntFilter(HuntFilterCmd::Del(ids_of(&rest, true)?)))),
                        other => Err(format!(
                            "bad hunt filter: {other} (status|off|allow [ids]|deny [ids]|add <ids>|del <ids>)"
                        )),
                    }
                }
                "status" => Ok(Some(Command::HuntStatus)),
                "save" => Ok(Some(Command::HuntSave)),
                // one manual attack round (rule-driven or typed by hand)
                "once" => Ok(Some(Command::HuntOnce)),
                other => Err(format!(
                    "bad hunt mode: {other} (on|off|attack [n]|skill [id] [n] [range] [mp]|hits <n>|atkcd <ms>|range <px>|pickup|damage <n>|controller on|off|stand on|off|once|status|save)"
                )),
            }
        }
        "use" | "drink" => {
            let id: i32 = rest
                .first()
                .ok_or("usage: use <itemid>")?
                .parse()
                .map_err(|_| "bad itemid")?;
            Ok(Some(Command::Use(id)))
        }
        "reward" | "open" => {
            let id: i32 = rest
                .first()
                .ok_or("usage: reward <itemid>")?
                .parse()
                .map_err(|_| "bad itemid")?;
            Ok(Some(Command::UseReward(id)))
        }
        "drop" => {
            let id: i32 = rest
                .first()
                .ok_or("usage: drop <itemid> [qty] [slot <n>]")?
                .parse()
                .map_err(|_| "bad itemid")?;
            let qty: i16 = match rest.get(1) {
                Some(s) => s.parse().map_err(|_| "bad qty")?,
                None => 1,
            };
            // Optional `slot <n>`: drop only that slot (used by the D key in the bag UI)
            let slot: Option<i16> = match rest.get(2) {
                Some(s) if *s == "slot" => {
                    let n: i16 = rest
                        .get(3)
                        .ok_or("usage: drop <itemid> [qty] slot <n>  (specific slot)")?
                        .parse()
                        .map_err(|_| "bad slot")?;
                    Some(n)
                }
                _ => None,
            };
            Ok(Some(Command::Drop(id, qty, slot)))
        }
        "dropmeso" | "dropm" => {
            let amount: i32 = rest
                .first()
                .ok_or("usage: dropmeso <amount>")?
                .parse()
                .map_err(|_| "bad amount")?;
            Ok(Some(Command::DropMeso(amount)))
        }
        "scroll" | "upgrade" => {
            // `scroll <scroll_itemid> <equip_itemid> [bless]`: the scroll lives in the USE tab;
            // the target is looked up in worn equips first, then the backpack equip tab. bless =
            // protect with a blessing scroll (ws=2, the server deducts the white scroll; not consumed
            // on failure); default ws=1 without blessing.
            let scroll_id: i32 = rest
                .first()
                .ok_or("usage: scroll <scroll_itemid> <equip_itemid> [bless]")?
                .parse()
                .map_err(|_| "bad scroll itemid")?;
            let equip_id: i32 = rest
                .get(1)
                .ok_or("usage: scroll <scroll_itemid> <equip_itemid> [bless]")?
                .parse()
                .map_err(|_| "bad equip itemid")?;
            let bless = matches!(rest.get(2).map(|s| s.to_ascii_lowercase()).as_deref(), Some("bless") | Some("1") | Some("true") | Some("on"));
            Ok(Some(Command::UseScroll(scroll_id, equip_id, bless)))
        }
        "ap" => {
            let stat = match rest.first() {
                Some(s) if s.eq_ignore_ascii_case("status") => {
                    return Ok(Some(Command::Ap(ApCmd::Status)));
                }
                Some(s) if s.eq_ignore_ascii_case("str") => ApStat::Str,
                Some(s) if s.eq_ignore_ascii_case("dex") => ApStat::Dex,
                Some(s) if s.eq_ignore_ascii_case("int") => ApStat::Int,
                Some(s) if s.eq_ignore_ascii_case("luk") => ApStat::Luk,
                Some(s) if s.eq_ignore_ascii_case("maxhp") || s.eq_ignore_ascii_case("hp") => {
                    ApStat::MaxHp
                }
                Some(s) if s.eq_ignore_ascii_case("maxmp") || s.eq_ignore_ascii_case("mp") => {
                    ApStat::MaxMp
                }
                Some(other) => {
                    return Err(format!(
                        "bad ap stat: {other} (str|dex|int|luk|maxhp|maxmp|status)"
                    ))
                }
                None => return Err("usage: ap <str|dex|int|luk|maxhp|maxmp> [n]".into()),
            };
            let n: i32 = match rest.get(1) {
                Some(v) => v
                    .parse()
                    .map_err(|_| format!("bad ap amount: {v}"))?,
                None => 1,
            };
            if n <= 0 {
                return Err("ap: amount must be >= 1".into());
            }
            Ok(Some(Command::Ap(ApCmd::Add(stat, n))))
        }
        "rule" => {
            let mut it = rest.iter();
            match it.next().map(|s| s.to_ascii_lowercase()).as_deref() {
                Some("open") => {
                    let id = it.next().ok_or("rule open: missing rule id")?;
                    Ok(Some(Command::Rule(RuleCmd::Open(id.to_string()))))
                }
                Some("close") => {
                    let id = it.next().ok_or("rule close: missing rule id")?;
                    Ok(Some(Command::Rule(RuleCmd::Close(id.to_string()))))
                }
                Some("status") => Ok(Some(Command::Rule(RuleCmd::Status(
                    it.next().map(|s| s.to_string()),
                )))),
                Some(other) => Err(format!(
                    "bad rule subcommand: {other} (open <id>|close <id>|status [id])"
                )),
                None => Err("usage: rule open <id>|close <id>|status [id]".into()),
            }
        }
        "group" => {
            let mut it = rest.iter();
            match it.next().map(|s| s.to_ascii_lowercase()).as_deref() {
                // run/stop: group run <id>|stop <id> (open/close are aliases)
                Some("run") | Some("open") => {
                    let id = it.next().ok_or("group run: missing group id")?;
                    Ok(Some(Command::Group(GroupCmd::Open(id.to_string()))))
                }
                Some("stop") | Some("close") => {
                    let id = it.next().ok_or("group stop: missing group id")?;
                    Ok(Some(Command::Group(GroupCmd::Close(id.to_string()))))
                }
                Some("status") => Ok(Some(Command::Group(GroupCmd::Status(
                    it.next().map(|s| s.to_string()),
                )))),
                Some(other) => Err(format!(
                    "bad group subcommand: {other} (run <id>|stop <id>|status [id])"
                )),
                None => Err("usage: group run <id>|stop <id>|status [id]".into()),
            }
        }
        "reconnect" => {
            let mut it = rest.iter();
            match it.next().map(|s| s.to_ascii_lowercase()).as_deref() {
                Some("on") => Ok(Some(Command::Reconnect(ReconnectCmd::On))),
                Some("off") => Ok(Some(Command::Reconnect(ReconnectCmd::Off))),
                Some("delay") => {
                    let n: u64 = it
                        .next()
                        .ok_or("reconnect delay: missing seconds")?
                        .parse()
                        .map_err(|_| "bad delay (seconds)".to_string())?;
                    Ok(Some(Command::Reconnect(ReconnectCmd::Delay(n))))
                }
                Some("max") => {
                    let n: u64 = it
                        .next()
                        .ok_or("reconnect max: missing count")?
                        .parse()
                        .map_err(|_| "bad max (count, 0 = unlimited)".to_string())?;
                    Ok(Some(Command::Reconnect(ReconnectCmd::Max(n))))
                }
                Some("status") => Ok(Some(Command::Reconnect(ReconnectCmd::Status))),
                Some(other) => Err(format!(
                    "bad reconnect subcommand: {other} (on|off|delay <secs>|max <count>|status)"
                )),
                None => Err("usage: reconnect on|off|delay <secs>|max <count>|status".into()),
            }
        }
        "party" => {
            match rest.first() {
                Some(a) if a.eq_ignore_ascii_case("create") || a.eq_ignore_ascii_case("new") => {
                    Ok(Some(Command::Party(PartyCmd::Create)))
                }
                Some(a) if a.eq_ignore_ascii_case("invite") => match rest.get(1) {
                    Some(name) => Ok(Some(Command::Party(PartyCmd::Invite(name.to_string())))),
                    None => Err("usage: party invite <name>".into()),
                },
                Some(a) if a.eq_ignore_ascii_case("invitecid") => match rest.get(1) {
                    Some(cid) => {
                        let cid: i32 = cid.parse().map_err(|_| "bad cid")?;
                        Ok(Some(Command::Party(PartyCmd::InviteCid(cid))))
                    }
                    None => Err("usage: party invitecid <cid>".into()),
                },
                Some(a) if a.eq_ignore_ascii_case("leave") || a.eq_ignore_ascii_case("quit") => {
                    Ok(Some(Command::Party(PartyCmd::Leave)))
                }
                Some(a) if a.eq_ignore_ascii_case("kick") => match rest.get(1) {
                    Some(name) => Ok(Some(Command::Party(PartyCmd::Kick(name.to_string())))),
                    None => Err("usage: party kick <name>".into()),
                },
                Some(a) if a.eq_ignore_ascii_case("kickcid") => match rest.get(1) {
                    Some(cid) => {
                        let cid: i32 = cid.parse().map_err(|_| "bad cid")?;
                        Ok(Some(Command::Party(PartyCmd::KickCid(cid))))
                    }
                    None => Err("usage: party kickcid <cid>".into()),
                },
                None => Err("usage: party <create|invite <name>|invitecid <cid>|leave|kick <name>|kickcid <cid>> (view via 'view party')".into()),
                Some(other) => Err(format!(
                    "bad party action: {other} (create|invite|invitecid|leave|kick|kickcid; view via 'view party')"
                )),
            }
        }
        "trade" => {
            match rest.first() {
                Some(a) if a.eq_ignore_ascii_case("invite") => match rest.get(1) {
                    Some(name) => Ok(Some(Command::Trade(TradeCmd::Invite(name.to_string())))),
                    None => Err("usage: trade invite <name>".into()),
                },
                Some(a) if a.eq_ignore_ascii_case("accept") || a.eq_ignore_ascii_case("join") => {
                    Ok(Some(Command::Trade(TradeCmd::Accept)))
                }
                Some(a) if a.eq_ignore_ascii_case("confirm") || a.eq_ignore_ascii_case("lock") => {
                    Ok(Some(Command::Trade(TradeCmd::Confirm)))
                }
                Some(a) if a.eq_ignore_ascii_case("decline") || a.eq_ignore_ascii_case("deny") => {
                    Ok(Some(Command::Trade(TradeCmd::Decline)))
                }
                Some(a) if a.eq_ignore_ascii_case("quit") || a.eq_ignore_ascii_case("exit") => {
                    Ok(Some(Command::Trade(TradeCmd::Quit)))
                }
                Some(a) if a.eq_ignore_ascii_case("put") => {
                    let itemid: i32 = rest
                        .get(1)
                        .ok_or("usage: trade put <itemid> [qty]")?
                        .parse()
                        .map_err(|_| "bad itemid")?;
                    let qty: i16 = match rest.get(2) {
                        Some(s) => s.parse().map_err(|_| "bad qty")?,
                        None => 1,
                    };
                    Ok(Some(Command::Trade(TradeCmd::Put(itemid, qty))))
                }
                Some(a) if a.eq_ignore_ascii_case("meso") || a.eq_ignore_ascii_case("money") => {
                    let n: i32 = rest
                        .get(1)
                        .ok_or("usage: trade meso <amount>")?
                        .parse()
                        .map_err(|_| "bad amount")?;
                    Ok(Some(Command::Trade(TradeCmd::Meso(n))))
                }
                None => Err(
                    "usage: trade <invite|accept|confirm|decline|quit|put|meso> (view via 'view trade')"
                        .into(),
                ),
                Some(other) => Err(format!(
                    "bad trade action: {other} (accept|confirm|decline|quit|put|meso; view via 'view trade')"
                )),
            }
        }
        "equip" => {
            let id: i32 = rest
                .first()
                .ok_or("usage: equip <itemid>")?
                .parse()
                .map_err(|_| "bad itemid")?;
            Ok(Some(Command::Equip(EquipCmd::Equip(id))))
        }
        "unequip" | "takeoff" => {
            let id: i32 = rest
                .first()
                .ok_or("usage: unequip <itemid>")?
                .parse()
                .map_err(|_| "bad itemid")?;
            Ok(Some(Command::Equip(EquipCmd::Unequip(id))))
        }
        "skill" | "skills" | "sp" => {
            let sub = rest.first().map(|s| s.to_ascii_lowercase());
            match sub.as_deref() {
                Some("learn") => {
                    let id: i32 = rest
                        .get(1)
                        .ok_or("usage: skill learn <skillid>")?
                        .parse()
                        .map_err(|_| "bad skillid")?;
                    Ok(Some(Command::Skill(SkillCmd::Learn(id))))
                }
                Some("cast") => {
                    if rest.len() < 3 {
                        return Err("usage: skill cast <skillid> <oid>".into());
                    }
                    let skillid: i32 = rest[1].parse().map_err(|_| "bad skillid")?;
                    let oid: i32 = rest[2].parse().map_err(|_| "bad oid")?;
                    Ok(Some(Command::Skill(SkillCmd::Cast(skillid, oid))))
                }
                Some("info") => {
                    let id: i32 = rest
                        .get(1)
                        .ok_or("usage: skill info <skillid>")?
                        .parse()
                        .map_err(|_| "bad skillid")?;
                    Ok(Some(Command::Skill(SkillCmd::Info(id))))
                }
                Some("status") => Ok(Some(Command::View(View::Skills))),
                Some(other) => {
                    // bare `sp <skillid>` / `skill <skillid>` = learn the skill
                    match other.parse::<i32>() {
                        Ok(id) => Ok(Some(Command::Skill(SkillCmd::Learn(id)))),
                        Err(_) => Err(format!(
                            "bad skill action: {other} (learn|cast|info; list via 'view skills')"
                        )),
                    }
                }
                None => Ok(Some(Command::View(View::Skills))),
            }
        }
        "cast" => {
            if rest.len() < 2 {
                return Err("usage: skill cast <skillid> <oid>".into());
            }
            let skillid: i32 = rest[0].parse().map_err(|_| "bad skillid")?;
            let oid: i32 = rest[1].parse().map_err(|_| "bad oid")?;
            Ok(Some(Command::Skill(SkillCmd::Cast(skillid, oid))))
        }
        "buff" => match rest.first().map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("cancel") => {
                let skillid: i32 = rest
                    .get(1)
                    .ok_or("usage: buff cancel <skillid>")?
                    .parse()
                    .map_err(|_| "bad skillid")?;
                Ok(Some(Command::BuffCancel(skillid)))
            }
            Some(_) => {
                let skillid: i32 = rest
                    .first()
                    .ok_or("usage: buff <skillid> | buff cancel <skillid>")?
                    .parse()
                    .map_err(|_| "bad skillid")?;
                Ok(Some(Command::BuffSkill(skillid)))
            }
            None => Err("usage: buff <skillid> | buff cancel <skillid>".into()),
        }
        "keymap" => {
            if rest.first().map(|s| *s) == Some("set") {
                if rest.len() < 4 {
                    return Err("usage: keymap set <key> <type> <action>".into());
                }
                let key: i32 = rest[1].parse().map_err(|_| "bad key")?;
                let ty: u8 = rest[2].parse().map_err(|_| "bad type")?;
                let action: i32 = rest[3].parse().map_err(|_| "bad action")?;
                Ok(Some(Command::KeymapSet(key, ty, action)))
            } else {
                Err("usage: keymap set <key> <type> <action> (view via 'view keymap')".into())
            }
        }
        "channel" | "ch" | "cc" => {
            let n: u8 = rest
                .first()
                .ok_or("usage: channel <n> (1-based)")?
                .parse()
                .map_err(|_| "bad channel")?;
            if n < 1 {
                return Err("channel must be >= 1 (1-based)".into());
            }
            Ok(Some(Command::ChangeChannel(n)))
        }
        "cashshop" | "cs" => match rest.first().map(|s| s.to_ascii_lowercase()).as_deref() {
            None | Some("enter") => Ok(Some(Command::CashShop)),
            Some("buy") => {
                // sn = product number from `cashshop list` (0x83 sale entries)
                let sn: i32 = rest
                    .get(1)
                    .ok_or("usage: cashshop buy <sn> [nx|points] (sn from 'cashshop list')")?
                    .parse()
                    .map_err(|_| "bad sn")?;
                let points = match rest.get(2).map(|s| s.to_ascii_lowercase()).as_deref() {
                    None | Some("nx") => false,
                    Some("points") | Some("p") | Some("maplepoints") => true,
                    Some(other) => return Err(format!("bad currency: {other} (nx|points)")),
                };
                Ok(Some(Command::CsBuy(sn, points)))
            }
            Some("get") => {
                // uniqueid = item instance id from `view cashshop` (only exists after purchase)
                let uid = match rest.get(1).copied() {
                    Some("last") | None => None,
                    Some(s) => Some(s.parse().map_err(|_| "bad uniqueid (from 'view cashshop')")?),
                };
                Ok(Some(Command::CsTakeOut(uid)))
            }
            Some("list") | Some("ls") => Ok(Some(Command::CsList)),
            Some("store") | Some("put") => {
                // uniqueid = item instance id from `view cashshop` (only cash items can be stored)
                let uid: i64 = rest
                    .get(1)
                    .ok_or("usage: cashshop store <uniqueid> (uniqueid from 'view cashshop')")?
                    .parse()
                    .map_err(|_| "bad uniqueid")?;
                Ok(Some(Command::CsStore(uid)))
            }
            Some("out") | Some("leave") => Ok(Some(Command::CsOut)),
            Some(other) => Err(format!(
                "bad cashshop cmd: {other} (enter|buy <sn from list> [nx|points]|get [uniqueid from view]|list|store <uniqueid from view>|out)"
            )),
        },
        "auction" | "mts" => {
            // Open the auction (0x8D ENTER_MTS, replacing the blocked reward);
            // continue the dialog flow with `npc reply <n>`.
            if !rest.is_empty() {
                return Err("usage: auction (no args — opens the auction)".into());
            }
            Ok(Some(Command::AuctionOpen))
        },
        "reenter" | "reentry" => {
            if !rest.is_empty() {
                return Err("usage: reenter (no args)".into());
            }
            Ok(Some(Command::Reenter))
        }
        "task" => {
            match rest.first().map(|s| s.to_ascii_lowercase()).as_deref() {
                // run/stop: task run <id> (start is an alias; kept for rule actions/docs)
                Some("run") | Some("start") => {
                    let id = rest.get(1).ok_or("usage: task run <id>")?.to_string();
                    Ok(Some(Command::Task(TaskCmd::Start(id))))
                }
                Some("stop") | Some("cancel") => Ok(Some(Command::Task(TaskCmd::Stop))),
                Some("status") | Some("?") | None => Ok(Some(Command::Task(TaskCmd::Status))),
                Some(other) => Err(format!("bad task cmd: {other} (run <id>|stop|status)")),
            }
        }
        "town" => {
            if rest.is_empty() {
                return Err("usage: town <mapid> [portal_name]".into());
            }
            let mapid: i32 = rest[0]
                .parse()
                .map_err(|_| format!("bad mapid: {}", rest[0]))?;
            let portal = match rest.get(1) {
                Some(s) => s.to_string(),
                None => "sp".to_string(),
            };
            Ok(Some(Command::ChangeMap(mapid, portal)))
        }
        "warp" => {
            let portal = rest
                .first()
                .ok_or("usage: warp <portal_name>")?
                .to_string();
            Ok(Some(Command::ChangeMapSpecial(portal)))
        }
        "movewarp" | "mw" => {
            let portal = rest
                .first()
                .ok_or("usage: movewarp <portal_name>")?
                .to_string();
            Ok(Some(Command::MoveWarp(portal)))
        }
        "npc" => {
            // Dialog commands live under the npc top-level scope:
            // npc <oid> talks; npc yes|no|next|prev|num|text|cancel|reply advance the dialog.
            if let Some(sub) = rest.first() {
                match *sub {
                    "yes" | "y" => return Ok(Some(Command::NpcYesNo(true))),
                    "no" | "n" => return Ok(Some(Command::NpcYesNo(false))),
                    "next" | "ok" | "ack" => return Ok(Some(Command::NpcNext)),
                    "prev" | "back" => return Ok(Some(Command::NpcPrev)),
                    "cancel" | "end" | "bye" => return Ok(Some(Command::NpcCancel)),
                    "reply" | "select" => {
                        let sel: i32 = rest
                            .get(1)
                            .ok_or("usage: npc reply <option_id>")?
                            .parse()
                            .map_err(|_| "bad option")?;
                        return Ok(Some(Command::NpcReply(sel)));
                    }
                    "num" | "number" => {
                        let n: i32 = rest
                            .get(1)
                            .ok_or("usage: npc num <value>")?
                            .parse()
                            .map_err(|_| "bad number")?;
                        return Ok(Some(Command::NpcNumber(n)));
                    }
                    "text" | "input" => {
                        return Ok(Some(Command::NpcText(rest[1..].join(" "))))
                    }
                    _ => {}
                }
            }
            if rest.is_empty() {
                return Err("usage: npc <oid> | npc yes|no|next|prev|num <n>|text <...>|cancel|reply <n>".into());
            }
            let oid: i32 = rest[0]
                .parse()
                .map_err(|_| format!("bad oid: {}", rest[0]))?;
            Ok(Some(Command::NpcTalk(oid)))
        }
        "buy" => {
            if rest.len() < 2 {
                return Err("usage: buy <itemid> [qty]".into());
            }
            let itemid: i32 = rest[0]
                .parse()
                .map_err(|_| format!("bad itemid: {}", rest[0]))?;
            let qty: i16 = match rest.get(1) {
                Some(s) => s.parse().map_err(|_| format!("bad qty: {s}"))?,
                None => 1,
            };
            Ok(Some(Command::NpcBuy(itemid, qty)))
        }
        "sell" => {
            if rest.is_empty() {
                return Err("usage: sell <itemid> [qty] [slot <n>] | tab <equip|consume|etc>".into());
            }
            match rest[0] {
                // `sell tab <kind>` = sell a whole inventory tab
                "tab" => {
                    let kind = match rest.get(1) {
                        Some(s) => SellTab::parse(s).ok_or_else(|| {
                            format!("bad sell tab: {s} (equip|consume|etc)")
                        })?,
                        None => SellTab::Equip,
                    };
                    Ok(Some(Command::SellType(kind)))
                }
                _ => {
                    let itemid: i32 = rest[0]
                        .parse()
                        .map_err(|_| format!("bad itemid: {}", rest[0]))?;
                    let qty: i16 = match rest.get(1) {
                        Some(s) => s.parse().map_err(|_| format!("bad qty: {s}"))?,
                        None => 1,
                    };
                    // Optional `slot <n>`: sell only that slot (used by "sell current" in the bag UI)
                    let slot: Option<i16> = match rest.get(2) {
                        Some(s) if *s == "slot" => {
                            let n: i16 = rest
                                .get(3)
                                .ok_or("usage: sell <itemid> [qty] slot <n>  (specific slot)")?
                                .parse()
                                .map_err(|_| "bad slot")?;
                            Some(n)
                        }
                        _ => None,
                    };
                    Ok(Some(Command::NpcSell(itemid, qty, slot)))
                }
            }
        }
        "sellammo" | "ammo" => Ok(Some(Command::SellAmmo)),
        "shop" | "leave" => Ok(Some(Command::ShopLeave)),
        "reactor" | "reactors" => {
            if rest.is_empty() {
                return Err("usage: reactor on|off|hit all|<oid>|cooldown <ms> (view via 'view reactor')".into());
            }
            match rest[0] {
                "on" | "off" => {
                    parse_bool(rest[0]).map(|v| Some(Command::Reactor(ReactorCmd::On(v))))
                }
                "hit" => match rest.get(1) {
                    Some(s) if s.eq_ignore_ascii_case("all") => {
                        Ok(Some(Command::Reactor(ReactorCmd::HitAll)))
                    }
                    Some(s) => {
                        let oid: i32 = s
                            .parse()
                            .map_err(|_| format!("bad reactor oid: {s}"))?;
                        Ok(Some(Command::Reactor(ReactorCmd::Hit(oid))))
                    }
                    None => Err("usage: reactor hit all|<oid>".into()),
                },
                "cooldown" => {
                    let ms: u64 = rest
                        .get(1)
                        .and_then(|s| s.parse().ok())
                        .ok_or("usage: reactor cooldown <ms> (0 = every tick)")?;
                    Ok(Some(Command::Reactor(ReactorCmd::Cooldown(ms))))
                }
                _ => Err("usage: reactor on|off|hit all|<oid>|cooldown <ms> (view via 'view reactor')".into()),
            }
        }
        "reload" | "cfg" => Ok(Some(Command::Reload)),
        // flow variables: `setvar <name> <value...>` / `clearvar <name>`
        "setvar" => {
            let name = rest
                .first()
                .ok_or("usage: setvar <name> <value>")?
                .to_string();
            if rest.len() < 2 {
                return Err("usage: setvar <name> <value>".into());
            }
            let value = rest[1..].join(" ");
            Ok(Some(Command::SetVar(name, value)))
        }
        "clearvar" => {
            let name = rest
                .first()
                .ok_or("usage: clearvar <name>")?
                .to_string();
            Ok(Some(Command::ClearVar(name)))
        }
        "login" => {
            let sub = rest.first().map(|s| s.to_ascii_lowercase());
            match sub.as_deref() {
                Some("world") => {
                    let idx: usize = rest
                        .get(1)
                        .ok_or("usage: login world <index> [channel]")?
                        .parse()
                        .map_err(|_| "bad world index".to_string())?;
                    let chan: u8 = match rest.get(2) {
                        Some(s) => s.parse().map_err(|_| "bad channel".to_string())?,
                        None => 1,
                    };
                    if chan < 1 {
                        return Err("channel must be >= 1 (1-based)".into());
                    }
                    Ok(Some(Command::Login(LoginCmd::World(idx, chan))))
                }
                Some("char") => {
                    let idx: usize = rest
                        .get(1)
                        .ok_or("usage: login char <index>")?
                        .parse()
                        .map_err(|_| "bad char index".to_string())?;
                    Ok(Some(Command::Login(LoginCmd::Char(idx))))
                }
                Some(other) => Err(format!("bad login step: {other} (world|char)")),
                None => Err("usage: login world <index> [channel] | login char <index>".into()),
            }
        }
        "help" | "h" => Ok(Some(Command::Help)),
        "quit" | "exit" | "q" => Ok(Some(Command::Quit)),
        _ => Err(format!("unknown command: {cmd} (try 'help')")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_commands_parse() {
        assert!(parse("login gender 0").is_err()); // gender belonged to auto-registration; removed
        assert!(matches!(
            parse("login world 1"),
            Ok(Some(Command::Login(LoginCmd::World(1, 1))))
        ));
        assert!(matches!(
            parse("login world 0 3"),
            Ok(Some(Command::Login(LoginCmd::World(0, 3))))
        ));
        // channels are 1-based
        assert!(parse("login world 0 0").is_err());
        assert!(matches!(
            parse("channel 2"),
            Ok(Some(Command::ChangeChannel(2)))
        ));
        assert!(parse("channel 0").is_err());
        assert!(parse("channel").is_err());
        assert!(matches!(
            parse("login char 2"),
            Ok(Some(Command::Login(LoginCmd::Char(2))))
        ));
        assert!(parse("login char").is_err());
        assert!(parse("login bogus").is_err());
        assert!(parse("login").is_err());
    }

    #[test]
    fn rule_commands_parse() {
        assert!(matches!(
            parse("rule open sellauto"),
            Ok(Some(Command::Rule(RuleCmd::Open(id)))) if id == "sellauto"
        ));
        assert!(matches!(
            parse("rule close trade_accept"),
            Ok(Some(Command::Rule(RuleCmd::Close(id)))) if id == "trade_accept"
        ));
        assert!(matches!(
            parse("rule status"),
            Ok(Some(Command::Rule(RuleCmd::Status(None))))
        ));
        assert!(matches!(
            parse("rule status sellauto"),
            Ok(Some(Command::Rule(RuleCmd::Status(Some(id))))) if id == "sellauto"
        ));
        assert!(parse("rule").is_err());
        assert!(parse("rule banana").is_err());
        assert!(parse("rule open").is_err());
        assert!(parse("rule close").is_err());
    }

    #[test]
    fn group_commands_parse() {
        assert!(matches!(
            parse("group run autosell"),
            Ok(Some(Command::Group(GroupCmd::Open(id)))) if id == "autosell"
        ));
        assert!(matches!(
            parse("group stop autosell"),
            Ok(Some(Command::Group(GroupCmd::Close(id)))) if id == "autosell"
        ));
        assert!(matches!(
            parse("group open autosell"),
            Ok(Some(Command::Group(GroupCmd::Open(id)))) if id == "autosell"
        ));
        assert!(matches!(
            parse("group close autosell"),
            Ok(Some(Command::Group(GroupCmd::Close(id)))) if id == "autosell"
        ));
        assert!(matches!(
            parse("group status"),
            Ok(Some(Command::Group(GroupCmd::Status(None))))
        ));
        assert!(matches!(
            parse("group status autosell"),
            Ok(Some(Command::Group(GroupCmd::Status(Some(id))))) if id == "autosell"
        ));
        assert!(parse("group").is_err());
        assert!(parse("group banana").is_err());
        assert!(parse("group run").is_err());
        assert!(parse("group stop").is_err());
        assert!(parse("group open").is_err());
        assert!(parse("group close").is_err());
    }

    #[test]
    fn sellammo_parses() {
        assert!(matches!(parse("sellammo"), Ok(Some(Command::SellAmmo))));
        assert!(matches!(parse("ammo"), Ok(Some(Command::SellAmmo))));
        assert!(matches!(parse("sell tab equip"), Ok(Some(Command::SellType(SellTab::Equip)))));
        assert!(matches!(parse("sell tab consume"), Ok(Some(Command::SellType(SellTab::Consume)))));
        assert!(matches!(parse("sell tab etc"), Ok(Some(Command::SellType(SellTab::Etc)))));
        assert!(parse("sell tab bogus").is_err());
        // `sell type` removed (may be reused later for item groups)
        assert!(parse("sell type equip").is_err());
        assert!(parse("sellall").is_err());
    }

    #[test]
    fn cashshop_commands_parse() {
        assert!(matches!(parse("cashshop"), Ok(Some(Command::CashShop))));
        assert!(matches!(parse("cs"), Ok(Some(Command::CashShop))));
        assert!(matches!(parse("auction"), Ok(Some(Command::AuctionOpen))));
        assert!(matches!(parse("mts"), Ok(Some(Command::AuctionOpen))));
        assert!(parse("auction 5").is_err());
        assert!(matches!(
            parse("cashshop enter"),
            Ok(Some(Command::CashShop))
        ));
        assert!(matches!(
            parse("cashshop buy 5550000"),
            Ok(Some(Command::CsBuy(5550000, false)))
        ));
        assert!(matches!(
            parse("cashshop buy 5550000 nx"),
            Ok(Some(Command::CsBuy(5550000, false)))
        ));
        assert!(matches!(
            parse("cashshop buy 5550000 points"),
            Ok(Some(Command::CsBuy(5550000, true)))
        ));
        assert!(parse("cashshop buy").is_err());
        assert!(parse("cashshop buy abc").is_err());
        assert!(parse("cashshop buy 1 gold").is_err());
        assert!(matches!(
            parse("cashshop get 12345"),
            Ok(Some(Command::CsTakeOut(Some(12345))))
        ));
        assert!(matches!(
            parse("cashshop get last"),
            Ok(Some(Command::CsTakeOut(None)))
        ));
        assert!(matches!(
            parse("cashshop get"),
            Ok(Some(Command::CsTakeOut(None)))
        ));
        assert!(parse("cashshop get xyz").is_err());
        assert!(matches!(parse("cashshop out"), Ok(Some(Command::CsOut))));
        assert!(matches!(parse("cashshop leave"), Ok(Some(Command::CsOut))));
        assert!(matches!(parse("cashshop list"), Ok(Some(Command::CsList))));
        assert!(matches!(parse("cashshop ls"), Ok(Some(Command::CsList))));
        assert!(matches!(
            parse("cashshop store 12345"),
            Ok(Some(Command::CsStore(12345)))
        ));
        assert!(matches!(
            parse("cashshop put 12345"),
            Ok(Some(Command::CsStore(12345)))
        ));
        assert!(parse("cashshop store").is_err());
        assert!(parse("cashshop store abc").is_err());
        assert!(parse("cashshop bogus").is_err());
        // top-level aliases csbuy/csget/csout removed (use the cashshop subcommands instead)
        assert!(parse("csbuy 5550000").is_err());
        assert!(parse("csget last").is_err());
        assert!(parse("csout").is_err());
        assert!(matches!(parse("reenter"), Ok(Some(Command::Reenter))));
        assert!(parse("reenter 104040000").is_err());
        assert!(matches!(
            parse("view cashshop"),
            Ok(Some(Command::View(View::CashShop)))
        ));
        assert!(matches!(
            parse("view cs"),
            Ok(Some(Command::View(View::CashShop)))
        ));
    }

    #[test]
    fn while_command_parses() {
        // 基本形式: while <谓词> do <命令>
        assert!(matches!(
            parse("while mobs>0 do hunt once"),
            Ok(Some(Command::While(pred, cmd))) if pred == "mobs>0" && cmd == "hunt once"
        ));
        // 谓词/命令可含空格
        assert!(matches!(
            parse("while drops>0 do pickup all"),
            Ok(Some(Command::While(pred, cmd))) if pred == "drops>0" && cmd == "pickup all"
        ));
        // 命令可以是 task run(复合流程按迭代内联展开)
        assert!(matches!(
            parse("while mobs>0 do task run hunt_cycle"),
            Ok(Some(Command::While(pred, cmd))) if pred == "mobs>0" && cmd == "task run hunt_cycle"
        ));
        // 缺 do 分隔 → 错误
        assert!(parse("while mobs>0").is_err());
        assert!(parse("while").is_err());
        // 谓词或命令为空 → 错误
        assert!(parse("while  do hunt once").is_err());
        assert!(parse("while mobs>0 do").is_err());
        // 谓词含 "==" 与 while 的 do 分隔不冲突
        assert!(matches!(
            parse("while mobs==1 do reactor hit all"),
            Ok(Some(Command::While(pred, cmd))) if pred == "mobs==1" && cmd == "reactor hit all"
        ));
        // 谓词/命令含 do 子串(无空格)不应被误判为 do 分隔符
        assert!(matches!(
            parse("while doors>0 do hunt once"),
            Ok(Some(Command::While(pred, cmd))) if pred == "doors>0" && cmd == "hunt once"
        ));
        assert!(matches!(
            parse("while mobs>0 do task run doors_open"),
            Ok(Some(Command::While(pred, cmd))) if pred == "mobs>0" && cmd == "task run doors_open"
        ));
    }
}







