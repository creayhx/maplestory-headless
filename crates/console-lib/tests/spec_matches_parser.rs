//! 命令规格表 ↔ 解析器 一致性测试。
//!
//! # 为什么需要这个测试
//!
//! 改造前有两份手工维护的真相:
//! - `completion.rs` 的 `COMMANDS` (驱动 Tab 补全 + F1 帮助)
//! - `openstory_bot::command::parse` (真正决定指令能不能执行)
//!
//! 两者已经漂移, 后果是"补全里看得到、敲下去却报错", 或者反过来
//! "能用但提示里没有"。这类漂移在改动时完全静默。
//!
//! 本测试把两份真相焊在一起: **表里宣称支持的每一种写法, 解析器必须接受**。
//! 新增/修改命令时, 只要表与解析器不一致, 这里立刻变红。
//!
//! 允许的例外必须**显式列出**并写明理由 (见 [`LOCAL_ONLY`]), 不能靠放宽断言
//! 蒙过去 —— 那样就把测试的价值抹掉了。

use command_spec::{ArgSpec, CmdSpec, COMMANDS};
use openstory_console_lib::command_spec;

/// 前端本地命令: 由控制台自己处理, 不经过 `command::parse`。
///
/// 这些命令的"解析器"是 `crates/console/src/app.rs` 的本地分支, 因此无法用
/// `command::parse` 校验; 一旦以后把它们并进核心解析器, 就应该从这里删掉。
const LOCAL_ONLY: &[(&str, &str)] = &[
    ("shop", "menu"), // 商店弹框开关 (TUI 本地)
    ("npmenu", ""),   // NPC 选项弹框开关 (TUI 本地, 含 on/off 子命令)
    ("clear", ""),    // 清空本地日志 (含别名 cls)
    ("bag", ""),      // 背包界面 (TUI 本地, headless 回退为 view inventory)
    // 阶段 8: 会话启停与账号管理 —— 都由控制台自己执行 (bot 不认识这些词,
    // 发过去只会回一句"未知指令")。解析器是 `App::submit` 里的内建分支,
    // 因此同样无法用 `command::parse` 校验。
    ("start", ""),
    ("stop", ""),
    ("profiles", ""), // 含 list/add/remove 子命令 (别名 accounts)
];

fn is_local_only(cmd: &str, sub: &str) -> bool {
    LOCAL_ONLY
        .iter()
        .any(|(c, s)| *c == cmd && (*s == sub || s.is_empty()))
}

/// 每个参数类型的一个合法示例值。用于把"带参数的写法"补成完整指令。
fn sample_value(arg: ArgSpec) -> Option<&'static str> {
    match arg {
        ArgSpec::None | ArgSpec::Free => None,
        ArgSpec::Number => Some("1"),
        ArgSpec::MapId => Some("100000000"),
        ArgSpec::MobOid => Some("50001"),
        ArgSpec::ReactorOid => Some("90001"),
        ArgSpec::NpcOid => Some("100001"),
        ArgSpec::Portal => Some("in00"),
        ArgSpec::RuleId => Some("r1"),
        ArgSpec::TaskId => Some("t1"),
        ArgSpec::GroupId => Some("g1"),
        ArgSpec::SkillId => Some("1001003"),
        ArgSpec::ItemId => Some("2000000"),
        ArgSpec::Values(vs) => vs.first().copied(),
        ArgSpec::Path => Some("profiles/x.json"),
    }
}

fn parse(line: &str) -> Result<Option<openstory_bot::command::Command>, String> {
    openstory_bot::command::parse(line)
}

/// 按参数格数产出示例参数串。
fn sample_args(arity: command_spec::Arity) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for n in 1..=arity.count() {
        parts.push(sample_value(arity.nth(n)).unwrap_or("x"));
    }
    parts.join(" ")
}

/// 把一个 (命令, 子命令) 组合补成**完整**指令。
///
/// 裸写 `view eqpinfo` / `skill cast` 是**不合法**的 (缺参数), 所以"校验子
/// 命令存在"必须把参数补齐, 否则测到的是"缺参数的写法被拒", 与表准不准无关。
fn full_line(spec: &CmdSpec, sub: Option<&command_spec::SubSpec>) -> String {
    match sub {
        Some(s) => join(spec.name, Some(s.name), s.arity),
        None => join(spec.name, None, spec.arity),
    }
}

/// 同 [`full_line`], 但用别名代替规范命令名。
fn full_line_for(spec: &CmdSpec, name: &str) -> String {
    join(name, None, spec.arity)
}

fn join(name: &str, sub: Option<&str>, arity: command_spec::Arity) -> String {
    let head = match sub {
        Some(s) => format!("{name} {s}"),
        None => name.to_string(),
    };
    let args = sample_args(arity);
    if args.is_empty() {
        head
    } else {
        format!("{head} {args}")
    }
}

/// 命令级完整示例: 优先显式 `sample` (复合写法如 `while x do y`),
/// 否则按 arity 拼。
fn full_line_owned(spec: &CmdSpec) -> String {
    match spec.sample {
        Some(s) => format!("{} {s}", spec.name),
        None => join(spec.name, None, spec.arity),
    }
}

/// 断言 `line` 能被解析器接受 (空指令 `Ok(None)` 也算接受)。
#[track_caller]
fn assert_parses(line: &str, why: &str) {
    match parse(line) {
        Ok(_) => {}
        Err(e) => panic!("{why}\n  指令: `{line}`\n  解析器: {e}"),
    }
}

/// 表里声明的每个子命令都必须被解析器接受。
///
/// 子命令自带参数类型时, 用该类型的示例值补全后校验 —— 这样同时覆盖了
/// "子命令存在"和"它接受这个位置的参数"两件事。
#[test]
fn every_declared_subcommand_is_accepted_by_the_parser() {
    let mut checked = 0usize;
    for spec in COMMANDS {
        for sub in spec.subs {
            if is_local_only(spec.name, sub.name) {
                continue;
            }
            // 子命令自身声明的参数用示例值补齐后再校验 (裸写缺参数会被拒,
            // 那是参数缺失而不是子命令不存在)。
            let with_arg = full_line(spec, Some(sub));
            assert_parses(
                &with_arg,
                &format!("表宣称 `{} {}` 可用", spec.name, sub.name),
            );
            checked += 1;
            // 子命令不带参数时, 顺带确认裸写法也合法 (真正的"无参数子命令")
            if sub.arity.count() == 0 {
                let bare = format!("{} {}", spec.name, sub.name);
                assert_parses(
                    &bare,
                    &format!("表宣称 `{} {}` 不接受参数", spec.name, sub.name),
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 50, "只校验了 {checked} 处, 表可能没被读到");
}

/// 每个别名都必须能当命令名使用 (含带子命令的写法)。
#[test]
fn every_alias_is_accepted_by_the_parser() {
    for spec in COMMANDS {
        // 本地命令 (clear / npmenu / ...) 及其别名由前端处理, 不走 command::parse
        if is_local_only(spec.name, "") {
            continue;
        }
        for alias in spec.aliases {
            let line = match spec.subs.iter().find(|s| !is_local_only(spec.name, s.name)) {
                Some(s) => join(alias, Some(s.name), s.arity),
                None => full_line_for(spec, alias),
            };
            // 别名 + 子命令也必须被解析器接受, 否则别名就是"半个别名"。
            assert_parses(
                &line,
                &format!("表宣称 `{alias}` 是 `{}` 的别名", spec.name),
            );
        }
    }
}

/// 命令名本身必须要么被解析器接受, 要么在 [`LOCAL_ONLY`] 里说明理由。
#[test]
fn every_command_name_is_either_parseable_or_declared_local() {
    for spec in COMMANDS {
        if is_local_only(spec.name, "") {
            continue;
        }
        // 用一个非本地子命令 (带它的必填参数) 或命令自身的参数构成完整指令
        let line = match spec.subs.iter().find(|s| !is_local_only(spec.name, s.name)) {
            Some(s) => full_line(spec, Some(s)),
            None => full_line_owned(spec),
        };
        assert_parses(&line, &format!("表里有命令 `{}`", spec.name));
    }
}

/// 反过来: 解析器认识、但表里**没有**的命令。用文档化清单固定住,
/// 防止"表漏了一条命令"这种静默缺失。
///
/// 清单里每一条都必须在 `docs/GUIDE.md` 或 `parse.rs` 里有对应实现。
#[test]
fn parser_has_no_undocumented_top_level_commands() {
    // 从 parse.rs 的顶层 match 分支里提取 (人工核对, 改动时同步)。
    // 只列"用户可以直接敲"的顶层命令名。
    const PARSER_COMMANDS: &[&str] = &[
        "move",
        "chat",
        "say",
        "c",
        "gather",
        "view",
        "v",
        "bag",
        "pickup",
        "sleep",
        "wait",
        "while",
        "attack",
        "atk",
        "hunt",
        "use",
        "drink",
        "reward",
        "open",
        "drop",
        "dropmeso",
        "dropm",
        "scroll",
        "upgrade",
        "ap",
        "rule",
        "group",
        "reconnect",
        "party",
        "trade",
        "equip",
        "unequip",
        "takeoff",
        "skill",
        "skills",
        "sp",
        "cast",
        "buff",
        "keymap",
        "channel",
        "ch",
        "cc",
        "cashshop",
        "cs",
        "auction",
        "mts",
        "reenter",
        "reentry",
        "task",
        "town",
        "warp",
        "movewarp",
        "mw",
        "npc",
        "buy",
        "sell",
        "sellammo",
        "ammo",
        "shop",
        "leave",
        "reactor",
        "reactors",
        "reload",
        "cfg",
        "setvar",
        "clearvar",
        "login",
        "help",
        "h",
        "quit",
        "exit",
        "q",
    ];
    for name in PARSER_COMMANDS {
        assert!(
            command_spec::find(name).is_some(),
            "解析器有 `{name}`, 但命令规格表里没有 -> 用户补不出来也看不到帮助"
        );
    }
}

/// `Values` 里写的字面量必须是解析器真的接受的写法。
///
/// 这条防的是"补全给出一个回车就报错的候选" —— 比没有候选更糟。
#[test]
fn every_values_literal_is_a_working_command() {
    for spec in COMMANDS {
        for (prefix, arg) in spec.dyn_args {
            if let ArgSpec::Values(vs) = arg {
                for v in *vs {
                    let line = format!("{} {v}", prefix.join(" "));
                    assert_parses(
                        &line,
                        &format!("dyn_args 前缀 {:?} 的取值 `{v}` 不可用", prefix),
                    );
                }
            }
        }
    }
}

/// 采样断言: 表与解析器在几个已知易漂移点上确实一致。
///
/// 这些点是改造前真实漂移过的地方, 单独钉住, 让回归一眼可见。
#[test]
fn known_drift_points_are_fixed() {
    // 解析器一直接受、改造前补全表漏掉的写法
    for line in [
        "task stop",
        "task status",
        "task run t1",
        "task start t1",
        "group run g1",
        "group stop g1",
        "group open g1",
        "group close g1",
        "group status",
        "view eqpinfo 1302000",
        "view portals",
        "view portals 100000000",
        "buff cancel 1001004",
        "cashshop buy 1",
        "cashshop get",
        "cashshop list",
        "cashshop store 1",
        "cashshop out",
    ] {
        assert_parses(line, "已知漂移点必须已被修复");
    }
    // 确认这些写法在表里也存在 (不是只让解析器通过)
    let view = command_spec::find("view").unwrap();
    assert!(view.subs.iter().any(|s| s.name == "eqpinfo"));
    assert!(view.subs.iter().any(|s| s.name == "portals"));
    let task = command_spec::find("task").unwrap();
    for s in ["run", "start", "stop", "status"] {
        assert!(task.subs.iter().any(|x| x.name == s), "task 缺子命令 {s}");
    }
    let group = command_spec::find("group").unwrap();
    for s in ["run", "open", "stop", "close", "status"] {
        assert!(group.subs.iter().any(|x| x.name == s), "group 缺子命令 {s}");
    }
}

/// **反向漂移**: 表里**不该**出现的写法 (解析器拒绝)。
///
/// 光校验"表里的都能用"还不够 —— 表比解析器更宽松时, 用户会被补全引导到
/// 一条必然报错的指令上。这条把"曾经写错又被发现"的项钉死。
#[test]
fn table_does_not_promise_what_the_parser_rejects() {
    // 曾经在表里、但解析器不接受的写法。
    //
    // 每一条都必须**先用解析器确认真的被拒**再加进来 —— 想当然地填会让测试
    // 自己变成错误来源。写这段时就把 `task stop t1` / `task cancel` 想错了:
    // 解析器接受它们 (`task stop` 后面的多余 token 被忽略)。
    let rejected = [
        "rhunt on",        // rhunt 从来不是别名 (只有 reactors)
        "group start g1",  // 解析器只认 run|open
        "group cancel g1", // 同上
        "hunt filter add", // add/del 需要 id
        "hunt filter del",
        "hunt filter bogus",
    ];
    for line in rejected {
        assert!(
            parse(line).is_err(),
            "这条本应被解析器拒绝: `{line}` —— 若解析器改了, 请同步表与测试"
        );
    }
    // 并且表里确实没有它们
    let reactor = command_spec::find("reactor").unwrap();
    assert!(!reactor.aliases.contains(&"rhunt"), "rhunt 不是别名");
    let group = command_spec::find("group").unwrap();
    for s in ["start", "cancel"] {
        assert!(
            !group.subs.iter().any(|x| x.name == s),
            "group 不该声称有 {s} 子命令"
        );
    }
    let task = command_spec::find("task").unwrap();
    assert!(!task.subs.iter().any(|x| x.name == "cancel"));
    // task stop 不带参数
    let stop = task.subs.iter().find(|s| s.name == "stop").unwrap();
    assert_eq!(stop.arity.count(), 0, "task stop 不接受 id");
    // hunt filter 只列不需要 id 的写法
    let hunt = command_spec::find("hunt").unwrap();
    let filter = hunt.subs.iter().find(|s| s.name == "filter").unwrap();
    match filter.arity.a1 {
        ArgSpec::Values(vs) => {
            assert!(!vs.contains(&"add"), "add 需要 id, 不能作为裸候选");
            assert!(!vs.contains(&"del"), "del 需要 id, 不能作为裸候选");
            assert!(vs.contains(&"off"));
        }
        other => panic!("hunt filter 的参数应是 Values, 实际 {other:?}"),
    }
}

/// `action_only` 的命令不能作为普通指令被执行 —— 帮助里必须标注,
/// 否则用户会照着敲然后困惑。
#[test]
fn action_only_commands_are_marked() {
    for name in ["wait", "while"] {
        let spec = command_spec::find(name).unwrap();
        assert!(spec.action_only, "`{name}` 只能在规则/任务动作里用, 应标注");
    }
    for name in ["hunt", "view", "npc"] {
        let spec = command_spec::find(name).unwrap();
        assert!(
            !spec.action_only,
            "`{name}` 是普通指令, 不该标注 action_only"
        );
    }
}

/// 帮助行必须覆盖表里每一条命令, 且带上子命令与参数说明。
#[test]
fn help_rows_are_complete() {
    let rows = command_spec::help_rows();
    assert_eq!(rows.len(), COMMANDS.len());
    for spec in COMMANDS {
        let row = rows.iter().find(|r| r.name == spec.name).unwrap();
        assert_eq!(row.usage, spec.usage);
        // 每个静态子命令都要出现在帮助里
        for s in spec.subs {
            assert!(
                row.subs.iter().any(|x| x == s.name),
                "{} 的子命令 {} 没出现在帮助里",
                spec.name,
                s.name
            );
        }
    }
}

/// 表本身的自洽性: 子命令唯一、别名不与其它命令冲突、dyn_args 前缀合法。
#[test]
fn spec_table_is_self_consistent() {
    let mut all_names: Vec<&str> = Vec::new();
    for spec in COMMANDS {
        assert!(!all_names.contains(&spec.name), "重复命令名 {}", spec.name);
        all_names.push(spec.name);
        for a in spec.aliases {
            assert!(!all_names.contains(a), "别名冲突 {a}");
            all_names.push(a);
        }
        let mut subs: Vec<&str> = Vec::new();
        for s in spec.subs {
            assert!(
                !subs.contains(&s.name),
                "{} 子命令重复 {}",
                spec.name,
                s.name
            );
            subs.push(s.name);
        }
        for (prefix, _) in spec.dyn_args {
            let head = prefix[0];
            assert!(
                head.eq_ignore_ascii_case(spec.name)
                    || spec.aliases.iter().any(|a| a.eq_ignore_ascii_case(head)),
                "{} 的 dyn_args 前缀 {prefix:?} 首 token 不匹配",
                spec.name
            );
        }
    }
}

/// `CmdSpec` 的字段都要被真正用到 (防止 builder 又被结构体更新语法静默覆盖)。
///
/// 这条是阶段 2 踩过的坑的回归: `CmdSpec { arg: X, ..cmd(..) }` 会让 `arg`
/// 被 base 里的 `None` 覆盖, 补全整段失效而编译毫无警告。
#[test]
fn builder_fields_are_not_silently_overwritten() {
    // 抽查几个"必须带 arg"的命令
    let cases: &[(&str, ArgSpec)] = &[
        ("move", ArgSpec::Free),
        ("attack", ArgSpec::MobOid),
        ("npc", ArgSpec::NpcOid),
        ("warp", ArgSpec::Portal),
        ("movewarp", ArgSpec::Portal),
        ("sell", ArgSpec::ItemId),
        ("use", ArgSpec::ItemId),
        ("town", ArgSpec::MapId),
        ("channel", ArgSpec::Number),
    ];
    for (name, want) in cases {
        let spec = command_spec::find(name).unwrap();
        assert_eq!(
            spec.arity.a1, *want,
            "`{name}` 的 arg 被覆盖了 (结构体更新语法?)"
        );
    }
    // 抽查几个"必须带 subs"的命令
    for (name, sub) in [
        ("view", "eqpinfo"),
        ("hunt", "pickup"),
        ("task", "run"),
        ("group", "status"),
        ("cashshop", "buy"),
    ] {
        let spec = command_spec::find(name).unwrap();
        assert!(
            spec.subs.iter().any(|s| s.name == sub),
            "`{name}` 的 subs 被覆盖了 (缺 `{sub}`)"
        );
    }
    // dyn_args 同理
    let move_spec = command_spec::find("move").unwrap();
    assert!(
        !move_spec.dyn_args.is_empty(),
        "`move` 的 dyn_args 被覆盖了 (传送门补全会失效)"
    );
}
