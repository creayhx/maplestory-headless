//! 命令规格表 —— **补全 / 帮助 / 解析器一致性测试的唯一数据源**。
//!
//! # 为什么需要这张表
//!
//! 改造前有两份手工维护的真相: 本表(补全/帮助) 与 `openstory_bot::command::parse`
//! (解析器)。两边已经漂移, 例如:
//!
//! - `task stop` 解析器接受, 补全表却没写 -> 用户敲 `task ` 后 Tab 看不到它
//! - `group start` / `task cancel` 同理
//! - `sell tab` / `view eqpinfo` / `view portals [mapid]` 完全缺失
//! - `buff cancel <skillid>`、`cashshop buy` 的参数无法补全
//! - 早期版本的 `group` 子命令表与 `parse.rs`、`docs/GUIDE.md` 三者措辞不一致
//!
//! 数据驱动改造后, 新增/修改命令只需动这一处, 并由
//! `tests/spec_matches_parser.rs` 强制校验: 表里宣称能用的写法, 解析器必须接受。
//! 漂移会直接让测试变红, 而不是让用户在使用中发现。
//!
//! # 三级补全如何由本表推导
//!
//! 1. 一级: [`COMMANDS`] 的 `name` + `aliases` (别名补全成规范名)
//! 2. 二级: [`CmdSpec::subs`] 的 `name` (静态子命令)
//! 3. 三级: [`CmdSpec::dyn_args`] 按**前缀匹配**给出动态候选
//!    (`npc ` -> oid+中文名, `warp ` -> 传送点名+目标地图名, ...)
//!
//! 有了 (3) 之后, 原先逐个命令手写 `match` 的 `sub_or_dyn` 被彻底删除:
//! 新增一条命令只改这张表一处。

/// 参数的**动态候选来源**。只描述"去哪里取候选", 不描述校验规则
/// (校验永远在解析器里, 本表不参与校验, 避免出现第三份真相)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgSpec {
    /// 无参数
    None,
    /// 自由文本 (补不出候选)
    Free,
    /// 数值
    Number,
    /// 地图 id
    MapId,
    /// 怪物 oid (当前地图)
    MobOid,
    /// 放置物 oid (当前地图)
    ReactorOid,
    /// NPC oid (当前地图, 带中文名描述)
    NpcOid,
    /// 传送点名 (当前地图, 带目标地图名描述)
    Portal,
    /// 配置里的规则 id
    RuleId,
    /// 配置里的任务 id
    TaskId,
    /// 配置里的功能组 id
    GroupId,
    /// 技能 id
    SkillId,
    /// 道具 id
    ItemId,
    /// 固定取值列表
    Values(&'static [&'static str]),
    /// 档案文件路径 (`profiles add <路径>`) —— 候选来自已扫描到的档案列表
    Path,
}

impl ArgSpec {
    /// 该参数是否**必须**取本表枚举的值 (供帮助文本与"补不出就继续列子命令"
    /// 的判断)。
    ///
    /// `Values` 一律返回 false: 表里的取值是"常用写法", 不是封闭集合 ——
    /// 例如 `hunt pickup on|off|<px>` 的 `Values(&["on","off"])` 之后还能接
    /// 像素半径。把它当必填会让补全在用户敲数字时错误地回退到子命令列表。
    pub fn is_required(self) -> bool {
        !matches!(
            self,
            ArgSpec::None | ArgSpec::Values(_) | ArgSpec::Free | ArgSpec::Path
        )
    }

    /// 帮助文本里的参数占位符, 如 `<mapid>` / `on|off`。
    pub fn placeholder(self) -> String {
        match self {
            ArgSpec::None | ArgSpec::Free => String::new(),
            ArgSpec::Path => "<路径>".into(),
            ArgSpec::Number => "<n>".into(),
            ArgSpec::MapId => "<mapid>".into(),
            ArgSpec::MobOid => "<mob_oid>".into(),
            ArgSpec::ReactorOid => "<reactor_oid>".into(),
            ArgSpec::NpcOid => "<npc_oid>".into(),
            ArgSpec::Portal => "<portal>".into(),
            ArgSpec::RuleId => "<rule_id>".into(),
            ArgSpec::TaskId => "<task_id>".into(),
            ArgSpec::GroupId => "<group_id>".into(),
            ArgSpec::SkillId => "<skillid>".into(),
            ArgSpec::ItemId => "<itemid>".into(),
            ArgSpec::Values(vs) => vs.join("|"),
        }
    }
}

/// 参数格数。
///
/// 描述"构成一条合法指令需要几格参数"以及每一格的候选来源。用于:
/// - 帮助文本的参数占位符
/// - 一致性测试把裸写法补成完整指令 (`skill cast` 单独写必然被解析器拒绝,
///   校验"子命令存在"时必须带上参数)
///
/// 绝大多数命令是 0 或 1 格; `skill cast <skillid> <oid>` 是 2 格;
/// `keymap set <key> <type> <action>` 是 3 格 (纯数值, 无候选)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arity {
    pub a1: ArgSpec,
    pub a2: ArgSpec,
    pub a3: ArgSpec,
}

impl Arity {
    pub const NONE: Arity = Arity {
        a1: ArgSpec::None,
        a2: ArgSpec::None,
        a3: ArgSpec::None,
    };

    pub const fn one(a1: ArgSpec) -> Arity {
        Arity {
            a1,
            a2: ArgSpec::None,
            a3: ArgSpec::None,
        }
    }

    pub const fn two(a1: ArgSpec, a2: ArgSpec) -> Arity {
        Arity {
            a1,
            a2,
            a3: ArgSpec::None,
        }
    }

    pub const fn three(a1: ArgSpec, a2: ArgSpec, a3: ArgSpec) -> Arity {
        Arity { a1, a2, a3 }
    }

    /// 需要几格参数。
    pub fn count(self) -> usize {
        if self.a1 == ArgSpec::None {
            0
        } else if self.a2 == ArgSpec::None {
            1
        } else if self.a3 == ArgSpec::None {
            2
        } else {
            3
        }
    }

    /// 第 `n` 格 (从 1 开始) 的参数类型。
    pub fn nth(self, n: usize) -> ArgSpec {
        match n {
            1 => self.a1,
            2 => self.a2,
            3 => self.a3,
            _ => ArgSpec::None,
        }
    }

    /// 帮助文本里的参数串。
    pub fn placeholders(self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for n in 1..=self.count() {
            let p = self.nth(n).placeholder();
            if !p.is_empty() {
                parts.push(p);
            }
        }
        parts.join(" ")
    }
}

/// 一个静态子命令。
#[derive(Debug, Clone, Copy)]
pub struct SubSpec {
    pub name: &'static str,
    /// 该子命令之后的参数 (用于帮助文本; 动态候选由
    /// [`CmdSpec::dyn_args`] 按前缀提供)。
    pub arity: Arity,
}

/// 一条命令的规格。
#[derive(Debug, Clone, Copy)]
pub struct CmdSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    /// 中文一句话说明 (帮助面板第二列)
    pub usage: &'static str,
    pub subs: &'static [SubSpec],
    /// 本命令自身的参数 (无子命令时使用)
    pub arity: Arity,
    /// 动态候选表: 前缀 token 序列 -> 该前缀之后的参数类型。
    ///
    /// 前缀匹配当前输入的前 N 个 token (`reactor hit ` 与 `reactor hit 9`
    /// 都命中 `["reactor", "hit"]`), 取最长匹配。
    pub dyn_args: &'static [(&'static [&'static str], ArgSpec)],
    /// 只能在规则动作 / 任务步骤里用, 不能直接敲 (帮助里标注)
    pub action_only: bool,
    /// 一个能通过解析器的完整示例尾巴 (`while x do y`)。
    ///
    /// `arity` 的模型是"每格一个 token", 对 `while <谓词> do <命令>` 这类
    /// 复合写法不成立。一致性测试优先用它, 没有时才按 arity 拼示例。
    pub sample: Option<&'static str>,
}

/// 便捷构造函数。
///
/// 注意 **不要**用结构体更新语法 (`..cmd(..)`) 去改 `arg` / `subs` / `dyn_args`:
/// 函数式更新会用基值**覆盖**同名字段, 写出来的 `arg: X` 会被静默丢掉
/// (曾因此让 `move `/`npc ` 的补全整段失效)。一律用下面的 builder 方法。
const fn cmd(name: &'static str, aliases: &'static [&'static str], usage: &'static str) -> CmdSpec {
    CmdSpec {
        name,
        aliases,
        usage,
        subs: &[],
        arity: Arity::NONE,
        dyn_args: &[],
        action_only: false,
        sample: None,
    }
}

impl CmdSpec {
    /// 设置命令自身的参数格数。
    const fn arity(mut self, arity: Arity) -> Self {
        self.arity = arity;
        self
    }

    /// 设置静态子命令表。
    const fn subs(mut self, subs: &'static [SubSpec]) -> Self {
        self.subs = subs;
        self
    }

    /// 设置动态参数前缀表。
    const fn dyn_args(mut self, dyn_args: &'static [(&'static [&'static str], ArgSpec)]) -> Self {
        self.dyn_args = dyn_args;
        self
    }

    /// 设置一个能通过解析器的完整示例尾巴。
    const fn sample(mut self, s: &'static str) -> Self {
        self.sample = Some(s);
        self
    }

    /// 标注"只能在规则/任务动作里使用"。
    const fn action_only(mut self) -> Self {
        self.action_only = true;
        self
    }
}

const fn sub(name: &'static str) -> SubSpec {
    SubSpec {
        name,
        arity: Arity::NONE,
    }
}

const fn sub_arg(name: &'static str, arg: ArgSpec) -> SubSpec {
    SubSpec {
        name,
        arity: Arity::one(arg),
    }
}

impl SubSpec {
    /// 设置该子命令的参数格数。
    const fn arity(mut self, arity: Arity) -> Self {
        self.arity = arity;
        self
    }
}

/// 全部命令。**新增命令只改这里一处。**
///
/// 一律用 `cmd(..).arg(..).subs(..).dyn_args(..)` 的 builder 形式 ——
/// 不要用结构体更新语法, 它会静默覆盖同名字段 (见 `cmd` 的说明)。
pub static COMMANDS: &[CmdSpec] = &[
    cmd("view", &["v"], "查看状态/对象")
        .subs(&[
            sub("status"),
            sub("attr"),
            sub("player"),
            sub("mobs"),
            sub("players"),
            sub("nearby"),
            sub("npcs"),
            sub("drops"),
            sub_arg(
                "inventory",
                ArgSpec::Values(&["equip", "use", "setup", "etc", "cash"]),
            ),
            sub("inv"),
            sub("skills"),
            sub("equiped"),
            sub("equipped"),
            sub_arg("eqpinfo", ArgSpec::ItemId),
            sub("party"),
            sub("trade"),
            sub("reactor"),
            sub("keymap"),
            sub("cashshop"),
            sub("cs"),
            sub_arg("portals", ArgSpec::MapId),
        ])
        .dyn_args(&[
            (&["view", "eqpinfo"], ArgSpec::ItemId),
            (&["view", "portals"], ArgSpec::MapId),
        ]),
    cmd("hunt", &[], "打怪挂机开关/配置")
        .subs(&[
            sub("on"),
            sub("off"),
            sub_arg("attack", ArgSpec::Number),
            sub_arg("skill", ArgSpec::SkillId),
            sub_arg("hits", ArgSpec::Number),
            sub_arg("atkcd", ArgSpec::Number),
            sub_arg("range", ArgSpec::Number),
            sub_arg("damage", ArgSpec::Number),
            sub_arg("pickup", ArgSpec::Values(&["on", "off"])),
            // `hunt filter add|del` 需要 id (解析器强制), 补出裸的 `add` 会立刻
            // 报错, 所以只列不需要 id 的写法。带 id 的写法由 dyn_args 补。
            sub_arg("filter", ArgSpec::Values(&["off", "allow", "deny"])),
            sub_arg("stand", ArgSpec::Values(&["on", "off"])),
            sub_arg("controller", ArgSpec::Values(&["on", "off"])),
            sub("once"),
            sub("status"),
            sub("save"),
        ])
        // `hunt filter <op>` 的 op 是枚举 (由子命令的 arg 补); 后面的物品 id
        // 不做候选 —— 没有可靠来源, 且会在 `hunt filter ` 这一格抢先给出 id。
        .dyn_args(&[]),
    cmd("gather", &[], "吸怪(独立于打怪)").subs(&[
        sub("on"),
        sub("off"),
        sub_arg("step", ArgSpec::Number),
        sub_arg("interval", ArgSpec::Number),
        sub_arg("max", ArgSpec::Number),
        sub_arg("controller", ArgSpec::Values(&["on", "off"])),
    ]),
    cmd("reactor", &["reactors"], "放置物挂机/打放置物")
        .subs(&[
            sub("on"),
            sub("off"),
            sub_arg("hit", ArgSpec::ReactorOid),
            sub_arg("cooldown", ArgSpec::Number),
        ])
        .dyn_args(&[(&["reactor", "hit"], ArgSpec::ReactorOid)]),
    cmd("rule", &[], "规则开关")
        .subs(&[
            sub_arg("open", ArgSpec::RuleId),
            sub_arg("close", ArgSpec::RuleId),
            sub_arg("status", ArgSpec::RuleId),
        ])
        .dyn_args(&[
            (&["rule", "open"], ArgSpec::RuleId),
            (&["rule", "close"], ArgSpec::RuleId),
            (&["rule", "status"], ArgSpec::RuleId),
        ]),
    cmd("group", &[], "功能组运行/停止")
        .subs(&[
            sub_arg("run", ArgSpec::GroupId),
            sub_arg("open", ArgSpec::GroupId),
            sub_arg("stop", ArgSpec::GroupId),
            sub_arg("close", ArgSpec::GroupId),
            sub_arg("status", ArgSpec::GroupId),
        ])
        .dyn_args(&[
            (&["group", "run"], ArgSpec::GroupId),
            (&["group", "open"], ArgSpec::GroupId),
            (&["group", "stop"], ArgSpec::GroupId),
            (&["group", "close"], ArgSpec::GroupId),
            (&["group", "status"], ArgSpec::GroupId),
        ]),
    cmd("task", &[], "任务控制")
        .subs(&[
            sub_arg("run", ArgSpec::TaskId),
            sub_arg("start", ArgSpec::TaskId),
            sub("stop"),
            sub("status"),
        ])
        .dyn_args(&[
            (&["task", "run"], ArgSpec::TaskId),
            (&["task", "start"], ArgSpec::TaskId),
        ]),
    cmd("chat", &["say", "c"], "地图发言").arity(Arity::one(ArgSpec::Free)),
    cmd("move", &[], "移动(坐标/传送点)")
        .arity(Arity::one(ArgSpec::Free))
        .dyn_args(&[(&["move"], ArgSpec::Portal)]),
    cmd("movewarp", &["mw"], "移动+过图(传送点)")
        .arity(Arity::one(ArgSpec::Portal))
        .dyn_args(&[(&["movewarp"], ArgSpec::Portal), (&["mw"], ArgSpec::Portal)]),
    cmd("attack", &["atk"], "普攻怪物(oid)").arity(Arity::one(ArgSpec::MobOid)),
    cmd("buff", &[], "辅助技能")
        .arity(Arity::one(ArgSpec::SkillId))
        .subs(&[sub_arg("cancel", ArgSpec::SkillId)])
        .dyn_args(&[(&["buff", "cancel"], ArgSpec::SkillId)]),
    cmd("skill", &["skills", "sp"], "技能系统")
        .subs(&[
            sub_arg("learn", ArgSpec::SkillId),
            sub("cast").arity(Arity::two(ArgSpec::SkillId, ArgSpec::MobOid)),
            sub_arg("info", ArgSpec::SkillId),
            sub("status"),
        ])
        .dyn_args(&[
            (&["skill", "learn"], ArgSpec::SkillId),
            (&["skill", "info"], ArgSpec::SkillId),
            (&["skill", "cast"], ArgSpec::MobOid),
        ]),
    cmd("cast", &[], "施放攻击技能(同 skill cast)")
        .arity(Arity::two(ArgSpec::SkillId, ArgSpec::MobOid)),
    cmd("keymap", &[], "键盘绑定").subs(&[sub("set").arity(Arity::three(
        ArgSpec::Number,
        ArgSpec::Number,
        ArgSpec::Number,
    ))]),
    cmd("channel", &["ch", "cc"], "换线").arity(Arity::one(ArgSpec::Number)),
    cmd("cashshop", &["cs"], "进商城/商城操作").subs(&[
        sub("enter"),
        sub_arg("buy", ArgSpec::Number),
        sub_arg("get", ArgSpec::Values(&["last"])),
        sub("list"),
        sub("ls"),
        sub_arg("store", ArgSpec::Number),
        sub_arg("put", ArgSpec::Number),
        sub("out"),
        sub("leave"),
    ]),
    cmd("auction", &["mts"], "打开拍卖"),
    cmd("reenter", &["reentry"], "重进当前图(刷新)"),
    cmd("npc", &[], "NPC 对话 (npc <oid> / 子命令)")
        .arity(Arity::one(ArgSpec::NpcOid))
        .subs(&[
            sub("yes"),
            sub("y"),
            sub("no"),
            sub("n"),
            sub("next"),
            sub("ok"),
            sub("ack"),
            sub("prev"),
            sub("back"),
            sub("cancel"),
            sub("end"),
            sub("bye"),
            sub_arg("reply", ArgSpec::Number),
            sub_arg("select", ArgSpec::Number),
            sub_arg("num", ArgSpec::Number),
            sub_arg("number", ArgSpec::Number),
            sub_arg("text", ArgSpec::Free),
            sub_arg("input", ArgSpec::Free),
        ]),
    // 解析器要求 itemid **和** qty 都在 (`rest.len() < 2` 直接报 usage),
    // 所以这是 2 格参数 —— usage 字样里的 `[qty]` 与实际不符, 以解析器为准。
    cmd("buy", &[], "买商店物品 buy <itemid> <qty>")
        .arity(Arity::two(ArgSpec::ItemId, ArgSpec::Number)),
    cmd("sell", &[], "卖物品 <id> [qty] [slot <n>] | tab 整栏")
        .arity(Arity::one(ArgSpec::ItemId))
        .subs(&[sub_arg(
            "tab",
            ArgSpec::Values(&["equip", "consume", "etc"]),
        )])
        .dyn_args(&[
            (&["sell"], ArgSpec::ItemId),
            (
                &["sell", "tab"],
                ArgSpec::Values(&["equip", "consume", "etc"]),
            ),
        ]),
    cmd("sellammo", &["ammo"], "一键卖弹药(箭/飞镖/子弹)"),
    cmd("shop", &["leave"], "商店窗口/弹框开关").subs(&[sub("leave"), sub("menu")]),
    cmd("trade", &[], "玩家交易").subs(&[
        sub_arg("invite", ArgSpec::Free),
        sub("accept"),
        sub("confirm"),
        sub("decline"),
        sub("quit"),
        sub_arg("put", ArgSpec::ItemId),
        sub_arg("meso", ArgSpec::Number),
    ]),
    cmd("equip", &[], "穿装备").arity(Arity::one(ArgSpec::ItemId)),
    cmd("unequip", &["takeoff"], "脱装备").arity(Arity::one(ArgSpec::ItemId)),
    cmd("use", &["drink"], "使用道具").arity(Arity::one(ArgSpec::ItemId)),
    cmd("scroll", &["upgrade"], "使用卷轴 <卷轴id> <装备id> [bless]")
        .arity(Arity::two(ArgSpec::ItemId, ArgSpec::ItemId)),
    cmd("reward", &["open"], "使用功能箱").arity(Arity::one(ArgSpec::ItemId)),
    cmd("drop", &[], "丢弃道具").arity(Arity::one(ArgSpec::ItemId)),
    cmd("dropmeso", &["dropm"], "丢金币").arity(Arity::one(ArgSpec::Number)),
    cmd("ap", &[], "加点").subs(&[
        sub_arg("str", ArgSpec::Number),
        sub_arg("dex", ArgSpec::Number),
        sub_arg("int", ArgSpec::Number),
        sub_arg("luk", ArgSpec::Number),
        sub_arg("maxhp", ArgSpec::Number),
        sub_arg("maxmp", ArgSpec::Number),
        sub("status"),
    ]),
    cmd("party", &[], "队伍").subs(&[
        sub("create"),
        sub_arg("invite", ArgSpec::Free),
        sub_arg("invitecid", ArgSpec::Number),
        sub("leave"),
        sub_arg("kick", ArgSpec::Free),
        sub_arg("kickcid", ArgSpec::Number),
    ]),
    cmd("town", &[], "跨图移动").arity(Arity::one(ArgSpec::MapId)),
    cmd("warp", &[], "传送门传送")
        .arity(Arity::one(ArgSpec::Portal))
        .dyn_args(&[(&["warp"], ArgSpec::Portal)]),
    cmd("reconnect", &[], "自动重连设置").subs(&[
        sub("on"),
        sub("off"),
        sub_arg("delay", ArgSpec::Number),
        sub_arg("max", ArgSpec::Number),
        sub("status"),
    ]),
    cmd("pickup", &[], "拾取全部掉落").subs(&[sub("all")]),
    cmd("login", &[], "交互登录步骤").subs(&[
        sub_arg("world", ArgSpec::Number),
        sub_arg("char", ArgSpec::Number),
    ]),
    cmd("sleep", &[], "暂停 N 秒").arity(Arity::one(ArgSpec::Number)),
    cmd("setvar", &[], "设置流程变量 setvar <名> <值>")
        .arity(Arity::two(ArgSpec::Free, ArgSpec::Free)),
    cmd("clearvar", &[], "清除流程变量").arity(Arity::one(ArgSpec::Free)),
    cmd("wait", &[], "规则动作: 等待谓词成立")
        .arity(Arity::one(ArgSpec::Free))
        .action_only(),
    cmd(
        "while",
        &[],
        "规则动作: while <谓词> do <命令>, 成立时每 tick 执行",
    )
    .sample("x do y")
    .action_only(),
    cmd("reload", &["cfg"], "热重载 config.json"),
    // ── 控制台本地指令 (不发给 bot) ──────────────────────────────────
    //
    // 这几个由前端自己执行: `start`/`stop` 启停会话, `profiles` 增删账号。
    // 放进表里的意义是**补全与帮助能看到它们** —— 用户不知道有这些指令就等于
    // 这些功能不存在。
    cmd("start", &[], "启动当前账号(本地)"),
    cmd("stop", &[], "停止当前账号(本地)"),
    cmd("profiles", &["accounts"], "账号管理(本地)").subs(&[
        sub("list"),
        sub_arg("add", ArgSpec::Path),
        sub_arg("remove", ArgSpec::Free),
    ]),
    cmd("clear", &["cls"], "清空日志(本地)"),
    cmd("bag", &[], "背包界面(本地)"),
    cmd("npmenu", &[], "NPC选项弹框开关(本地)").subs(&[sub("on"), sub("off")]),
    cmd("help", &["h"], "帮助"),
    cmd("quit", &["exit", "q"], "退出"),
];

/// 按名字或别名查规格 (不区分大小写)。**精确匹配**, 不做前缀匹配
/// (一级补全的前缀匹配在 `completion::cmd_name_complete`)。
pub fn find(name: &str) -> Option<&'static CmdSpec> {
    let n = name.to_ascii_lowercase();
    COMMANDS
        .iter()
        .find(|c| c.name == n || c.aliases.iter().any(|a| *a == n))
}

/// 在该命令的 `dyn_args` 里找**最长匹配**的前缀。
///
/// `tokens` 是已敲入的前缀 token (不含正在输入的那一段)。返回匹配到的参数类型。
pub fn dyn_arg_for(spec: &CmdSpec, tokens: &[&str]) -> Option<ArgSpec> {
    let mut best: Option<(usize, ArgSpec)> = None;
    for (prefix, arg) in spec.dyn_args {
        if prefix.len() > tokens.len() {
            continue;
        }
        let matches = prefix
            .iter()
            .zip(tokens.iter())
            .all(|(p, t)| p.eq_ignore_ascii_case(t));
        if !matches {
            continue;
        }
        if best.is_none_or(|(len, _)| prefix.len() > len) {
            best = Some((prefix.len(), *arg));
        }
    }
    best.map(|(_, a)| a)
}

/// 帮助面板的一行: `(命令名, 说明, 子命令摘录)`。
pub fn help_rows() -> Vec<HelpRow> {
    COMMANDS
        .iter()
        .map(|c| {
            let mut subs: Vec<String> = c.subs.iter().map(|s| s.name.to_string()).collect();
            // 动态参数前缀里那些**不是**静态子命令的部分 (如 `reactor hit <oid>`
            // 的 `hit` 已在 subs 里; `view eqpinfo <itemid>` 的 `eqpinfo` 也是)。
            // 这里只补上 subs 里没有的前缀, 保证帮助不漏项。
            for (prefix, _) in c.dyn_args {
                if let Some(last) = prefix.last() {
                    if !subs.iter().any(|s| s == last) {
                        subs.push((*last).to_string());
                    }
                }
            }
            HelpRow {
                name: c.name,
                usage: c.usage,
                subs,
                args: c.arity.placeholders(),
                action_only: c.action_only,
            }
        })
        .collect()
}

/// [`help_rows`] 的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpRow {
    pub name: &'static str,
    pub usage: &'static str,
    pub subs: Vec<String>,
    /// 参数占位串 (`<npc_oid>` / `on|off` / `<itemid> <qty>`), 无参数时为空。
    pub args: String,
    pub action_only: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_aliases_are_unique() {
        let mut seen: Vec<&str> = Vec::new();
        for c in COMMANDS {
            assert!(!seen.contains(&c.name), "命令名重复: {}", c.name);
            seen.push(c.name);
            for a in c.aliases {
                assert!(!seen.contains(a), "别名与已有命令冲突: {a}");
                seen.push(a);
            }
        }
    }

    #[test]
    fn subs_within_a_command_are_unique() {
        for c in COMMANDS {
            let mut seen: Vec<&str> = Vec::new();
            for s in c.subs {
                assert!(
                    !seen.contains(&s.name),
                    "{} 的子命令重复: {}",
                    c.name,
                    s.name
                );
                seen.push(s.name);
            }
        }
    }

    #[test]
    fn find_matches_names_and_aliases() {
        assert_eq!(find("view").unwrap().name, "view");
        assert_eq!(find("v").unwrap().name, "view");
        assert_eq!(find("V").unwrap().name, "view");
        assert_eq!(find("cc").unwrap().name, "channel");
        // rhunt 从来不是别名: 解析器只认 reactors (曾被表写错, 现由一致性测试钉住)
        assert!(find("nope").is_none());
        assert!(find("").is_none());
    }

    #[test]
    fn dyn_arg_longest_prefix_wins() {
        let skill = find("skill").unwrap();
        // 单 token 前缀 `skill` 没有 dyn_args 条目 -> 只有真正的双 token 前缀命中
        assert_eq!(dyn_arg_for(skill, &["skill"]), None);
        assert_eq!(
            dyn_arg_for(skill, &["skill", "learn"]),
            Some(ArgSpec::SkillId)
        );
        assert_eq!(
            dyn_arg_for(skill, &["skill", "info"]),
            Some(ArgSpec::SkillId)
        );
        // `skill cast <skillid> <oid>`: 第二格是目标 oid, 不是技能 id
        assert_eq!(
            dyn_arg_for(skill, &["skill", "cast"]),
            Some(ArgSpec::MobOid)
        );
        // 大小写不敏感
        assert_eq!(
            dyn_arg_for(skill, &["SKILL", "LEARN"]),
            Some(ArgSpec::SkillId)
        );
        // 未知子命令
        assert_eq!(dyn_arg_for(skill, &["skill", "bogus"]), None);
    }

    #[test]
    fn dyn_arg_covers_every_referenced_prefix() {
        // 每个 dyn_args 前缀的第一个 token 必须是该命令名或其别名之一,
        // 否则永远不会被命中 (静默失效的配置错误)。
        for c in COMMANDS {
            for (prefix, _) in c.dyn_args {
                assert!(!prefix.is_empty(), "{} 有空前缀", c.name);
                let head = prefix[0];
                assert!(
                    head.eq_ignore_ascii_case(c.name)
                        || c.aliases.iter().any(|a| a.eq_ignore_ascii_case(head)),
                    "{} 的 dyn_args 前缀 {prefix:?} 首 token 与命令名/别名不符",
                    c.name
                );
                // 前缀的后续 token 必须是已声明的子命令 (否则补全表内部就不自洽)
                for t in &prefix[1..] {
                    assert!(
                        c.subs.iter().any(|s| s.name.eq_ignore_ascii_case(t)),
                        "{} 的 dyn_args 前缀 {prefix:?} 里的 `{t}` 不是已声明子命令",
                        c.name
                    );
                }
            }
        }
    }

    #[test]
    fn arg_spec_placeholders() {
        assert_eq!(ArgSpec::None.placeholder(), "");
        assert_eq!(ArgSpec::Free.placeholder(), "");
        assert_eq!(ArgSpec::MapId.placeholder(), "<mapid>");
        assert_eq!(ArgSpec::Values(&["on", "off"]).placeholder(), "on|off");
        assert!(!ArgSpec::None.is_required());
        assert!(!ArgSpec::Free.is_required(), "Free 没有枚举值可列");
        assert!(ArgSpec::MapId.is_required());
        assert!(
            !ArgSpec::Values(&["on"]).is_required(),
            "Values 是常用写法而非封闭集合"
        );
    }

    /// `Values` 里的每个取值必须**逐字**被解析器接受。
    ///
    /// 补全给的是字面量, 所以表里不能写别名: 例如 `hunt pickup` 的
    /// `on|off` 实际上被解析器当成 `parse_bool` 的别名 (`1/true` 也行),
    /// 但补出来的字面量 `on` 必须真的能用。写错的字面量会让用户"补全出来
    /// 却执行失败"。
    #[test]
    fn every_values_literal_is_accepted_by_the_parser() {
        for c in COMMANDS {
            // 命令级 Values (无子命令时)
            if let ArgSpec::Values(vs) = c.arity.a1 {
                for v in vs {
                    let line = format!("{} {v}", c.name);
                    assert!(
                        openstory_bot::command::parse(&line).is_ok(),
                        "{} 的取值 `{v}` 解析器不接受 (`{line}`)",
                        c.name
                    );
                }
            }
            // 子命令级 Values
            for s in c.subs {
                if let ArgSpec::Values(vs) = s.arity.a1 {
                    for v in vs {
                        let line = format!("{} {} {v}", c.name, s.name);
                        assert!(
                            openstory_bot::command::parse(&line).is_ok(),
                            "{} {} 的取值 `{v}` 解析器不接受 (`{line}`)",
                            c.name,
                            s.name
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn help_rows_cover_all_commands_once() {
        let rows = help_rows();
        assert_eq!(rows.len(), COMMANDS.len());
        for (r, c) in rows.iter().zip(COMMANDS) {
            assert_eq!(r.name, c.name);
            assert_eq!(r.usage, c.usage);
        }
        // 抽检: 子命令摘录包含解析器接受但补全表原先漏掉的项
        let task = rows.iter().find(|r| r.name == "task").unwrap();
        assert!(task.subs.contains(&"stop".to_string()));

        assert!(task.subs.contains(&"start".to_string()));
        let group = rows.iter().find(|r| r.name == "group").unwrap();

        assert!(group.subs.contains(&"status".to_string()));
    }

    #[test]
    fn action_only_commands_are_flagged() {
        assert!(find("wait").unwrap().action_only);
        assert!(find("while").unwrap().action_only);
        assert!(!find("hunt").unwrap().action_only);
    }
}
