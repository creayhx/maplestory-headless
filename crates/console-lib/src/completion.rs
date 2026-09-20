//! 命令补全 (纯逻辑, 可单测)。Tab 在候选间轮换。
//!
//! 三级补全**全部由 [`crate::command_spec`] 驱动**:
//! 命令名 → 静态子命令 → 动态 id/oid/名字 (来自游戏状态快照)。
//! 候选可携带展示用描述 (如 `npc 100001 -- 米娅`), 选中后输入框
//! 只填入 `cmd` (描述不进入指令)。
//!
//! 改造前这里是逐个命令手写的 `match` (`sub_or_dyn`), 新增命令要同时改
//! 本文件与规格表两处; 现在只改规格表一处。

use crate::command_spec::{self, ArgSpec, CmdSpec};

/// 补全候选: `display` 用于弹窗展示 (可带 -- 描述), `cmd` 是敲回车后
/// 真正进入输入框的指令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub cmd: String,
    pub display: String,
}

impl Candidate {
    fn plain(cmd: String) -> Self {
        Candidate {
            display: cmd.clone(),
            cmd,
        }
    }

    /// 带描述: 弹窗显示 `display`, 回车只填 `cmd`。
    fn described(cmd: String, desc: impl std::fmt::Display) -> Self {
        Candidate {
            display: format!("{cmd} -- {desc}"),
            cmd,
        }
    }
}

/// 动态补全上下文 (来自游戏状态快照)。
#[derive(Default)]
pub struct Ctx<'a> {
    /// (oid, npcid) 对 — NPC 名字由 npcid 解析
    pub npcs: &'a [(i32, i32)],
    pub mob_oids: &'a [i32],
    pub reactor_oids: &'a [i32],
    pub rule_ids: &'a [&'a str],
    pub task_ids: &'a [&'a str],
    pub group_ids: &'a [&'a str],
    /// 当前地图传送门: (name, target_map_id)
    pub portals: &'a [(String, i32)],
    /// 背包 + 已装备的道具 id (用于 `view eqpinfo` / `sell` / `drop` 等)
    pub item_ids: &'a [i32],
    /// 已学技能 id (用于 `skill learn/info`)
    pub skill_ids: &'a [i32],
    /// **多会话目标**: 全部账号档案名 (用于 `@all` / `@<前缀>` 补全)。
    /// 单会话时留空, `@` 目标补全自然不出现 (不引入"发给谁"这个概念)。
    pub profiles: &'a [String],
}

/// `@` 目标补全的"全部"候选。
const TARGET_ALL: &str = "@all";

/// 补全一条命令的**目标前缀** (`@all` / `@serverA_1`)。
///
/// 返回空 = 当前位置不是目标前缀 (不以 `@` 开头) 或不需要目标 (单会话)。
/// 只在**首 token** 生效 —— 聊天内容里的 `@某人` 不该弹出账号列表。
pub fn target_candidates(typed: &str, profiles: &[String]) -> Vec<Candidate> {
    if !typed.starts_with('@') {
        return Vec::new();
    }
    if profiles.len() < 2 {
        // 单会话没有"发给谁"的问题, 不引入这个概念
        return Vec::new();
    }
    let prefix = &typed[1..];
    let mut out: Vec<Candidate> = Vec::new();
    if TARGET_ALL.starts_with(typed) {
        out.push(Candidate::plain(TARGET_ALL.to_string()));
    }
    for p in profiles {
        if p.eq_ignore_ascii_case(prefix) {
            continue; // 已完整输入
        }
        if p.to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
        {
            out.push(Candidate::plain(format!("@{p}")));
        }
    }
    out.dedup_by(|a, b| a.cmd == b.cmd);
    out
}

/// 补全当前输入, 返回完整候选列表。空输入 = 全部命令名。
///
/// 无状态入口: 只补命令名/子命令, 没有动态候选 (UI 走 `complete_with`)。
#[cfg(test)]
pub fn complete(input: &str) -> Vec<Candidate> {
    complete_with(input, &Ctx::default())
}

/// 带游戏状态上下文的补全 (命令名 → 子命令 → 动态 id/oid/名字)。
///
/// 候选里回显的部分一律用规格表里的**规范命令名** (敲 `v ` 也补 `view ...`),
/// 与改造前行为一致。
pub fn complete_with(input: &str, ctx: &Ctx) -> Vec<Candidate> {
    let trimmed = input.trim_start();
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.is_empty() {
        return command_names();
    }
    let trailing_ws = trimmed.ends_with(char::is_whitespace);
    let first = tokens[0].to_ascii_lowercase();
    // 一级补全: 只有一个 token 且没有尾随空格 = 还在敲命令名 (可能是前缀,
    // 也可能是别名), 必须走前缀匹配而不是精确查表。
    if tokens.len() == 1 && !trailing_ws {
        // `@...` 是"发给谁"的目标前缀, 不是命令名 —— 先在账号列表里补
        let targets = target_candidates(&first, ctx.profiles);
        if !targets.is_empty() {
            return targets;
        }
        return cmd_name_complete(&first);
    }
    // 之后一律需要精确命中一条命令 (命令名或别名)。
    let Some(spec) = command_spec::find(&first) else {
        return Vec::new();
    };
    resolve_tokens(spec, &tokens, trailing_ws, ctx)
}

/// 补全的核心决策。
///
/// `words` 是已敲入的 token (首 token 已换成规格表里的规范命令名),
/// `typed` 是正在输入的那一段 (`trailing_ws` 时为空)。
///
/// 优先级 (每一级有候选就返回):
/// 1. 还没敲子命令 (`cmd `): 命令自身的参数 (`move ` -> 传送门、`npc ` -> oid);
///    `npc` 例外 —— oid 与对话子命令同时给出
/// 2. 显式 `dyn_args` 前缀, 最长优先 (`reactor hit ` / `task run ` /
///    `view eqpinfo `)
/// 3. 已敲完的子命令自带的参数 (`hunt pickup ` / `gather controller `, 以及
///    参数敲了一半的 `hunt pickup o`)
/// 4. 命令自身的参数, 正在敲它的第一格 (`npc 20` / `attack 5` / `sell 400`)
/// 5. 正在敲子命令名时, 列出匹配的静态子命令
fn resolve_tokens(spec: &CmdSpec, tokens: &[&str], trailing_ws: bool, ctx: &Ctx) -> Vec<Candidate> {
    let typed = if trailing_ws {
        ""
    } else {
        *tokens.last().unwrap_or(&"")
    };
    // 小写化并把首 token 换成规范命令名 (别名 `mw` 也能命中以规范名写的表)。
    let mut words: Vec<String> = tokens.iter().map(|t| t.to_ascii_lowercase()).collect();
    words[0] = spec.name.to_string();
    let words: Vec<&str> = words.iter().map(|s| s.as_str()).collect();

    // 1. `cmd ` —— 还没敲子命令
    if words.len() == 1 {
        // 命令自身的参数: 显式单 token 前缀 (`move ` -> 传送门) 优先于
        // 命令级的 `arg` (`move` 的 arg 是 Free, 因为也接受坐标)。
        let arg = command_spec::dyn_arg_for(spec, &[spec.name]).unwrap_or(spec.arity.a1);
        let dyn_cands = candidates_for(arg, spec.name, "", ctx);
        if !dyn_cands.is_empty() && spec.name != "npc" {
            return dyn_cands;
        }
        return merge_subs_and_dyn(spec, "", dyn_cands);
    }
    // 已敲完的部分 = words[..n_committed]
    let n_committed = if trailing_ws {
        words.len()
    } else {
        words.len() - 1
    };
    let prev_word = words[n_committed - 1];
    let prev_is_sub = spec
        .subs
        .iter()
        .any(|s| s.name.eq_ignore_ascii_case(prev_word));

    // 2. 显式 dyn_args 前缀 (最长优先)
    if let Some(arg) = command_spec::dyn_arg_for(spec, &words[..n_committed]) {
        let base = words[..n_committed].join(" ");
        let cands = candidates_for(arg, &base, typed, ctx);
        if !cands.is_empty() {
            return cands;
        }
        // 特例: 上一格是带枚举值的子命令, 且当前正在敲的就是这些枚举值之一
        // (`hunt filter ` -> off|allow|deny)。dyn_args 若在这一格给出别的
        // 候选类型 (物品 id), 不能让它的空结果把枚举候选吃掉。
        if let Some(s) = spec
            .subs
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(prev_word))
        {
            if let ArgSpec::Values(vs) = s.arity.a1 {
                if vs.iter().any(|v| v.starts_with(typed)) {
                    let sub_base = format!("{} {}", spec.name, s.name);
                    return candidates_for(s.arity.a1, &sub_base, typed, ctx);
                }
            }
        }
    }
    // 3. 上一格是子命令 -> 用该子命令声明的参数类型。
    //
    //    实测校验: 表里声明的子命令在该位置必须真的被解析器接受, 否则宁可
    //    不补也不能补出无效命令 (`task stop hunt` -> 解析器报
    //    "bad task cmd: hunt", 因为 `task stop` 不接受 id)。
    if let Some(s) = spec
        .subs
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(prev_word))
    {
        // 枚举型参数 (`Values`) 即使 is_required()=false 也要走这里: 它给出
        // 的是"常用写法", 匹配不上时应当**不补** (用户可能在敲数字/坐标),
        // 而不是回退去列子命令。
        let enum_like = matches!(s.arity.a1, ArgSpec::Values(_));
        if (s.arity.a1.is_required() && sub_takes_arg(spec.name, s.name)) || enum_like {
            let base = format!("{} {}", spec.name, s.name);
            let cands = candidates_for(s.arity.a1, &base, typed, ctx);
            if !cands.is_empty() {
                return cands;
            }
            if enum_like {
                return Vec::new();
            }
        }
    }
    // 4. 命令自身的参数, 正在敲它的第一格 (`npc 20` / `attack 5` / `sell 400`)
    if n_committed == 1 && !prev_is_sub {
        let cands = candidates_for(spec.arity.a1, spec.name, typed, ctx);
        if !cands.is_empty() {
            return cands;
        }
    }
    // 5. 静态子命令候选 —— 只在"这一格正是子命令名"时给出:
    //    - 还没开始敲 (typed 为空), 或
    //    - 上一格还不是一个合法子命令
    //    否则这一格是参数 (如 `hunt pickup o`), 列出子命令会串味。
    if typed.is_empty() || !prev_is_sub {
        let subs: Vec<Candidate> = spec
            .subs
            .iter()
            .filter(|s| s.name.starts_with(typed))
            .map(|s| Candidate::plain(format!("{} {}", spec.name, s.name)))
            .collect();
        if !subs.is_empty() {
            return subs;
        }
    }
    Vec::new()
}

/// `cmd sub <arg>` 这种写法解析器是否真的接受?
///
/// 用解析器实测而不是再维护一份规则: 补全给出一个回车就被拒的候选是最糟的
/// 体验。只在"该子命令声明了参数类型"时才需要问 (没声明就不会补)。
///
/// 1 token 的填充量只用于判断"会不会因为缺少参数而失败" —— 参数类型已知时
/// 一定能补出合法的第一格。
fn sub_takes_arg(cmd: &str, sub: &str) -> bool {
    openstory_bot::command::parse(&format!("{cmd} {sub} x")).is_ok()
}

/// 全部命令名 (空输入时的候选)。
fn command_names() -> Vec<Candidate> {
    command_spec::COMMANDS
        .iter()
        .map(|c| Candidate::plain(c.name.to_string()))
        .collect()
}

/// 一级补全: 命令名 + 别名 (别名命中也补成规范名)。
fn cmd_name_complete(prefix: &str) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for c in command_spec::COMMANDS {
        let hit = c.name.starts_with(prefix) || c.aliases.iter().any(|a| a.starts_with(prefix));
        if hit {
            out.push(Candidate::plain(c.name.to_string()));
        }
    }
    out.dedup_by(|a, b| a.cmd == b.cmd);
    out.sort_by(|a, b| a.cmd.cmp(&b.cmd));
    out
}

/// 静态子命令 + 动态候选合并。
///
/// `npc` 是特例: oid 数字候选排前面 (更常用), 对话子命令排后面;
/// 其余命令子命令在前。
fn merge_subs_and_dyn(spec: &CmdSpec, typed: &str, dyn_cands: Vec<Candidate>) -> Vec<Candidate> {
    let mut subs: Vec<Candidate> = Vec::new();
    for s in spec.subs {
        if s.name.starts_with(typed) {
            subs.push(Candidate::plain(format!("{} {}", spec.name, s.name)));
        }
    }
    if spec.name == "npc" {
        dyn_cands.into_iter().chain(subs).collect()
    } else {
        subs.into_iter().chain(dyn_cands).collect()
    }
}

/// 按参数类型产出候选。`base` 是候选要拼上的前缀 (命令名或命令名+子命令),
/// `typed` 是正在输入的那一段。
fn candidates_for(arg: ArgSpec, base: &str, typed: &str, ctx: &Ctx) -> Vec<Candidate> {
    match arg {
        // 没有已知候选来源: 交给用户自己敲 (解析器会校验)。
        ArgSpec::None | ArgSpec::Free | ArgSpec::Number | ArgSpec::MapId => Vec::new(),
        ArgSpec::Values(vs) => {
            let mut out: Vec<Candidate> = vs
                .iter()
                .filter(|v| v.starts_with(typed))
                .map(|v| Candidate::plain(format!("{base} {v}")))
                .collect();
            out.dedup_by(|a, b| a.cmd == b.cmd);
            out
        }
        ArgSpec::MobOid => prefixed_ids(base, typed, ctx.mob_oids),
        ArgSpec::ReactorOid => prefixed_ids(base, typed, ctx.reactor_oids),
        ArgSpec::NpcOid => npc_candidates(base, typed, ctx.npcs),
        ArgSpec::Portal => portal_candidates(base, typed, ctx.portals),
        ArgSpec::RuleId => prefixed_ids(base, typed, ctx.rule_ids),
        ArgSpec::TaskId => prefixed_ids(base, typed, ctx.task_ids),
        ArgSpec::GroupId => prefixed_ids(base, typed, ctx.group_ids),
        ArgSpec::SkillId => prefixed_ids(base, typed, ctx.skill_ids),
        ArgSpec::ItemId => prefixed_ids(base, typed, ctx.item_ids),
        // `profiles add <路径>`: 候选来自已扫描到的档案列表。
        //
        // 值用 `profiles/<名>.json` 而不是裸档案名 —— 这一格要的是**文件路径**,
        // 补出一个名字用户还得自己加目录和扩展名。
        ArgSpec::Path => ctx
            .profiles
            .iter()
            .map(|p| format!("{}/{p}.json", crate::profiles_dir()))
            .filter(|s| s.starts_with(typed))
            .map(|s| Candidate::plain(format!("{base} {s}")))
            .collect(),
    }
}

fn prefixed_ids<T: ToString>(base: &str, prefix: &str, ids: &[T]) -> Vec<Candidate> {
    ids.iter()
        .map(|i| i.to_string())
        .filter(|s| s.starts_with(prefix))
        .map(|s| Candidate::plain(format!("{base} {s}")))
        .collect()
}

/// npc oid 候选: `npc 100001 -- 米娅` (描述展示 NPC 名, 指令不含描述)。
fn npc_candidates(base: &str, prefix: &str, npcs: &[(i32, i32)]) -> Vec<Candidate> {
    npcs.iter()
        .filter(|(oid, _)| oid.to_string().starts_with(prefix))
        .map(|(oid, npcid)| {
            Candidate::described(
                format!("{base} {oid}"),
                openstory_bot::names::npc_name(*npcid),
            )
        })
        .collect()
}

/// 传送门候选: `warp in00 -- 射手村东部` / `move out00 -- 射手村东部`。
///
/// 展示目标地图名, 指令不含描述。
fn portal_candidates(base: &str, prefix: &str, portals: &[(String, i32)]) -> Vec<Candidate> {
    portals
        .iter()
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, target_map)| {
            Candidate::described(
                format!("{base} {name}"),
                openstory_bot::names::map_name_text(*target_map),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmds(c: &[Candidate]) -> Vec<String> {
        c.iter().map(|c| c.cmd.clone()).collect()
    }

    #[test]
    fn completes_command_names() {
        let c = complete("hu");
        let names = cmds(&c);
        assert!(names.contains(&"hunt".to_string()));
        assert!(!names.contains(&"huntreactor".to_string()));
        assert!(!names.contains(&"view".to_string()));
        // reactor 子命令含 on/off
        let c = complete("reactor ");
        let names = cmds(&c);
        assert!(names.contains(&"reactor on".to_string()));
        assert!(names.contains(&"reactor off".to_string()));
        assert!(names.contains(&"reactor hit".to_string()));
        assert!(names.contains(&"reactor cooldown".to_string()));
    }

    #[test]
    fn completes_aliases_to_canonical() {
        let c = complete("cc");
        assert_eq!(c[0].cmd, "channel");
        assert_eq!(c[0].display, "channel");
        let c = complete("say");
        assert_eq!(c[0].cmd, "chat");
    }

    #[test]
    fn completes_subcommands() {
        let c = complete("hunt ");
        let names = cmds(&c);
        assert!(names.contains(&"hunt on".to_string()));
        assert!(names.contains(&"hunt skill".to_string()));
        assert!(names.contains(&"hunt attack".to_string()));
        assert!(names.contains(&"hunt range".to_string()));
        assert!(!names.contains(&"hunt all".to_string()));
        assert!(!names.contains(&"hunt bogus".to_string()));
    }

    #[test]
    fn completes_partial_sub() {
        let c = complete("hunt sk");
        assert_eq!(c[0].cmd, "hunt skill");
    }

    #[test]
    fn no_sub_no_completion() {
        assert!(complete("chat ").is_empty());
        assert!(complete("move 100 ").is_empty());
    }

    #[test]
    fn empty_input_lists_all() {
        let c = complete("");
        assert_eq!(c.len(), command_spec::COMMANDS.len());
        assert!(c.iter().any(|c| c.cmd == "view"));
        assert!(c.iter().any(|c| c.cmd == "quit"));
    }

    fn ctx() -> Ctx<'static> {
        static NPCS: [(i32, i32); 3] = [(100001, 9010011), (100002, 9000068), (200001, 9010011)];
        static MOBS: [i32; 2] = [50001, 50002];
        static REACTORS: [i32; 1] = [90001];
        static RULES: [&str; 2] = ["sellauto", "trade_accept"];
        static TASKS: [&str; 2] = ["sell", "hunt"];
        static GROUPS: [&str; 2] = ["bf_autosell", "autosell"];
        static ITEMS: [i32; 3] = [2000000, 2000001, 4000000];
        static SKILLS: [i32; 2] = [1001003, 1001005];
        static PORTALS: [(String, i32); 3] = [
            (String::new(), 101000000),
            (String::new(), 101000100),
            (String::new(), 100000000),
        ];
        // 静态数组里没法写 String, 用 leak 一次性构造 (测试专用)。
        let portals: &'static [(String, i32)] = Box::leak(Box::new([
            ("in00".to_string(), 101000000),
            ("in01".to_string(), 101000100),
            ("sp".to_string(), 100000000),
        ]));
        let _ = PORTALS;
        Ctx {
            npcs: &NPCS,
            mob_oids: &MOBS,
            reactor_oids: &REACTORS,
            rule_ids: &RULES,
            task_ids: &TASKS,
            group_ids: &GROUPS,
            portals,
            item_ids: &ITEMS,
            skill_ids: &SKILLS,
            profiles: &[],
        }
    }

    #[test]
    fn dynamic_npc_oids() {
        let c = complete_with("npc ", &ctx());
        // oid 数字候选在前, 对话子命令在后
        assert!(c[0].cmd.starts_with("npc 10"), "got {:?}", cmds(&c));
        assert!(c.iter().any(|c| c.cmd == "npc yes"));
        assert!(c.iter().any(|c| c.cmd == "npc reply"));
        assert!(c.iter().any(|c| c.cmd == "npc 100001"));
        assert!(c.iter().any(|c| c.cmd == "npc 200001"));
        // 展示带 -- 描述, 指令不含描述
        let npc1 = c.iter().find(|c| c.cmd == "npc 100001").unwrap();
        assert!(npc1.display.starts_with("npc 100001 -- "));
        assert_eq!(npc1.cmd, "npc 100001");
        let c2 = complete_with("npc 20", &ctx());
        assert_eq!(c2[0].cmd, "npc 200001");
        let c3 = complete_with("npc y", &ctx());
        assert_eq!(c3[0].cmd, "npc yes");
    }

    #[test]
    fn dynamic_attack_and_reactor() {
        let c = complete_with("attack 5", &ctx());
        assert_eq!(c[0].cmd, "attack 50001");
        assert_eq!(c[1].cmd, "attack 50002");
        let c2 = complete_with("reactor hit ", &ctx());
        assert_eq!(c2[0].cmd, "reactor hit 90001");
    }

    #[test]
    fn dynamic_warp_portals() {
        let c = complete_with("warp ", &ctx());
        assert!(c.iter().any(|c| c.cmd == "warp in00"));
        assert!(c.iter().any(|c| c.cmd == "warp in01"));
        assert!(c.iter().any(|c| c.cmd == "warp sp"));
        let in00 = c.iter().find(|c| c.cmd == "warp in00").unwrap();
        assert!(in00.display.contains("warp in00"));
        assert!(in00.display.contains("--"));
        let c2 = complete_with("warp in", &ctx());
        assert_eq!(c2.len(), 2);
        assert!(c2.iter().any(|c| c.cmd == "warp in00"));
        assert!(c2.iter().any(|c| c.cmd == "warp in01"));
    }

    #[test]
    fn dynamic_move_portals() {
        let c = complete_with("move ", &ctx());
        assert!(c.iter().any(|c| c.cmd == "move in00"), "got {:?}", cmds(&c));
        let c2 = complete_with("move in0", &ctx());
        assert_eq!(c2[0].cmd, "move in00");
        let c3 = complete_with("movewarp in0", &ctx());
        assert_eq!(c3[0].cmd, "movewarp in00");
        // 别名同样生效
        let c4 = complete_with("mw in0", &ctx());
        assert_eq!(c4[0].cmd, "movewarp in00");
    }

    #[test]
    fn dynamic_rule_task_group_ids() {
        let c = complete_with("rule open s", &ctx());
        assert_eq!(c[0].cmd, "rule open sellauto");
        let c2 = complete_with("task run ", &ctx());
        assert_eq!(c2[0].cmd, "task run sell");
        assert_eq!(c2[1].cmd, "task run hunt");
        let c2b = complete_with("task start s", &ctx());
        assert_eq!(c2b[0].cmd, "task start sell");
        let c3 = complete_with("group run b", &ctx());
        assert_eq!(c3[0].cmd, "group run bf_autosell");
        let c3b = complete_with("group close b", &ctx());
        assert_eq!(c3b[0].cmd, "group close bf_autosell");
        let c4 = complete_with("group run ", &ctx());
        assert_eq!(c4[0].cmd, "group run bf_autosell");
        assert_eq!(c4[1].cmd, "group run autosell");
        let c5 = complete_with("group stop ", &ctx());
        assert_eq!(c5[0].cmd, "group stop bf_autosell");
        let c7 = complete_with("task stop h", &ctx());
        assert!(
            c7.is_empty(),
            "`task stop` 不接受 id (解析器拒绝 `task stop hunt`), 不应补出无效命令: {:?}",
            cmds(&c7)
        );
    }

    #[test]
    fn dynamic_item_and_skill_ids() {
        let c = complete_with("view eqpinfo 200", &ctx());
        assert_eq!(
            cmds(&c),
            vec!["view eqpinfo 2000000", "view eqpinfo 2000001"]
        );
        let c2 = complete_with("sell 400", &ctx());
        assert_eq!(cmds(&c2), vec!["sell 4000000"]);
        let c3 = complete_with("skill learn 100", &ctx());
        assert_eq!(
            cmds(&c3),
            vec!["skill learn 1001003", "skill learn 1001005"]
        );
    }

    #[test]
    fn fixed_value_arguments_complete() {
        // 直接测参数层, 排除上层分支的干扰
        let ctx = ctx();
        // 前缀按字面过滤: `o` 同时命中 on 与 off (两者都以 o 开头)
        assert_eq!(
            cmds(&candidates_for(
                ArgSpec::Values(&["on", "off"]),
                "hunt pickup",
                "o",
                &ctx
            )),
            vec!["hunt pickup on", "hunt pickup off"]
        );
        assert_eq!(
            cmds(&candidates_for(
                ArgSpec::Values(&["on", "off"]),
                "hunt pickup",
                "of",
                &ctx
            )),
            vec!["hunt pickup off"]
        );
        let c = complete_with("hunt pickup ", &ctx);
        assert_eq!(cmds(&c), vec!["hunt pickup on", "hunt pickup off"]);
        let c2 = complete_with("hunt pickup of", &ctx);
        assert_eq!(cmds(&c2), vec!["hunt pickup off"]);
        // `hunt pickup <px>` 也合法 (像素半径): 敲数字时不该回退去列子命令
        let c2b = complete_with("hunt pickup 4", &ctx);
        assert!(
            c2b.is_empty(),
            "枚举型参数匹配不上时应不补, 而不是列子命令: {:?}",
            cmds(&c2b)
        );
        let c3 = complete_with("hunt filter ", &ctx);
        assert!(cmds(&c3).contains(&"hunt filter off".to_string()));
        assert!(cmds(&c3).contains(&"hunt filter allow".to_string()));
        // 子命令自身带 Values 参数 (没有 dyn_args 时走后一条路径)
        let c4 = complete_with("gather controller ", &ctx);
        assert_eq!(
            cmds(&c4),
            vec!["gather controller on", "gather controller off"]
        );
    }

    #[test]
    fn static_subs_still_work() {
        let c = complete_with("hunt a", &ctx());
        assert!(c.iter().any(|c| c.cmd == "hunt attack"));
        let c3 = complete("as");
        assert!(!c3.iter().any(|c| c.cmd == "assist"));
        let c4 = complete("ste");
        assert!(!c4.iter().any(|c| c.cmd == "stealth"));
    }

    /// 漂移回归: 这些子命令解析器一直接受, 但改造前的补全表漏了。
    ///
    /// 注意方向: 只补"解析器真的接受"的写法。曾经在这里写过 `task cancel` /
    /// `group start` —— 那是**反向漂移** (补出来必然报错), 已由
    /// `tests/spec_matches_parser.rs` 钉死。
    #[test]
    fn previously_missing_subs_now_complete() {
        let names = cmds(&complete("task "));
        for s in ["task stop", "task status", "task start", "task run"] {
            assert!(names.iter().any(|n| n == s), "{s} 应可补全: {names:?}");
        }
        assert!(
            !names.iter().any(|n| n == "task cancel"),
            "task cancel 解析器不接受, 不该补: {names:?}"
        );
        let names = cmds(&complete("group "));
        for s in [
            "group run",
            "group open",
            "group stop",
            "group close",
            "group status",
        ] {
            assert!(names.iter().any(|n| n == s), "{s} 应可补全: {names:?}");
        }
        assert!(
            !names.iter().any(|n| n == "group start"),
            "group start 解析器不接受, 不该补: {names:?}"
        );
        let names = cmds(&complete("view "));
        assert!(
            names.contains(&"view eqpinfo".to_string()),
            "view eqpinfo 应可补全"
        );
        assert!(names.contains(&"view portals".to_string()));
        let names = cmds(&complete("sell "));
        assert!(names.contains(&"sell tab".to_string()));
    }

    #[test]
    fn new_commands_in_completion() {
        let names = cmds(&complete("hunt "));
        assert!(names.contains(&"hunt atkcd".to_string()));
        assert!(names.contains(&"hunt save".to_string()));
        let c = complete("reco");
        assert_eq!(c[0].cmd, "reconnect");
        let names = cmds(&complete("reconnect "));
        assert!(names.contains(&"reconnect on".to_string()));
        assert!(names.contains(&"reconnect delay".to_string()));
        assert!(names.contains(&"reconnect status".to_string()));
        // 别名补成规范名
        assert_eq!(complete("atk")[0].cmd, "attack");
        assert_eq!(complete("drink")[0].cmd, "use");
        assert!(cmds(&complete("wa")).contains(&"warp".to_string()));
        assert_eq!(complete("ch")[0].cmd, "channel");
        // view 快捷方式已移除: 所有查看都走 `view <obj>`
        assert!(complete("mobs").is_empty());
        assert!(complete("inv").is_empty());
        assert!(complete("status").is_empty());
    }

    #[test]
    fn unknown_command_yields_nothing() {
        assert!(complete("nosuchcmd ").is_empty());
        assert!(complete("nosuchcmd x").is_empty());
    }

    // ── `@` 多会话目标补全 ────────────────────────────────────────────

    fn profiles() -> Vec<String> {
        vec![
            "serverA_100000003".into(),
            "serverA_100000004".into(),
            "serverB_100000003".into(),
        ]
    }

    fn ctx_with_profiles(p: &[String]) -> Ctx<'_> {
        Ctx {
            profiles: p,
            ..Ctx::default()
        }
    }

    #[test]
    fn target_completion_lists_all_and_matching_profiles() {
        let p = profiles();
        let ctx = ctx_with_profiles(&p);
        // 裸 `@` -> @all + 全部账号
        let c = complete_with("@", &ctx);
        let names = cmds(&c);
        assert!(names.contains(&"@all".to_string()), "{names:?}");
        assert!(names.contains(&"@serverA_100000003".to_string()));
        assert!(names.contains(&"@serverB_100000003".to_string()));
        // `@serverA` -> 只剩 serverA_* (前缀过滤同样作用于 @all: `@all` 不以 `@serverA` 开头)
        let c = complete_with("@serverA", &ctx);
        let names = cmds(&c);
        assert!(!names.contains(&"@all".to_string()), "{names:?}");
        assert!(names.contains(&"@serverA_100000003".to_string()));
        assert!(!names.contains(&"@serverB_100000003".to_string()));
        // `@a` -> 只剩 @all
        let c = complete_with("@a", &ctx);
        assert_eq!(cmds(&c), vec!["@all"]);
        // `@serverB` -> 只剩 serverB
        let c = complete_with("@serverB", &ctx);
        assert_eq!(cmds(&c), vec!["@serverB_100000003"]);
    }

    #[test]
    fn target_completion_is_case_insensitive() {
        let p = profiles();
        let ctx = ctx_with_profiles(&p);
        let c = complete_with("@SERVERA", &ctx);
        assert!(cmds(&c).contains(&"@serverA_100000003".to_string()));
        let c = complete_with("@ALL", &ctx);
        assert!(cmds(&c).contains(&"@all".to_string()));
    }

    #[test]
    fn target_completion_skips_fully_typed_profile() {
        let p = profiles();
        let ctx = ctx_with_profiles(&p);
        // 已经完整敲出来的那个不再重复列 (否则 Tab 会在原地打转)
        let c = complete_with("@serverB_100000003", &ctx);
        assert!(
            !cmds(&c).contains(&"@serverB_100000003".to_string()),
            "{:?}",
            cmds(&c)
        );
    }

    #[test]
    fn target_completion_absent_for_single_session() {
        // 单会话不该出现"发给谁"的概念 —— 裸 @ 仍然是错的输入, 不补
        let one = vec!["only_1".to_string()];
        let ctx = ctx_with_profiles(&one);
        assert!(complete_with("@", &ctx).is_empty());
        assert!(complete_with("@o", &ctx).is_empty());
    }

    #[test]
    fn target_completion_ignores_non_target_position() {
        let p = profiles();
        let ctx = ctx_with_profiles(&p);
        // 没有 @ 前缀 -> 走普通命令补全 (`zz` 不是任何命令的前缀)
        let c = complete_with("zz", &ctx);
        assert!(cmds(&c).is_empty(), "没有以 zz 开头的命令: {:?}", cmds(&c));
        // 指令中间的 @ 不触发目标补全 (聊天里的 @某人)
        let c = complete_with("chat @bai", &ctx);
        assert!(c.is_empty(), "指令中间不该补目标: {:?}", cmds(&c));
    }

    #[test]
    fn target_completion_does_not_break_normal_commands() {
        let p = profiles();
        let ctx = ctx_with_profiles(&p);
        // 有账号列表时普通命令补全不受影响
        let c = complete_with("hu", &ctx);
        assert_eq!(cmds(&c), vec!["hunt"]);
        // 命令名 + 空格: 走子命令补全 (不是目标补全)
        let c = complete_with("hunt ", &ctx);
        assert!(cmds(&c).contains(&"hunt on".to_string()));
    }
}
