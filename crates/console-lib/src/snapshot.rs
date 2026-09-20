//! 游戏状态快照: 渲染线程 try_lock 一次取全, 渲染期间不持锁。

use openstory_bot::state::{BotState, EntityKind, Phase};

#[derive(Debug, Clone, Default)]
pub struct SceneCounts {
    pub mobs: usize,
    pub players: usize,
    pub drops: usize,
    pub npcs: usize,
    pub reactors: usize,
}

#[derive(Debug, Clone, Default)]
pub struct InvCounts {
    pub equip: usize,
    pub consume: usize,
    pub setup: usize,
    pub etc: usize,
    pub cash: usize,
    pub worn: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Toggles {
    pub hunt: bool,
    pub hunt_reactor: bool,
    pub attack_mode: String,
    pub pickup_enabled: bool,
    pub pickup_range: i32,
    /// 拾取过滤: off/allow/deny 模式 + 双清单
    pub pickup_filter_mode: String,
    pub pickup_allow: Vec<i32>,
    pub pickup_deny: Vec<i32>,
    pub attack_skill: i32,
    // ---- hunt 配置详情 (侧边栏挂机面板展示, 免查 hunt status) ----
    pub attack_cooldown: u64,
    pub attack_controller_only: bool,
    pub attack_range: i32,
    pub teleport_delay: u64,
    pub attack_max_targets: usize,
    pub skill_range: i32,
    pub skill_max_targets: usize,
    pub skill_mp_cost: i16,
    pub skill_hits: u8,
    pub damage: Option<i32>,
    pub until: Option<String>,
    /// (自动重连开关, 间隔秒, 最大次数 0=无限)
    pub reconnect: (bool, u64, u64),
    pub gather: bool,
    pub gather_step: i32,
    pub gather_interval: u64,
    pub gather_max: usize,
    pub hunt_stand: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub phase: Phase,
    pub hp: i16,
    pub maxhp: i16,
    pub mp: i16,
    pub maxmp: i16,
    pub exp: i32,
    pub level: u8,
    pub ap: i16,
    pub sp: i16,
    pub meso: i32,
    pub mapid: i32,
    /// 当前频道 (0 起)
    pub channel: i32,
    pub pos: (i16, i16),
    pub ground_y: i16,
    pub facing: u8,
    pub my_cid: i32,
    /// 当前角色名
    pub name: String,
    pub counts: SceneCounts,
    pub inv: InvCounts,
    pub toggles: Toggles,
    /// (partyid, 成员数, 队长名)
    pub party: Option<(i32, usize, String)>,
    /// 交易描述 (活动窗口/邀请)
    pub trade: Option<String>,
    pub shop_open: bool,
    pub dialog_open: bool,
    /// 当前 NPC 商店商品: (itemid, price, qty)。TUI 商店购买弹框的数据源
    /// (shop menu 开关开启时自动弹出, 每次 0x146 到达刷新)。
    pub shop_items: Vec<(i32, i32, i16)>,
    /// 商店清单版本号: 每次服务器打开/刷新商店 +1, 关闭时也 +1。
    /// TUI 用它与本地已见 seq 对比判断"新清单到了"。
    pub shop_seq: u64,
    /// 当前 NPC sendSimple 菜单选项: (option id, 清洗后标签)。
    /// TUI 选项弹框的选单数据 (开关开启时自动弹出)。
    pub npc_options: Vec<(i32, String)>,
    /// NPC 对话内容版本号: 每次服务器发来新对话页 +1。
    /// TUI 用它与本地已见 seq 对比判断"新菜单到了"。
    pub npc_options_seq: u64,
    pub skills: usize,
    pub keymap: usize,
    /// 任务栈 (栈顶在前): (def, step, total)
    pub task_stack: Vec<(String, usize, usize)>,
    /// 规则: (开启?, id)
    pub rules: Vec<(bool, String)>,
    /// 组: (开启?, id)
    pub groups: Vec<(bool, String)>,
    /// 商城 (nx, points, items)
    pub cs: Option<(i32, i32, usize)>,
    /// 世界列表: (wid, name, 频道数)
    pub worlds: Vec<(i8, String, u8)>,
    /// 角色列表 (去重): (id, name, job, level)
    pub characters: Vec<(i32, String, i16, u8)>,
    /// 动态补全数据源
    /// NPC 候选: (oid, npcid) — 展示名由 npcid 解析
    pub npcs: Vec<(i32, i32)>,
    pub mob_oids: Vec<i32>,
    pub reactor_oids: Vec<i32>,
    pub rule_ids: Vec<String>,
    pub task_ids: Vec<String>,
    pub group_ids: Vec<String>,
    /// 当前地图传送门: (name, target_map_id) — 用于 warp 补全
    pub portal_data: Vec<(String, i32)>,
    /// 背包明细 (bag 界面): (tab, slot, itemid, qty)，按 (tab, slot) 排序
    pub inventory: Vec<(u8, i16, i32, i16)>,
    /// 背包 + 已装备的道具 id (去重) — `view eqpinfo` / `sell` / `drop` 补全用
    pub item_ids: Vec<i32>,
    /// 已学技能 id (去重) — `skill learn` / `skill info` 补全用
    pub skill_ids: Vec<i32>,
    /// 「状态」页: 运行时状态行 (左列)
    pub status_state: Vec<String>,
    /// 「状态」页: 内存 config 行 (右列)
    pub status_cfg: Vec<String>,
}

impl Snapshot {
    pub fn from(st: &BotState) -> Self {
        let mut mobs = 0;
        let mut players = 0;
        for e in st.entities.values() {
            match e.kind {
                EntityKind::Mob => mobs += 1,
                EntityKind::Player => players += 1,
                EntityKind::Unknown => {}
            }
        }
        let mut inv = InvCounts {
            worn: st.equipped.len(),
            ..InvCounts::default()
        };
        for ((tab, _), _) in &st.inventory {
            match tab {
                1 => inv.equip += 1,
                2 => inv.consume += 1,
                3 => inv.setup += 1,
                4 => inv.etc += 1,
                5 => inv.cash += 1,
                _ => {}
            }
        }
        let party = st.party.as_ref().map(|p| {
            let leader = p
                .members
                .values()
                .find(|m| m.id == p.leader_id)
                .map(|m| m.name.clone())
                .unwrap_or_else(|| format!("id={}", p.leader_id));
            (p.partyid, p.members.len(), leader)
        });
        let trade = if st.trade.active {
            Some(format!(
                "与 '{}' 交易中{}",
                st.trade.partner,
                if st.trade.locked { " [已锁定]" } else { "" }
            ))
        } else if let Some(inv_) = &st.trade.invite {
            Some(format!(
                "'{}' 邀请中 ({}s前)",
                inv_.name,
                inv_.time.elapsed().as_secs()
            ))
        } else {
            None
        };
        let mut characters: Vec<(i32, String, i16, u8)> = Vec::new();
        for c in &st.characters {
            if !characters.iter().any(|(id, _, _, _)| *id == c.id) {
                characters.push((c.id, c.stats.name.clone(), c.stats.job, c.stats.level));
            }
        }
        let task_stack = st
            .task_stack
            .iter()
            .rev()
            .map(|rt| (rt.def_id.clone(), rt.step_idx + 1, rt.steps.len()))
            .collect();
        Self {
            phase: st.phase,
            hp: st.hp,
            maxhp: st.maxhp,
            mp: st.mp,
            maxmp: st.maxmp,
            exp: st.exp,
            level: st.level,
            ap: st.ap,
            sp: st.sp,
            meso: st.meso,
            mapid: st.mapid,
            channel: st.channel,
            pos: st.position,
            ground_y: st.ground_y,
            facing: st.facing,
            my_cid: st.my_cid,
            name: st.name.clone(),
            counts: SceneCounts {
                mobs,
                players,
                drops: st.drops.len(),
                npcs: st.npcs.len(),
                reactors: st.reactors.len(),
            },
            inv,
            toggles: Toggles {
                hunt: st.hunt,
                hunt_reactor: st.hunt_reactor,
                attack_mode: st.cfg.hunt.attack_mode.clone(),
                pickup_enabled: st.cfg.hunt.pickup_enabled,
                pickup_range: st.cfg.hunt.pickup_range,
                pickup_filter_mode: st.cfg.hunt.pickup_filter_mode.clone(),
                pickup_allow: st.cfg.hunt.pickup_allow.clone(),
                pickup_deny: st.cfg.hunt.pickup_deny.clone(),
                attack_skill: st.cfg.hunt.attack_skill,
                attack_cooldown: st.cfg.hunt.attack_cooldown,
                attack_controller_only: st.cfg.hunt.attack_controller_only,
                attack_range: st.cfg.hunt.attack_range,
                teleport_delay: st.cfg.hunt.teleport_delay,
                attack_max_targets: st.cfg.hunt.attack_max_targets,
                skill_range: st.cfg.hunt.attack_skill_range,
                skill_max_targets: st.cfg.hunt.attack_skill_max_targets,
                skill_mp_cost: st.cfg.hunt.attack_skill_mp_cost,
                skill_hits: st.cfg.hunt.attack_skill_hits,
                damage: (st.cfg.hunt.damage > 0).then_some(st.cfg.hunt.damage),
                until: st.cfg.hunt.until.clone(),
                reconnect: (
                    st.cfg.reconnect,
                    st.cfg.reconnect_delay,
                    st.cfg.reconnect_max as u64,
                ),
                gather: st.gather,
                gather_step: st.cfg.hunt.gather_step,
                gather_interval: st.cfg.hunt.gather_interval,
                gather_max: st.cfg.hunt.gather_max,
                hunt_stand: st.hunt_stand,
            },
            party,
            trade,
            shop_open: st.shop_open,
            dialog_open: st.dialog_open,
            shop_items: st
                .shop_items
                .iter()
                .map(|it| (it.itemid, it.price, it.qty))
                .collect(),
            shop_seq: st.shop_seq,
            npc_options: st.npc_options.clone(),
            npc_options_seq: st.npc_options_seq,
            skills: st.skills.len(),
            keymap: st.keymap.len(),
            task_stack,
            rules: st
                .cfg
                .rules
                .iter()
                .map(|r| (r.enabled, r.id.clone()))
                .collect(),
            groups: st
                .cfg
                .groups
                .iter()
                .map(|g| (g.enabled, g.id.clone()))
                .collect(),
            cs: if st.phase == Phase::CashShop {
                Some((st.cs_nx, st.cs_points, st.cs_inventory.len()))
            } else {
                None
            },
            worlds: {
                let mut ws: Vec<(i8, String, u8)> = Vec::new();
                for w in &st.worlds {
                    if !ws.iter().any(|(wid, _, _)| *wid == w.wid) {
                        ws.push((w.wid, w.name.clone(), w.channelcount));
                    }
                }
                ws
            },
            characters,
            npcs: st.npcs.values().map(|n| (n.oid, n.npcid)).collect(),
            inventory: st
                .inventory
                .iter()
                .map(|((tab, slot), it)| (*tab, *slot, it.itemid, it.qty))
                .collect(),
            mob_oids: st
                .entities
                .values()
                .filter(|e| e.kind == EntityKind::Mob)
                .map(|e| e.oid)
                .collect(),
            reactor_oids: st.reactors.keys().copied().collect(),
            rule_ids: st.cfg.rules.iter().map(|r| r.id.clone()).collect(),
            task_ids: st.cfg.tasks.iter().map(|t| t.id.clone()).collect(),
            group_ids: st.cfg.groups.iter().map(|g| g.id.clone()).collect(),
            portal_data: openstory_bot::names::map_portals(st.mapid)
                .into_iter()
                .map(|p| (p.name, p.target_map))
                .collect(),
            item_ids: {
                let mut v: Vec<i32> = st
                    .inventory
                    .values()
                    .map(|it| it.itemid)
                    .chain(st.equipped.values().map(|it| it.itemid))
                    .collect();
                v.sort_unstable();
                v.dedup();
                v
            },
            skill_ids: {
                let mut v: Vec<i32> = st.skills.keys().copied().collect();
                v.sort_unstable();
                v.dedup();
                v
            },
            status_state: status_state_rows(st),
            status_cfg: status_cfg_rows(&st.cfg),
        }
    }
}

/// 「状态」页左列: 运行时 bot 状态一览 (诊断, 全中文)。
fn status_state_rows(st: &BotState) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    v.push(format!("阶段: {}", phase_zh(st.phase)));
    v.push(format!(
        "地图: {} ({})",
        openstory_bot::names::map_name(st.mapid),
        st.mapid
    ));
    v.push(format!("频道: {}", st.channel));
    v.push(format!(
        "位置: ({},{})  地面: {}  朝向: {}",
        st.position.0,
        st.position.1,
        st.ground_y,
        if st.facing & 1 == 1 { "左" } else { "右" }
    ));
    v.push(format!(
        "血量: {}/{}  蓝量: {}/{}",
        st.hp, st.maxhp, st.mp, st.maxmp
    ));
    v.push(format!(
        "经验: {}  等级: {}  升级还需: {}  金币: {}",
        st.exp,
        st.level,
        openstory_bot::exptable::exp_remain(st.level as i32, st.exp as i64),
        st.meso
    ));
    v.push(format!("属性点: {}  技能点: {}", st.ap, st.sp));
    v.push(format!("角色: {} (id {})", st.name, st.my_cid));
    v.push(format!(
        "挂机图: {}",
        openstory_bot::names::map_name(st.hunt_mapid)
    ));
    v.push(format!(
        "打怪: {}  原地: {}",
        on_off(st.hunt),
        on_off(st.hunt_stand)
    ));
    v.push(format!(
        "吸怪: {}  拾取: {}",
        on_off(st.gather),
        on_off(st.cfg.hunt.pickup_enabled)
    ));
    v.push(format!(
        "攻击目标: {}  追踪: {}",
        st.attack_target
            .map(|o| o.to_string())
            .unwrap_or_else(|| "无".into()),
        st.hunt_target
            .map(|o| o.to_string())
            .unwrap_or_else(|| "无".into())
    ));
    v.push(format!(
        "伤害: {}",
        match (st.cfg.hunt.damage > 0).then_some(st.cfg.hunt.damage) {
            Some(d) => format!("固定{d}"),
            None => "自动(等级²/2)".to_string(),
        }
    ));
    v.push(format!(
        "自动停: {}",
        st.cfg.hunt.until.as_deref().unwrap_or("无")
    ));
    let mobs = st
        .entities
        .values()
        .filter(|e| e.kind == EntityKind::Mob)
        .count();
    let players = st
        .entities
        .values()
        .filter(|e| e.kind == EntityKind::Player)
        .count();
    v.push(format!(
        "怪物: {mobs}  玩家: {players}  掉落: {}",
        st.drops.len()
    ));
    v.push(format!(
        "NPC: {}  放置物: {}",
        st.npcs.len(),
        st.reactors.len()
    ));
    let equip = st
        .inventory
        .iter()
        .filter(|((tab, _), _)| *tab == 1)
        .count();
    let consume = st
        .inventory
        .iter()
        .filter(|((tab, _), _)| *tab == 2)
        .count();
    let other = st
        .inventory
        .iter()
        .filter(|((tab, _), _)| *tab == 3 || *tab == 4)
        .count();
    let cash = st
        .inventory
        .iter()
        .filter(|((tab, _), _)| *tab == 5)
        .count();
    v.push(format!(
        "背包: 装备{equip}  消耗{consume}  其他{other}  现金{cash}  已穿{}",
        st.equipped.len()
    ));
    v.push(format!(
        "技能: {}  按键: {}  商店: {}  对话: {}",
        st.skills.len(),
        st.keymap.len(),
        on_off(st.shop_open),
        on_off(st.dialog_open)
    ));
    let task_stack: Vec<String> = st
        .task_stack
        .iter()
        .rev()
        .map(|rt| format!("{}({}/{})", rt.def_id, rt.step_idx + 1, rt.steps.len()))
        .collect();
    v.push(format!(
        "任务栈: {}",
        if task_stack.is_empty() {
            "空".to_string()
        } else {
            task_stack.join(" ")
        }
    ));
    v.push(format!(
        "规则: {}",
        if st.cfg.rules.is_empty() {
            "无".to_string()
        } else {
            st.cfg
                .rules
                .iter()
                .map(|r| format!("{}{}", r.id, if r.enabled { "(开)" } else { "(关)" }))
                .collect::<Vec<_>>()
                .join(" ")
        }
    ));
    v.push(format!(
        "分组: {}",
        if st.cfg.groups.is_empty() {
            "无".to_string()
        } else {
            st.cfg
                .groups
                .iter()
                .map(|g| format!("{}{}", g.id, if g.enabled { "(开)" } else { "(关)" }))
                .collect::<Vec<_>>()
                .join(" ")
        }
    ));
    v
}

fn phase_zh(p: Phase) -> &'static str {
    match p {
        Phase::Disconnected => "未连接",
        Phase::Connecting => "连接中",
        Phase::LoggingIn => "登录中",
        Phase::GenderPick => "选择性别",
        Phase::WorldSelect => "选择服务器",
        Phase::CharSelect => "选择角色",
        Phase::EnteringMap => "进入地图",
        Phase::InGame => "游戏中",
        Phase::CashShop => "商城中",
    }
}

fn on_off(b: bool) -> &'static str {
    if b {
        "开"
    } else {
        "关"
    }
}

fn attack_mode_zh(m: &str) -> String {
    match m {
        "skill" => "技能".to_string(),
        "attack" => "普攻".to_string(),
        other => other.to_string(),
    }
}

/// Pickup filter summary with item Chinese names: 仅捡/禁捡 + count + first
/// few names (long lists truncated for one-line display).
pub fn pickup_filter_zh(mode: &str, allow: &[i32], deny: &[i32]) -> String {
    let (label, ids) = match mode {
        "allow" => ("仅捡", allow),
        "deny" => ("禁捡", deny),
        _ => return "不过滤".to_string(),
    };
    if ids.is_empty() {
        return format!("{label} 0种");
    }
    let mut names: Vec<String> = ids
        .iter()
        .take(3)
        .map(|id| openstory_bot::names::item_name_text(*id))
        .collect();
    if ids.len() > 3 {
        names.push(format!("+{}种", ids.len() - 3));
    }
    format!("{label} {}种: {}", ids.len(), names.join(" "))
}

/// 「状态」页右列: 内存 config 一览 (诊断, 全中文)。
fn status_cfg_rows(cfg: &openstory_bot::runtime_config::RuntimeConfig) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    let h = &cfg.hunt;
    v.push(format!(
        "打怪·攻击方式: {}  技能 id: {}",
        attack_mode_zh(&h.attack_mode),
        h.attack_skill
    ));
    v.push(format!(
        "打怪·攻击距离: {}px  技能距离: {}px",
        h.attack_range, h.attack_skill_range
    ));
    v.push(format!(
        "打怪·普攻数量: {}  技能目标: {}",
        h.attack_max_targets, h.attack_skill_max_targets
    ));
    v.push(format!("打怪·技能蓝耗: {}", h.attack_skill_mp_cost));
    v.push(format!("打怪·攻击冷却: {}ms", h.attack_cooldown));
    v.push(format!("打怪·瞬移冷却: {}ms", h.teleport_delay));
    v.push(format!(
        "打怪·拾取: {} ({})",
        on_off(h.pickup_enabled),
        if h.pickup_range <= 0 {
            "全图".to_string()
        } else {
            format!("{}px", h.pickup_range)
        }
    ));
    if h.pickup_enabled {
        v.push(format!(
            "打怪·过滤: {}",
            pickup_filter_zh(&h.pickup_filter_mode, &h.pickup_allow, &h.pickup_deny)
        ));
    }
    v.push(format!(
        "吸怪·步长: {}px  间隔: {}ms  每轮: {} 只",
        h.gather_step, h.gather_interval, h.gather_max
    ));
    v.push(format!(
        "打怪·自动停: {}",
        h.until.as_deref().unwrap_or("无")
    ));
    v.push(format!(
        "主循环: {}ms  喝药冷却: {}ms",
        cfg.tick_ms, cfg.potion_cooldown
    ));
    v.push(format!(
        "自动重连: {} ({} 秒 ×{})",
        on_off(cfg.reconnect),
        cfg.reconnect_delay,
        cfg.reconnect_max
    ));
    v.push(format!("密语白名单: {:?}", cfg.chat_admins));
    v.push(format!(
        "登录: {}:{} ({})",
        cfg.login.ip,
        cfg.login.port.unwrap_or(8484),
        cfg.login.account
    ));
    if cfg.potion.is_empty() {
        v.push("喝药规则: 无".to_string());
    } else {
        v.push(format!(
            "喝药规则: {}",
            cfg.potion
                .iter()
                .map(|p| format!("{}{}%", p.stat, p.threshold_pct))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    v.push(format!(
        "规则: {}",
        if cfg.rules.is_empty() {
            "无".to_string()
        } else {
            cfg.rules
                .iter()
                .map(|r| format!("{}({})", r.id, if r.enabled { "开" } else { "关" }))
                .collect::<Vec<_>>()
                .join(" ")
        }
    ));
    v.push(format!(
        "任务: {}",
        if cfg.tasks.is_empty() {
            "无".to_string()
        } else {
            cfg.tasks
                .iter()
                .map(|t| format!("{}{}", t.id, if t.recurring { "(循环)" } else { "" }))
                .collect::<Vec<_>>()
                .join(" ")
        }
    ));
    v.push(format!(
        "分组: {}",
        if cfg.groups.is_empty() {
            "无".to_string()
        } else {
            cfg.groups
                .iter()
                .map(|g| format!("{}({})", g.id, if g.enabled { "开" } else { "关" }))
                .collect::<Vec<_>>()
                .join(" ")
        }
    ));
    v
}
