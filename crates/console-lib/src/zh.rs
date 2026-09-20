//! TUI 中文渲染层: 按结构化字段 (emit::Field) 自由渲染中文,
//! 与底层英文文本完全解耦。未携带字段的事件回退显示英文原文。

use openstory_bot::emit::Field;
use openstory_bot::names;

/// 字段中文标签 (一次性词汇表, 新消息复用字段即自动获得中文渲染)。
pub fn field_label(f: Field) -> &'static str {
    match f {
        Field::Oid => "目标",
        Field::Cid => "角色",
        Field::MobId => "怪物",
        Field::ItemId => "物品",
        Field::NpcId => "NPC",
        Field::MapId => "地图",
        Field::PartyId => "队伍",
        Field::SkillId => "技能",
        Field::ReactorId => "放置物",
        Field::Id => "ID",
        Field::Job => "职业",
        Field::Count => "数量",
        Field::Qty => "数量",
        Field::Slot => "槽位",
        Field::Items => "物品",
        Field::Equips => "装备",
        Field::Skills => "技能",
        Field::Hp => "血量",
        Field::MaxHp => "血量上限",
        Field::Mp => "蓝量",
        Field::MaxMp => "蓝量上限",
        Field::Exp => "经验",
        Field::Level => "等级",
        Field::Ap => "属性点",
        Field::Sp => "技能点",
        Field::Meso => "金币",
        Field::Damage => "伤害",
        Field::Percent => "血量比",
        Field::Range => "范围",
        Field::Targets => "目标数",
        Field::Channel => "频道",
        Field::Selection => "选项",
        Field::Value => "值",
        Field::Number => "数字",
        Field::Duration => "时长",
        Field::Price => "价格",
        Field::Pos => "位置",
        Field::Ground => "地面",
        Field::Name => "名称",
        Field::Text => "内容",
        Field::World => "世界",
        Field::Portal => "传送门",
        Field::MoveMode => "移动方式",
        Field::AttackMode => "攻击方式",
        Field::Toggle => "状态",
        Field::Until => "直到",
        Field::Step => "步骤",
        Field::Phase => "阶段",
        Field::Status => "状态",
    }
}

/// 字段值渲染: 状态/模式词表 + id 名称解析。
fn render_value(k: Field, v: &str) -> String {
    match k {
        Field::Toggle => match v {
            "on" | "true" | "1" => "开".to_string(),
            "off" | "false" | "0" => "关".to_string(),
            _ => v.to_string(),
        },
        Field::MoveMode => match v {
            "walk" => "走路".to_string(),
            "teleport" | "tp" => "传送".to_string(),
            _ => v.to_string(),
        },
        Field::AttackMode => match v {
            "normal" => "普攻".to_string(),
            "skill" => "技能".to_string(),
            "all" | "area" => "范围攻击".to_string(),
            _ => v.to_string(),
        },
        Field::MobId => v
            .parse()
            .map(names::mob_name)
            .unwrap_or_else(|_| v.to_string()),
        Field::NpcId => v
            .parse()
            .map(names::npc_name)
            .unwrap_or_else(|_| v.to_string()),
        Field::ItemId => v
            .parse()
            .map(names::item_name)
            .unwrap_or_else(|_| v.to_string()),
        Field::SkillId => v
            .parse()
            .map(names::skill_name)
            .unwrap_or_else(|_| v.to_string()),
        Field::MapId => v
            .parse()
            .map(names::map_name)
            .unwrap_or_else(|_| v.to_string()),
        Field::Percent => format!("{v}%"),
        _ => v.to_string(),
    }
}

/// 把结构化字段渲染成一行中文: `击杀 目标:123 · 怪物:120100·蓝蜗牛`。
/// HP/MP 与上限字段成对出现时合并为 `血量:4849/4741`。
pub fn render_fields(fields: &[(Field, String)]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < fields.len() {
        let (k, v) = &fields[i];
        let paired = |next: Option<&(Field, String)>, want: Field| {
            next.map(|(nk, _)| *nk == want).unwrap_or(false)
        };
        match k {
            Field::Hp if paired(fields.get(i + 1), Field::MaxHp) => {
                out.push(format!("血量:{v}/{}", fields[i + 1].1));
                i += 2;
                continue;
            }
            Field::Mp if paired(fields.get(i + 1), Field::MaxMp) => {
                out.push(format!("蓝量:{v}/{}", fields[i + 1].1));
                i += 2;
                continue;
            }
            Field::MaxHp | Field::MaxMp => {
                i += 1;
                continue;
            }
            _ => {}
        }
        out.push(format!("{}:{}", field_label(*k), render_value(*k, v)));
        i += 1;
    }
    out.join(" · ")
}

/// 从空白分隔的字段列表里取 `key=值` 的值 (无则该 key 返回 None)。
fn field<'a>(parts: &[&'a str], key: &str) -> Option<&'a str> {
    parts
        .iter()
        .find_map(|p| p.strip_prefix(key).and_then(|v| v.strip_prefix('=')))
}

/// 在文本中取 `key<sep>值` (值到空白为止); 无则 None。
fn kv_in(body: &str, key: &str, sep: char, label: &str) -> Option<String> {
    let pat = format!("{key}{sep}");
    let pos = body.find(&pat)?;
    let rest = &body[pos + pat.len()..];
    let val: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
    if val.is_empty() {
        None
    } else {
        Some(format!("{label}{val}"))
    }
}

/// 已知 tag+正文组合的中文整句 (无结构化字段的固定消息)。
/// 覆盖我们自己的提示语/状态语, 与 CLI 英文原文一一对应。
pub fn zh_body(tag: &str, body: &str) -> Option<String> {
    let zh = match (tag, body) {
        ("[serverlist]", "no worlds received") => "未收到世界列表".to_string(),
        ("[login]", _) if body.starts_with("failed, reason=") => {
            let reason: u8 = body["failed, reason=".len()..].parse().unwrap_or(0);
            match reason {
                0 => "登录成功".to_string(),
                1 => "登录失败: 账号或密码错误".to_string(),
                2 => "登录失败: 已连接/重复登录".to_string(),
                3 => "登录失败: 未注册的账号".to_string(),
                4 => "登录失败: 账号或密码错误".to_string(),
                5 => "登录失败: 账号被封禁".to_string(),
                6 => "登录失败: 被GM踢下线".to_string(),
                7 => "登录失败: 连接数过多".to_string(),
                8 => "登录失败: 无效用户名".to_string(),
                9 => "登录失败: 密码过短".to_string(),
                10 => "登录失败: IP被封".to_string(),
                _ => format!("登录失败: 原因码 {reason}"),
            }
        }
        ("[charlist]", "no characters to select") => "没有可选角色".to_string(),
        ("[charlist]", "--charlist-only: stopping before map entry (no CHAR_SELECT sent)") => {
            "已停在角色列表 (--charlist-only, 未发送进图)".to_string()
        }
        ("[server_ip]", "reconnected to channel server") => "已重连频道服务器".to_string(),
        ("[channel]", _) if body.starts_with("switch -> ") => {
            format!("切换到 {}", &body["switch -> ".len()..])
        }
        ("[server_ip]", _) if body.starts_with("channel at ") => {
            let rest = &body["channel at ".len()..];
            let addr = rest.split(" (original").next().unwrap_or(rest);
            format!("频道服: {addr}")
        }
        (tag, body) if tag.starts_with("[view ") => {
            let sub = tag.trim_start_matches("[view ").trim_end_matches(']');
            translate_view(sub, body)
        }
        ("[rule]", body) if body.starts_with("open ") => format!("开启规则 {}", &body[5..]),
        ("[rule]", body) if body.starts_with("close ") => format!("关闭规则 {}", &body[6..]),
        ("[group]", body) if body.starts_with("open ") => format!("运行组 {}", &body[5..]),
        ("[group]", body) if body.starts_with("close ") => format!("停止组 {}", &body[6..]),
        ("[group]", body) if body.starts_with("exclusive: closed ") => {
            format!("互斥: 已关闭 {}", &body["exclusive: closed ".len()..])
        }
        ("[party]", "create sent") => "已发送创建队伍".to_string(),
        ("[party]", "leave sent") => "已离开队伍".to_string(),
        ("[party]", body) if body.starts_with("invite '") => {
            format!(
                "已发送邀请 '{}'",
                body.trim_start_matches("invite '")
                    .trim_end_matches("' sent")
            )
        }
        ("[party]", body) if body.starts_with("kick '") => {
            format!(
                "已发送踢出 '{}'",
                body.trim_start_matches("kick '")
                    .split("' ")
                    .next()
                    .unwrap_or("?")
            )
        }
        ("[party]", body) if body.starts_with("created partyid=") => {
            format!(
                "组队创建成功 编号={}",
                body.trim_start_matches("created partyid=")
            )
        }
        ("[party]", body) if body.starts_with("invited by '") => {
            format!(
                "收到 '{}' 的组队邀请",
                body.trim_start_matches("invited by '")
                    .split('\'')
                    .next()
                    .unwrap_or("?")
            )
        }
        ("[party]", body) if body.contains("requests to join") => {
            let name = body.split('\'').nth(1).unwrap_or("?");
            format!("'{name}' 请求加入队伍")
        }
        ("[party]", body) if body.starts_with("sync partyid=") => {
            let rest = &body["sync partyid=".len()..];
            let pid = rest.split(' ').next().unwrap_or("?");
            let mut out = format!("队伍同步: 编号={pid}");
            if let Some(m) = rest.split("members=").nth(1) {
                out.push_str(&format!(" 成员数={}", m.split(' ').next().unwrap_or("?")));
            }
            if let Some(l) = rest.split("leader=").nth(1) {
                out.push_str(&format!(" 队长={}", l.split(' ').next().unwrap_or("?")));
            }
            if let Some(c) = rest.split("(changed: ").nth(1) {
                out.push_str(&format!(" 变更={}", c.trim_end_matches(')')));
            }
            out
        }
        ("[party]", body) if body.contains("left/expelled") => {
            // "member CCCCCW (3951) left/expelled (partyid=15)"
            let rest = body
                .trim_start_matches("member ")
                .split(" (")
                .next()
                .unwrap_or(body);
            format!("成员离开/被踢: {rest}")
        }
        ("[party]", "we left/disbanded") => "已离开/解散队伍".to_string(),
        ("[party]", body) if body.starts_with("invite failed") => {
            "邀请失败 (对方已在队伍中)".to_string()
        }
        ("[member]", body) => {
            // "id=3951 name=CCCCCW job=100 lvl=49" -> 编号/名称/职业/等级
            let mut out = String::from("队员: ");
            for kv in body.split_whitespace() {
                let (k, v) = kv.split_once('=').unwrap_or((kv, "?"));
                let label = match k {
                    "id" => "编号",
                    "name" => "名称",
                    "job" => "职业",
                    "lvl" | "level" => "等级",
                    "chan" | "channel" => "频道",
                    "map" | "mapid" => "地图",
                    other => other,
                };
                out.push_str(&format!("{label}={v} "));
            }
            out.trim_end().to_string()
        }
        ("[party]", body) if body.starts_with("unhandled op=") => format!("未知组队消息: {}", body),
        ("[party]", body) if body.contains("not in party members nor on this map") => {
            let name = body.split('\'').nth(1).unwrap_or("?");
            format!("'{name}' 不在队伍成员/当前地图中")
        }
        ("[pickup]", body) if body.starts_with("sent ") => {
            format!("已发送拾取 {} 个", &body["sent ".len()..])
        }
        // ---- rule/task/group status 多行 (续行继承首行 tag) ----
        ("[rule]", "no top-level rules defined (config.json \"rules\")") => {
            "未定义顶层规则 (config.json \"rules\")".to_string()
        }
        ("[rule]", body) if body.starts_with("when: ") => format!("条件: {}", &body[6..]),
        ("[rule]", body) if body.starts_with("then: ") => {
            format!("动作: {}", translate_command(&body[6..]))
        }
        ("[rule]", body) if body.starts_with("cooldown: ") => {
            format!("冷却: {}", &body["cooldown: ".len()..])
        }
        ("[rule]", body) if body.contains(" (on)") || body.contains(" (off)") => {
            // "1. trade_accept (off)" — 数字.id (开/关)
            let on = body.contains(" (on)");
            let id = body.split_whitespace().nth(1).unwrap_or(body).to_string();
            let num: String = body
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            format!(
                "规则 {}{} ({})",
                num.trim_end_matches('.'),
                id,
                if on { "开" } else { "关" }
            )
        }
        ("[group]", "no groups defined (config.json \"groups\")") => {
            "未定义功能组 (config.json \"groups\")".to_string()
        }
        ("[group]", body) if body.starts_with("rule ") => {
            // "rule 1: sell_loop" / "when: .." / "then: .." / "cooldown: .."
            let rest = &body[5..];
            if let Some((n, id)) = rest.split_once(": ") {
                format!("规则 {n}: {id}")
            } else {
                body.to_string()
            }
        }
        ("[group]", body) if body.starts_with("task ") => {
            let rest = &body[5..];
            if let Some((n, id)) = rest.split_once(": ") {
                format!("任务 {n}: {id}")
            } else {
                body.to_string()
            }
        }
        ("[group]", body) if body.starts_with("step ") => translate_status_step(body),
        ("[group]", body) if body.starts_with("when: ") => format!("条件: {}", &body[6..]),
        ("[group]", body) if body.starts_with("then: ") => {
            format!("动作: {}", translate_command(&body[6..]))
        }
        ("[group]", body) if body.starts_with("cooldown: ") => {
            format!("冷却: {}", &body["cooldown: ".len()..])
        }
        ("[group]", body) if body.contains("rules=") => {
            // "1. bf_money (off, exclusive) rules=2 tasks=1"
            let parts: Vec<&str> = body.split_whitespace().collect();
            let id = parts
                .get(1)
                .copied()
                .unwrap_or("")
                .trim_end_matches("(")
                .to_string();
            let on = body.contains(" (on");
            let excl = if body.contains("exclusive") {
                ", 互斥"
            } else {
                ""
            };
            let rules = field(&parts, "rules").unwrap_or("?");
            let tasks = field(&parts, "tasks").unwrap_or("?");
            let num: String = body
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            format!(
                "组 {}{} ({}{}) 规则={rules} 任务={tasks}",
                num.trim_end_matches('.'),
                id,
                if on { "开" } else { "关" },
                excl
            )
        }
        ("[task]", "no active task") => "无活动任务".to_string(),
        ("[task]", "no top-level tasks defined (config.json \"tasks\")") => {
            "未定义顶层任务 (config.json \"tasks\")".to_string()
        }
        ("[task]", body) if body.contains("step ") && body.contains("/") => {
            // "-> 'bf_open_shop' step 2/4" 活动栈行
            let marker = if body.trim_start().starts_with("->") {
                "-> "
            } else {
                ""
            };
            let rest = body.trim_start().trim_start_matches("->").trim();
            if let Some((id, step)) = rest.split_once("' step ") {
                format!("{marker}'{}' 步骤 {step}", id.trim_matches('\''))
            } else {
                body.to_string()
            }
        }
        ("[task]", body) if body.starts_with("step ") => translate_status_step(body),
        ("[task]", body) if body.contains("priority=") => {
            // "1. town_sell priority=10 steps=10"
            let parts: Vec<&str> = body.split_whitespace().collect();
            let id = parts.get(1).copied().unwrap_or("").to_string();
            let pri = field(&parts, "priority").unwrap_or("?");
            let steps = field(&parts, "steps").unwrap_or("?");
            let num: String = body
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            format!(
                "任务 {}{} (优先级{pri} 步骤{steps})",
                num.trim_end_matches('.'),
                id
            )
        }
        ("[hunt]", body) if body.starts_with("status=") => {
            // "status=on mode=attack skill=1001005" / "mode=skill skill=0"
            let mut on = "关";
            let mut mode = "普攻";
            let mut skill = String::new();
            for kv in body.split_whitespace() {
                if let Some((k, v)) = kv.split_once('=') {
                    match k {
                        "status" => on = if v == "on" { "开" } else { "关" },
                        "mode" => {
                            mode = match v {
                                "attack" => "普攻",
                                "skill" => "技能",
                                "reactor" => "放置物",
                                _ => v,
                            }
                        }
                        "skill" => {
                            if v != "0" {
                                skill = format!("技能 {v} ");
                            }
                        }
                        _ => {}
                    }
                }
            }
            format!("状态={on} 攻击方式={mode} {skill}")
        }
        // 原地模式开关 (hunt stand on|off)
        ("[hunt]", body) if body.starts_with("stand=") => {
            let on = if body.trim().ends_with("on") {
                "开"
            } else {
                "关"
            };
            format!("原地={on}")
        }
        // 吸怪开关 (gather on|off)
        ("[gather]", body) if body.starts_with("= ") => {
            let on = if body.trim().ends_with("on") {
                "开"
            } else {
                "关"
            };
            format!("吸怪={on}")
        }
        // 吸怪步长 (gather step <px>)
        ("[gather]", body) if body.starts_with("step ") => {
            // "step = 150px"
            format!(
                "吸怪步长={}px",
                body.trim_start_matches("step ")
                    .trim_start_matches("= ")
                    .trim_end_matches("px")
            )
        }
        // 吸怪拉取条数 (gather: pulled N mob(s) toward player) — 一条消息
        ("[gather]", body) if body.starts_with("pulled ") => {
            let n: &str = body
                .trim_start_matches("pulled ")
                .split(' ')
                .next()
                .unwrap_or("0");
            format!("吸怪: 拉取 {n} 只怪物")
        }
        // 吸怪轮次间隔 / 每轮数量 / 控制权过滤
        ("[gather]", body) if body.starts_with("interval = ") => {
            format!(
                "吸怪间隔={}ms",
                body.trim_start_matches("interval = ")
                    .trim_end_matches("ms")
            )
        }
        ("[gather]", body) if body.starts_with("max = ") => {
            let n: &str = body
                .trim_start_matches("max = ")
                .split(' ')
                .next()
                .unwrap_or("0");
            format!("吸怪每轮={n} 只")
        }
        ("[gather]", body) if body.starts_with("controller-only = ") => {
            let on = if body.trim().ends_with("on") {
                "开"
            } else {
                "关"
            };
            format!("只拉有控制权的怪={on}")
        }
        ("[hunt]", body) if body.starts_with("controller-only = ") => {
            let on = if body.trim().ends_with("on") {
                "开"
            } else {
                "关"
            };
            format!("只打有控制权的怪={on}")
        }
        ("[hunt]", body) if body.starts_with("damage = auto") => {
            "伤害 = 自动公式 (等级²)".to_string()
        }
        ("[hunt]", body) if body.starts_with("skill hits = ") => {
            format!(
                "技能段数 = {}",
                body.trim_start_matches("skill hits = ")
                    .trim_end_matches(" (per target)")
            )
        }
        ("[reconnect]", body) if body.starts_with("on (every ") => {
            format!("已开启重连 {}", body.trim_start_matches("on (every "))
        }
        ("[reconnect]", body) if body.starts_with("off (every ") => {
            format!("已关闭重连 {}", body.trim_start_matches("off (every "))
        }
        ("[scroll]", body) if body.starts_with("scroll ") => {
            format!(
                "卷轴 {} 不在背包 USE 栏",
                body.trim_start_matches("scroll ")
                    .split(' ')
                    .next()
                    .unwrap_or("?")
            )
        }
        ("[scroll]", body) if body.starts_with("equip ") => {
            let id = body
                .trim_start_matches("equip ")
                .split(' ')
                .next()
                .unwrap_or("?");
            format!("装备 {id} 未穿戴也未在背包装备栏 (先 equip)")
        }
        ("[cashshop]", body) if body.starts_with("entered: ") => {
            let n = body
                .trim_start_matches("entered: ")
                .split(' ')
                .next()
                .unwrap_or("?");
            format!("已进入商城, 在售 {n} 件")
        }
        ("[cashshop]", body) if body.starts_with("entered (sale list unparsed") => {
            let n = body.split(' ').next().unwrap_or("?");
            format!("已进入商城 (在售列表未解析, 剩余 {n} 字节)")
        }
        ("[cashshop]", body) if body.starts_with("entering") => "正在请求进入商城...".to_string(),
        ("[cashshop]", body) if body.starts_with("requesting entrance") => {
            "正在请求进入商城...".to_string()
        }
        ("[cashshop]", body) if body.contains("sale entries") => {
            format!(
                "商城共 {} 个在售条目:",
                body.split_whitespace().next().unwrap_or("?")
            )
        }
        ("[cashshop]", body) if body.starts_with("sn=") => {
            // "sn=5550000 item=5050000 qty=1 price=0 meso=0"
            let parts: Vec<&str> = body.split_whitespace().collect();
            let sn = field(&parts, "sn").unwrap_or("?");
            let item = field(&parts, "item").unwrap_or("?");
            let qty = field(&parts, "qty").unwrap_or("?");
            let price = field(&parts, "price").unwrap_or("?");
            let meso = field(&parts, "meso").unwrap_or("?");
            format!("  商品 sn={sn} 道具={item} 数量={qty} 折后价={price} 金币价={meso}")
        }
        ("[cashshop]", body) if body.starts_with("store unique=") => {
            // "store unique=58 item=5050000 tab=1"
            let parts: Vec<&str> = body.split_whitespace().collect();
            let uid = field(&parts, "unique").unwrap_or("?");
            let item = field(&parts, "item").unwrap_or("?");
            let tab = field(&parts, "tab").unwrap_or("?");
            format!("存入商城 unique={uid} 道具={item} 栏={tab}")
        }
        ("[cashshop]", body) if body.starts_with("no item with unique=") => {
            format!("背包中找不到该唯一id的物品 (仅点装/现金道具可存入)")
        }
        ("[cashshop]", body) if body.starts_with("sale list empty") => {
            "商城在售列表为空".to_string()
        }
        ("[cashshop]", body) if body.starts_with("(no item entries") => {
            "  (无带道具id的条目)".to_string()
        }
        ("[scroll]", body) if body.contains("-> equip ") => {
            // "2043000 slot=1 -> equip 1302007 dst=-11 ws=2 sent"
            let parts: Vec<&str> = body.split_whitespace().collect();
            let scroll_id = parts.first().copied().unwrap_or("?");
            let slot = field(&parts, "slot").unwrap_or("?");
            let equip_id = parts.get(4).copied().unwrap_or("?");
            let dst = field(&parts, "dst").unwrap_or("?");
            let ws = field(&parts, "ws").unwrap_or("?");
            let bless = if ws == "2" { " (使用祝福)" } else { "" };
            format!(
                "已使用卷轴 {scroll_id} (槽位{slot}) 对装备 {equip_id} (位置{dst}) ws={ws}{bless}"
            )
        }
        ("[config]", body) if body.contains("invalid") => "配置无效, 使用默认值".to_string(),
        ("[hunt]", body) if body.starts_with("pickup=") => {
            // "pickup=500 enabled cooldown=1500ms" / "pickup=0 enabled ..."（0=全图）
            let mut range = "关".to_string();
            for kv in body.split_whitespace() {
                if let Some((k, v)) = kv.split_once('=') {
                    match k {
                        "pickup" => {
                            range = if v == "disabled" {
                                "关".to_string()
                            } else if let Ok(n) = v.parse::<i32>() {
                                if n <= 0 {
                                    "全图".to_string()
                                } else {
                                    format!("{n}px")
                                }
                            } else {
                                v.to_string()
                            }
                        }
                        "cooldown" => {
                            let ms: String = v.trim_end_matches("ms").to_string();
                            return Some(format!("拾取={range} 攻击冷却={ms}ms"));
                        }
                        _ => {}
                    }
                }
            }
            format!("拾取={range}")
        }
        ("[hunt]", body) if body.starts_with("filter=") => {
            // "filter=allow allow=[2000000,2000001] deny=[4000000]"
            let (mode, rest) = body["filter=".len()..]
                .split_once(" allow=[")
                .unwrap_or((&body["filter=".len()..], ""));
            let names_of = |raw: &str| -> String {
                let ids = raw.trim_end_matches(']').trim_start_matches('[');
                if ids.is_empty() {
                    return "无".to_string();
                }
                ids.split(',')
                    .filter_map(|s| s.parse::<i32>().ok())
                    .map(|id| openstory_bot::names::item_name_text(id))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let (allow_raw, deny_raw) = match rest.split_once(" deny=[") {
                Some((a, d)) => (a, d),
                None => (rest, ""),
            };
            let allow = names_of(allow_raw);
            let deny = names_of(deny_raw);
            match mode {
                "off" => "拾取过滤: 不过滤".to_string(),
                "allow" => format!("拾取过滤: 仅捡(生效) 仅拾取=[{allow}] 仅过滤=[{deny}]"),
                "deny" => format!("拾取过滤: 禁捡(生效) 仅拾取=[{allow}] 仅过滤=[{deny}]"),
                m => format!("拾取过滤: {m}"),
            }
        }
        ("[hunt]", body) if body.starts_with("pickup_filter=") => {
            // "pickup_filter=allow items=[2000002,2000001]" / =deny / =off
            let rest = &body["pickup_filter=".len()..];
            let (mode, items) = match rest.split_once(" items=[") {
                Some((m, tail)) => (m, tail.trim_end_matches(']')),
                None => (rest, ""),
            };
            let names: Vec<String> = items
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| {
                    s.parse::<i32>()
                        .map(|id| openstory_bot::names::item_name_text(id))
                        .unwrap_or_else(|_| s.to_string())
                })
                .collect();
            match (mode, names.len()) {
                ("off", _) => "拾取过滤: 不过滤".to_string(),
                ("allow", 0) => "拾取过滤: 仅捡(空清单)".to_string(),
                ("allow", n) => format!("拾取过滤: 仅捡{}种 {}", n, names.join(" ")),
                ("deny", 0) => "拾取过滤: 禁捡(空清单)".to_string(),
                ("deny", n) => format!("拾取过滤: 禁捡{}种 {}", n, names.join(" ")),
                (m, _) => format!("拾取过滤: {m}"),
            }
        }
        ("[hunt]", body) if body.starts_with("range=") => {
            // "range=70 teleport_delay=0ms"
            let mut r = "?";
            let mut td = "?";
            for kv in body.split_whitespace() {
                if let Some((k, v)) = kv.split_once('=') {
                    match k {
                        "range" => r = v,
                        "teleport_delay" => td = v.trim_end_matches("ms"),
                        _ => {}
                    }
                }
            }
            format!("攻击距离={r} 传送间隔={td}ms")
        }
        ("[hunt]", body) if body.starts_with("attack_max=") => {
            // "attack_max=6 skill_range=200 max_targets=6 mp_cost=7 hits=2"
            let mut am = "?";
            let mut sr = "?";
            let mut mt = "?";
            let mut mc = "?";
            let mut h = "?";
            for kv in body.split_whitespace() {
                if let Some((k, v)) = kv.split_once('=') {
                    match k {
                        "attack_max" => am = v,
                        "skill_range" => sr = v,
                        "max_targets" => mt = v,
                        "mp_cost" => mc = v,
                        "hits" => h = v,
                        _ => {}
                    }
                }
            }
            format!("普攻数量={am} 技能范围={sr} 技能目标={mt} 耗蓝={mc} 段数={h}")
        }
        ("[hunt]", body) if body.starts_with("skill hits") => {
            // "skill hits = 2 (每目标段数)"
            let n = body.split_whitespace().nth(3).unwrap_or("?");
            format!("技能段数={n}")
        }
        ("[hunt]", body) if body.starts_with("damage=") => {
            // "damage=auto(lvl^2/2) until=none"
            let mut dmg = "?".to_string();
            let mut until = "无";
            for kv in body.split_whitespace() {
                if let Some((k, v)) = kv.split_once('=') {
                    match k {
                        "damage" => {
                            dmg = if v.starts_with("auto") {
                                "自动 (等级²/2)".to_string()
                            } else if let Some(o) = v.strip_prefix("override(") {
                                format!("覆盖 ({})", o.trim_end_matches(')'))
                            } else {
                                v.to_string()
                            }
                        }
                        "until" => {
                            if v != "none" {
                                until = v;
                            }
                        }
                        _ => {}
                    }
                }
            }
            format!("伤害={dmg} 自动停止={until}")
        }
        ("[teleport]", body) if body.starts_with("far seek step -> ") => {
            // "far seek step -> (tx,ty) toward (ex,ey)" — 分段靠近一步
            let rest = body.trim_start_matches("far seek step -> ");
            if let Some((step, target)) = rest.split_once(" toward ") {
                format!("就近寻怪步进 → {step} 目标 {target}")
            } else {
                format!("就近寻怪步进 → {rest}")
            }
        }
        ("[teleport]", body) if body.starts_with("far seek -> ") => {
            // "far seek -> (x,y) dist=NNNpx" — 圈内无怪时传送到最近的怪
            let rest = body.trim_start_matches("far seek -> ");
            let (pos, dist) = match rest.split_once(" dist=") {
                Some((p, d)) => (p, d.trim_end_matches("px")),
                None => (rest, "?"),
            };
            format!("就近寻怪传送 → {pos} 距离{dist}px")
        }
        ("[exp]", body) if body.starts_with('+') => {
            // "+135 (total 3829533 rem 95467)" — 击杀入账 + 累计 + 升级还需
            let parts: Vec<&str> = body.split_whitespace().collect();
            if parts.len() >= 5 && parts[1] == "(total" && parts[3] == "rem" {
                format!(
                    "经验 {} (累计 {} 升级还需 {})",
                    parts[0],
                    parts[2].trim_end_matches(')'),
                    parts[4].trim_end_matches(')')
                )
            } else {
                body.to_string()
            }
        }
        // ---- 放置物挂机 (rhunt): 与 [attack] 同款格式 ----
        ("[rhunt]", body) if body.starts_with("on") => {
            // "on - hit all reactors + insta-pickup drops"
            "已开启：全图打放置物 + 立即拾取".to_string()
        }
        ("[rhunt]", body) if body.starts_with("off") => "已关闭".to_string(),
        ("[rhunt]", body) if body.contains(" reactor(s) hit [") => {
            // "3 reactor(s) hit [1102000@5, 1102000@6]" — 与 "[attack] N mob(s) hit" 同构
            let (n, rest) = body
                .split_once(" reactor(s) hit [")
                .map(|(n, r)| (n, format!("[{r}")))
                .unwrap_or(("?", body.to_string()));
            format!("{n} 个放置物被攻击 {rest}")
        }
        ("[rhunt]", body) if body.ends_with(" drop(s) picked") => {
            // "5 drop(s) picked"
            let n = body.strip_suffix(" drop(s) picked").unwrap_or("?");
            format!("已拾取 {n} 个掉落")
        }
        // ---- 放置物 (reactor): 生命周期 / 手动命令 ----
        ("[reactor]", body) if body.starts_with("spawn ") => {
            // "spawn oid=123 rid=1102000 pos=(x,y)"
            format!(
                "生成 {}",
                &body["spawn ".len()..].replacen("pos=", "坐标=", 1)
            )
        }
        ("[reactor]", body) if body.starts_with("destroyed ") => {
            format!("销毁 {}", &body["destroyed ".len()..])
        }
        ("[reactor]", body) if body.starts_with("hit ") => {
            // "hit 3 reactor(s)" / "hit oid=123"
            let rest = &body[4..];
            match rest.strip_suffix(" reactor(s)") {
                Some(n) => format!("已打 {n} 个放置物"),
                None => format!("已打放置物 {rest}"),
            }
        }
        ("[reactor]", body) if body.starts_with("cooldown = ") => {
            // "cooldown = 1000ms (per hit round)" / "cooldown = 0ms (every tick)"
            let rest = &body["cooldown = ".len()..];
            let ms = rest
                .split_whitespace()
                .next()
                .unwrap_or("?")
                .trim_end_matches("ms");
            let mode = if body.contains("every tick") {
                "每 tick"
            } else if body.contains("per hit round") {
                "每轮"
            } else {
                ""
            };
            format!("攻击间隔 = {ms}ms ({mode})")
        }
        ("[reactor]", body) if body.starts_with("oid=") => {
            // "oid=123 not tracked on this map"
            let oid = body.split_whitespace().next().unwrap_or("?");
            format!("{oid} 不在本图放置物列表")
        }
        ("[attack]", body) if body.contains(" mob(s) hit") => {
            // "6 mob(s) hit" — 攻击日志一次攻击一行, 只报命中数量;
            // 每怪剩余血量由 [mobhp] 回显显示。
            let n = body.split(" mob(s) hit").next().unwrap_or("?");
            format!("{n} 只怪被攻击")
        }
        ("[var]", body) if body.starts_with("set ") => {
            // "set boss_summoned=1"
            let kv = &body[4..];
            format!("变量 {kv} 已设置")
        }
        ("[var]", body) if body.starts_with("cleared ") => {
            format!("变量 {} 已清除", &body[8..])
        }
        ("[var]", body) if body.ends_with(" not set") => {
            format!("变量 {} 未设置", body.trim_end_matches(" not set"))
        }
        ("[hunt]", body) if body.starts_with("attack mode: normal") => {
            // "attack mode: normal (one packet per mob, max 6 packets)"
            let max = body
                .rsplit("max ")
                .next()
                .unwrap_or("?")
                .trim_end_matches(" packets")
                .trim_end_matches(')');
            format!("攻击方式 = 普攻 (每怪一包, 最多 {max} 包)")
        }
        ("[hunt]", body) if body.starts_with("attack mode: skill 0") => {
            // "attack mode: skill 0 (no-skill group attack, max 6 targets)"
            let max = body
                .rsplit("max ")
                .next()
                .unwrap_or("?")
                .trim_end_matches(" targets")
                .trim_end_matches(')');
            format!("攻击方式 = 无技能群发 (最多 {max} 目标)")
        }
        ("[hunt]", body) if body.starts_with("attack mode: skill ") => {
            // "attack mode: skill 1001005 (range 200px, max 6 targets, mp 7)"
            let id = body
                .trim_start_matches("attack mode: skill ")
                .split_whitespace()
                .next()
                .unwrap_or("?");
            let range = body
                .split("range ")
                .nth(1)
                .unwrap_or("?")
                .split("px")
                .next()
                .unwrap_or("?");
            let max = body
                .split("max ")
                .nth(1)
                .unwrap_or("?")
                .split(" targets")
                .next()
                .unwrap_or("?");
            let mp = body
                .split("mp ")
                .nth(1)
                .unwrap_or("?")
                .trim_end_matches(')');
            format!("攻击方式 = 技能 {id} (范围{range}px, 最多 {max} 目标, 蓝耗 {mp})")
        }
        ("[hunt]", body) if body.starts_with("attack range = ") => {
            // "attack range = 300px"
            format!("攻击范围 = {}", body.trim_start_matches("attack range = "))
        }
        ("[system]", "log cleared") => "日志已清空".to_string(),
        ("[potion]", body) => {
            // "hp 5% (50/1000) <25% -> 2000006 slot=3" / "mp 1% (4/292) <50% -> 2000004 slot=2"
            let stat = if body.starts_with("hp ") {
                "血量"
            } else if body.starts_with("mp ") {
                "蓝量"
            } else {
                "状态"
            };
            let parts: Vec<&str> = body.split_whitespace().collect();
            let cur_pct = parts.get(1).copied().unwrap_or("?");
            let cur_max = parts.get(2).copied().unwrap_or("?");
            let th = parts.get(3).copied().unwrap_or("?");
            let itemid = parts
                .iter()
                .position(|p| *p == "->")
                .and_then(|i| parts.get(i + 1))
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(0);
            let slot = parts
                .iter()
                .find_map(|p| p.strip_prefix("slot="))
                .unwrap_or("?");
            format!(
                "{stat} {cur_pct} ({cur_max}) {th} → {} 槽位{slot}",
                names::item_name_text(itemid)
            )
        }
        _ => return None,
    };
    Some(zh)
}

/// TUI 中整条隐藏的消息 (交互提示由鼠标/弹窗完成, 无需打印到日志;
/// [ctrl] 为 CLI 排查用的控制权诊断, TUI 不展示)。
pub fn zh_hidden(tag: &str, body: &str) -> bool {
    if tag == "[ctrl]" {
        return true;
    }
    matches!(
        (tag, body),
        (
            "[serverlist]",
            "interactive: pick with 'login world <index> [channel]'"
        ) | ("[charlist]", "interactive: pick with 'login char <index>'")
    )
}

/// 原始 tag 中文翻译 (稳定 token 映射, 与消息正文格式无关)。
pub fn translate_tag(tag: &str) -> String {
    let zh = match tag {
        "[view]" => "[查看]",
        "[chat]" => "[聊天]",
        "[whisper]" => "[密语]",
        "[notice]" => "[公告]",
        "[npc]" => "[NPC]",
        "[npc_talk]" => "[NPC对话]",
        "[shop]" => "[商店]",
        "[hunt]" => "[打怪]",
        "[attack]" => "[攻击]",
        "[var]" => "[变量]",
        "[dmg]" => "[伤害]",
        "[kill]" => "[击杀]",
        "[exp]" => "[经验]",
        "[level]" => "[升级]",
        "[pickup]" => "[拾取]",
        "[reactor]" => "[放置物]",
        "[reconnect]" => "[重连]",
        "[rhunt]" => "[放置物挂机]",
        "[gather]" => "[吸怪]",
        "[discover]" => "[发现]",
        "[mob]" => "[怪物]",
        "[mobhp]" => "[怪血]",
        "[self]" => "[自身]",
        "[stand]" => "[站位]",
        "[teleport]" => "[传送]",
        "[pose]" => "[姿态]",
        "[move]" => "[移动]",
        "[rule]" => "[规则]",
        "[rules]" => "[规则]",
        "[group]" => "[组]",
        "[task]" => "[任务]",
        "[party]" => "[队伍]",
        "[member]" => "[成员]",
        "[trade]" => "[交易]",
        "[buff]" => "[辅助]",
        "[skill]" => "[技能]",
        "[ap]" => "[加点]",
        "[sp]" => "[技能点]",
        "[scroll]" => "[卷轴]",
        "[bag]" => "[背包]",
        "[keymap]" => "[按键]",
        "[equip]" => "[装备]",
        "[use]" => "[使用]",
        "[reward]" => "[奖励]",
        "[drop]" => "[丢弃]",
        "[dropmeso]" => "[丢金币]",
        "[channel]" => "[换线]",
        "[town]" => "[跨图]",
        "[warp]" => "[传送]",
        "[reenter]" => "[重进图]",
        "[cashshop]" => "[商城]",
        "[auction]" => "[拍卖]",
        "[login]" => "[登录]",
        "[world]" => "[世界]",
        "[serverlist]" => "[服务器]",
        "[serverstatus]" => "[服务器状态]",
        "[char]" => "[角色]",
        "[charlist]" => "[角色列表]",
        "[charinfo]" => "[角色信息]",
        "[server_ip]" => "[频道服]",
        "[choose_gender]" => "[选择性别]",
        "[gender_set]" => "[性别已设]",
        "[set_field]" => "[进图]",
        "[spawn_player]" => "[玩家出生]",
        "[config]" => "[配置]",
        "[report]" => "[播报]",
        "[fatal]" => "[致命]",
        "[warn]" => "[警告]",
        "[timeout]" => "[超时]",
        "[connection closed by server]" => "[连接断开]",
        "[duration elapsed]" => "[时长已到]",
        "[command error]" => "[指令错误]",
        "[handler error]" => "[处理错误]",
        "[tick error]" => "[周期错误]",
        "[recv error]" => "[接收错误]",
        "[auto]" => "[自动指令]",
        "[chatctl]" => "[聊天控制]",
        "[bc-own]" => "[攻击广播]",
        "[message]" => "[公告]",
        tag if tag.starts_with("[message") => "[公告]", // [message type=11]
        tag if tag.starts_with("[view") => "[查看]",    // [view xxx] 子命令
        _ => return tag.to_string(),
    };
    zh.to_string()
}

/// status 步骤行: "step 1: reward 2022467 (wait: dialog==1)" → "步骤 1: ..."
/// 新指令 `while <pred> do <cmd>` 在 TUI 中显示为 `当 <pred> 执行 <cmd>`。
fn translate_command(cmd: &str) -> String {
    if let Some(rest) = cmd.strip_prefix("while ") {
        if let Some(pos) = rest.find(" do ") {
            let pred = &rest[..pos];
            let body = &rest[pos + 4..];
            return format!("当 {pred} 执行 {body}");
        }
    }
    cmd.to_string()
}

fn translate_status_step(body: &str) -> String {
    let rest = body.trim_start_matches("step ").trim_start();
    if let Some((n, cmd)) = rest.split_once(": ") {
        let mut out = format!("步骤 {n}: {}", translate_command(cmd));
        if let Some(pos) = out.find(" (wait: ") {
            let wait = &out[pos + 8..out.len() - 1];
            out = format!("{} (等待: {wait})", &out[..pos]);
        }
        out
    } else {
        body.to_string()
    }
}

/// 装备属性块汉化: "lvl:0 req:1 slots:7/7 [watk:31 str:1]" →
/// "强化:0 需求等级:1 升级次数:7/7 [物攻:31 力量:1]"。
fn translate_equip_stats(s: &str) -> String {
    let map: &[(&str, &str)] = &[
        ("lvl:", "强化:"),
        ("req:", "需求等级:"),
        ("slots:", "升级次数:"),
        ("str:", "力量:"),
        ("dex:", "敏捷:"),
        ("int:", "智力:"),
        ("luk:", "运气:"),
        ("hp:", "血量:"),
        ("mp:", "蓝量:"),
        ("watk:", "物攻:"),
        ("matk:", "魔攻:"),
        ("wdef:", "物防:"),
        ("mdef:", "魔防:"),
        ("acc:", "命中:"),
        ("avoid:", "回避:"),
        ("hands:", "手技:"),
        ("speed:", "移速:"),
        ("jump:", "跳跃:"),
    ];
    let mut out = s.to_string();
    for (k, v) in map {
        out = out.replace(k, v);
    }
    out
}

/// view 明细行字段汉化: "  oid=5 mobid=100100 名称 hp=50% pos=(1,2)" →
/// "编号=5 怪物=100100 名称 血量=50% 坐标=(1,2)"。英文 key 统一转中文标签。
fn translate_line_fields(body: &str) -> String {
    let mut out = String::from("  ");
    for kv in body.trim_start().split_whitespace() {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        let label = match k {
            "oid" | "cid" => "编号",
            "mid" | "mobid" => "怪物",
            "npcid" => "NPC",
            "itemid" | "rid" => "类型",
            "name" => "名称",
            "hp" => "血量",
            "pos" => "坐标",
            "state" => "状态",
            "fh" => "平台",
            other => other,
        };
        if v.is_empty() {
            out.push_str(&format!("{label} "));
        } else {
            out.push_str(&format!("{label}={v} "));
        }
    }
    out.trim_end().to_string()
}

/// `view <子命令>` 行正文汉化: "[view attr] hp:.. mp:.." → "角色: 血量.."。
fn translate_view(sub: &str, body: &str) -> String {
    // attr 行用 `key:值` 格式, 其余用 `key=值` 格式
    let kv_sep = if sub == "attr" { ':' } else { '=' };
    let kv = |key: &str, label: &str| kv_in(body, key, kv_sep, label);
    let count = |body: &str| -> Option<i32> { body.split_whitespace().next()?.parse().ok() };
    match sub {
        "attr" => {
            let mut parts: Vec<String> = [
                ("hp", "血量"),
                ("mp", "蓝量"),
                ("exp", "经验"),
                ("rem", "升级还需"),
                ("level", "等级"),
            ]
            .iter()
            .filter_map(|(k, l)| kv(k, l))
            .collect();
            let attrs: Vec<String> = [
                ("str", "力量"),
                ("dex", "敏捷"),
                ("int", "智力"),
                ("luk", "运气"),
                ("ap", "属性点"),
                ("sp", "技能点"),
                ("meso", "金币"),
            ]
            .iter()
            .filter_map(|(k, l)| kv(k, l))
            .collect();
            if parts.is_empty() && attrs.is_empty() {
                body.to_string()
            } else {
                if !attrs.is_empty() {
                    parts.push(attrs.join(" "));
                }
                format!("角色: {}", parts.join(" "))
            }
        }
        "mobs" => {
            if body.starts_with("  ") {
                translate_line_fields(body)
            } else {
                match count(body) {
                    Some(n) => format!("怪物: {n} 只"),
                    None => body.to_string(),
                }
            }
        }
        "nearby" | "players" => {
            if body.starts_with("  ") {
                translate_line_fields(body)
            } else {
                match count(body) {
                    Some(n) => format!("玩家: {n} 名"),
                    None => body.to_string(),
                }
            }
        }
        "npcs" => {
            if body.starts_with("  ") {
                translate_line_fields(body)
            } else {
                match count(body) {
                    Some(n) => format!("NPC: {n} 个"),
                    None => body.to_string(),
                }
            }
        }
        "drops" => match count(body) {
            Some(n) => format!("掉落: {n} 个"),
            None => body.to_string(),
        },
        "inventory" => match count(body) {
            // "26 items (equip=4 use=3 etc=5 meso=1005106)"
            Some(n) => {
                let detail = body
                    .find('(')
                    .map(|i| body[i..].trim_end_matches(')').to_string())
                    .unwrap_or_default();
                if detail.is_empty() {
                    format!("背包: {n} 件")
                } else {
                    let d = detail
                        .trim_start_matches('(')
                        .replace("equip=", "装备")
                        .replace("use=", "消耗")
                        .replace("etc=", "其他")
                        .replace("meso=", "金币")
                        .replace(" cash=", " 商城")
                        .replace("worn=", "穿着");
                    format!("背包: {n} 件 ({d})")
                }
            }
            None => body.to_string(),
        },
        "party" => {
            if body.starts_with("  ") {
                // 成员行: "  id=1 name=a job=100 lvl=49 chan=0 map=1000000"
                let mut out = String::from("  队员: ");
                for kv in body.split_whitespace() {
                    let (k, v) = kv.split_once('=').unwrap_or((kv, "?"));
                    let label = match k {
                        "id" => "编号",
                        "name" => "名称",
                        "job" => "职业",
                        "lvl" | "level" => "等级",
                        "chan" | "channel" => "频道",
                        "map" | "mapid" => "地图",
                        other => other,
                    };
                    out.push_str(&format!("{label}={v} "));
                }
                out.trim_end().to_string()
            } else {
                let id = kv("id", "编号 ").unwrap_or_default();
                let leader = kv("leader", "队长").unwrap_or_default();
                let members = kv("members", "成员数").unwrap_or_default();
                format!("队伍: {id}{leader} {members}")
            }
        }
        "skills" => {
            if body.contains("sp available") {
                let n = count(body).unwrap_or(0);
                format!("技能点: {n} 可用")
            } else if body.contains("learned") {
                format!("已学技能: {body}")
            } else {
                body.to_string()
            }
        }
        "equiped" | "equips" => {
            // 已穿戴装备: 汇总行 "N worn" + 明细行 "  武器 itemid=.. (名) 属性"
            if body.contains("nothing worn") {
                "未穿戴装备".to_string()
            } else if body.ends_with("worn") && !body.starts_with("  ") {
                // 汇总行: "3 worn"
                if let Some(n) = body.split_whitespace().next() {
                    format!("已穿戴装备: {n} 件")
                } else {
                    body.to_string()
                }
            } else {
                // 明细行: "  武器 itemid=1302000 (木剑) lvl:0 req:1 slots:7/7 [watk:31]"
                let t = body.trim_start();
                if let Some(pos) = t.find(" itemid=") {
                    format!("{}: {}", &t[..pos], translate_equip_stats(&t[pos + 8..]))
                } else {
                    t.to_string()
                }
            }
        }
        "eqpinfo" => {
            // 汇总行: "item 1302000: 2 found (1 in backpack, 1 worn)"
            // 明细行: "  backpack slot=5 itemid=.." / "  worn 武器 itemid=.."
            let t = body.trim_start();
            if t.contains("not found") {
                format!("未找到装备: {}", t)
            } else if let Some(rest) = t.split_once(": ") {
                if rest.1.contains("found") {
                    // 汇总: "item 1302000: 2 found (1 in backpack, 1 worn)"
                    let inside = rest.1.split(['(', ')']).nth(1).unwrap_or("");
                    let total = rest.1.split_whitespace().next().unwrap_or("?");
                    let n_bp = inside
                        .split(',')
                        .next()
                        .unwrap_or("?")
                        .split_whitespace()
                        .next()
                        .unwrap_or("?");
                    let n_worn = inside
                        .split(',')
                        .nth(1)
                        .unwrap_or("?")
                        .split_whitespace()
                        .next()
                        .unwrap_or("?");
                    let item = rest.0.strip_prefix("item ").unwrap_or(rest.0);
                    return format!("装备 {item}: 共 {total} 件 (背包 {n_bp}, 已穿戴 {n_worn})");
                }
                t.to_string()
            } else if t.starts_with("backpack ") {
                let rest = t.trim_start_matches("backpack ");
                if let Some(pos) = rest.find(" itemid=") {
                    // "slot=5 itemid=1302000 (木剑) lvl:0 ... [watk:31]"
                    let slot = rest[..pos].strip_prefix("slot=").unwrap_or(&rest[..pos]);
                    format!(
                        "背包 槽位{slot}: {}",
                        translate_equip_stats(&rest[pos + 8..])
                    )
                } else {
                    t.to_string()
                }
            } else if t.starts_with("worn ") {
                let rest = t.trim_start_matches("worn ");
                if let Some(pos) = rest.find(" itemid=") {
                    // "武器 itemid=1302000 (木剑) lvl:0 ... [watk:31]"
                    format!(
                        "已穿戴 {}: {}",
                        &rest[..pos],
                        translate_equip_stats(&rest[pos + 8..])
                    )
                } else {
                    t.to_string()
                }
            } else {
                t.to_string()
            }
        }
        "reactor" => {
            if body.contains("none on map") {
                "图上无放置物".to_string()
            } else if body.starts_with("oid=") {
                // "oid=5 rid=1102000 state=0 pos=(10,20)"
                let mut oid = "?";
                let mut rid = "?";
                let mut st = "?";
                let mut pos = "?";
                for kv in body.split_whitespace() {
                    if let Some((k, v)) = kv.split_once('=') {
                        match k {
                            "oid" => oid = v,
                            "rid" => rid = v,
                            "state" => st = v,
                            "pos" => pos = v,
                            _ => {}
                        }
                    }
                }
                format!("编号={oid} 类型={rid} 状态={st} 坐标={pos}")
            } else {
                body.to_string()
            }
        }
        "keymap" => {
            if body.contains("not received") {
                "尚未收到按键配置".to_string()
            } else {
                body.to_string()
            }
        }
        "cashshop" => {
            if body.contains("not in cash shop") {
                "不在商城中".to_string()
            } else {
                body.to_string()
            }
        }
        _ => body.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_kill_line() {
        let f = vec![
            (Field::Oid, "123".to_string()),
            (Field::MobId, "120100".to_string()),
        ];
        let out = render_fields(&f);
        assert!(out.starts_with("目标:123 · 怪物:120100·"), "got {out}");
    }

    #[test]
    fn hp_pair_merges() {
        let f = vec![
            (Field::Hp, "4849".to_string()),
            (Field::MaxHp, "4741".to_string()),
            (Field::Meso, "88".to_string()),
        ];
        assert_eq!(render_fields(&f), "血量:4849/4741 · 金币:88");
    }

    #[test]
    fn toggle_and_modes_translate() {
        let f = vec![
            (Field::Toggle, "on".to_string()),
            (Field::MoveMode, "teleport".to_string()),
            (Field::AttackMode, "all".to_string()),
            (Field::Percent, "42".to_string()),
        ];
        assert_eq!(
            render_fields(&f),
            "状态:开 · 移动方式:传送 · 攻击方式:范围攻击 · 血量比:42%"
        );
    }

    #[test]
    fn unknown_ids_fallback() {
        let f = vec![(Field::MobId, "999999999".to_string())];
        assert_eq!(render_fields(&f), "怪物:999999999·未知");
    }

    #[test]
    fn skill_id_resolves_name() {
        // 服务器发送的技能 id (如 1001003) 应命中 skill.json 词典
        // (测试 cwd 可能读不到 data/, 只断言前缀格式)
        let f = vec![(Field::SkillId, "1001003".to_string())];
        let out = render_fields(&f);
        assert!(out.starts_with("技能:1001003·"), "got {out}");
    }

    #[test]
    fn tags_translate_and_fallback() {
        assert_eq!(translate_tag("[chat]"), "[聊天]");
        assert_eq!(translate_tag("[command error]"), "[指令错误]");
        assert_eq!(translate_tag("[some_new_tag]"), "[some_new_tag]");
    }

    #[test]
    fn known_bodies_translate() {
        assert_eq!(
            zh_body("[server_ip]", "reconnected to channel server").as_deref(),
            Some("已重连频道服务器")
        );
        assert_eq!(
            zh_body("[channel]", "switch -> 1.2.3.4:7575").as_deref(),
            Some("切换到 1.2.3.4:7575")
        );
        assert_eq!(zh_body("[chat]", "hello").as_deref(), None);
        assert_eq!(
            zh_body("[member]", "id=1 name=a lvl=2 chan=0 map=3").as_deref(),
            Some("队员: 编号=1 名称=a 等级=2 频道=0 地图=3")
        );
        // [exp] 击杀入账行: "+135 (total 3829533 rem 95467)" → 累计 + 升级还需
        assert_eq!(
            zh_body("[exp]", "+135 (total 3829533 rem 95467)").as_deref(),
            Some("经验 +135 (累计 3829533 升级还需 95467)")
        );
        // [rhunt] 与 [attack] 同款格式的中文翻译
        assert_eq!(
            zh_body("[rhunt]", "on - hit all reactors + insta-pickup drops").as_deref(),
            Some("已开启：全图打放置物 + 立即拾取")
        );
        assert_eq!(zh_body("[rhunt]", "off").as_deref(), Some("已关闭"));
        assert_eq!(
            zh_body("[rhunt]", "3 reactor(s) hit [1102000@5, 1102000@6]").as_deref(),
            Some("3 个放置物被攻击 [1102000@5, 1102000@6]")
        );
        assert_eq!(
            zh_body("[rhunt]", "5 drop(s) picked").as_deref(),
            Some("已拾取 5 个掉落")
        );
        // [reactor] 生命周期 / 手动命令
        assert_eq!(
            zh_body("[reactor]", "spawn oid=5 rid=1102000 pos=(10,20)").as_deref(),
            Some("生成 oid=5 rid=1102000 坐标=(10,20)")
        );
        assert_eq!(
            zh_body("[reactor]", "destroyed oid=5").as_deref(),
            Some("销毁 oid=5")
        );
        assert_eq!(
            zh_body("[reactor]", "hit 3 reactor(s)").as_deref(),
            Some("已打 3 个放置物")
        );
        assert_eq!(
            zh_body("[reactor]", "hit oid=5").as_deref(),
            Some("已打放置物 oid=5")
        );
        assert_eq!(
            zh_body("[reactor]", "oid=5 not tracked on this map").as_deref(),
            Some("oid=5 不在本图放置物列表")
        );
        // [cashshop] 在售列表 (首行计数 + 续行商品行)
        assert_eq!(
            zh_body("[cashshop]", "2424 sale entries:").as_deref(),
            Some("商城共 2424 个在售条目:")
        );
        assert_eq!(
            zh_body(
                "[cashshop]",
                "sn=5550000 item=5050000 qty=2 price=99 meso=0"
            )
            .as_deref(),
            Some("  商品 sn=5550000 道具=5050000 数量=2 折后价=99 金币价=0")
        );
        assert_eq!(
            zh_body("[cashshop]", "sale list empty").as_deref(),
            Some("商城在售列表为空")
        );
        assert_eq!(
            zh_body("[cashshop]", "store unique=58 item=5050000 tab=1").as_deref(),
            Some("存入商城 unique=58 道具=5050000 栏=1")
        );
        assert_eq!(
            zh_body("[cashshop]", "entered: 2424 item(s) on sale").as_deref(),
            Some("已进入商城, 在售 2424 件")
        );
        assert_eq!(
            zh_body("[reactor]", "cooldown = 1000ms (per hit round)").as_deref(),
            Some("攻击间隔 = 1000ms (每轮)")
        );
        assert_eq!(
            zh_body("[reactor]", "cooldown = 0ms (every tick)").as_deref(),
            Some("攻击间隔 = 0ms (每 tick)")
        );
    }

    #[test]
    fn view_subcommands_translate() {
        assert_eq!(translate_tag("[view]"), "[查看]");
        assert_eq!(translate_tag("[view mobs]"), "[查看]");
        assert_eq!(
            zh_body(
                "[view attr]",
                "hp:4849/4741 mp:700/515 exp:3829533 rem:95467 level:83"
            )
            .as_deref(),
            Some("角色: 血量4849/4741 蓝量700/515 经验3829533 升级还需95467 等级83")
        );
        assert_eq!(
            zh_body(
                "[view attr]",
                "str:10 dex:20 int:30 luk:40 ap:125 sp:155 meso:1005106"
            )
            .as_deref(),
            Some("角色: 力量10 敏捷20 智力30 运气40 属性点125 技能点155 金币1005106")
        );
        assert_eq!(
            zh_body("[view mobs]", "20 tracked").as_deref(),
            Some("怪物: 20 只")
        );
        assert_eq!(
            zh_body("[view reactor]", "oid=5 rid=1102000 state=0 pos=(10,20)").as_deref(),
            Some("编号=5 类型=1102000 状态=0 坐标=(10,20)")
        );
        assert_eq!(
            zh_body("[view nearby]", "2 players tracked").as_deref(),
            Some("玩家: 2 名")
        );
        assert_eq!(
            zh_body(
                "[view inventory]",
                "26 items (equip=4 use=3 etc=5 meso=1005106)"
            )
            .as_deref(),
            Some("背包: 26 件 (装备4 消耗3 其他5 金币1005106)")
        );
        assert_eq!(
            zh_body("[view drops]", "7 tracked").as_deref(),
            Some("掉落: 7 个")
        );
        // 已穿戴装备: 汇总行 + 明细行
        assert_eq!(
            zh_body("[view equiped]", "3 worn").as_deref(),
            Some("已穿戴装备: 3 件")
        );
        assert_eq!(
            zh_body("[view equiped]", "武器 itemid=1302000 (木剑) +5攻").as_deref(),
            Some("武器: 1302000 (木剑) +5攻")
        );
        assert_eq!(
            zh_body("[view equiped]", "nothing worn").as_deref(),
            Some("未穿戴装备")
        );
        // 装备信息: 汇总 + 背包/已穿戴分组
        assert_eq!(
            zh_body(
                "[view eqpinfo]",
                "item 1302000: 2 found (1 in backpack, 1 worn)"
            )
            .as_deref(),
            Some("装备 1302000: 共 2 件 (背包 1, 已穿戴 1)")
        );
        assert_eq!(
            zh_body(
                "[view eqpinfo]",
                "backpack slot=5 itemid=1302000 (木剑) +5攻"
            )
            .as_deref(),
            Some("背包 槽位5: 1302000 (木剑) +5攻")
        );
        assert_eq!(
            zh_body("[view eqpinfo]", "worn 武器 itemid=1302000 (木剑) +5攻").as_deref(),
            Some("已穿戴 武器: 1302000 (木剑) +5攻")
        );
        assert_eq!(
            zh_body("[view eqpinfo]", "item 9999999 not found").as_deref(),
            Some("未找到装备: item 9999999 not found")
        );
    }

    #[test]
    fn status_lines_translate() {
        // rule status
        assert_eq!(
            zh_body("[rule]", "1. sellauto (off)").as_deref(),
            Some("规则 1sellauto (关)")
        );
        assert_eq!(
            zh_body("[rule]", "when: mapid==104040000").as_deref(),
            Some("条件: mapid==104040000")
        );
        assert_eq!(
            zh_body("[rule]", "then: task run town_sell").as_deref(),
            Some("动作: task run town_sell")
        );
        assert_eq!(
            zh_body("[rule]", "cooldown: 60000ms").as_deref(),
            Some("冷却: 60000ms")
        );
        // group status
        assert_eq!(
            zh_body("[group]", "1. bf_money (off, exclusive) rules=2 tasks=0").as_deref(),
            Some("组 1bf_money (关, 互斥) 规则=2 任务=0")
        );
        assert_eq!(
            zh_body("[group]", "rule 1: bf_money_open").as_deref(),
            Some("规则 1: bf_money_open")
        );
        assert_eq!(
            zh_body("[group]", "task 1: bf_open_shop").as_deref(),
            Some("任务 1: bf_open_shop")
        );
        assert_eq!(
            zh_body("[group]", "step 1: reward 2022467 (wait: dialog==1)").as_deref(),
            Some("步骤 1: reward 2022467 (等待: dialog==1)")
        );
        // task status
        assert_eq!(
            zh_body("[task]", "no active task").as_deref(),
            Some("无活动任务")
        );
        assert_eq!(
            zh_body("[task]", "-> 'bf_open_shop' step 2/4").as_deref(),
            Some("-> 'bf_open_shop' 步骤 2/4")
        );
        assert_eq!(
            zh_body("[task]", "1. town_sell priority=10 steps=10").as_deref(),
            Some("任务 1town_sell (优先级10 步骤10)")
        );
        assert_eq!(
            zh_body("[task]", "step 1: town 100000000 east00").as_deref(),
            Some("步骤 1: town 100000000 east00")
        );
    }

    #[test]
    fn hunt_status_translate() {
        assert_eq!(
            zh_body("[hunt]", "status=on mode=skill skill=0").as_deref(),
            Some("状态=开 攻击方式=技能 ")
        );
        assert_eq!(
            zh_body("[hunt]", "status=off mode=skill skill=1001005").as_deref(),
            Some("状态=关 攻击方式=技能 技能 1001005 ")
        );
        assert_eq!(
            zh_body("[hunt]", "pickup=500 enabled cooldown=1500ms").as_deref(),
            Some("拾取=500px 攻击冷却=1500ms")
        );
        assert_eq!(
            zh_body("[hunt]", "pickup=0 enabled cooldown=700ms").as_deref(),
            Some("拾取=全图 攻击冷却=700ms")
        );
        assert_eq!(
            zh_body("[hunt]", "pickup=disabled cooldown=600ms").as_deref(),
            Some("拾取=关 攻击冷却=600ms")
        );
        assert_eq!(
            zh_body("[hunt]", "pickup_filter=off items=[]").as_deref(),
            Some("拾取过滤: 不过滤")
        );
        // Note: item names render as "未知" in TUI tests (data/ is not on the
        // test cwd path); the real-name mapping is asserted in openstory-bot's
        // names tests. This asserts the structure/count.
        assert_eq!(
            zh_body("[hunt]", "pickup_filter=deny items=[2000000,2000001]").as_deref(),
            Some("拾取过滤: 禁捡2种 未知 未知")
        );
        assert_eq!(
            zh_body("[hunt]", "pickup_filter=allow items=[2000002]").as_deref(),
            Some("拾取过滤: 仅捡1种 未知")
        );
        assert_eq!(
            zh_body("[hunt]", "filter=allow allow=[2000001] deny=[4000000]").as_deref(),
            Some("拾取过滤: 仅捡(生效) 仅拾取=[未知] 仅过滤=[未知]")
        );
        assert_eq!(
            zh_body("[hunt]", "filter=off allow=[] deny=[]").as_deref(),
            Some("拾取过滤: 不过滤")
        );
        assert_eq!(
            zh_body("[hunt]", "range=70 teleport_delay=0ms").as_deref(),
            Some("攻击距离=70 传送间隔=0ms")
        );
        assert_eq!(
            zh_body(
                "[hunt]",
                "attack_max=6 skill_range=200 max_targets=6 mp_cost=7 hits=2"
            )
            .as_deref(),
            Some("普攻数量=6 技能范围=200 技能目标=6 耗蓝=7 段数=2")
        );
        assert_eq!(
            zh_body("[hunt]", "damage=auto(lvl^2/2) until=none").as_deref(),
            Some("伤害=自动 (等级²/2) 自动停止=无")
        );
        assert_eq!(
            zh_body("[hunt]", "damage=override(200) until=equips>=90").as_deref(),
            Some("伤害=覆盖 (200) 自动停止=equips>=90")
        );
        assert_eq!(zh_body("[hunt]", "stand=on").as_deref(), Some("原地=开"));
        assert_eq!(zh_body("[hunt]", "stand=off").as_deref(), Some("原地=关"));
        assert_eq!(zh_body("[gather]", "= on").as_deref(), Some("吸怪=开"));
        assert_eq!(zh_body("[gather]", "= off").as_deref(), Some("吸怪=关"));
        assert_eq!(
            zh_body("[gather]", "step = 150px").as_deref(),
            Some("吸怪步长=150px")
        );
        assert_eq!(
            zh_body("[gather]", "pulled 12 mob(s) toward player").as_deref(),
            Some("吸怪: 拉取 12 只怪物")
        );
    }

    #[test]
    fn interactive_hints_hidden() {
        assert!(zh_hidden(
            "[serverlist]",
            "interactive: pick with 'login world <index> [channel]'"
        ));
        assert!(zh_hidden(
            "[charlist]",
            "interactive: pick with 'login char <index>'"
        ));
        assert!(!zh_hidden(
            "[charlist]",
            "--charlist-only: stopping before map entry (no CHAR_SELECT sent)"
        ));
        assert!(!zh_hidden("[chat]", "hi"));
    }
}
