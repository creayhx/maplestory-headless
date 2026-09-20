//! Runtime configuration (config.json loading + in-memory state) and predicate DSL evaluation.
//!
//! All configurable behavior lives here: potion rules, generic rules
//! (when predicate → action), and linear tasks (command sequences + cursor
//! advancement). Loaded from `config.json` at startup and deep-merged with
//! built-in defaults — the file may override only the fields it cares about.

use std::time::Instant;

use serde::{Deserialize, Serialize};

pub const CONFIG_PATH: &str = "config.json";

/// Process-level active config file chain. Empty = [`CONFIG_PATH`] (legacy
/// single-file behavior). Set once at frontend startup (`--config <path>`,
/// repeatable) or by the TUI profile picker. The LAST path is the write-back
/// target (`hunt save`, wizard credential save, defaults bootstrap).
static ACTIVE_PATHS: std::sync::RwLock<Vec<std::path::PathBuf>> =
    std::sync::RwLock::new(Vec::new());

/// Set the active config chain (later files override earlier ones on merge).
pub fn set_config_paths(paths: Vec<std::path::PathBuf>) {
    *ACTIVE_PATHS.write().expect("config path lock") = paths;
}

/// Active chain: CLI-provided files or the legacy default `[CONFIG_PATH]`.
pub fn config_paths() -> Vec<std::path::PathBuf> {
    let guard = ACTIVE_PATHS.read().expect("config path lock");
    if guard.is_empty() {
        vec![std::path::PathBuf::from(CONFIG_PATH)]
    } else {
        guard.clone()
    }
}

/// Write-back target: the last file of the active chain.
pub fn write_path() -> std::path::PathBuf {
    let guard = ACTIVE_PATHS.read().expect("config path lock");
    guard
        .last()
        .cloned()
        .unwrap_or_else(|| std::path::PathBuf::from(CONFIG_PATH))
}

/// The chain used when a session carries no explicit `--config` paths: the
/// legacy single file `./config.json`.
///
/// Exists so session-scoped code can fall back identically to [`config_paths`]
/// without reading the global `ACTIVE_PATHS` (which another session may have
/// pointed at its own profile).
pub fn legacy_config_paths() -> Vec<std::path::PathBuf> {
    vec![std::path::PathBuf::from(CONFIG_PATH)]
}

/// Peek at a profile's `data_dir` without building a whole [`RuntimeConfig`]
/// and without any of the merge/emit/bootstrap side effects of
/// [`RuntimeConfig::load_from`].
///
/// Frontends call this **before** starting a bot session so their own
/// name-dependent rendering (sidebar, Chinese log rendering) uses the same
/// dictionary the session will use. Returns an empty string when the file is
/// missing, unreadable, malformed, or has no `data_dir` — callers pass that to
/// [`crate::names::NameTable::for_dir`], which maps empty to the default
/// `data/`.
pub fn peek_data_dir(path: &std::path::Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| {
            v.get("data_dir")
                .and_then(|d| d.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default()
}

/// Potion rule: when stat (hp/mp) drops below threshold_pct (percent), use potions in itemids priority order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PotionRule {
    /// "hp" or "mp"
    pub stat: String,
    pub threshold_pct: i32,
    /// Potion itemids in priority order (the first one in stock is used).
    /// May be omitted in the config template (empty); fill the potion list at use time.
    #[serde(default)]
    pub itemids: Vec<i32>,
}

/// Generic rule: when the `when` predicate matches and the cooldown has
/// elapsed, run `then` in order. Actions are command strings (reusing the
/// stdin/chat command pipeline). `enabled` (config-writable, off by default)
/// is checked by the rule engine each tick; toggle at runtime with
/// `rule open <id>` / `rule close <id>`, reverting to the file value on `reload`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    /// Absent key = enabled (legacy group rules like bf_money omit it and
    /// must keep firing when their group is open).
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub when: String,
    pub then: Vec<String>,
    /// Cooldown: minimum interval (ms) between triggers after a predicate match.
    #[serde(default)]
    pub cooldown: u64,
}

/// Task step: wait for the `wait` predicate (default = immediately), then run
/// `cmd`; if the wait exceeds `timeout`, skip this step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepDef {
    pub cmd: String,
    #[serde(default)]
    pub wait: Option<String>,
    /// Wait timeout (ms): skip this step if the wait predicate times out.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Loop predicate for a `while <pred> do <cmd>` step: while the predicate
    /// holds, run `cmd` each tick and stay on this step; once the predicate goes
    /// false, advance immediately. Any inter-phase spawn gap is handled manually
    /// with a `wait <pred>` step — there is no hidden grace window. None = normal
    /// (one-shot) step. The owning group can stop the task at any time, so the
    /// loop is always manually killable.
    #[serde(default)]
    pub loop_pred: Option<String>,
    /// Pre-parsed command for steps whose `cmd` contains no `{placeholder}`.
    /// Built lazily on the first tick (see `command::tasks::step_command`) and
    /// cached here so the tick loop doesn't re-parse the same static command
    /// string every tick. Steps with placeholders are always re-resolved +
    /// re-parsed per tick (their values change). Not serialized.
    #[serde(skip)]
    pub cmd_parsed: Option<crate::command::Command>,
}

/// Task definition: a linear command sequence, advanced one step per tick.
/// On completion the lock is released (unlocked behaviors like hunt resume
/// naturally); there is no resume chain.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TaskDef {
    pub id: String,
    #[serde(default)]
    pub priority: i32,
    /// Defaulted so a bare override entry (`{"id": "t1", "priority": 5}`)
    /// or a minimal definition stays parseable under by-id merging.
    #[serde(default)]
    pub steps: Vec<StepDef>,
    /// Task-level default step timeout (ms): applied when a step has no own
    /// `timeout`; None = no step timeout.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Task-level variable table (key → value): at start, `{key}` in a step's
    /// `cmd`/`wait` is replaced with the value. Enables reusing generic flows
    /// like "open shop" — one step template, groups only change vars
    /// (box id / option id / NPC differences parameterized). Substitution
    /// order: vars before state placeholders ({hunt_mapid}/{mapid}).
    #[serde(default)]
    pub vars: std::collections::BTreeMap<String, String>,
    /// Looping task: restart from the beginning automatically when done
    /// (cursor reset, lock kept). Default false = run once and stop
    /// (rule-driven repeats are triggered by rule conditions + cooldown).
    #[serde(default, rename = "loop")]
    pub recurring: bool,
}

/// Replace `{key}` placeholders with vars (unmatched keys stay as-is, left for state placeholders).
pub fn expand_vars(cmd: &str, vars: &std::collections::BTreeMap<String, String>) -> String {
    let mut out = cmd.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// Running task state (pushed onto BotState's task lock stack). Steps are
/// expanded and frozen at start, unaffected by later config changes.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskRuntime {
    pub def_id: String,
    pub steps: Vec<StepDef>,
    pub step_idx: usize,
    pub step_since: Instant,
    /// Owning group id (None for top-level tasks). Used to clean up running
    /// tasks when a group closes / mutual exclusion cascades.
    pub group: Option<String>,
    /// Whether hunt was on when the task started (snapshot). Steps like town
    /// may turn hunt off; when the whole lock stack releases, restore from the
    /// oldest task's snapshot so the bot returns to hunting after selling.
    pub hunt_before: bool,
    /// Looping-task flag (TaskDef.recurring snapshot): auto-restart when done.
    pub recurring: bool,
}

/// Hunt behavior parameters (config-driven; replaces the switches formerly scattered in state).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct HuntConfig {
    /// Pickup radius (px): 0 = pick up across the whole map. Default 400.
    #[serde(default = "default_pickup_range")]
    pub pickup_range: i32,
    /// Whether to pick up drops (false = ignore; for pure mob-farming speed tests / fewer packets).
    #[serde(default = "default_pickup_enabled")]
    pub pickup_enabled: bool,
    /// Pickup filter mode: "off" (no filtering, default) | "allow" (pick ONLY
    /// `pickup_allow`) | "deny" (pick everything EXCEPT `pickup_deny`).
    /// Meso always bypasses. `hunt filter` switches the mode at runtime.
    #[serde(default)]
    pub pickup_filter_mode: String,
    /// Pickup whitelist: used when `pickup_filter_mode == "allow"`.
    #[serde(default)]
    pub pickup_allow: Vec<i32>,
    /// Pickup blacklist: used when `pickup_filter_mode == "deny"`.
    #[serde(default)]
    pub pickup_deny: Vec<i32>,
    /// Attack skill id: 0 = no-skill multi-target (skill=0, one packet, multiple
    /// targets; the server treats it as normal attack per target, ignoring skill
    /// attributes: no MP cost/cooldown, arbitrary hit count); >0 = skill id.
    /// All skill parameters come from user config (id/range/count); the program
    /// hardcodes nothing.
    #[serde(default)]
    pub attack_skill: i32,
    /// Skill attack range (px): max distance from the character at which a
    /// skill can hit mobs; used when `hunt skill` has no range. 0 = not set
    /// (falls back to the 70px normal-attack range).
    #[serde(default)]
    pub attack_skill_range: i32,
    /// Max targets one skill attack can hit: 0 = not set (falls back to 1, i.e. single target).
    #[serde(default)]
    pub attack_skill_max_targets: usize,
    /// Skill MP cost (checked before casting; if insufficient, fall back to
    /// normal attack); 0 = don't check.
    #[serde(default)]
    pub attack_skill_mp_cost: i16,
    /// Hit count per target per skill attack (attackCount): e.g. a 2-hit AoE
    /// skill sends 2 damage values per target (total damage split evenly,
    /// each hit under the per-hit validation cap). Default 1, max 15
    /// (low 4 bits of tbyte).
    #[serde(default = "default_skill_hits")]
    pub attack_skill_hits: u8,
    /// Predicate: stop hunting automatically when satisfied (e.g. "equips>=24").
    /// None = never auto-stop
    #[serde(default)]
    pub until: Option<String>,
    /// Teleport interval in teleport movement mode (ms; formerly `tp delay`).
    /// Default 0 — teleport movement is never throttled by this (only
    /// `attack_cooldown` gates `hunt once`; the main loop's chase teleport is
    /// also unthrottled). Set >0 only for rate-sensitive servers.
    #[serde(default)]
    pub teleport_delay: u64,
    /// Attack cooldown (ms): minimum gap between two attack packets. Default
    /// 700 (can be lowered to 100 for faster normal attacks on lenient
    /// servers; too low may trigger the whole-map attack warning).
    #[serde(default = "default_attack_cooldown")]
    pub attack_cooldown: u64,
    /// Only attack mobs the client controls (0xF0 aggro=1 controller mobs).
    /// Default true — attacking without control is flagged as abnormal by
    /// some servers. Set false to attack all mobs when control is split among
    /// players on the same map or not sent by the server.
    #[serde(default = "default_true")]
    pub attack_controller_only: bool,
    /// Attack range (px): only send attack packets for mobs within this
    /// distance; teleport closer for farther mobs. Default 70 — attacking
    /// from 300px triggers the server's "abnormal behavior detected" in practice.
    #[serde(default = "default_attack_range")]
    pub attack_range: i32,
    /// Max attack packets per mob for normal attack / max targets per skill
    /// packet (how many mobs to hit at once in range). Normal attack: one
    /// packet per mob in range, up to n. Skill: one packet with up to n
    /// targets. Default 6 (map mob groups are usually ≤6); 15 is the protocol
    /// cap (tbyte 4 bits) — larger values may get detected by the server.
    #[serde(default = "default_max_attack_targets")]
    pub attack_max_targets: usize,
    /// Attack mode (hunting; `hunt attack` / `hunt skill`):
    /// "attack" = normal attack (one packet per mob in range, up to attack_max_targets);
    /// "skill" = skill (attack_skill=0 = skill-less multi-target single packet,
    /// >0 = skill packet). Default "attack".
    #[serde(default)]
    pub attack_mode: String,
    /// Gather step distance (px): how far `gather` pulls out-of-range mobs
    /// toward the character per round. 0 = pull directly to the character
    /// (when the server allows it). Default 150 — avoids MOB_VAC detection
    /// (reduce_x>200 || reduce_y>150 counts as a violation).
    #[serde(default = "default_gather_step")]
    pub gather_step: i32,
    /// Gather round interval (ms): `gather interval <ms>`. 0 = one round per
    /// tick (default); increase it to reduce the signature on servers that
    /// are sensitive to movement packet frequency.
    #[serde(default)]
    pub gather_interval: u64,
    /// Max mobs pulled per round: `gather max <n>`. 0 = unlimited (all
    /// out-of-range mobs). 3~5 recommended on batch-move-sensitive servers.
    #[serde(default)]
    pub gather_max: usize,
    /// Reactor attack round interval (ms): `reactor cooldown <ms>`. 0 = hit
    /// everything every tick (default). Increase it on packet-rate-sensitive
    /// servers (e.g. 1000 = once per second); only reactor hit packets are
    /// gated — drop pickup still runs every tick.
    #[serde(default)]
    pub reactor_cooldown: u64,
    /// Periodic client report packet (0x15 STRANGE_DATA, 01+random+0,
    /// ~3-4 per second). On standard servers 0x15 is CS_USE (cash shop) and
    /// the content is ignored. On by default: the server may use it to check
    /// client liveness.
    #[serde(default = "default_true")]
    pub strange_report: bool,
    /// Damage reported in attack packets (part of `hunt`, persisted by
    /// `hunt save`): 0 = estimated by level formula (lvl²/2); >0 = fixed
    /// value. The server validates reported damage (compared to the player's
    /// actual panel damage); overestimates trigger "abnormal behavior
    /// detected" disconnects. A fixed value in the character's real damage
    /// range (e.g. 400) is stable.
    #[serde(default)]
    pub damage: i32,
}

fn default_gather_step() -> i32 {
    150
}

fn default_max_attack_targets() -> usize {
    6
}


fn default_attack_cooldown() -> u64 {
    700
}

fn default_attack_range() -> i32 {
    70
}

fn default_pickup_range() -> i32 {
    400
}

fn default_pickup_enabled() -> bool {
    true
}

impl Default for HuntConfig {
    fn default() -> Self {
        Self {
            pickup_range: default_pickup_range(),
            pickup_enabled: default_pickup_enabled(),
            pickup_filter_mode: String::new(),
            pickup_allow: Vec::new(),
            pickup_deny: Vec::new(),
            // All skill params default to 0 = unconfigured: the program
            // hardcodes no skills; plain normal attack works out of the box,
            // skills come from config.json.
            attack_skill: 0,
            attack_skill_range: 0,
            attack_skill_max_targets: 0,
            attack_skill_mp_cost: 0,
            attack_skill_hits: default_skill_hits(),
            until: None,
            teleport_delay: 0,
            attack_cooldown: default_attack_cooldown(),
            attack_controller_only: default_true(),
            attack_range: default_attack_range(),
            attack_max_targets: default_max_attack_targets(),
            attack_mode: "attack".into(),
            gather_step: default_gather_step(),
            gather_interval: 0,
            gather_max: 0,
            reactor_cooldown: 0,
            strange_report: default_true(),
            damage: 0,
        }
    }
}

/// Pickup filter decision for an ITEM drop (meso is handled by the caller —
/// it always bypasses). "allow" mode picks only the whitelist, "deny" mode
/// picks everything except the blacklist, anything else = no filtering.
/// Evaluated once per drop spawn (never per tick), so a linear scan of the
/// small lists is optimal.
pub fn pickup_allowed(cfg: &HuntConfig, itemid: i32) -> bool {
    match cfg.pickup_filter_mode.as_str() {
        "allow" => cfg.pickup_allow.contains(&itemid),
        "deny" => !cfg.pickup_deny.contains(&itemid),
        _ => true,
    }
}

/// Effective filter mode for status/log output: "off" | "allow" | "deny"
/// (unknown/legacy values normalize to "off").
pub fn pickup_filter_mode(cfg: &HuntConfig) -> &'static str {
    match cfg.pickup_filter_mode.as_str() {
        "allow" => "allow",
        "deny" => "deny",
        _ => "off",
    }
}

/// Feature group: bundles a set of rules + tasks into one group (e.g.
/// "autosell", "autopotion"). The group is a master switch: its rules only
/// participate in the rule engine and its tasks can only `task start` when
/// the group is `enabled`. Per-rule `enabled` fields inside a group are
/// ignored (governed by the group switch). Opening a group with
/// `exclusive: true` automatically closes other open exclusive groups — when
/// several features fight over one resource (e.g. the shop window), only one
/// exclusive group runs at a time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupDef {
    pub id: String,
    /// Group master switch (config-writable, off by default); toggled at
    /// runtime with `group open/close <id>`, reverting to the file value on `reload`.
    #[serde(default)]
    pub enabled: bool,
    /// Exclusive group: opening it auto-closes other open exclusive groups.
    #[serde(default)]
    pub exclusive: bool,
    /// Group rules: only participate in the rule engine while the group is on
    /// (per-rule `enabled` is ignored).
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Group tasks: only startable via `task start` while the group is on.
    #[serde(default)]
    pub tasks: Vec<TaskDef>,
}

/// Complete runtime configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub potion: Vec<PotionRule>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub tasks: Vec<TaskDef>,
    /// Feature groups (organization + master switch + exclusivity). Empty by
    /// default; no side effects.
    #[serde(default)]
    pub groups: Vec<GroupDef>,
    #[serde(default)]
    pub hunt: HuntConfig,
    /// Whisper-control whitelist (in-game `#` command senders). Persistent
    /// config goes here; CLI `--chat-admin` still works and is merged into
    /// this list at startup.
    #[serde(default)]
    pub chat_admins: Vec<String>,
    /// Main loop tick period (ms). Default 50 (20 ticks/s); lower it for
    /// stress-testing/high-frequency scenarios (e.g. 10 = 100 ticks/s) —
    /// rules, tasks and hunting all speed up with this rhythm.
    #[serde(default = "default_tick_ms")]
    pub tick_ms: u64,
    /// Potion cooldown (ms): minimum gap between two potions. Default 800.
    #[serde(default = "default_potion_cooldown")]
    pub potion_cooldown: u64,
    /// Auto-reconnect on disconnect/connection failure. Reconnect keeps
    /// config-like in-memory state (rule open/group run switches, hunt config,
    /// damage, etc.) and only resets session data. On by default.
    #[serde(default = "default_true")]
    pub reconnect: bool,
    /// Reconnect interval (seconds). Default 5 (the character takes a few
    /// seconds to log out; too short gets rejected with "account already in game").
    #[serde(default = "default_reconnect_delay")]
    pub reconnect_delay: u64,
    /// Max consecutive reconnects; 0 = retry forever. Default 30. Exceeding
    /// it ends the session (the TUI returns to the wizard).
    #[serde(default = "default_reconnect_max")]
    pub reconnect_max: u32,
    /// Login info (read at TUI/CLI startup): account/password/optional AES
    /// key. No runtime effect; missing `login` section = use CLI args or the
    /// login wizard.
    #[serde(default)]
    pub login: crate::login::LoginInfo,
    /// Per-profile data directory for name dictionaries (items/mobs/npcs/maps/
    /// portals/skills). Empty = default `data/`. When set, data files are
    /// loaded from this path with fallback to `data/` for missing files.
    #[serde(default)]
    pub data_dir: String,
}

fn default_skill_hits() -> u8 {
    1
}

fn default_true() -> bool {
    true
}

fn default_reconnect_delay() -> u64 {
    5
}

fn default_reconnect_max() -> u32 {
    30
}

fn default_tick_ms() -> u64 {
    50
}

fn default_potion_cooldown() -> u64 {
    800
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            // exe built-in defaults = minimal core: engine behavior + whitelist
            // only; **no potion/rule/task/group** — those are business config
            // with the repo's config.json as the example source (deep-merged
            // at startup; file arrays replace built-in empties wholesale,
            // see the default_matches_config_file test).
            potion: vec![],
            // No built-in rules: example rules (trade_accept / trade_confirm /
            // sellauto) live in config.json; enable with `rule open <id>`.
            rules: vec![],
            // No built-in tasks: the example task (sell flow) lives in config.json.
            tasks: vec![],
            // Default hunt config: plain normal attack (no built-in skill
            // params, see HuntConfig::default).
            hunt: HuntConfig::default(),
            // Default whisper whitelist (aligned with the repo's config.json)
            chat_admins: vec!["CCCCCW".into()],
            // No built-in feature groups: example groups (autosell/autopotion
            // exclusive groups) live in config.json (off by default, no side effects).
            groups: vec![],
            tick_ms: default_tick_ms(),
            potion_cooldown: default_potion_cooldown(),
            reconnect: default_true(),
            reconnect_delay: default_reconnect_delay(),
            reconnect_max: default_reconnect_max(),
            login: crate::login::LoginInfo::default(),
            data_dir: String::new(),
        }
    }
}

/// Recursively merge two JSON values: objects merge key by key (nested
/// objects recurse); arrays/scalars take `over`. Used so the file only needs
/// to write the fields it wants to override.
/// Keys whose array values merge element-wise by their `id` field instead of
/// being replaced wholesale. Lets a profile override single fields of — or
/// disable via `"enabled": false` — an entry inherited from a shared library
/// layer, while new ids simply append. Applies recursively (e.g. a group's
/// nested `rules`). Other arrays (`potion`, `chat_admins`, ...) still replace.
const ID_MERGE_KEYS: &[&str] = &["rules", "tasks", "groups"];

fn entry_id(v: &serde_json::Value) -> Option<&str> {
    v.get("id").and_then(|id| id.as_str())
}

fn merge_by_id(base: Vec<serde_json::Value>, over: Vec<serde_json::Value>) -> serde_json::Value {
    let mut out = base;
    for o in over {
        match entry_id(&o) {
            Some(id) => match out.iter().position(|e| entry_id(e) == Some(id)) {
                Some(i) => {
                    out[i] = if out[i].is_object() && o.is_object() {
                        merge_json(out[i].clone(), o)
                    } else {
                        o
                    };
                }
                None => out.push(o),
            },
            None => out.push(o),
        }
    }
    serde_json::Value::Array(out)
}

fn merge_json(base: serde_json::Value, over: serde_json::Value) -> serde_json::Value {
    match (base, over) {
        (serde_json::Value::Object(mut b), serde_json::Value::Object(o)) => {
            for (k, v) in o {
                let id_merge = ID_MERGE_KEYS.contains(&k.as_str());
                match b.remove(&k) {
                    Some(bv) if bv.is_object() && v.is_object() => {
                        b.insert(k, merge_json(bv, v));
                    }
                    Some(bv) if id_merge && bv.is_array() && v.is_array() => {
                        let base = match bv {
                            serde_json::Value::Array(a) => a,
                            _ => unreachable!("checked is_array"),
                        };
                        let over = match v {
                            serde_json::Value::Array(a) => a,
                            _ => unreachable!("checked is_array"),
                        };
                        b.insert(k, merge_by_id(base, over));
                    }
                    // Not a merge case: the later layer wins wholesale.
                    _ => {
                        b.insert(k, v);
                    }
                }
            }
            serde_json::Value::Object(b)
        }
        (_, v) => v,
    }
}

/// Recursively collect `path` plus everything it pulls in via its top-level
/// `"include": ["file.json", ...]` directive. Includes are resolved relative
/// to the declaring file and load depth-first in listed order, BEFORE the
/// declaring file itself; each file loads at most once per launch (a repeat
/// or an include cycle is a no-op, so the entry point always merges last).
/// `missing_ok` only applies to the entry path itself (legacy tolerance for
/// the main config chain); a missing or invalid *included* file is always an
/// error — an explicit reference must resolve.
fn collect_layers(
    path: &std::path::Path,
    missing_ok: bool,
    out: &mut Vec<(std::path::PathBuf, serde_json::Value)>,
    done: &mut std::collections::HashSet<std::path::PathBuf>,
) -> Result<(), String> {
    let display = path.display().to_string();
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            if missing_ok && e.kind() == std::io::ErrorKind::NotFound {
                crate::emit_f!([crate::emit::Field::Text => display] =>
                    "[config] {display} missing — skipped");
                return Ok(());
            }
            return Err(format!("cannot read {display}: {e}"));
        }
    };
    // Canonicalize for identity; fall back to the raw path when the target
    // does not exist (or the FS denies it).
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !done.insert(key) {
        return Ok(()); // already collected once this launch
    }
    let file: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("{display} parse error: {e}"))?;
    if let Some(list) = file.get("include").and_then(|v| v.as_array()) {
        let dir = path.parent().unwrap_or(std::path::Path::new("."));
        for inc in list {
            let Some(rel) = inc.as_str() else {
                return Err(format!("{display}: include entries must be strings"));
            };
            collect_layers(&dir.join(rel), false, out, done)?;
        }
    }
    out.push((path.to_path_buf(), file));
    Ok(())
}

impl RuntimeConfig {

    /// Load from the active config chain: write defaults if no file exists;
    /// deep-merge existing files in order (later files override earlier).
    ///
    /// Uses the **process-level** chain. Session-scoped code must call
    /// [`Self::load_from`] with its own profile chain, otherwise one account's
    /// reload would read another account's file.
    pub fn load() -> Self {
        Self::load_from(&config_paths())
    }

    /// Load from an explicit path chain (pure w.r.t. globals; used by tests
    /// and by [`Self::load`]). Semantics:
    /// - each file may declare `"include": [...]`; referenced layers load
    ///   depth-first before the declaring file (see [`collect_layers`]);
    /// - every readable+valid layer contributes, merged left to right over
    ///   the built-in defaults (objects deep-merge; `rules`/`tasks`/`groups`
    ///   arrays merge by entry id, other arrays replace wholesale);
    /// - a missing entry path simply contributes nothing; ANY invalid layer
    ///   (parse error, missing/bad include) falls back to bare defaults
    ///   entirely — a broken reference must not silently disable the rest;
    /// - if NO file exists, defaults are bootstrapped onto the LAST **user**
    ///   path.
    pub fn load_from(paths: &[std::path::PathBuf]) -> Self {
        // Write-back target stays the user's last path regardless of includes.
        let write_target = paths.last().cloned().unwrap_or_else(write_path);
        let mut base = serde_json::to_value(Self::default()).expect("default config serializes");
        let mut layers: Vec<(std::path::PathBuf, serde_json::Value)> = Vec::new();
        let mut done = std::collections::HashSet::new();
        for path in paths {
            if let Err(e) = collect_layers(path, true, &mut layers, &mut done) {
                crate::emit_f!([crate::emit::Field::Text => e] =>
                    "[config] {e} — using defaults");
                return Self::default();
            }
        }
        let mut any_exists = false;
        for (path, file) in layers {
            any_exists = true;
            base = merge_json(base, file);
            let display = path.display().to_string();
            crate::emit_f!([crate::emit::Field::Text => display] => "[config] loaded {display}");
        }
        if !any_exists {
            // Bootstrap: nothing on disk yet → write defaults to the write-back
            // target (the user's last path, not a library layer).
            let cfg = Self::default();
            match cfg.save_to(&write_target) {
                Ok(()) => {
                    let display = write_target.display().to_string();
                    crate::emit_f!([crate::emit::Field::Text => display] => "[config] no config file — wrote defaults to {display}");
                }
                Err(e) => crate::emit_f!([crate::emit::Field::Text => e] => "[config] cannot write default config: {e}"),
            }
            return cfg;
        }
        match serde_json::from_value::<RuntimeConfig>(base) {
            Ok(cfg) => cfg,
            Err(e) => {
                crate::emit_f!([crate::emit::Field::Text => e] =>
                    "[config] merged config invalid: {e} — using defaults (potion/rules/tasks/hunt/groups all fall back)");
                Self::default()
            }
        }
    }

    /// Write the config back to the write-back target (used by #cfg save etc.).
    pub fn save(&self) -> Result<(), String> {
        self.save_to(&write_path())
    }

    /// Write the config back to an explicit path (parent dirs auto-created).
    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())
    }

    /// Persist only the current in-memory `hunt` section into the write-back
    /// target (`hunt save`), leaving every other key on disk untouched —
    /// runtime rule/group toggles and other in-memory values are NOT written
    /// back. On the next launch the saved hunt parameters are reused as-is.
    pub fn save_hunt(&self) -> Result<(), String> {
        self.save_hunt_to(&write_path())
    }

    /// Persist only the `hunt` section into an explicit path. Refuses to
    /// touch a corrupt target (parse error) instead of silently replacing
    /// the whole file with just the hunt key.
    pub fn save_hunt_to(&self, path: &std::path::Path) -> Result<(), String> {
        let hunt_value = serde_json::to_value(&self.hunt).map_err(|e| e.to_string())?;
        let mut root: serde_json::Value = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| {
                format!(
                    "{} is not valid JSON ({e}) — hunt save refused to overwrite",
                    path.display()
                )
            })?,
            Err(_) => serde_json::Value::Object(Default::default()),
        };
        match root.as_object_mut() {
            Some(obj) => {
                obj.insert("hunt".into(), hunt_value);
            }
            None => return Err(format!("{} root must be an object", path.display())),
        }
        let text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())
    }

    /// Hot reload from the active chain: re-read every file (file wins; hunt
    /// parameters changed at runtime are overwritten by file values). Tasks/
    /// rules/potion take effect immediately; running tasks' steps were
    /// expanded at start and are unaffected. Returns Err on ANY missing or
    /// invalid layer, **keeping the current runtime config** (not wiped
    /// wholesale) — a half-broken edit must not silently drop one layer's
    /// overrides.
    pub fn reload(&mut self) -> Result<(), String> {
        self.reload_from(&config_paths())
    }

    /// Hot reload from an explicit path chain (includes resolved, same
    /// semantics as [`Self::load_from`]).
    pub fn reload_from(&mut self, paths: &[std::path::PathBuf]) -> Result<(), String> {
        let mut layers: Vec<(std::path::PathBuf, serde_json::Value)> = Vec::new();
        let mut done = std::collections::HashSet::new();
        for path in paths {
            // Strict here (unlike load): a vanished file must fail the
            // reload and keep the current runtime config, not silently drop.
            collect_layers(path, false, &mut layers, &mut done)?;
        }
        let base = serde_json::to_value(Self::default()).expect("default config serializes");
        let mut acc = base;
        for (_, file) in layers {
            acc = merge_json(acc, file);
        }
        let fresh: RuntimeConfig = serde_json::from_value(acc)
            .map_err(|e| format!("merged config invalid: {e}"))?;
        *self = fresh;
        Ok(())
    }

    /// Find a task definition by id (top-level tasks + all group tasks, no
    /// group gating).
    pub fn task(&self, id: &str) -> Option<TaskDef> {
        self.task_with_group(id).map(|(t, _, _)| t)
    }

    /// Find a task definition by id and return group gating info:
    /// `(TaskDef, Option<group id>, group enabled)`. Top-level task →
    /// (def, None, true). Group tasks start only when the owning group is
    /// `enabled`.
    pub fn task_with_group(&self, id: &str) -> Option<(TaskDef, Option<String>, bool)> {
        if let Some(t) = self.tasks.iter().find(|t| t.id == id) {
            return Some((t.clone(), None, true));
        }
        for g in &self.groups {
            if let Some(t) = g.tasks.iter().find(|t| t.id == id) {
                return Some((t.clone(), Some(g.id.clone()), g.enabled));
            }
        }
        None
    }

    /// Find a group definition by id.
    pub fn group(&self, id: &str) -> Option<&GroupDef> {
        self.groups.iter().find(|g| g.id == id)
    }

    /// Find a group definition by id (mutable).
    pub fn group_mut(&mut self, id: &str) -> Option<&mut GroupDef> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    /// All currently effective rules: top-level rules (their `enabled` field
    /// applies) + rules of enabled groups (per-rule `enabled` is ignored and
    /// treated as on — the group switch governs).
    pub fn effective_rules(&self) -> Vec<Rule> {
        let mut out = self.rules.clone();
        for g in &self.groups {
            if g.enabled {
                // Inner rules keep their OWN enabled flag: an open group only
                // makes them eligible — a rule explicitly disabled in config
                // stays skipped (temp-action toggling without group churn).
                out.extend(g.rules.iter().cloned());
            }
        }
        out
    }
}

/// Boolean predicate keys (evaluate to true/false; used with ==1 / ==0).
const BOOL_KEYS: &[&str] = &[
    "hunt",
    "hunt_reactor",
    "teleport",
    "shop_open",
    "dialog",
    "trade_invite",
    "trade_active",
    "trade_partner_locked",
    "trade_locked",
    "task_active",
];

fn bool_val(state: &crate::state::BotState, key: &str) -> bool {
    match key {
        "hunt" => state.hunt,
        "hunt_reactor" => state.hunt_reactor,
        "shop_open" => state.shop_open,
        "dialog" => state.dialog_open,
        "trade_invite" => state.trade.invite.is_some(),
        "trade_active" => state.trade.active,
        "trade_partner_locked" => state.trade.partner_locked,
        "trade_locked" => state.trade.locked,
        "task_active" => !state.task_stack.is_empty(),
        _ => false,
    }
}

fn pct(cur: i16, max: i16) -> i32 {
    if max <= 0 {
        100
    } else {
        (cur as i32 * 100 / max as i32).clamp(0, 100)
    }
}

fn num_val(state: &crate::state::BotState, key: &str) -> Option<i32> {
    match key {
        "hp_pct" => Some(pct(state.hp, state.maxhp)),
        "mp_pct" => Some(pct(state.mp, state.maxmp)),
        "hp" => Some(state.hp as i32),
        "mp" => Some(state.mp as i32),
        "ap" => Some(state.ap as i32),
        "sp" => Some(state.sp as i32),
        "meso" => Some(state.meso),
        "level" => Some(state.level as i32),
        "equips" => Some(
            state
                .inventory
                .iter()
                .filter(|((tab, _), _)| *tab == 1)
                .count() as i32,
        ),
        "equips_worn" => Some(state.equipped.len() as i32),
        // Players on the same map (excluding self and party): for "log off
        // when someone arrives" style rules
        "players" => Some(
            state
                .entities
                .values()
                .filter(|e| {
                    e.kind == crate::state::EntityKind::Player && e.oid != state.my_cid
                })
                .filter(|e| {
                    state
                        .party
                        .as_ref()
                        .map(|p| !p.members.contains_key(&e.oid))
                        .unwrap_or(true)
                })
                .count() as i32,
        ),
        "mapid" => Some(state.mapid),
        // mobs / drops / npcs: live object counts on the current map — the
        // gating keys for rule-driven flows (fight while mobs>0, loot while
        // drops>0, proceed when mobs==0).
        "mobs" => Some(
            state
                .entities
                .values()
                .filter(|e| e.kind == crate::state::EntityKind::Mob)
                .count() as i32,
        ),
        "drops" => Some(state.drops.len() as i32),
        "npcs" => Some(state.npcs.len() as i32),
        // reactors: count of tracked reactors on the current map. Servers push
        // reactor spawn packets shortly after entering a room, so `reactors>0`
        // is the "data arrived" gate before acting on them.
        "reactors" => Some(state.reactors.len() as i32),
        _ => None,
    }
}

fn cmp(op: &str, a: i32, b: i32) -> bool {
    match op {
        "<" => a < b,
        "<=" => a <= b,
        ">" => a > b,
        ">=" => a >= b,
        "==" => a == b,
        _ => false,
    }
}

/// Evaluate a predicate expression (e.g. `hp_pct<30`, `trade_invite==1`,
/// `always`), supporting `&&` (and) and `||` (or) chains. Unknown keys or
/// malformed input safely return false.
pub fn eval_predicate(state: &crate::state::BotState, pred: &str) -> bool {
    let p = pred.trim();
    if p.is_empty() || p == "always" {
        return true;
    }
    // Split on || (or): any clause true suffices; within a clause split on && (and).
    p.split("||").any(|or_part| {
        or_part
            .split("&&")
            .all(|and_part| eval_single(state, and_part))
    })
}

/// Evaluate a single condition (key op value).
fn eval_single(state: &crate::state::BotState, pred: &str) -> bool {
    let p = pred.trim();
    if p.is_empty() || p == "always" {
        return true;
    }
    // Split key, op, val: the first < > = char is the operator start.
    let split = p
        .char_indices()
        .find(|(_, c)| matches!(c, '<' | '>' | '='));
    let (key, rest) = match split {
        Some((i, _)) => (&p[..i], &p[i..]),
        None => (p, ""),
    };
    let key = key.trim();
    let mut op = "";
    let mut val = "";
    if !rest.is_empty() {
        let mut chars = rest.chars();
        let first = chars.next().unwrap();
        op = &rest[..first.len_utf8()];
        let mut end = first.len_utf8();
        if matches!(op, "<" | ">" | "=") {
            if let Some(&c) = rest.as_bytes().get(end) {
                if (op == ">" && c == b'=') || (op == "<" && c == b'=') || (op == "=" && c == b'=') {
                    end += 1;
                }
            }
        }
        op = &rest[..end];
        val = rest[end..].trim();
    }
    // npc==<npcid>: an NPC with that npcid exists on the current map (filled by SPAWN_NPC on map entry).
    if key == "npc" {
        if op == "==" {
            let id: i32 = val.parse().unwrap_or(-1);
            return state.npcs.values().any(|n| n.npcid == id);
        }
        return false;
    }
    // var==<name>:<value>: user flow variable exact match (`setvar`/`clearvar`).
    if key == "var" {
        if op == "==" {
            return match val.split_once(':') {
                Some((n, v)) => state.vars.get(n).map(String::as_str) == Some(v),
                None => false,
            };
        }
        return false;
    }
    if BOOL_KEYS.contains(&key) {
        let b = bool_val(state, key);
        // Only ==1 / ==0 supported (bare key treated as ==1)
        if op == "==" {
            return (val == "1") == b;
        }
        return op.is_empty() && b;
    }
    let Some(a) = num_val(state, key) else {
        return false;
    };
    // RHS supports variable keys (reserved for future use, e.g.
    // `equips>=threshold`) — parse as a literal number first, then fall back
    // to the current value of a numeric key.
    let b = match val.parse::<i32>() {
        Ok(n) => n,
        Err(_) => match num_val(state, val) {
            Some(n) => n,
            None => return false,
        },
    };
    cmp(op, a, b)
}

/// Potion rule match: when the stat percentage is below the threshold, return
/// the first potion itemid in stock.
pub fn potion_pick(state: &crate::state::BotState, rule: &PotionRule) -> Option<i32> {
    let cur = match rule.stat.as_str() {
        "hp" => pct(state.hp, state.maxhp),
        "mp" => pct(state.mp, state.maxmp),
        _ => return None,
    };
    if cur >= rule.threshold_pct {
        return None;
    }
    rule.itemids.iter().copied().find(|id| {
        state
            .inventory
            .iter()
            .any(|((tab, _), it)| *tab == 2 && it.itemid == *id && it.qty > 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{BotState, Item};

    fn state_with_hp(hp: i16, maxhp: i16) -> BotState {
        let mut s = BotState::default();
        s.hp = hp;
        s.maxhp = maxhp;
        s
    }

    #[test]
    fn eval_numeric_predicates() {
        let s = state_with_hp(20, 100);
        assert!(eval_predicate(&s, "hp_pct<30"));
        assert!(!eval_predicate(&s, "hp_pct<10"));
        assert!(eval_predicate(&s, "hp<25"));
        assert!(!eval_predicate(&s, "hp>=30"));
        assert!(eval_predicate(&s, "hp_pct<=20"));
        assert!(eval_predicate(&s, "hp_pct>19"));
        assert!(eval_predicate(&s, "hp_pct==20"));
    }

    #[test]
    fn eval_bool_predicates() {
        let mut s = BotState::default();
        assert!(eval_predicate(&s, "trade_invite==0"));
        assert!(eval_predicate(&s, "hunt==0"));
        s.hunt = true;
        assert!(eval_predicate(&s, "hunt==1"));
        assert!(!eval_predicate(&s, "hunt==0"));
        assert!(!eval_predicate(&s, "hunt==2"));
    }

    #[test]
    fn eval_players_excludes_self_and_party() {
        use crate::state::{Entity, EntityKind, Party, PartyMember};
        let mut s = BotState::default();
        s.my_cid = 1;
        // self doesn't count
        s.entities.insert(1, Entity { oid: 1, kind: EntityKind::Player, ..Default::default() });
        assert!(!eval_predicate(&s, "players>=1"));
        // mobs don't count
        s.entities.insert(9, Entity { oid: 9, kind: EntityKind::Mob, ..Default::default() });
        assert!(!eval_predicate(&s, "players>=1"));
        // passing players count
        s.entities.insert(2, Entity { oid: 2, kind: EntityKind::Player, ..Default::default() });
        assert!(eval_predicate(&s, "players>=1"));
        // party members don't count
        s.entities.insert(3, Entity { oid: 3, kind: EntityKind::Player, ..Default::default() });
        s.party = Some(Party {
            partyid: 10,
            leader_id: 1,
            members: {
                let mut m = std::collections::BTreeMap::new();
                m.insert(3, PartyMember { id: 3, ..Default::default() });
                m
            },
        });
        assert!(eval_predicate(&s, "players==1"));
        assert!(!eval_predicate(&s, "players==2"));
    }

    #[test]
    fn eval_shop_and_dialog_predicates() {
        let mut s = BotState::default();
        assert!(eval_predicate(&s, "shop_open==0"));
        assert!(eval_predicate(&s, "dialog==0"));
        s.shop_open = true;
        s.dialog_open = true;
        assert!(eval_predicate(&s, "shop_open==1"));
        assert!(eval_predicate(&s, "dialog==1"));
        // a new dialog page from the server sets dialog=true; optimistically
        // false after sending a reply
        s.dialog_open = false;
        assert!(eval_predicate(&s, "dialog==0"));
    }

    #[test]
    fn eval_always_and_unknown() {
        let s = BotState::default();
        assert!(eval_predicate(&s, "always"));
        assert!(!eval_predicate(&s, "bogus==1"));
        assert!(!eval_predicate(&s, "hp_pct>abc"));
        assert!(!eval_predicate(&s, "hp_pct"));
    }

    #[test]
    fn eval_and_or_chains() {
        let mut s = BotState::default();
        // && chain: any false makes the whole chain false
        s.ap = 5;
        s.hunt = false;
        assert!(!eval_predicate(&s, "hunt==1 && ap>0"));
        s.hunt = true;
        assert!(eval_predicate(&s, "hunt==1 && ap>0"));
        s.ap = 0;
        assert!(!eval_predicate(&s, "hunt==1 && ap>0"));
        // || chain: any true suffices
        s.hunt = false;
        assert!(!eval_predicate(&s, "hunt==1 || ap>0"));
        s.ap = 3;
        assert!(eval_predicate(&s, "hunt==1 || ap>0"));
        // mixed
        s.hunt = true;
        s.ap = 3;
        assert!(eval_predicate(&s, "hunt==1 && ap>0"));
        assert!(!eval_predicate(&s, "hunt==0 || ap<0"));
    }

    #[test]
    fn eval_removed_keys_return_false() {
        let s = BotState::default();
        // Removed keys (interval / sell_threshold / ap_auto / trade_auto /
        // trade_puts_*...) are always false; equips is valid (0 items), and
        // unknown RHS sell_threshold → false
        assert!(!eval_predicate(&s, "interval>5"));
        assert!(!eval_predicate(&s, "equips>=sell_threshold"));
        assert!(!eval_predicate(&s, "ap_auto==1"));
        assert!(!eval_predicate(&s, "trade_auto==1"));
        assert!(!eval_predicate(&s, "trade_puts_pending==1"));
        assert!(!eval_predicate(&s, "trade_puts_done==0"));
    }

    #[test]
    fn eval_equips_worn_vs_bag() {
        let mut s = BotState::default();
        // empty bag + nothing worn
        assert!(eval_predicate(&s, "equips==0"));
        assert!(eval_predicate(&s, "equips_worn==0"));
        // 2 equipment items in the bag (tab==1)
        s.inventory.insert((1, 1), Item { itemid: 1002000, qty: 1, ..Item::default() });
        s.inventory.insert((1, 2), Item { itemid: 1002001, qty: 1, ..Item::default() });
        assert!(eval_predicate(&s, "equips==2"));
        assert!(eval_predicate(&s, "equips_worn==0"));
        // 1 worn item (negative body slot); bag doesn't affect equips_worn
        s.equipped.insert(-1, Item { itemid: 1002002, qty: 1, ..Item::default() });
        assert!(eval_predicate(&s, "equips==2"));
        assert!(eval_predicate(&s, "equips_worn==1"));
        // USE tab doesn't count toward equips
        s.inventory.insert((2, 1), Item { itemid: 2000000, qty: 10, ..Item::default() });
        assert!(eval_predicate(&s, "equips==2"));
    }

    #[test]
    fn eval_variable_rhs_and_task_active() {
        let mut s = BotState::default();
        assert!(eval_predicate(&s, "task_active==0"));
        s.task_stack.push(TaskRuntime {
            def_id: "sell".into(),
            steps: Vec::new(),
            step_idx: 0,
            step_since: Instant::now(),
            group: None,
            hunt_before: false,
            recurring: false,
        });
        assert!(eval_predicate(&s, "task_active==1"));
    }

    #[test]
    fn eval_npc_presence_on_map() {
        let mut s = BotState::default();
        // no NPCs on the map → not present
        assert!(!eval_predicate(&s, "npc==1011100"));
        // insert the Henesys general-store NPC → present
        s.npcs.insert(77, crate::state::Npc { npcid: 1011100, oid: 77, x: 0, y: 0 });
        assert!(eval_predicate(&s, "npc==1011100"));
        assert!(!eval_predicate(&s, "npc==2093001"));
        // operators other than == unsupported
        assert!(!eval_predicate(&s, "npc>0"));
    }

    #[test]
    fn eval_var_flow_variable() {
        let mut s = BotState::default();
        // unset variable → false (no implicit defaults)
        assert!(!eval_predicate(&s, "var==boss_summoned:1"));
        s.vars.insert("boss_summoned".into(), "1".into());
        assert!(eval_predicate(&s, "var==boss_summoned:1"));
        // value mismatch → false
        assert!(!eval_predicate(&s, "var==boss_summoned:0"));
        // name mismatch → false
        assert!(!eval_predicate(&s, "var==other:1"));
        // malformed (no colon) → false, never panics
        assert!(!eval_predicate(&s, "var==boss_summoned"));
        // operators other than == unsupported
        assert!(!eval_predicate(&s, "var>boss_summoned:1"));
        // works inside && / || chains
        assert!(eval_predicate(&s, "mobs==0 && var==boss_summoned:1"));
        // clear → back to false
        s.vars.remove("boss_summoned");
        assert!(!eval_predicate(&s, "var==boss_summoned:1"));
    }

    #[test]
    fn eval_mobs_drops_npcs_counts() {
        let mut s = BotState::default();
        assert!(eval_predicate(&s, "mobs==0"));
        assert!(eval_predicate(&s, "drops==0"));
        assert!(eval_predicate(&s, "npcs==0"));
        use crate::state::EntityKind;
        s.entities.insert(
            1,
            crate::state::Entity {
                oid: 1,
                kind: EntityKind::Mob,
                mobid: 8800000,
                x: 10,
                y: 10,
                ..Default::default()
            },
        );
        s.entities.insert(
            2,
            crate::state::Entity {
                oid: 2,
                kind: EntityKind::Mob,
                mobid: 8800000,
                x: 20,
                y: 20,
                ..Default::default()
            },
        );
        // a player entity must not count toward mobs
        s.entities.insert(
            3,
            crate::state::Entity {
                oid: 3,
                kind: EntityKind::Player,
                charname: "stranger".into(),
                x: 30,
                y: 30,
                ..Default::default()
            },
        );
        s.npcs.insert(77, crate::state::Npc { npcid: 1011100, oid: 77, x: 0, y: 0 });
        assert!(eval_predicate(&s, "mobs==2"));
        assert!(eval_predicate(&s, "mobs>0"));
        assert!(eval_predicate(&s, "npcs==1"));
        // reactors counts tracked reactors; zero until the server pushes spawns
        assert!(!eval_predicate(&s, "reactors>0"));
        use crate::state::Reactor;
        s.reactors.insert(55, Reactor { oid: 55, rid: 2201004, state: 0, x: -181, y: -439 });
        assert!(eval_predicate(&s, "reactors>0"));
        assert!(eval_predicate(&s, "reactors==1 && mobs==2"));
        assert!(!eval_predicate(&s, "reactors==2"));
        // drops count follows state.drops (insert via Drop struct)
        s.drops.insert(
            9,
            crate::state::Drop {
                oid: 9,
                itemid: 4000000,
                is_meso: false,
                x: 5,
                y: 5,
                last_try: None,
                tries: 0,
            },
        );
        assert!(eval_predicate(&s, "drops==1 && mobs==2 && npcs==1"));
    }

    #[test]
    fn group_inner_rules_respect_own_enabled_and_default_true() {
        // An open group makes inner rules ELIGIBLE, but
        // each one still runs by its own enabled flag; an absent enabled key
        // defaults to true (legacy bf_money rules must keep firing).
        let parsed: RuntimeConfig = serde_json::from_str(
            r#"{"groups":[{"id":"g","enabled":true,"exclusive":false,
                "rules":[
                  {"id":"on","when":"always","then":[]},
                  {"id":"off","enabled":false,"when":"always","then":[]}
                ],
                "tasks":[]}]}"#,
        )
        .unwrap();
        assert!(parsed.groups[0].rules[0].enabled, "absent enabled = true");
        assert!(!parsed.groups[0].rules[1].enabled);
        let eff = parsed.effective_rules();
        let on = eff.iter().find(|r| r.id == "on").unwrap();
        let off = eff.iter().find(|r| r.id == "off").unwrap();
        assert!(on.enabled);
        assert!(!off.enabled, "open group must NOT force-enable inner rules");
    }

    #[test]
    fn default_has_no_rules_tasks_groups() {
        // exe built-in default = minimal core: no rules/tasks/groups/potion
        // (business config comes from config.json, see default_matches_config_file).
        let cfg = RuntimeConfig::default();
        assert!(cfg.rules.is_empty(), "exe built-in must not ship rules");
        assert!(cfg.tasks.is_empty(), "exe built-in must not ship tasks");
        assert!(cfg.groups.is_empty(), "exe built-in must not ship groups");
        // no hardcoded potion rules (thresholds/potion ids are business
        // config provided by config)
        assert!(cfg.potion.is_empty(), "exe built-in must not ship potion rules");
        assert_eq!(cfg.chat_admins, vec!["CCCCCW"]);
        // default hunt config: plain normal attack, no built-in skill params
        // (user-configured)
        assert_eq!(cfg.hunt.pickup_range, 400);
        assert_eq!(cfg.hunt.attack_skill, 0); // no built-in skill
        assert_eq!(cfg.hunt.attack_skill_range, 0);
        assert_eq!(cfg.hunt.attack_skill_max_targets, 0);
        assert_eq!(cfg.hunt.attack_skill_mp_cost, 0);
        assert_eq!(cfg.hunt.attack_skill_hits, 1); // hits default to 1
        assert!(cfg.hunt.until.is_none());
        assert_eq!(cfg.hunt.teleport_delay, 0);
    }

    #[test]
    fn default_matches_config_file() {
        // Semantics: the exe has no built-in rule/task/group; config.default.json
        // is the repo template (deep-merged at startup, file arrays replace
        // built-in empties wholesale). Assert: 1) code defaults have no
        // rules/tasks/groups; 2) the template parses and its engine parts
        // match the built-ins (drift guard).
        let code = RuntimeConfig::default();
        assert!(code.rules.is_empty() && code.tasks.is_empty() && code.groups.is_empty());

        let text = std::fs::read_to_string("config.default.json").expect("config.default.json exists");
        let file = serde_json::from_str::<RuntimeConfig>(&text).expect("config.default.json parses");

        // Engine-built-in part: the template ships no whisper whitelist (the
        // program has a built-in default)
        assert!(
            file.chat_admins.is_empty(),
            "config.default.json must not ship a whisper whitelist (built-in default)"
        );
        // Potion rules/tasks/groups are business config: the program ships no
        // defaults (empty) and the template provides them; only assert the
        // template does configure potions (no hardcoded fallback to rely on).
        assert!(!file.potion.is_empty(), "config.default.json must configure potion rules");
        // tick_ms is user-tunable (lower for stress tests); only check the
        // default and parsing
        assert_eq!(default_tick_ms(), 50);
        // Example-source part: template groups are off by default (no side effects)
        for g in &file.groups {
            assert!(!g.enabled, "template groups must default to off (zero side effects)");
        }
    }

    #[test]
    fn top_level_damage_is_not_a_config_key() {
        let dir = tmp_dir("dmgstrict");
        std::fs::write(dir.join("old.json"), r#"{"damage":1234}"#).unwrap();
        let cfg = RuntimeConfig::load_from(&[dir.join("old.json")]);
        assert_eq!(
            cfg.hunt.damage, 0,
            "damage lives in hunt only: a stray top-level key is ignored"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hunt_save_persists_runtime_damage() {
        let dir = tmp_dir("dmgsave");
        let path = dir.join("p.json");
        std::fs::write(&path, r#"{"hunt":{"damage":1000},"tick_ms":33}"#).unwrap();
        let mut cfg = RuntimeConfig::load_from(std::slice::from_ref(&path));
        // Runtime change (as `hunt damage 777` would do) must survive save.
        cfg.hunt.damage = 777;
        cfg.save_hunt_to(&path).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["hunt"]["damage"], serde_json::json!(777), "damage persisted via hunt save");
        assert_eq!(raw["tick_ms"], serde_json::json!(33), "non-hunt keys untouched");
        // Reload round-trips the saved value.
        cfg.reload_from(std::slice::from_ref(&path)).unwrap();
        assert_eq!(cfg.hunt.damage, 777);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pickup_allowed_mode_semantics() {
        use crate::runtime_config::{pickup_allowed, pickup_filter_mode};
        let mut cfg = HuntConfig::default();
        // mode off (empty string) → everything allowed
        assert!(pickup_allowed(&cfg, 4000000));
        assert_eq!(pickup_filter_mode(&cfg), "off");
        // allow mode
        cfg.pickup_filter_mode = "allow".into();
        cfg.pickup_allow = vec![4000000, 4000006];
        assert!(pickup_allowed(&cfg, 4000000));
        assert!(!pickup_allowed(&cfg, 2000000));
        assert_eq!(pickup_filter_mode(&cfg), "allow");
        // deny mode
        cfg.pickup_filter_mode = "deny".into();
        cfg.pickup_deny = vec![4000000];
        assert!(!pickup_allowed(&cfg, 4000000));
        assert!(pickup_allowed(&cfg, 2000000));
        assert_eq!(pickup_filter_mode(&cfg), "deny");
        // unknown mode normalizes to off (pick everything)
        cfg.pickup_filter_mode = "bogus".into();
        assert!(pickup_allowed(&cfg, 4000000));
        assert_eq!(pickup_filter_mode(&cfg), "off");
    }

    #[test]
    fn expand_vars_substitutes_and_leaves_state_placeholders() {
        use std::collections::BTreeMap;
        let mut vars = BTreeMap::new();
        vars.insert("box".to_string(), "2022552".to_string());
        vars.insert("s1".to_string(), "5".to_string());
        assert_eq!(
            expand_vars("reward {box} -> reply {s1}", &vars),
            "reward 2022552 -> reply 5"
        );
        // unmatched {hunt_mapid}/{mapid} stay as-is (left for state placeholders)
        assert_eq!(
            expand_vars("town {hunt_mapid} west00", &vars),
            "town {hunt_mapid} west00"
        );
        // empty table = unchanged
        assert_eq!(expand_vars("chat hi", &BTreeMap::new()), "chat hi");
    }

    #[test]
    fn groups_default_empty() {
        let cfg = RuntimeConfig::default();
        assert!(cfg.groups.is_empty());
        assert!(cfg.group("autosell").is_none());
    }

    #[test]
    fn task_with_group_gating() {
        let mut cfg = RuntimeConfig::default();
        cfg.tasks.push(TaskDef {
            id: "top".into(),
            priority: 1,
            steps: vec![StepDef { cmd: "chat hi".into(), wait: None, timeout: None, loop_pred: None, cmd_parsed: None }],
            timeout: None,
            vars: Default::default(),
            recurring: false,
        });
        cfg.groups.push(GroupDef {
            id: "autosell".into(),
            enabled: false,
            exclusive: false,
            rules: vec![],
            tasks: vec![TaskDef {
                id: "open_shop".into(),
                priority: 10,
                steps: vec![StepDef { cmd: "chat hi".into(), wait: None, timeout: None, loop_pred: None, cmd_parsed: None }],
                timeout: None,
                vars: Default::default(),
            recurring: false,
            }],
        });
        // top-level task: no group gating
        let (def, gid, ok) = cfg.task_with_group("top").unwrap();
        assert_eq!(def.id, "top");
        assert!(gid.is_none());
        assert!(ok);
        // group task: group off → not startable
        let (def, gid, ok) = cfg.task_with_group("open_shop").unwrap();
        assert_eq!(def.id, "open_shop");
        assert_eq!(gid.as_deref(), Some("autosell"));
        assert!(!ok);
        // group on → startable
        cfg.group_mut("autosell").unwrap().enabled = true;
        let (_, _, ok) = cfg.task_with_group("open_shop").unwrap();
        assert!(ok);
        // unknown task
        assert!(cfg.task_with_group("nope").is_none());
        // wrapper: task() does no gating
        assert!(cfg.task("open_shop").is_some());
    }

    #[test]
    fn effective_rules_group_gating() {
        let mut cfg = RuntimeConfig::default();
        cfg.rules.clear(); // drop the default 3, focus on test data
        cfg.rules.push(Rule {
            id: "top_off".into(),
            enabled: false,
            when: "always".into(),
            then: vec![],
            cooldown: 0,
        });
        cfg.groups.push(GroupDef {
            id: "g_on".into(),
            enabled: true,
            exclusive: false,
            rules: vec![Rule {
                id: "g_rule".into(),
                // An open group makes inner rules ELIGIBLE
                // but never force-enables them — own flag decides.
                enabled: false,
                when: "always".into(),
                then: vec!["chat x".into()],
                cooldown: 100,
            }],
            tasks: vec![],
        });
        cfg.groups.push(GroupDef {
            id: "g_off".into(),
            enabled: false,
            exclusive: false,
            rules: vec![Rule {
                id: "g_off_rule".into(),
                enabled: true,
                when: "always".into(),
                then: vec![],
                cooldown: 0,
            }],
            tasks: vec![],
        });
        let eff = cfg.effective_rules();
        let ids: Vec<&str> = eff.iter().map(|r| r.id.as_str()).collect();
        // top-level rules kept as-is; rules of an OPEN group are included but
        // keep their own flag; closed groups excluded entirely
        assert_eq!(ids, vec!["top_off", "g_rule"]);
        assert!(!eff[0].enabled);
        assert!(
            !eff[1].enabled,
            "open group must not force-enable inner rules"
        );
    }

    #[test]
    fn potion_pick_threshold_and_inventory() {
        let mut s = state_with_hp(10, 100);
        let rule = PotionRule {
            stat: "hp".into(),
            threshold_pct: 25,
            itemids: vec![2000002, 2000000],
        };
        // out of stock → no potion
        assert_eq!(potion_pick(&s, &rule), None);
        // 2000000 in stock → second priority
        s.inventory
            .insert((2, 1), Item { itemid: 2000000, qty: 10, ..Item::default() });
        assert_eq!(potion_pick(&s, &rule), Some(2000000));
        s.inventory.insert(
            (2, 2),
            Item { itemid: 2000002, qty: 10, ..Item::default() },
        );
        assert_eq!(potion_pick(&s, &rule), Some(2000002));
        // HP sufficient → no potion
        let s2 = state_with_hp(80, 100);
        assert_eq!(potion_pick(&s2, &rule), None);
        // items outside the USE tab (e.g. same id in EQUIP tab) don't count
        let mut s3 = state_with_hp(10, 100);
        s3.inventory
            .insert((1, 1), Item { itemid: 2000000, qty: 1, ..Item::default() });
        assert_eq!(potion_pick(&s3, &rule), None);
    }

    #[test]
    fn merge_json_deep_merge() {
        let base: serde_json::Value =
            serde_json::from_str(r#"{"a": 1, "b": {"x": 1, "y": 2}, "list": [1,2]}"#).unwrap();
        let over: serde_json::Value =
            serde_json::from_str(r#"{"b": {"y": 9, "z": 3}, "list": [9], "c": 4}"#).unwrap();
        let merged = merge_json(base, over);
        assert_eq!(
            merged,
            serde_json::json!({"a": 1, "b": {"x": 1, "y": 9, "z": 3}, "list": [9], "c": 4})
        );
    }

    #[test]
    fn load_falls_back_to_defaults_on_bad_file() {
        // config.json in the cwd may be missing or contain anything: load must never panic.
        let _ = RuntimeConfig::load();
    }

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ost_rcfg_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_from_chain_merges_in_order() {
        let dir = tmp_dir("chain");
        let a = dir.join("a.json");
        let b = dir.join("b.json");
        std::fs::write(
            &a,
            r#"{"tick_ms":10,"rules":[{"id":"r1","when":"always","then":["chat a"]}]}"#,
        )
        .unwrap();
        std::fs::write(&b, r#"{"tick_ms":20}"#).unwrap();
        let cfg = RuntimeConfig::load_from(&[a.clone(), b.clone()]);
        assert_eq!(cfg.tick_ms, 20, "later file overrides earlier");
        assert_eq!(cfg.rules.len(), 1, "earlier file's rules survive");
        assert_eq!(cfg.rules[0].id, "r1");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn id_merge_arrays_override_fields_and_append() {
        let base = serde_json::json!({
            "rules": [
                {"id": "a", "enabled": true, "when": "hunt==1", "cooldown": 100},
                {"id": "b", "enabled": false}
            ]
        });
        let over = serde_json::json!({
            "rules": [
                {"id": "a", "cooldown": 500},
                {"id": "c", "enabled": true}
            ]
        });
        let merged = merge_json(base, over);
        let arr = merged["rules"].as_array().expect("array");
        assert_eq!(arr.len(), 3, "a overridden in place, b kept, c appended");
        assert_eq!(arr[0]["id"], serde_json::json!("a"));
        assert_eq!(arr[0]["cooldown"], serde_json::json!(500), "field override");
        assert_eq!(arr[0]["when"], serde_json::json!("hunt==1"), "untouched field survives");
        assert_eq!(arr[0]["enabled"], serde_json::json!(true));
        assert_eq!(arr[2]["id"], serde_json::json!("c"));
    }

    #[test]
    fn id_merge_disable_via_enabled_false() {
        let base = serde_json::json!({"tasks": [{"id": "t1", "priority": 10}]});
        let over = serde_json::json!({"tasks": [{"id": "t1", "loop": false}]});
        let merged = merge_json(base, over);
        let t = &merged["tasks"][0];
        assert_eq!(t["priority"], serde_json::json!(10));
        assert_eq!(t["loop"], serde_json::json!(false));
    }

    #[test]
    fn non_id_arrays_still_replace_wholesale() {
        let base = serde_json::json!({"potion": [{"stat": "hp", "threshold_pct": 25, "itemids": [1]}]});
        let over = serde_json::json!({"potion": []});
        let merged = merge_json(base, over);
        assert!(
            merged["potion"].as_array().expect("array").is_empty(),
            "potion is not an id-merge key: later layer replaces"
        );
    }

    #[test]
    fn include_loads_library_and_merges_by_id() {
        let dir = tmp_dir("incl");
        std::fs::write(
            dir.join("lib.json"),
            r#"{"rules":[{"id":"evade","enabled":true,"when":"players>=1","then":["quit"],"cooldown":5000}],"tasks":[{"id":"sell","priority":9}]}"#,
        )
        .unwrap();
        // Profile: pulls the library in, disables the inherited rule, adds its own task.
        std::fs::write(
            dir.join("profile.json"),
            r#"{"include":["lib.json"],"rules":[{"id":"evade","enabled":false}],"tasks":[{"id":"custom_store","priority":1}]}"#,
        )
        .unwrap();
        let cfg = RuntimeConfig::load_from(&[dir.join("profile.json")]);
        let evade = cfg.rules.iter().find(|r| r.id == "evade").expect("included rule");
        assert!(!evade.enabled, "profile disabled the included rule");
        assert_eq!(evade.when, "players>=1", "other fields come from the library");
        assert_eq!(evade.cooldown, 5000);
        assert!(cfg.task("sell").is_some(), "library task inherited");
        assert!(cfg.task("custom_store").is_some(), "profile task appended");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn no_include_key_loads_single_file_only() {
        let dir = tmp_dir("plain");
        std::fs::write(
            dir.join("lib.json"),
            r#"{"rules":[{"id":"evade","enabled":true,"when":"always","then":["quit"]}]}"#,
        )
        .unwrap();
        std::fs::write(dir.join("config.json"), r#"{"tick_ms":33}"#).unwrap();
        let cfg = RuntimeConfig::load_from(&[dir.join("config.json")]);
        assert_eq!(cfg.tick_ms, 33);
        assert!(
            cfg.rules.iter().all(|r| r.id != "evade"),
            "sibling files must be ignored without an explicit include"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_include_falls_back_to_defaults() {
        let dir = tmp_dir("inclmiss");
        std::fs::write(
            dir.join("profile.json"),
            r#"{"include":["ghost.json"],"tick_ms":33}"#,
        )
        .unwrap();
        let cfg = RuntimeConfig::load_from(&[dir.join("profile.json")]);
        assert_eq!(cfg, RuntimeConfig::default(), "broken reference = defaults, not half-config");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn nested_and_repeated_includes_load_once_in_order() {
        let dir = tmp_dir("nest");
        std::fs::write(dir.join("a.json"), r#"{"include":["b.json"],"potion_cooldown":100}"#).unwrap();
        std::fs::write(
            dir.join("b.json"),
            r#"{"include":["c.json","c.json"],"rules":[{"id":"r_b","when":"always","then":["chat b"]}]}"#,
        )
        .unwrap();
        std::fs::write(dir.join("c.json"), r#"{"tick_ms":10}"#).unwrap();
        let cfg = RuntimeConfig::load_from(&[dir.join("a.json")]);
        assert_eq!(
            cfg.potion_cooldown, 100,
            "entry file merges last (overrides c's sibling keys)"
        );
        assert_eq!(cfg.tick_ms, 10, "nested include applied");
        assert_eq!(cfg.rules.len(), 1, "repeated include loads only once");
        assert_eq!(cfg.rules[0].id, "r_b");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn include_cycle_terminates() {
        let dir = tmp_dir("cycle");
        std::fs::write(
            dir.join("a.json"),
            r#"{"include":["b.json"],"rules":[{"id":"ra","when":"always","then":["chat a"]}]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("b.json"),
            r#"{"include":["a.json"],"rules":[{"id":"rb","when":"always","then":["chat b"]}]}"#,
        )
        .unwrap();
        let cfg = RuntimeConfig::load_from(&[dir.join("a.json")]);
        let ids: Vec<&str> = cfg.rules.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["rb", "ra"], "cycle is a dedupe no-op: entry still merges last");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reload_with_broken_include_keeps_current_config() {
        let dir = tmp_dir("relinc");
        std::fs::write(dir.join("ok.json"), r#"{"tick_ms":7}"#).unwrap();
        let mut cfg = RuntimeConfig::load_from(&[dir.join("ok.json")]);
        std::fs::write(
            dir.join("broken.json"),
            r#"{"include":["ghost.json"]}"#,
        )
        .unwrap();
        let err = cfg.reload_from(&[dir.join("broken.json")]);
        assert!(err.is_err(), "missing include must fail the reload");
        assert_eq!(cfg.tick_ms, 7, "current runtime config kept on failed reload");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn load_from_bootstraps_defaults_when_nothing_exists() {
        let dir = tmp_dir("bootstrap");
        let missing = vec![dir.join("nope1.json"), dir.join("nope2.json")];
        let cfg = RuntimeConfig::load_from(&missing);
        assert_eq!(cfg, RuntimeConfig::default());
        // defaults bootstrapped onto the LAST path (write-back target)
        assert!(missing[1].exists(), "last path gets the bootstrap write");
        assert!(!missing[0].exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn load_from_corrupt_layer_falls_back_entirely() {
        let dir = tmp_dir("corrupt");
        let good = dir.join("good.json");
        let bad = dir.join("bad.json");
        std::fs::write(&good, r#"{"tick_ms":9}"#).unwrap();
        std::fs::write(&bad, "{not json").unwrap();
        let cfg = RuntimeConfig::load_from(&[good, bad]);
        assert_eq!(cfg, RuntimeConfig::default(), "any broken layer → bare defaults");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_and_reload_roundtrip_on_explicit_path() {
        let dir = tmp_dir("roundtrip");
        let p = dir.join("cfg.json");
        let mut cfg = RuntimeConfig::default();
        cfg.tick_ms = 7;
        cfg.save_to(&p).unwrap();
        let mut fresh = RuntimeConfig::load_from(&[p.clone()]);
        assert_eq!(fresh.tick_ms, 7);
        // runtime change then hot reload from file restores file value
        fresh.tick_ms = 99;
        fresh.reload_from(&[p.clone()]).unwrap();
        assert_eq!(fresh.tick_ms, 7);
        // reload with a missing layer errors and keeps current config
        let gone = dir.join("gone.json");
        fresh.tick_ms = 99;
        assert!(fresh.reload_from(&[p.clone(), gone]).is_err());
        assert_eq!(fresh.tick_ms, 99);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_hunt_only_touches_hunt_key() {
        let dir = tmp_dir("huntsave");
        let p = dir.join("cfg.json");
        std::fs::write(&p, r#"{"tick_ms":5,"rules":[{"id":"keep","when":"always","then":[]}]}"#)
            .unwrap();
        let mut cfg = RuntimeConfig::default();
        cfg.hunt.pickup_range = 123;
        cfg.save_hunt_to(&p).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["tick_ms"], 5, "other keys untouched");
        assert!(v["rules"].as_array().is_some());
        assert_eq!(v["hunt"]["pickup_range"], 123);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_hunt_refuses_to_wipe_corrupt_target() {
        // 回归: 旧实现对解析失败的文件 unwrap_or(空对象), hunt save 会把
        // 整个配置静默替换成只剩 hunt 一节 — 现在必须报错且原文件不动。
        let dir = tmp_dir("huntsave_bad");
        let p = dir.join("broken.json");
        let corrupt = "{not valid json";
        std::fs::write(&p, corrupt).unwrap();
        let cfg = RuntimeConfig::default();
        assert!(cfg.save_hunt_to(&p).is_err(), "损坏目标必须拒绝写入");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), corrupt, "原文件不得被改动");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn active_paths_default_to_legacy_single_file() {
        assert_eq!(config_paths(), vec![std::path::PathBuf::from(CONFIG_PATH)]);
        assert_eq!(write_path(), std::path::PathBuf::from(CONFIG_PATH));
    }
}










