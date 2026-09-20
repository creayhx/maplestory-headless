//! Task engine: linear command sequences + cursor advancement (lock-stack model).
//!
//! Each main-loop round: rule engine (matching, non-blocking) → task engine (lock holder runs) →
//! when no task holds the lock, default behaviors like hunt run. A task = the current lock holder;
//! only the stack-top task's cursor advances. Completion/failure/stop pops the stack and releases
//! the lock, and the main loop continues next round (hunt resumes naturally, no resume chain).
//! Nested tasks A→B→C pop in C→B→A order (LIFO).
//!
//! Steps are expanded and frozen at start; placeholders (`{hunt_mapid}`, `{mapid}`) resolve before execution.

use std::time::Instant;

use crate::runtime_config::{StepDef, TaskDef, TaskRuntime};
use crate::session::Session;
use crate::state::{BotState, Phase};

/// Replace placeholders before execution: {hunt_mapid} → the hunt map snapshotted at start;
/// {mapid} → the current map. NPC dialogs write a fixed npcid in the step
/// (`npc 1011100`); the oid is resolved dynamically by the npc command.
fn resolve_cmd(state: &BotState, cmd: &str) -> Result<String, String> {
    let mut out = cmd.replace("{hunt_mapid}", &state.hunt_mapid.to_string());
    out = out.replace("{mapid}", &state.mapid.to_string());
    Ok(out)
}

/// Resolve (and cache) the command for the runtime's current step. Static steps
/// (no `{placeholder}`) are parsed once and cached in `StepDef::cmd_parsed` so the
/// tick loop doesn't re-parse the same string every tick; dynamic steps are
/// resolved + parsed each tick because their values change.
fn step_command(state: &mut BotState, rt_idx: usize) -> Result<crate::command::Command, String> {
    let (cmd_str, cached) = {
        let rt = &state.task_stack[rt_idx];
        let s = &rt.steps[rt.step_idx];
        (s.cmd.clone(), s.cmd_parsed.clone())
    };
    if cmd_str.contains('{') {
        let resolved = resolve_cmd(state, &cmd_str)?;
        return crate::command::parse(&resolved)
            .map_err(|e| e)?
            .ok_or_else(|| format!("empty command step: {resolved}"));
    }
    if let Some(c) = cached {
        return Ok(c);
    }
    match crate::command::parse(&cmd_str) {
        Ok(Some(c)) => {
            if let Some(rt) = state.task_stack.get_mut(rt_idx) {
                if let Some(s) = rt.steps.get_mut(rt.step_idx) {
                    s.cmd_parsed = Some(c.clone());
                }
            }
            Ok(c)
        }
        Ok(None) => Err(format!("empty command step: {cmd_str}")),
        Err(e) => Err(e),
    }
}

/// Start a task: push onto the lock stack (LIFO — the later start runs first).
/// Same id already on the stack → no-op. A task in a group only starts while its group is open
/// (group = master feature switch, see `RuntimeConfig::task_with_group`). Snapshot the current map
/// as the hunt site (for {hunt_mapid}) and the hunt switch (restored on pop); steps are expanded
/// and frozen — steps without their own `timeout` inherit the task default (TaskDef.timeout).
/// While a task holds the lock, tick yields to it; steps like town turn hunt off internally, and
/// when the whole lock stack releases, hunt is restored from the snapshot (see pop_top / stop_task).
pub fn start_task(state: &mut BotState, id: &str) -> Result<(), String> {
    let (def, group_id, group_ok) = state
        .cfg
        .task_with_group(id)
        .ok_or_else(|| format!("unknown task: {id}"))?;
    // Group gate: a task only starts while its group is open (group = master feature switch).
    if let Some(gid) = group_id.as_ref() {
        if !group_ok {
            return Err(format!(
                "task '{id}' is in disabled group '{gid}' — open the group first (group open {gid})"
            ));
        }
    }
    if state.task_stack.iter().any(|rt| rt.def_id == id) {
        return Ok(());
    }
    // Snapshot the current map as the hunt site (for {hunt_mapid}).
    state.hunt_mapid = state.mapid;
    // Apply the task-level default timeout to steps without their own (expanded and frozen at start),
    // and substitute task vars into cmd/wait ({key} → value; unmatched {hunt_mapid}
    // /{mapid} state placeholders are left for resolve_cmd at execution time).
    let steps = expanded_steps(&def);
    state.task_stack.push(TaskRuntime {
        def_id: id.to_string(),
        steps,
        step_idx: 0,
        step_since: Instant::now(),
        group: group_id,
        hunt_before: state.hunt,
        recurring: def.recurring,
    });
    crate::emit_f!([crate::emit::Field::Name => id, crate::emit::Field::MapId => state.hunt_mapid] => "[task] started '{id}' (map snapshot {})", state.hunt_mapid);
    Ok(())
}

/// Expand a task's steps (vars substituted, default timeout applied),
/// shared by `start_task` and rule-driven task inlining.
pub fn expanded_steps(def: &TaskDef) -> Vec<StepDef> {
    def.steps
        .iter()
        .map(|s| {
            let mut step = if s.timeout.is_some() {
                s.clone()
            } else {
                StepDef {
                    timeout: def.timeout,
                    ..s.clone()
                }
            };
            step.cmd = crate::runtime_config::expand_vars(&step.cmd, &def.vars);
            if let Some(w) = step.wait.take() {
                step.wait = Some(crate::runtime_config::expand_vars(&w, &def.vars));
            }
            // Pre-build: static steps (no `{placeholder}`) are parsed once here,
            // at task expansion, so the hot tick loop reuses the cached
            // `Command` instead of re-parsing the string every tick. Steps with
            // placeholders stay unparsed (resolved + parsed per tick in
            // `step_command`, since their values change).
            if !step.cmd.contains('{') {
                step.cmd_parsed = crate::command::parse(&step.cmd).ok().flatten();
            }
            step
        })
        .collect()
}

/// Stop every running task of a feature group (called on group close / exclusive cascade) and release their locks.
/// All tasks of the group are removed from the stack; when the stack empties, restore the hunt snapshot
/// of the earliest-started (bottom) removed task (same semantics as stop_task). Returns the number removed.
pub fn stop_tasks_in_group(state: &mut BotState, group_id: &str) -> usize {
    let mut removed: Vec<TaskRuntime> = Vec::new();
    let mut i = 0;
    while i < state.task_stack.len() {
        if state.task_stack[i].group.as_deref() == Some(group_id) {
            removed.push(state.task_stack.remove(i));
        } else {
            i += 1;
        }
    }
    if !removed.is_empty() {
        for rt in &removed {
            crate::emit_f!([crate::emit::Field::Name => rt.def_id, crate::emit::Field::Text => group_id] => 
                "[task] '{}' stopped (group '{group_id}' closed)",
                rt.def_id);
        }
        // Only restore hunt when the stack is empty (leave it alone while a task holds the lock)
        if state.task_stack.is_empty() {
            if let Some(rt) = removed.first() {
                state.hunt = rt.hunt_before;
            }
        }
    }
    removed.len()
}

/// Stop all tasks: release the lock stack + reset cursors (stack cleared, tasks as if never run),
/// and restore hunt from the earliest task's snapshot (a town step may have turned it off).
pub fn stop_task(state: &mut BotState) {
    if state.task_stack.is_empty() {
        crate::emit!("[task] no active task");
        return;
    }
    let ids: Vec<String> = state.task_stack.iter().map(|rt| rt.def_id.clone()).collect();
    let restore_hunt = state.task_stack.first().map(|rt| rt.hunt_before);
    state.task_stack.clear();
    if let Some(h) = restore_hunt {
        state.hunt = h;
        if h {
            // Restore hunting: mark the hunt clock fresh so post-task mobs
            // aren't treated as stale.
            state.last_kill = std::time::Instant::now();
        }
    }
    crate::emit_f!([crate::emit::Field::Count => ids.len(), crate::emit::Field::Text => ids.join(" <- ")] => "[task] stopped: {} (locks released, cursors reset)", ids.join(" <- "));
}

/// Whether all task steps are done.
pub fn task_finished(rt: &TaskRuntime) -> bool {
    rt.step_idx >= rt.steps.len()
}

/// Whether the current step is still waiting (wait predicate unmet and not timed out).
/// State placeholders in wait ({hunt_mapid}/{mapid}) resolve before execution,
/// matching resolve_cmd for cmd (vars are already expanded at start).
pub fn step_waiting(state: &BotState, step: &StepDef) -> bool {
    match &step.wait {
        Some(w) => {
            // resolve_cmd always succeeds (only replaces placeholders); on failure evaluate the raw string
            let resolved = resolve_cmd(state, w).unwrap_or_else(|_| w.clone());
            !crate::runtime_config::eval_predicate(state, &resolved)
        }
        None => false,
    }
}

/// Whether the current step has timed out waiting.
pub fn step_timed_out(rt: &TaskRuntime, step: &StepDef) -> bool {
    match step.timeout {
        Some(ms) => rt.step_since.elapsed().as_millis() as u64 >= ms,
        None => false,
    }
}

/// Pop the stack-top task. Returns true = stack still non-empty (lock held, main loop yields).
/// When the whole lock stack releases, restore the earliest task's hunt snapshot (a town step
/// turns hunt off; otherwise "back from selling" wouldn't resume hunting). Nested case: popping
/// to a non-empty stack doesn't restore; only the final pop to empty restores the bottom task's hunt value.
fn pop_top(state: &mut BotState) -> bool {
    if let Some(rt) = state.task_stack.pop() {
        crate::emit_f!([crate::emit::Field::Name => rt.def_id] => "[task] '{}' released lock", rt.def_id);
        if state.task_stack.is_empty() {
            state.hunt = rt.hunt_before;
        }
    }
    !state.task_stack.is_empty()
}

/// Drive tasks each tick: returns true while the task lock is held (later behaviors yield).
pub async fn tick_tasks(state: &mut BotState, session: &mut Session) -> Result<bool, String> {
    let Some(rt) = state.task_stack.last().cloned() else {
        return Ok(false);
    };
    // Stack position of the runtime this tick is driving. Pushes append and
    // pops remove from the top, so the runtime stays at this index even when a
    // nested `task start B` pushes above it. Index (not def_id) is used for
    // advancement: a wait-inlined rule can legitimately leave several queued
    // instances with the same def_id, and `find(def_id)` would advance the
    // oldest instead of the one being driven.
    let rt_idx = state.task_stack.len() - 1;
    // Phase gate: don't advance during stages like map transitions.
    if state.phase != Phase::InGame {
        return Ok(true);
    }

    if task_finished(&rt) {
        if rt.recurring {
            // Recurring task: reset the cursor and rerun from the start, keep the lock
            if let Some(a) = state.task_stack.last_mut() {
                a.step_idx = 0;
                a.step_since = Instant::now();
            }
            crate::emit_f!([crate::emit::Field::Name => rt.def_id] => "[task] '{}' complete, looping", rt.def_id);
            return Ok(true);
        }
        crate::emit_f!([crate::emit::Field::Name => rt.def_id] => "[task] '{}' complete", rt.def_id);
        return Ok(pop_top(state));
    }

    let step = rt.steps[rt.step_idx].clone();
    if step_waiting(state, &step) {
        if step_timed_out(&rt, &step) {
            // Wait cap reached: this is a fallback to prevent an infinite stall,
            // NOT a failure. Continue to the next step (same semantics as a
            // `wait` field on a normal action step) instead of abandoning the
            // task — the remaining actions still run. `wait <pred>` with no ms
            // never times out (blocks until satisfied), so this only fires when
            // an explicit `ms` cap was given.
            crate::emit_f!([crate::emit::Field::Name => rt.def_id, crate::emit::Field::Step => rt.step_idx + 1, crate::emit::Field::Text => step.wait.as_deref().unwrap_or("")] =>
                "[task] '{}' step {} wait '{}' timed out — continuing",
                rt.def_id, rt.step_idx + 1, step.wait.as_deref().unwrap_or(""));
            if let Some(a) = state.task_stack.last_mut() {
                a.step_idx += 1;
                a.step_since = Instant::now();
            }
        }
        return Ok(true);
    }

    // Loop step (`while <pred> do <cmd>`): while the predicate holds, run the
    // body once per tick and stay on this step; once it goes false, advance
    // immediately. Any spawn gap between boss phases is handled manually with a
    // `wait <pred>` step (e.g. `wait mobs>0 2000`), NOT a hidden grace window —
    // the user controls the timing. The owning group can stop the task at any
    // time (stop_tasks_in_group), so the loop is always manually killable.
    if let Some(pred) = &step.loop_pred {
        let resolved_pred = resolve_cmd(state, pred).unwrap_or_else(|_| pred.clone());
        if crate::runtime_config::eval_predicate(state, &resolved_pred) {
            let body = match step_command(state, rt_idx) {
                Ok(c) => c,
                Err(e) => {
                    crate::emit_f!([crate::emit::Field::Name => rt.def_id, crate::emit::Field::Step => rt.step_idx + 1, crate::emit::Field::Text => e] =>
                        "[task] '{}' step {} loop body resolve failed: {e} — abandoning", rt.def_id, rt.step_idx + 1);
                    return Ok(pop_top(state));
                }
            };
            if let Err(e) = crate::command::rules::run_action_cmd(state, session, body).await {
                crate::emit_f!([crate::emit::Field::Name => rt.def_id, crate::emit::Field::Step => rt.step_idx + 1, crate::emit::Field::Text => e] =>
                    "[task] '{}' step {} loop error: {e} — abandoning", rt.def_id, rt.step_idx + 1);
                return Ok(pop_top(state));
            }
            // predicate true: hold on this step until it flips false
            return Ok(true);
        }
        // predicate false: advance immediately (no hidden grace window — manual
        // `wait <pred>` steps control any inter-phase gap)
        crate::emit_f!([crate::emit::Field::Name => rt.def_id, crate::emit::Field::Step => rt.step_idx + 1] =>
            "[task] '{}' step {} loop predicate false — advancing", rt.def_id, rt.step_idx + 1);
        if let Some(a) = state.task_stack.get_mut(rt_idx) {
            if a.def_id == rt.def_id {
                a.step_idx += 1;
                a.step_since = Instant::now();
            }
        }
        return Ok(true);
    }

    if step.cmd.trim().is_empty() && step.wait.is_some() {
        // Pure wait gate satisfied — advance the current (rt_idx) runtime.
        crate::emit_f!([crate::emit::Field::Name => rt.def_id, crate::emit::Field::Step => rt.step_idx + 1, crate::emit::Field::Text => step.wait.as_deref().unwrap_or("")] => "[task] '{}' step {} wait satisfied", rt.def_id, rt.step_idx + 1);
        if let Some(a) = state.task_stack.get_mut(rt_idx) {
            if a.def_id == rt.def_id {
                a.step_idx += 1;
                a.step_since = Instant::now();
            }
        }
        return Ok(true);
    }

    let cmd = match step_command(state, rt_idx) {
        Ok(c) => c,
        Err(e) => {
            // Step resolution failed (e.g. shop NPC not on the map) → abandon the task, pop and release the lock.
            crate::emit_f!([crate::emit::Field::Name => rt.def_id, crate::emit::Field::Step => rt.step_idx + 1, crate::emit::Field::Text => e] => "[task] '{}' step {} failed: {e} — abandoning", rt.def_id, rt.step_idx + 1);
            return Ok(pop_top(state));
        }
    };
    crate::emit_f!([crate::emit::Field::Name => rt.def_id, crate::emit::Field::Step => rt.step_idx + 1, crate::emit::Field::Text => &step.cmd] => "[task] '{}' step {}: {}", rt.def_id, rt.step_idx + 1, step.cmd);
    if let Err(e) = crate::command::rules::run_action_cmd(state, session, cmd).await {
        // Step execution failed → abandon the task, pop and release the lock (no retry; main loop continues).
        crate::emit_f!([crate::emit::Field::Name => rt.def_id, crate::emit::Field::Step => rt.step_idx + 1, crate::emit::Field::Text => e] => "[task] '{}' step {} error: {e} — abandoning", rt.def_id, rt.step_idx + 1);
        return Ok(pop_top(state));
    }
    // Advance the runtime this tick drove (rt_idx, guarded by def_id in case
    // the stack was reshuffled). Steps like `task start B` push a new task
    // (nested) — the original stays at rt_idx, leaving the new one alone.
    if let Some(a) = state.task_stack.get_mut(rt_idx) {
        if a.def_id == rt.def_id {
            a.step_idx += 1;
            a.step_since = Instant::now();
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;
    use crate::runtime_config::{RuntimeConfig, TaskDef};
    use crate::state::BotState;

    fn state_with_task() -> BotState {
        let mut s = BotState::default();
        s.cfg = RuntimeConfig {
            tasks: vec![
                TaskDef {
                    id: "demo".into(),
                    priority: 10,
                    timeout: Some(5000),
                    steps: vec![
                        StepDef {
                            cmd: "chat hi".into(),
                            wait: None,
                            timeout: None,
                            loop_pred: None, cmd_parsed: None,
                        },
                        StepDef {
                            cmd: "chat bye".into(),
                            wait: Some("mapid==104040000".into()),
                            timeout: Some(1000),
                            loop_pred: None, cmd_parsed: None,
                        },
                    ],
                    vars: Default::default(),
                    recurring: false,
                },
                TaskDef {
                    id: "after".into(),
                    priority: 5,
                    timeout: None,
                    steps: vec![StepDef {
                        cmd: "chat done".into(),
                        wait: None,
                        timeout: None,
                        loop_pred: None, cmd_parsed: None,
                    }],
                    vars: Default::default(),
                    recurring: false,
                },
            ],
            ..RuntimeConfig::default()
        };
        s
    }

    #[test]
    fn start_unknown_task_fails() {
        let mut s = BotState::default();
        assert!(start_task(&mut s, "nope").is_err());
        assert!(s.task_stack.is_empty());
    }

    #[test]
    fn start_task_respects_group_gate() {
        let mut s = BotState::default();
        s.cfg.groups.push(crate::runtime_config::GroupDef {
            id: "autosell".into(),
            enabled: false,
            exclusive: false,
            rules: vec![],
            tasks: vec![TaskDef {
                id: "open_shop".into(),
                priority: 10,
                timeout: None,
                steps: vec![StepDef {
                    cmd: "chat hi".into(),
                    wait: None,
                    timeout: None,
                    loop_pred: None, cmd_parsed: None,
                }],
                vars: Default::default(),
            recurring: false,
            }],
        });
        // Group closed → reject the start with a hint to open it
        let err = start_task(&mut s, "open_shop").unwrap_err();
        assert!(err.contains("group open autosell"), "err={err}");
        assert!(s.task_stack.is_empty());
        // Group open → starts normally
        s.cfg.group_mut("autosell").unwrap().enabled = true;
        start_task(&mut s, "open_shop").unwrap();
        assert_eq!(s.task_stack.len(), 1);
        assert_eq!(s.task_stack.last().unwrap().def_id, "open_shop");
    }

    #[test]
    fn task_vars_expand_into_cmd_and_wait_at_start() {
        use std::collections::BTreeMap;
        let mut s = BotState::default();
        let mut vars = BTreeMap::new();
        vars.insert("box".to_string(), "2022552".to_string());
        vars.insert("s1".to_string(), "5".to_string());
        s.cfg.tasks.push(TaskDef {
            id: "open_shop".into(),
            priority: 10,
            timeout: None,
            steps: vec![
                StepDef {
                    cmd: "reward {box}".into(),
                    wait: None,
                    timeout: None,
                    loop_pred: None, cmd_parsed: None,
                },
                StepDef {
                    cmd: "reply {s1}".into(),
                    wait: Some("dialog==1".into()),
                    timeout: None,
                    loop_pred: None, cmd_parsed: None,
                },
                // Unmatched placeholders are left for the state layer ({hunt_mapid})
                StepDef {
                    cmd: "town {hunt_mapid} west00".into(),
                    wait: Some("mapid=={hunt_mapid}".into()),
                    timeout: None,
                    loop_pred: None, cmd_parsed: None,
                },
            ],
            vars,
            recurring: false,
        });
        start_task(&mut s, "open_shop").unwrap();
        let rt = s.task_stack.last().unwrap();
        assert_eq!(rt.steps[0].cmd, "reward 2022552");
        assert_eq!(rt.steps[1].cmd, "reply 5");
        assert_eq!(rt.steps[1].wait.as_deref(), Some("dialog==1"));
        // State placeholders stay untouched; resolve_cmd replaces them at execution
        assert_eq!(rt.steps[2].cmd, "town {hunt_mapid} west00");
        assert_eq!(rt.steps[2].wait.as_deref(), Some("mapid=={hunt_mapid}"));
    }

    #[test]
    fn task_pop_and_stop_restore_hunt_snapshot() {
        // hunt=on when the task starts; a step (e.g. town) turns hunt off →
        // both pop on completion and stop must restore it, so the bot keeps hunting after selling.
        let mut s = state_with_task();
        s.hunt = true;
        start_task(&mut s, "demo").unwrap();
        assert!(s.task_stack.last().unwrap().hunt_before);
        s.hunt = false; // simulate a town step turning hunt off
        let still_held = pop_top(&mut s);
        assert!(!still_held);
        assert!(s.task_stack.is_empty());
        assert!(s.hunt, "hunt must be restored to its pre-task state after pop");

        let mut s2 = state_with_task();
        s2.hunt = true;
        start_task(&mut s2, "demo").unwrap();
        s2.hunt = false;
        stop_task(&mut s2);
        assert!(s2.task_stack.is_empty());
        assert!(s2.hunt, "hunt must be restored to its pre-task state after stop");

        // hunt=off before the task → restored as off (no accidental enabling)
        let mut s3 = state_with_task();
        s3.hunt = false;
        start_task(&mut s3, "demo").unwrap();
        s3.hunt = false;
        stop_task(&mut s3);
        assert!(!s3.hunt);
    }

    #[test]
    fn start_and_finish_detection() {
        let mut s = state_with_task();
        start_task(&mut s, "demo").unwrap();
        let rt = s.task_stack.last().cloned().unwrap();
        assert_eq!(rt.step_idx, 0);
        assert_eq!(rt.steps.len(), 2);
        assert!(!task_finished(&rt));
        // All steps done → finished
        let rt2 = TaskRuntime {
            def_id: "demo".into(),
            steps: vec![StepDef {
                cmd: "x".into(),
                wait: None,
                timeout: None,
                loop_pred: None, cmd_parsed: None,
            }],
            step_idx: 1,
            step_since: Instant::now(),
            group: None,
            hunt_before: false,
            recurring: false,
        };
        assert!(task_finished(&rt2));
    }

    #[test]
    fn duplicate_start_is_noop() {
        let mut s = state_with_task();
        start_task(&mut s, "demo").unwrap();
        assert!(start_task(&mut s, "demo").is_ok());
        assert_eq!(s.task_stack.len(), 1);
        assert_eq!(s.task_stack.last().unwrap().step_idx, 0);
    }

    #[test]
    fn nested_tasks_lifo_order() {
        let mut s = state_with_task();
        // A → B → C pushed: LIFO, so C releases first
        start_task(&mut s, "demo").unwrap();
        start_task(&mut s, "after").unwrap();
        start_task(&mut s, "demo").unwrap(); // already on the stack → no-op
        assert_eq!(s.task_stack.len(), 2);
        assert_eq!(s.task_stack.last().unwrap().def_id, "after");
        // Pops release in C→B→A order
        assert!(pop_top(&mut s));
        assert_eq!(s.task_stack.last().unwrap().def_id, "demo");
        assert!(!pop_top(&mut s));
        assert!(s.task_stack.is_empty());
    }

    #[test]
    fn stop_clears_whole_stack() {
        let mut s = state_with_task();
        start_task(&mut s, "demo").unwrap();
        start_task(&mut s, "after").unwrap();
        stop_task(&mut s);
        assert!(s.task_stack.is_empty());
    }

    #[test]
    fn step_wait_resolves_state_placeholders() {
        // {hunt_mapid}/{mapid} in wait resolve at execution (vars expanded at start)
        let mut s = BotState::default();
        s.hunt_mapid = 104040000;
        s.mapid = 104040000;
        let step = StepDef {
            cmd: "chat x".into(),
            wait: Some("mapid=={hunt_mapid}".into()),
            timeout: None,
            loop_pred: None, cmd_parsed: None,
        };
        // After resolution mapid==104040000 is true → no waiting
        assert!(!step_waiting(&s, &step));
        s.mapid = 100000000;
        assert!(step_waiting(&s, &step));
    }

    #[test]
    fn step_wait_gating_and_timeout() {
        let mut s = state_with_task();
        start_task(&mut s, "demo").unwrap();
        let rt = s.task_stack.last().cloned().unwrap();
        // Step 2 waits for mapid==104040000
        let step = &rt.steps[1];
        assert!(step_waiting(&s, step));
        assert!(!step_timed_out(&rt, step)); // just started, not timed out
        s.mapid = 104040000;
        assert!(!step_waiting(&s, step));
        // Timeout: mark as timed out after waiting long enough
        let rt2 = TaskRuntime {
            def_id: "demo".into(),
            steps: rt.steps.clone(),
            step_idx: 1,
            step_since: Instant::now() - std::time::Duration::from_secs(5),
            group: None,
            hunt_before: false,
            recurring: false,
        };
        assert!(step_timed_out(&rt2, step));
    }

    #[test]
    fn sell_task_steps_are_static() {
        // Sample sell task structure (verified inline, independent of config.json):
        // full step list, no {sell_route} placeholder, last step returns to the hunt map.
        let mut s = BotState::default();
        s.cfg.tasks.push(TaskDef {
            id: "sell".into(),
            priority: 10,
            timeout: Some(120_000),
            vars: Default::default(),
            recurring: false,
            steps: vec![
                StepDef { cmd: "town 100000000 east00".into(), wait: None, timeout: None, loop_pred: None, cmd_parsed: None },
                StepDef { cmd: "town 100000100 in00".into(), wait: None, timeout: None, loop_pred: None, cmd_parsed: None },
                StepDef { cmd: "town 100000102 in01".into(), wait: None, timeout: None, loop_pred: None, cmd_parsed: None },
                StepDef { cmd: "npc 1011100".into(), wait: Some("npc==1011100".into()), timeout: None, loop_pred: None, cmd_parsed: None },
                StepDef { cmd: "sell tab equip".into(), wait: None, timeout: None, loop_pred: None, cmd_parsed: None },
                StepDef { cmd: "shop leave".into(), wait: None, timeout: None, loop_pred: None, cmd_parsed: None },
                StepDef { cmd: "town 100000100 out01".into(), wait: None, timeout: None, loop_pred: None, cmd_parsed: None },
                StepDef { cmd: "town 100000000 out00".into(), wait: None, timeout: None, loop_pred: None, cmd_parsed: None },
                StepDef { cmd: "town 104040000 west00".into(), wait: None, timeout: None, loop_pred: None, cmd_parsed: None },
            ],
        });
        s.hunt_mapid = 104040000;
        start_task(&mut s, "sell").unwrap();
        let rt = s.task_stack.last().unwrap();
        assert!(rt.steps.len() >= 9);
        assert!(rt.steps.iter().all(|s| !s.cmd.contains("{sell_route}")));
        assert_eq!(rt.steps.last().unwrap().cmd, "town 104040000 west00");
    }

    #[test]
    fn task_level_timeout_applies_to_unset_steps() {
        let mut s = state_with_task();
        start_task(&mut s, "demo").unwrap();
        let rt = s.task_stack.last().unwrap();
        // Step 1 has no own timeout → inherits the task-level 5000
        assert_eq!(rt.steps[0].timeout, Some(5000));
        // Step 2 has its own 1000 → not overridden by the task level
        assert_eq!(rt.steps[1].timeout, Some(1000));
        // A task with None timeout → steps stay timeout-free
        start_task(&mut s, "after").unwrap();
        let rt = s.task_stack.last().unwrap();
        assert_eq!(rt.steps[0].timeout, None);
    }

    #[test]
    fn resolve_cmd_placeholder_substitution() {
        let mut s = BotState::default();
        s.hunt_mapid = 104040000;
        s.mapid = 100000102;
        assert_eq!(resolve_cmd(&s, "town {hunt_mapid} sp").unwrap(), "town 104040000 sp");
        assert_eq!(resolve_cmd(&s, "map {mapid}").unwrap(), "map 100000102");
        // A fixed npcid in npc is unaffected by placeholders
        assert_eq!(resolve_cmd(&s, "npc 1011100").unwrap(), "npc 1011100");
    }

    #[test]
    fn expanded_steps_preserves_loop_pred() {
        let def = TaskDef {
            id: "loop".into(),
            priority: 1,
            timeout: Some(1000),
            vars: Default::default(),
            recurring: false,
            steps: vec![
                StepDef { cmd: "hunt once".into(), wait: None, timeout: None, loop_pred: Some("mobs>0".into()), cmd_parsed: None },
                StepDef { cmd: "setvar done 1".into(), wait: None, timeout: None, loop_pred: None, cmd_parsed: None },
            ],
        };
        let steps = expanded_steps(&def);
        assert_eq!(steps[0].loop_pred.as_deref(), Some("mobs>0"));
        assert_eq!(steps[0].timeout, Some(1000)); // 继承任务默认超时
        assert_eq!(steps[1].loop_pred, None);
    }

    #[test]
    fn while_step_is_not_a_wait_gate() {
        // loop 步没有 wait → step_waiting 必须返回 false, 否则会被当成阻塞 gate 而卡住
        let step = StepDef { cmd: "hunt once".into(), wait: None, timeout: None, loop_pred: Some("mobs>0".into()), cmd_parsed: None };
        assert!(!step_waiting(&BotState::default(), &step));
    }

    #[test]
    fn stop_tasks_in_group_removes_only_matching_group() {
        let mut s = BotState::default();
        s.hunt = true;
        // 两个属于 group_a, 一个无 group(顶层规则内联)
        s.task_stack.push(TaskRuntime { def_id: "ga1".into(), steps: vec![], step_idx: 0, step_since: Instant::now(), group: Some("group_a".into()), hunt_before: false, recurring: false });
        s.task_stack.push(TaskRuntime { def_id: "ga2".into(), steps: vec![], step_idx: 0, step_since: Instant::now(), group: Some("group_a".into()), hunt_before: false, recurring: false });
        s.task_stack.push(TaskRuntime { def_id: "top".into(), steps: vec![], step_idx: 0, step_since: Instant::now(), group: None, hunt_before: false, recurring: false });
        let removed = stop_tasks_in_group(&mut s, "group_a");
        assert_eq!(removed, 2);
        assert_eq!(s.task_stack.len(), 1);
        assert_eq!(s.task_stack[0].def_id, "top");
        // 栈未空 → 不动 hunt 快照
        assert!(s.hunt);
    }

    #[test]
    fn stop_tasks_in_group_restores_hunt_when_stack_empty() {
        let mut s = BotState::default();
        s.hunt = true;
        s.task_stack.push(TaskRuntime { def_id: "ga1".into(), steps: vec![], step_idx: 0, step_since: Instant::now(), group: Some("group_a".into()), hunt_before: false, recurring: false });
        let removed = stop_tasks_in_group(&mut s, "group_a");
        assert_eq!(removed, 1);
        assert!(s.task_stack.is_empty());
        // 栈空 → 恢复最早被移除任务的 hunt 快照(false)
        assert!(!s.hunt);
    }

    #[test]
    fn stop_tasks_in_group_unknown_group_removes_nothing() {
        let mut s = BotState::default();
        s.task_stack.push(TaskRuntime { def_id: "top".into(), steps: vec![], step_idx: 0, step_since: Instant::now(), group: None, hunt_before: false, recurring: false });
        assert_eq!(stop_tasks_in_group(&mut s, "nope"), 0);
        assert_eq!(s.task_stack.len(), 1);
    }

    // ---- pre-build (cmd_parsed cache) tests ---------------------------------

    /// Build a BotState with a single-step task on the stack and return it.
    fn state_with_single_step(cmd: &str) -> BotState {
        let mut s = BotState::default();
        s.cfg.tasks.push(TaskDef {
            id: "t".into(),
            priority: 1,
            timeout: None,
            steps: vec![StepDef {
                cmd: cmd.into(),
                wait: None,
                timeout: None,
                loop_pred: None,
                cmd_parsed: None,
            }],
            vars: Default::default(),
            recurring: false,
        });
        start_task(&mut s, "t").unwrap();
        s
    }

    #[test]
    fn expanded_steps_prebuilds_static_command() {
        // Build-time pre-build: static steps are parsed once at task expansion;
        // dynamic steps (with a `{placeholder}`) are intentionally left unparsed.
        let def = TaskDef {
            id: "t".into(),
            priority: 1,
            timeout: None,
            steps: vec![
                StepDef { cmd: "hunt once".into(), wait: None, timeout: None, loop_pred: None, cmd_parsed: None },
                StepDef { cmd: "chat {hunt_mapid}".into(), wait: None, timeout: None, loop_pred: None, cmd_parsed: None },
            ],
            vars: Default::default(),
            recurring: false,
        };
        let steps = expanded_steps(&def);
        assert_eq!(steps[0].cmd_parsed, Some(Command::HuntOnce));
        // dynamic step is NOT pre-built (resolved + parsed per tick instead)
        assert!(steps[1].cmd_parsed.is_none());
    }

    #[test]
    fn step_command_static_cache_miss_then_reuses() {
        let mut s = state_with_single_step("hunt once");
        let rt_idx = s.task_stack.len() - 1;
        // expanded_steps already pre-built the cache at start_task time
        assert!(s.task_stack[rt_idx].steps[0].cmd_parsed.is_some());
        // simulate a cache miss (e.g. a step built outside expanded_steps)
        s.task_stack[rt_idx].steps[0].cmd_parsed = None;
        let c1 = step_command(&mut s, rt_idx).unwrap();
        assert_eq!(c1, Command::HuntOnce);
        assert!(s.task_stack[rt_idx].steps[0].cmd_parsed.is_some());
        // cache hit: even if the raw cmd string is later mutated, the cached
        // Command is returned (no re-parse) — proves the cache is actually used.
        s.task_stack[rt_idx].steps[0].cmd = "chat bye".into();
        let c2 = step_command(&mut s, rt_idx).unwrap();
        assert_eq!(c2, Command::HuntOnce);
    }

    #[test]
    fn step_command_dynamic_not_cached() {
        let mut s = state_with_single_step("chat {hunt_mapid}");
        let rt_idx = s.task_stack.len() - 1;
        s.hunt_mapid = 555;
        let c = step_command(&mut s, rt_idx).unwrap();
        assert_eq!(c, Command::Chat("555".into()));
        // dynamic steps are never cached (their resolved value changes per tick)
        assert!(s.task_stack[rt_idx].steps[0].cmd_parsed.is_none());
    }

    #[test]
    fn step_command_empty_returns_err() {
        let mut s = state_with_single_step("");
        let rt_idx = s.task_stack.len() - 1;
        assert!(step_command(&mut s, rt_idx).is_err());
    }

    #[test]
    fn step_command_parse_error_returns_err() {
        let mut s = state_with_single_step("zzznotacommand");
        let rt_idx = s.task_stack.len() - 1;
        assert!(step_command(&mut s, rt_idx).is_err());
    }

    #[test]
    fn reload_keeps_static_prebuilt_cache() {
        // reload swaps the whole config object; the running runtime's cached
        // Command (parsed data, independent of cfg) must survive unchanged.
        let mut s = state_with_single_step("hunt once");
        let rt_idx = s.task_stack.len() - 1;
        let _ = step_command(&mut s, rt_idx).unwrap();
        assert!(s.task_stack[rt_idx].steps[0].cmd_parsed.is_some());
        let reloaded = s.cfg.clone();
        s.cfg = reloaded;
        let c = step_command(&mut s, rt_idx).unwrap();
        assert_eq!(c, Command::HuntOnce);
        assert!(s.task_stack[rt_idx].steps[0].cmd_parsed.is_some());
    }

    #[test]
    fn reload_dynamic_step_reresolves() {
        // dynamic steps carry no cache, so after a reload (new hunt_mapid) they
        // re-resolve against the fresh config instead of returning stale data.
        let mut s = state_with_single_step("chat {hunt_mapid}");
        let rt_idx = s.task_stack.len() - 1;
        s.hunt_mapid = 555;
        assert_eq!(step_command(&mut s, rt_idx).unwrap(), Command::Chat("555".into()));
        s.hunt_mapid = 999;
        assert_eq!(step_command(&mut s, rt_idx).unwrap(), Command::Chat("999".into()));
        assert!(s.task_stack[rt_idx].steps[0].cmd_parsed.is_none());
    }
}

/// Regression coverage for the `wait` / `while` engine semantics:
/// - `wait <pred> ms` times out → CONTINUE (advance), never abandons the task
/// - `wait <pred>` (no ms) → blocks until satisfied, but is interruptible
/// - `while <pred> do <cmd>` advances immediately when pred goes false (no grace)
/// - every branch is interruptible at any time via `stop_task` / `stop_tasks_in_group`
#[cfg(test)]
mod wait_while_regression {
    use super::*;
    use crate::runtime_config::{RuntimeConfig, StepDef, TaskDef};
    use crate::state::{BotState, Drop, Entity, EntityKind, Phase};
    use crate::session::Session;
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A connected `Session` over a local socket (handshake only) so `tick_tasks`
    /// can drive action commands without a real game server.
    async fn dummy_session() -> Session {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream.write_all(&[0u8; 16]).await.ok();
            let mut buf = [0u8; 1024];
            loop {
                match stream.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    _ => {}
                }
            }
        });
        Session::connect_ex(&addr, 75, false, false).await.unwrap()
    }

    fn state_with_task(steps: Vec<StepDef>, group: Option<String>) -> BotState {
        let mut s = BotState::default();
        s.phase = Phase::InGame;
        s.cfg = RuntimeConfig {
            tasks: vec![TaskDef {
                id: "reg".into(),
                priority: 1,
                timeout: None,
                steps,
                vars: Default::default(),
                recurring: false,
            }],
            ..RuntimeConfig::default()
        };
        start_task(&mut s, "reg").unwrap();
        if let Some(g) = group {
            if let Some(rt) = s.task_stack.last_mut() {
                rt.group = Some(g);
            }
        }
        s
    }

    fn wait_step(pred: &str, ms: Option<u64>) -> StepDef {
        StepDef { cmd: String::new(), wait: Some(pred.into()), timeout: ms, loop_pred: None, cmd_parsed: None }
    }
    fn while_step(pred: &str, cmd: &str) -> StepDef {
        StepDef { cmd: cmd.into(), wait: None, timeout: None, loop_pred: Some(pred.into()), cmd_parsed: None }
    }
    fn cmd_step(cmd: &str) -> StepDef {
        StepDef { cmd: cmd.into(), wait: None, timeout: None, loop_pred: None, cmd_parsed: None }
    }
    fn add_mob(s: &mut BotState, oid: i32) {
        s.entities.insert(oid, Entity { oid, kind: EntityKind::Mob, ..Default::default() });
    }
    fn add_drop(s: &mut BotState, oid: i32) {
        s.drops.insert(oid, Drop::default());
    }

    // `wait <pred> ms` times out → advance to the next step (CONTINUE), not abandon.
    #[tokio::test]
    async fn wait_with_ms_times_out_and_continues() {
        let mut s = state_with_task(vec![wait_step("drops>0", Some(500)), cmd_step("setvar done 1")], None);
        let mut session = dummy_session().await;
        s.task_stack.last_mut().unwrap().step_since = Instant::now() - Duration::from_secs(1);
        let held = tick_tasks(&mut s, &mut session).await.unwrap();
        assert!(held);
        assert_eq!(s.task_stack.len(), 1, "wait timeout must NOT abandon the task");
        assert_eq!(s.task_stack[0].step_idx, 1, "wait timeout must continue to the next step");
    }

    // `wait <pred>` (no ms) blocks forever until satisfied, but is interruptible.
    #[tokio::test]
    async fn wait_without_ms_blocks_but_is_interruptible() {
        let mut s = state_with_task(vec![wait_step("drops>0", None), cmd_step("setvar done 1")], None);
        let mut session = dummy_session().await;
        tick_tasks(&mut s, &mut session).await.unwrap();
        assert_eq!(s.task_stack[0].step_idx, 0, "no-ms wait must block (no timeout)");
        assert_eq!(s.task_stack.len(), 1);
        stop_task(&mut s); // interrupt an infinite wait → no permanent stall
        assert!(s.task_stack.is_empty(), "infinite wait must be interruptible via stop_task");
    }

    // group stop also interrupts an infinite wait.
    #[tokio::test]
    async fn wait_without_ms_interruptible_via_group_stop() {
        let mut s = state_with_task(vec![wait_step("drops>0", None)], Some("g".into()));
        let mut session = dummy_session().await;
        tick_tasks(&mut s, &mut session).await.unwrap();
        assert_eq!(s.task_stack.len(), 1);
        stop_tasks_in_group(&mut s, "g");
        assert!(s.task_stack.is_empty(), "group stop must interrupt the wait");
    }

    // `wait <pred> ms` satisfied immediately → advances with no stall.
    #[tokio::test]
    async fn wait_satisfied_advances_immediately() {
        let mut s = state_with_task(vec![wait_step("drops>0", Some(500)), cmd_step("setvar done 1")], None);
        add_drop(&mut s, 1);
        let mut session = dummy_session().await;
        tick_tasks(&mut s, &mut session).await.unwrap();
        assert_eq!(s.task_stack[0].step_idx, 1, "satisfied wait must advance at once");
    }

    // `while <pred> do <cmd>`: runs body while true, advances immediately when false (no grace).
    #[tokio::test]
    async fn while_advances_immediately_when_pred_false() {
        let mut s = state_with_task(vec![while_step("mobs>0", "setvar x 1"), cmd_step("setvar done 1")], None);
        add_mob(&mut s, 10);
        let mut session = dummy_session().await;
        tick_tasks(&mut s, &mut session).await.unwrap();
        assert_eq!(s.task_stack[0].step_idx, 0, "while must stay while pred true");
        assert_eq!(s.vars.get("x").map(String::as_str), Some("1"), "while body must run");
        s.entities.clear(); // pred flips false
        tick_tasks(&mut s, &mut session).await.unwrap();
        assert_eq!(s.task_stack[0].step_idx, 1, "while must advance immediately when pred false (no grace)");
    }

    // `while` loop is interruptible at any time.
    #[tokio::test]
    async fn while_loop_is_interruptible() {
        let mut s = state_with_task(vec![while_step("mobs>0", "setvar x 1")], Some("g".into()));
        add_mob(&mut s, 10);
        let mut session = dummy_session().await;
        tick_tasks(&mut s, &mut session).await.unwrap();
        assert_eq!(s.task_stack.len(), 1);
        stop_tasks_in_group(&mut s, "g");
        assert!(s.task_stack.is_empty(), "while loop must be interruptible via group stop");
    }

    // Manual inter-phase gap: `while` + `wait` sequence works without hidden stalls.
    #[tokio::test]
    async fn while_then_wait_sequence() {
        let mut s = state_with_task(vec![
            while_step("mobs>0", "setvar phase1 1"),
            wait_step("mobs>0", Some(2000)),
            while_step("mobs>0", "setvar phase2 1"),
        ], None);
        add_mob(&mut s, 10);
        let mut session = dummy_session().await;
        tick_tasks(&mut s, &mut session).await.unwrap();
        assert_eq!(s.task_stack[0].step_idx, 0, "first while runs while mob present");
        s.entities.clear(); // phase 1 cleared
        tick_tasks(&mut s, &mut session).await.unwrap();
        assert_eq!(s.task_stack[0].step_idx, 1, "after while, advance to the wait gap step");
        add_mob(&mut s, 20); // next wave spawns → wait gap satisfied
        tick_tasks(&mut s, &mut session).await.unwrap();
        assert_eq!(s.task_stack[0].step_idx, 2, "wait gap satisfied → advance to phase 2 while");
    }
}






