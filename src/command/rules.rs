//! Rule engine: checked unconditionally every tick (side-channel behavior).
//!
//! Two input kinds:
//! - Potion rules (`cfg.potion`): stat percent below the threshold → use the first potion in stock
//!   by priority (shared 800ms cooldown, at most one potion per tick).
//! - General rules (`cfg.rules`): predicate matched and cooldown elapsed → run actions in order.
//!   Actions = command strings (reuse the parse/run pipeline, same entry as task steps).

use std::time::{Duration, Instant};

use crate::runtime_config::{PotionRule, Rule};
use crate::session::Session;
use crate::state::BotState;

/// Run a configured action: parse and execute through the command pipeline (same entry as task steps).
/// A `quit` action ends the session: returns Err("quit"), propagated up through tick → runtime,
/// which ends the session with RunOutcome::Quit.
pub async fn run_action(state: &mut BotState, session: &mut Session, action: &str) -> Result<(), String> {
    match crate::command::parse(action) {
        Ok(Some(cmd)) => run_action_cmd(state, session, cmd).await,
        Ok(None) => Ok(()),
        Err(e) => {
            crate::emit_f!([crate::emit::Field::Text => e] => "[rules] bad action '{action}': {e}");
            Ok(())
        }
    }
}

/// Run an already-parsed command. Used by the task engine, which pre-parses and
/// caches each step's command so the hot tick loop doesn't re-parse strings.
pub async fn run_action_cmd(
    state: &mut BotState,
    session: &mut Session,
    cmd: crate::command::Command,
) -> Result<(), String> {
    match cmd {
        crate::command::Command::Quit => {
            crate::emit!("bye");
            Err("quit".to_string())
        }
        cmd => {
            crate::command::run(cmd, state, session).await?;
            Ok(())
        }
    }
}

/// Rules to fire this tick: enabled, predicate matched, cooldown elapsed (pure function, testable).
/// Cooldown and predicate evaluation share `rule_last_fire` (behavior unchanged).
/// The return value only borrows `rules` (independent of `state`, so callers can mutably borrow state freely).
pub fn rules_to_fire<'s, 'r>(
    state: &'s BotState,
    rules: &'r [Rule],
) -> Vec<&'r Rule> {
    rules
        .iter()
        .filter(|r| {
            if !r.enabled {
                return false;
            }
            let last = state.rule_last_fire.get(&r.id).copied();
            if !crate::runtime_config::eval_predicate(state, &r.when) {
                return false;
            }
            let cd = Duration::from_millis(r.cooldown);
            match last {
                Some(t) if t.elapsed() < cd => false,
                _ => true,
            }
        })
        .collect()
}

/// Called every tick: potion rules + general rules.
pub async fn run_rules(state: &mut BotState, session: &mut Session) -> Result<(), String> {
    // Potion (shared cooldown, at most one per tick; the cooldown ms comes from config potion_cooldown)
    if state.last_potion.elapsed()
        >= std::time::Duration::from_millis(state.cfg.potion_cooldown)
    {
        for rule in &state.cfg.potion {
            if let Some(itemid) = potion_pick(state, rule) {
                let found = state.inventory.iter().find(|((tab, _), it)| {
                    *tab == 2 && it.itemid == itemid && it.qty > 0
                });
                if let Some(((_, slot), _)) = found {
                    session.send_packet(crate::packets::item::use_item(*slot, itemid)).await?;
                    state.last_potion = Instant::now();
                    let (cur, max) = match rule.stat.as_str() {
                        "hp" => (state.hp, state.maxhp),
                        "mp" => (state.mp, state.maxmp),
                        _ => (0, 0),
                    };
                    let cur_pct = if max > 0 {
                        cur as i32 * 100 / max as i32
                    } else {
                        0
                    };
                    crate::emit_f!([crate::emit::Field::ItemId => itemid, crate::emit::Field::Slot => slot, crate::emit::Field::Percent => rule.threshold_pct] => 
                        "[potion] {} {cur_pct}% ({cur}/{max}) <{}% -> {itemid} slot={slot}",
                        rule.stat, rule.threshold_pct);
                    break;
                }
            }
        }
    }

    // General rules (top-level rules + rules of open groups; in-group rules are governed by the
    // group switch, see RuntimeConfig::effective_rules)
    let rules = state.cfg.effective_rules();
    // Collect indices first to avoid borrow conflict: the filter borrows `state`
    // immutably (eval_predicate + rule_last_fire), but the loop body needs &mut state.
    let fire_indices: Vec<usize> = rules
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            if !r.enabled {
                return false;
            }
            let last = state.rule_last_fire.get(&r.id).copied();
            if !crate::runtime_config::eval_predicate(state, &r.when) {
                return false;
            }
            let cd = Duration::from_millis(r.cooldown);
            match last {
                Some(t) if t.elapsed() < cd => false,
                _ => true,
            }
        })
        .map(|(i, _)| i)
        .collect();
    for &idx in &fire_indices {
        let rule = &rules[idx];
        // Record the cooldown before running: failed actions also count, preventing per-tick
        // retry spam (the failure is logged once, then gated by cooldown).
        state.rule_last_fire.insert(rule.id.clone(), Instant::now());
        let mut actions = rule.then.iter();
        while let Some(action) = actions.next() {
            // `wait <pred> [ms]` or `while <pred> do <cmd>` anywhere in the action
            // list: this rule's remaining sequence becomes an inline task. `wait`
            // becomes a pure-gate step; `while` becomes a loop step (loop_pred).
            // The flow pauses across ticks instead of racing the 1ms tick. The
            // inline runtime carries the owning group (looked up from the config)
            // so `group stop` can kill it — otherwise an inlined `while` loop
            // would keep running forever after the group is closed.
            let parsed = crate::command::parse(action);
            if let Ok(Some(c)) = &parsed {
                if matches!(
                    c,
                    crate::command::Command::Wait(_, _) | crate::command::Command::While(_, _)
                ) {
                    // Re-entrancy guard: one queued instance per rule (mirrors
                    // start_task's same-id no-op). Without it, a rule firing again
                    // while an earlier instance is still queued would pile up
                    // unbounded runtimes on the stack.
                    if state
                        .task_stack
                        .iter()
                        .any(|rt| rt.def_id == rule.id)
                    {
                        crate::emit_f!([crate::emit::Field::Name => rule.id] => "[rules] '{}' flow already queued — skip", rule.id);
                        break;
                    }
                    // Note: `actions` is consumed here — the matched action plus the
                    // remaining actions move into the steps.
                    let mut seq: Vec<String> = Vec::with_capacity(actions.size_hint().0 + 1);
                    seq.push(action.clone());
                    seq.extend(actions.cloned());
                    let steps = wait_inline_steps(&seq);
                    let step_count = steps.len();
                    let owning_group = state
                        .cfg
                        .groups
                        .iter()
                        .find(|g| g.rules.iter().any(|r| r.id == rule.id))
                        .map(|g| g.id.clone());
                    state.task_stack.push(crate::runtime_config::TaskRuntime {
                        def_id: rule.id.clone(),
                        steps,
                        step_idx: 0,
                        step_since: Instant::now(),
                        group: owning_group.clone(),
                        hunt_before: state.hunt,
                        recurring: false,
                    });
                    crate::emit_f!([crate::emit::Field::Name => rule.id, crate::emit::Field::Count => step_count] => "[rules] '{}' inlined {} step(s) (group {:?})", rule.id, step_count, owning_group);
                    break;
                }
            }
            // `task run <id>` inlines the task: its steps (with wait predicates)
            // are pushed first, then the rule's remaining actions as plain steps
            // — one runtime drives the whole flow via tick_tasks, so the rule
            // can reuse setup tasks (e.g. open_shop) without duplicating steps.
            if let Ok(Some(crate::command::Command::Task(crate::command::TaskCmd::Start(id)))) =
                crate::command::parse(action)
            {
                let rest: Vec<String> = actions.cloned().collect();
                match inline_task_steps(state, &id, &rest) {
                    Ok(steps) => {
                        // Carry the task's owning group (from task_with_group) so
                        // `group stop` can kill an inlined task run — otherwise the
                        // runtime would have group:None and loop forever after stop.
                        let grp = state.cfg.task_with_group(&id).and_then(|(_, g, _)| g);
                        state.task_stack.push(crate::runtime_config::TaskRuntime {
                            def_id: id.clone(),
                            steps,
                            step_idx: 0,
                            step_since: Instant::now(),
                            group: grp,
                            hunt_before: state.hunt,
                            recurring: false,
                        });
                        crate::emit_f!([crate::emit::Field::Name => rule.id, crate::emit::Field::Text => &id, crate::emit::Field::Count => rest.len()] => 
                            "[rules] '{}' inlined task '{id}' + {} action(s)", rule.id, rest.len());
                    }
                    Err(e) => {
                        crate::emit_f!([crate::emit::Field::Name => rule.id, crate::emit::Field::Text => e] => "[rules] '{}' task run failed: {e}", rule.id);
                        // failed to resolve: run the remaining actions directly
                        for a in &rest {
                            if let Err(e) = run_action(state, session, a).await {
                                if e == "quit" {
                                    return Err(e);
                                }
                                crate::emit_f!([crate::emit::Field::Name => rule.id, crate::emit::Field::Text => e] => "[rules] '{}' action '{a}' error: {e}", rule.id);
                            }
                        }
                    }
                }
                break;
            }
            if let Err(e) = run_action(state, session, action).await {
                if e == "quit" {
                    return Err(e);
                }
                crate::emit_f!([crate::emit::Field::Name => rule.id, crate::emit::Field::Text => e] => "[rules] '{}' action '{action}' error: {e}", rule.id);
            }
        }
    }
    Ok(())
}

/// Inline a `task run <id>` action inside a rule: the referenced task's steps
/// (vars/default-timeout expanded, wait predicates kept) followed by the
/// rule's remaining actions as plain steps (no wait, no timeout).
pub fn inline_task_steps(
    state: &BotState,
    task_id: &str,
    rest: &[String],
) -> Result<Vec<crate::runtime_config::StepDef>, String> {
    let (def, _, _) = state
        .cfg
        .task_with_group(task_id)
        .ok_or_else(|| format!("unknown task: {task_id}"))?;
    let mut steps = crate::command::tasks::expanded_steps(&def);
    steps.extend(
        rest.iter()
            .map(|cmd| crate::runtime_config::StepDef {
                cmd: cmd.clone(),
                wait: None,
                timeout: None,
                loop_pred: None, cmd_parsed: None,
            }),
    );
    Ok(steps)
}

/// Convert a rule's action sequence (from the first `wait` onward) into inline-task
/// steps: EVERY `wait <pred> [ms]` action becomes a pure-gate step (empty cmd,
/// wait+timeout), and every other action becomes a plain step. Previously only the
/// FIRST wait was converted, leaving later waits as literal `wait` command steps that
/// error at runtime ("wait 非独立命令") and abandon the task — see `boss_summon`'s
/// `wait reactors>0 … wait mobs>0 …` chain.
pub fn wait_inline_steps(actions: &[String]) -> Vec<crate::runtime_config::StepDef> {
    let mut steps = Vec::with_capacity(actions.len());
    for a in actions {
        match crate::command::parse(a) {
            Ok(Some(crate::command::Command::Wait(pred, ms))) => {
                steps.push(crate::runtime_config::StepDef {
                    cmd: String::new(),
                    wait: Some(pred),
                    // No implicit default: `wait <pred>` (no ms) blocks until the
                    // predicate is satisfied; `wait <pred> ms` caps the wait and
                    // continues to the next step on timeout (see tick_tasks).
                    timeout: ms,
                    loop_pred: None, cmd_parsed: None,
                });
            }
            Ok(Some(crate::command::Command::While(pred, cmd))) => {
                // Loop step: `cmd` runs each tick while `pred` holds; tick_tasks
                // advances immediately once pred goes false (no hidden grace window).
                steps.push(crate::runtime_config::StepDef {
                    cmd,
                    wait: None,
                    timeout: None,
                    loop_pred: Some(pred), cmd_parsed: None,
                });
            }
            // Plain action (or unparseable — run_action surfaces the error at runtime,
            // same as a normal rule action).
            _ => steps.push(crate::runtime_config::StepDef {
                cmd: a.clone(),
                wait: None,
                timeout: None,
                loop_pred: None, cmd_parsed: None,
            }),
        }
    }
    steps
}

/// Potion selection (pure function, testable): stat percent below threshold → first potion in stock.
pub fn potion_pick(state: &BotState, rule: &PotionRule) -> Option<i32> {
    crate::runtime_config::potion_pick(state, rule)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, enabled: bool, when: &str, cd: u64) -> Rule {
        Rule {
            id: id.into(),
            enabled,
            when: when.into(),
            then: Vec::new(),
            cooldown: cd,
        }
    }

    #[test]
    fn engine_skips_disabled_rules() {
        let s = BotState::default();
        let rules = vec![
            rule("on", true, "always", 0),
            rule("off", false, "always", 0),
        ];
        let fired = rules_to_fire(&s, &rules);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].id, "on");
    }

    #[test]
    fn wait_action_converts_to_gate_plus_rest() {
        let steps = wait_inline_steps(&[
            "wait mobs>0 5000".to_string(),
            "reactor hit all".to_string(),
            "setvar boss_summoned 1".to_string(),
        ]);
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].cmd, "");
        assert_eq!(steps[0].wait.as_deref(), Some("mobs>0"));
        assert_eq!(steps[0].timeout, Some(5000));
        assert_eq!(steps[1].cmd, "reactor hit all");
        assert_eq!(steps[1].wait, None);
        assert_eq!(steps[2].cmd, "setvar boss_summoned 1");
        // 无超时参数 → 无限等(直到谓词满足, 无上限, 不跳过)
        let steps2 = wait_inline_steps(&["wait dialog==0".to_string()]);
        assert_eq!(steps2.len(), 1);
        assert_eq!(steps2[0].wait.as_deref(), Some("dialog==0"));
        assert_eq!(steps2[0].timeout, None);
        // 非 wait 动作 → 普通步(不转 gate)
        let steps3 = wait_inline_steps(&["warp in00".to_string()]);
        assert_eq!(steps3.len(), 1);
        assert_eq!(steps3[0].cmd, "warp in00");
        assert_eq!(steps3[0].wait, None);
        // 谓词内含数字的尾 token 不会被误判为超时 (wait mobs==1 是单 token 谓词)
        let steps4 = wait_inline_steps(&["wait mobs==1".to_string()]);
        assert_eq!(steps4[0].wait.as_deref(), Some("mobs==1"));
        assert_eq!(steps4[0].timeout, None);
    }

    #[test]
    fn wait_action_converts_every_wait_in_sequence() {
        // boss_summon 形态: 多个 wait 都必须转成 gate, 不能把第二个 wait 当成命令步
        // (旧 bug: 第二个 wait 变成 cmd="wait mobs>0 5000" → 运行时报 "wait 非独立命令" 并放弃)
        let steps = wait_inline_steps(&[
            "wait reactors>0 5000".to_string(),
            "reactor hit all".to_string(),
            "wait mobs>0 5000".to_string(),
            "setvar boss_summoned 1".to_string(),
        ]);
        assert_eq!(steps.len(), 4);
        // step 0: gate reactors
        assert_eq!(steps[0].cmd, "");
        assert_eq!(steps[0].wait.as_deref(), Some("reactors>0"));
        assert_eq!(steps[0].timeout, Some(5000));
        // step 1: plain reactor hit
        assert_eq!(steps[1].cmd, "reactor hit all");
        assert_eq!(steps[1].wait, None);
        // step 2: gate mobs (关键: 必须是 gate, 不是 cmd="wait mobs>0 5000")
        assert_eq!(steps[2].cmd, "");
        assert_eq!(steps[2].wait.as_deref(), Some("mobs>0"));
        assert_eq!(steps[2].timeout, Some(5000));
        // step 3: plain setvar
        assert_eq!(steps[3].cmd, "setvar boss_summoned 1");
        assert_eq!(steps[3].wait, None);
    }

    #[test]
    fn engine_respects_predicate_and_cooldown() {
        let mut s = BotState::default();
        let rules = vec![
            rule("no", true, "hunt==1", 0),
            rule("cd", true, "always", 60_000),
        ];
        assert_eq!(rules_to_fire(&s, &rules).len(), 1); // only cd fires (hunt==0 blocks no)
        s.hunt = true;
        assert_eq!(rules_to_fire(&s, &rules).len(), 2);
        // Cooldown: re-check right after firing → cd is blocked, no (no cooldown) still fires
        s.rule_last_fire.insert("cd".into(), Instant::now());
        s.rule_last_fire.insert("no".into(), Instant::now());
        let fired: Vec<_> = rules_to_fire(&s, &rules)
            .iter()
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(fired, vec!["no"]);
    }

    #[test]
    fn engine_all_disabled_fires_nothing() {
        let s = BotState::default();
        let rules = vec![rule("a", false, "always", 0), rule("b", false, "always", 0)];
        assert!(rules_to_fire(&s, &rules).is_empty());
    }

    #[test]
    fn while_action_converts_to_loop_step() {
        // 单个 while → 一个 loop step(cmd + loop_pred), 其余动作保持普通步
        let steps = wait_inline_steps(&[
            "while mobs>0 do hunt once".to_string(),
            "setvar done 1".to_string(),
        ]);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].cmd, "hunt once");
        assert_eq!(steps[0].loop_pred.as_deref(), Some("mobs>0"));
        assert_eq!(steps[0].wait, None);
        assert_eq!(steps[0].timeout, None);
        assert_eq!(steps[1].cmd, "setvar done 1");
        assert_eq!(steps[1].loop_pred, None);
        // while 紧跟纯 wait 混合 → 各自正确转换
        let steps2 = wait_inline_steps(&[
            "wait reactors>0 5000".to_string(),
            "while mobs>0 do hunt once".to_string(),
            "while drops>0 do pickup all".to_string(),
        ]);
        assert_eq!(steps2.len(), 3);
        assert_eq!(steps2[0].wait.as_deref(), Some("reactors>0"));
        assert_eq!(steps2[1].loop_pred.as_deref(), Some("mobs>0"));
        assert_eq!(steps2[2].loop_pred.as_deref(), Some("drops>0"));
    }

    #[test]
    fn while_inline_uses_rule_owning_group_for_group_stop() {
        // 当 while 出现在某个 group 的 rule 内联流程里, 内联 runtime 必须带上该 group,
        // 否则 group stop 无法杀掉正在跑的 while 循环。
        use crate::runtime_config::{GroupDef, Rule};
        let mut s = BotState::default();
        s.cfg.groups.push(GroupDef {
            id: "paplatus".into(),
            enabled: true,
            exclusive: true,
            rules: vec![Rule {
                id: "boss_fight".into(),
                enabled: true,
                when: "always".into(),
                then: vec!["while mobs>0 do hunt once".to_string()],
                cooldown: 0,
            }],
            tasks: vec![],
        });
        // 模拟 run_rules 的内联逻辑: owning_group 从 cfg.groups 反查 rule.id
        let owning_group = s
            .cfg
            .groups
            .iter()
            .find(|g| g.rules.iter().any(|r| r.id == "boss_fight"))
            .map(|g| g.id.clone());
        assert_eq!(owning_group.as_deref(), Some("paplatus"));
        // 带上 owning_group 的 runtime 应被 group stop 杀掉
        s.task_stack.push(crate::runtime_config::TaskRuntime {
            def_id: "boss_fight".into(),
            steps: crate::command::tasks::expanded_steps(&crate::runtime_config::TaskDef {
                id: "x".into(),
                priority: 1,
                timeout: None,
                vars: Default::default(),
                recurring: false,
                steps: vec![crate::runtime_config::StepDef {
                    cmd: "hunt once".into(),
                    wait: None,
                    timeout: None,
                    loop_pred: Some("mobs>0".into()), cmd_parsed: None,
                }],
            }),
            step_idx: 0,
            step_since: std::time::Instant::now(),
            group: owning_group.clone(),
            hunt_before: false,
            recurring: false,
        });
        assert_eq!(crate::command::tasks::stop_tasks_in_group(&mut s, "paplatus"), 1);
        assert!(s.task_stack.is_empty());
    }

    #[test]
    fn inline_task_steps_concatenates_task_then_rule_actions() {
        // cfg with a task `open_shop` (2 steps, one with a wait)
        let mut s = BotState::default();
        s.cfg.tasks = vec![crate::runtime_config::TaskDef {
            id: "open_shop".into(),
            priority: 10,
            timeout: Some(5000),
            vars: std::collections::BTreeMap::new(),
            recurring: false,
            steps: vec![
                crate::runtime_config::StepDef {
                    cmd: "auction".into(),
                    wait: None,
                    timeout: None,
                    loop_pred: None, cmd_parsed: None,
                },
                crate::runtime_config::StepDef {
                    cmd: "sleep 0".into(),
                    wait: Some("shop_open==1".into()),
                    timeout: None,
                    loop_pred: None, cmd_parsed: None,
                },
            ],
        }];
        let rest = vec!["sell tab equip".to_string(), "shop leave".to_string()];
        let steps = inline_task_steps(&s, "open_shop", &rest).expect("inline");
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[0].cmd, "auction");
        assert_eq!(steps[0].timeout, Some(5000)); // task default timeout applied
        assert_eq!(steps[1].cmd, "sleep 0");
        assert_eq!(steps[1].wait.as_deref(), Some("shop_open==1")); // wait kept
        assert_eq!(steps[2].cmd, "sell tab equip");
        assert_eq!(steps[2].wait, None); // rule actions: plain steps
        assert_eq!(steps[3].cmd, "shop leave");
        // unknown task: error
        assert!(inline_task_steps(&s, "nope", &rest).is_err());
    }
}
