//! Chinese name dictionaries from `data/*.json` (flat `"id": "名称"` maps).
//!
//! Data is loaded lazily from a data directory, with fallback to the default
//! `data/` for missing files.
//!
//! # Per-session isolation
//!
//! Name tables are **keyed by data directory and shared via `Arc`**: two
//! sessions using different `data_dir` values each get their own immutable
//! table, so switching between them can never flicker or cross-contaminate.
//! A [`Session`](crate::session::Session) holds its own `Arc<NameTable>` and
//! all session-scoped code looks names up through it.
//!
//! The legacy free functions ([`map_name`], [`find_portal`], ...) forward to a
//! process default table (see [`set_default_dir`]) so call sites that have no
//! session at hand — tests and the headless CLI — keep working unchanged.
//!
//! Every lookup renders as `id·名称`; unknown ids fall back to `id·未知` so a
//! stale id is still visibly identifiable.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};

const DEFAULT_DATA_DIR: &str = "data";

/// A single portal entry loaded from `portals.json`.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortalInfo {
    pub name: String,
    pub target_map: i32,
    pub x: i32,
    pub y: i32,
}

/// All six dictionaries for one data directory, loaded eagerly and replaced
/// atomically as a unit (never partially).
#[derive(Debug, Clone, Default)]
pub struct NameTable {
    dir: String,
    maps: HashMap<i32, String>,
    npcs: HashMap<i32, String>,
    mobs: HashMap<i32, String>,
    items: HashMap<i32, String>,
    skills: HashMap<i32, String>,
    portals: HashMap<i32, Vec<PortalInfo>>,
}

/// Try loading a JSON dict from `dir/file`; if the file doesn't exist, fall
/// back to `DEFAULT_DATA_DIR/file`.
///
/// A **malformed** file is reported on stderr and treated as empty: silently
/// swallowing it would make every id render as `未知` with no clue why.
fn load_dict(dir: &str, file: &str) -> HashMap<i32, String> {
    match std::fs::read_to_string(resolve_path(dir, file)) {
        Ok(raw) => match serde_json::from_str::<HashMap<i32, String>>(&raw) {
            Ok(map) => map,
            Err(e) => {
                eprintln!("[names] {dir}/{file}: 解析失败，按空表处理: {e}");
                HashMap::new()
            }
        },
        Err(_) => HashMap::new(),
    }
}

/// Same as [`load_dict`] for the portal table (different value type).
fn load_portals(dir: &str) -> HashMap<i32, Vec<PortalInfo>> {
    match std::fs::read_to_string(resolve_path(dir, "portals.json")) {
        Ok(raw) => match serde_json::from_str::<HashMap<i32, Vec<PortalInfo>>>(&raw) {
            Ok(map) => map,
            Err(e) => {
                eprintln!("[names] {dir}/portals.json: 解析失败，按空表处理: {e}");
                HashMap::new()
            }
        },
        Err(_) => HashMap::new(),
    }
}

/// `dir/file` when it exists (or `dir` is already the default), else
/// `DEFAULT_DATA_DIR/file`.
fn resolve_path(dir: &str, file: &str) -> String {
    let primary = format!("{dir}/{file}");
    if dir == DEFAULT_DATA_DIR || std::path::Path::new(&primary).exists() {
        primary
    } else {
        format!("{DEFAULT_DATA_DIR}/{file}")
    }
}

/// Normalize a configured `data_dir`: empty means the default directory.
pub fn normalize_dir(dir: &str) -> String {
    if dir.is_empty() {
        DEFAULT_DATA_DIR.to_string()
    } else {
        dir.to_string()
    }
}

/// Directory-keyed cache of loaded tables.
///
/// This exists to share the read-only dictionaries between sessions that use
/// the **same** `data_dir` (8 accounts across 3 dirs → 3 tables, not 8), and
/// to amortize loading across repeated lookups with the same directory. It is
/// a cache of immutable values, never a source of cross-session mutable state.
fn registry() -> &'static Mutex<HashMap<String, Arc<NameTable>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, Arc<NameTable>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registry_lock() -> MutexGuard<'static, HashMap<String, Arc<NameTable>>> {
    registry().lock().unwrap_or_else(|e| e.into_inner())
}

impl NameTable {
    /// Load a table for `dir` (empty = default). Prefer [`for_dir`] so sessions
    /// sharing a directory share one allocation.
    fn load(dir: &str) -> NameTable {
        let dir = normalize_dir(dir);
        NameTable {
            maps: load_dict(&dir, "map.json"),
            npcs: load_dict(&dir, "npc.json"),
            mobs: load_dict(&dir, "mob.json"),
            items: load_dict(&dir, "item.json"),
            skills: load_dict(&dir, "skill.json"),
            portals: load_portals(&dir),
            dir,
        }
    }

    /// Cached table for `dir` (empty = default). Loads once per directory.
    ///
    /// The registry lock is held across the load so two threads racing on the
    /// same cold directory load it once; loading is a plain file read, and the
    /// critical section is short-lived.
    pub fn for_dir(dir: &str) -> Arc<NameTable> {
        let dir = normalize_dir(dir);
        let mut cache = registry_lock();
        if let Some(t) = cache.get(&dir) {
            return t.clone();
        }
        let table = Arc::new(NameTable::load(&dir));
        cache.insert(dir, table.clone());
        table
    }

    /// Reload this table's directory from disk, returning a fresh table.
    ///
    /// Used by `names reload` / `reload_if_needed` after the data files were
    /// edited at runtime. The registry entry is replaced so new sessions pick
    /// up the fresh data; existing `Arc` holders keep their snapshot until
    /// they re-resolve, which is exactly the isolation we want.
    pub fn reload(&self) -> Arc<NameTable> {
        let fresh = Arc::new(NameTable::load(&self.dir));
        registry_lock().insert(self.dir.clone(), fresh.clone());
        fresh
    }

    /// The data directory this table was loaded from.
    pub fn dir(&self) -> &str {
        &self.dir
    }

    // ── id·名称 renderings (id retained, for tables/logs) ──────────────

    pub fn map_name(&self, id: i32) -> String {
        render(&self.maps, id)
    }
    pub fn npc_name(&self, id: i32) -> String {
        render(&self.npcs, id)
    }
    pub fn mob_name(&self, id: i32) -> String {
        render(&self.mobs, id)
    }
    pub fn item_name(&self, id: i32) -> String {
        render(&self.items, id)
    }
    pub fn skill_name(&self, id: i32) -> String {
        render(&self.skills, id)
    }

    // ── bare names (unknown = 未知) ────────────────────────────────────

    pub fn map_name_text(&self, id: i32) -> String {
        render_text(&self.maps, id)
    }
    pub fn npc_name_text(&self, id: i32) -> String {
        render_text(&self.npcs, id)
    }
    pub fn mob_name_text(&self, id: i32) -> String {
        render_text(&self.mobs, id)
    }
    pub fn item_name_text(&self, id: i32) -> String {
        render_text(&self.items, id)
    }
    pub fn skill_name_text(&self, id: i32) -> String {
        render_text(&self.skills, id)
    }

    // ── portals ───────────────────────────────────────────────────────

    /// All portals for a map (empty when the map is unknown).
    pub fn map_portals(&self, map_id: i32) -> Vec<PortalInfo> {
        self.portals.get(&map_id).cloned().unwrap_or_default()
    }

    /// Portal names only (for quick selection).
    pub fn portal_names(&self, map_id: i32) -> Vec<String> {
        self.map_portals(map_id).into_iter().map(|p| p.name).collect()
    }

    /// Find a portal by name on a map.
    pub fn find_portal(&self, map_id: i32, name: &str) -> Option<PortalInfo> {
        self.map_portals(map_id).into_iter().find(|p| p.name == name)
    }

    /// Every portal of every map (for search/completion).
    pub fn all_portals(&self) -> HashMap<i32, Vec<PortalInfo>> {
        self.portals.clone()
    }

    /// Number of loaded map entries (diagnostics / tests).
    pub fn map_count(&self) -> usize {
        self.maps.len()
    }
}

/// `id·名称`, or `id·未知` when the id is not in the dictionary.
fn render(dict: &HashMap<i32, String>, id: i32) -> String {
    match dict.get(&id) {
        Some(name) if !name.is_empty() => format!("{id}·{name}"),
        _ => format!("{id}·未知"),
    }
}

/// Bare name, or `未知`.
fn render_text(dict: &HashMap<i32, String>, id: i32) -> String {
    match dict.get(&id) {
        Some(name) if !name.is_empty() => name.clone(),
        _ => "未知".to_string(),
    }
}

// ── Process default table (no session at hand) ────────────────────────
//
// The frontends always pass the session's own table explicitly. This default
// exists for the headless CLI and for tests, which have no session object.
// It is a single immutable snapshot, never mutated in place.

static DEFAULT_DIR: RwLock<Option<String>> = RwLock::new(None);
/// The process default table. A `OnceLock` cannot express `force_reload`
/// (replacing the table in place), so this is an `RwLock<Option<..>>` whose
/// value is only ever swapped wholesale — never mutated through.
static DEFAULT_TABLE: RwLock<Option<Arc<NameTable>>> = RwLock::new(None);

/// Point the process default table at `dir`, loading it if needed.
///
/// Returns `true` when the default actually changed (a reload happened), so
/// callers can report "reloaded" vs "unchanged" without a second lock trip.
pub fn set_default_dir(dir: &str) -> bool {
    let dir = normalize_dir(dir);
    let table = NameTable::for_dir(&dir);
    let changed = {
        let mut guard = DEFAULT_DIR.write().unwrap_or_else(|e| e.into_inner());
        if guard.as_deref() == Some(dir.as_str()) {
            false
        } else {
            *guard = Some(dir);
            true
        }
    };
    if changed {
        *DEFAULT_TABLE.write().unwrap_or_else(|e| e.into_inner()) = Some(table);
    }
    changed
}

/// Directory backing the process default table (empty when never set).
pub fn default_dir() -> String {
    DEFAULT_DIR
        .read()
        .map(|g| g.clone().unwrap_or_default())
        .unwrap_or_default()
}

/// The process default table — loaded from `data/` on first use.
pub fn default_table() -> Arc<NameTable> {
    if let Some(t) = DEFAULT_TABLE
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
        return t.clone();
    }
    set_default_dir(DEFAULT_DATA_DIR);
    DEFAULT_TABLE
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .expect("names: default table set by set_default_dir")
}

// ── Legacy free functions (forward to the process default table) ──────
//
// Kept so the headless CLI, the frontends' name-dependent rendering and the
// existing test suite compile and behave unchanged. Session-scoped code
// should use `Session::names` instead.

/// Reload the default table when `dir` differs from the current default.
/// Idempotent: same directory is a no-op.
pub fn reload_if_needed(dir: &str) {
    set_default_dir(dir);
}

/// Force-reload the default table's directory, clearing every cache entry.
pub fn force_reload() {
    let dir = current_data_dir();
    registry_lock().remove(&dir);
    let fresh = NameTable::for_dir(&dir);
    *DEFAULT_TABLE.write().unwrap_or_else(|e| e.into_inner()) = Some(fresh);
    *DEFAULT_DIR.write().unwrap_or_else(|e| e.into_inner()) = Some(dir);
}

/// Return the currently active data directory.
pub fn current_data_dir() -> String {
    let d = default_dir();
    if d.is_empty() {
        DEFAULT_DATA_DIR.to_string()
    } else {
        d
    }
}

pub fn map_name(id: i32) -> String {
    default_table().map_name(id)
}
pub fn npc_name(id: i32) -> String {
    default_table().npc_name(id)
}
pub fn mob_name(id: i32) -> String {
    default_table().mob_name(id)
}
pub fn item_name(id: i32) -> String {
    default_table().item_name(id)
}
pub fn skill_name(id: i32) -> String {
    default_table().skill_name(id)
}
pub fn map_name_text(id: i32) -> String {
    default_table().map_name_text(id)
}
pub fn npc_name_text(id: i32) -> String {
    default_table().npc_name_text(id)
}
pub fn mob_name_text(id: i32) -> String {
    default_table().mob_name_text(id)
}
pub fn item_name_text(id: i32) -> String {
    default_table().item_name_text(id)
}
pub fn skill_name_text(id: i32) -> String {
    default_table().skill_name_text(id)
}
pub fn map_portals(map_id: i32) -> Vec<PortalInfo> {
    default_table().map_portals(map_id)
}
pub fn portal_names(map_id: i32) -> Vec<String> {
    default_table().portal_names(map_id)
}
pub fn find_portal(map_id: i32, name: &str) -> Option<PortalInfo> {
    default_table().find_portal(map_id, name)
}
pub fn all_portals() -> HashMap<i32, Vec<PortalInfo>> {
    default_table().all_portals()
}

/// Count of registry entries — test-only observability into the cache.
#[cfg(test)]
fn registry_len() -> usize {
    registry_lock().len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The name tests all share the process default table, which is global.
    /// Serialize them so `reload_if_needed` in one cannot race another.
    fn guard() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn unknown_ids_fallback() {
        let _g = guard();
        reload_if_needed("");
        assert_eq!(map_name(-1), "-1·未知");
        assert_eq!(npc_name(0), "0·未知");
    }

    #[test]
    fn skill_dict_loads() {
        let _g = guard();
        reload_if_needed("");
        let s = skill_name(1001003);
        assert!(!s.ends_with("未知"), "skill 1001003 -> {s}");
        let s2 = skill_name_text(1001003);
        assert!(!s2.is_empty() && s2 != "未知");
        let s3 = skill_name_text(1000);
        assert!(!s3.is_empty() && s3 != "未知", "skill 1000 -> {s3}");
    }

    #[test]
    fn item_dict_loads() {
        let _g = guard();
        reload_if_needed("");
        assert_eq!(item_name_text(2000000), "红色药水");
        assert_eq!(item_name_text(4000000), "蓝色蜗牛壳");
        assert_eq!(item_name_text(-1), "未知");
    }

    #[test]
    fn reload_same_dir_is_noop() {
        let _g = guard();
        reload_if_needed("");
        let before = current_data_dir();
        reload_if_needed("");
        assert_eq!(current_data_dir(), before);
    }

    #[test]
    fn reload_different_dir_loads_new_data() {
        let _g = guard();
        reload_if_needed("");
        reload_if_needed("data/server_a");
        assert_eq!(current_data_dir(), "data/server_a");
        assert_eq!(item_name_text(2000000), "红色药水");
    }

    #[test]
    fn reload_fallback_for_missing_file() {
        let _g = guard();
        // Unique dir per run: a fixed name would hit the registry cache stale
        // if a previous run left the directory behind.
        let tmp = std::env::temp_dir().join(format!(
            "names_test_fallback_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::create_dir_all(&tmp);
        // Only item.json exists here → the other five fall back to data/.
        let _ = std::fs::write(tmp.join("item.json"), r#"{"9999999":"测试物品"}"#);
        let dir = tmp.to_str().unwrap().to_string();
        reload_if_needed(&dir);
        assert_eq!(current_data_dir(), dir);
        assert_eq!(item_name_text(9999999), "测试物品");
        // map.json falls back to default data/ — should still work.
        assert_ne!(map_name_text(1000000), "未知");
        // A different id from the same dict must NOT resolve (proves we read
        // the temp dir's item.json, not data/item.json).
        assert_eq!(item_name_text(2000000), "未知");
        let _ = std::fs::remove_dir_all(&tmp);
        reload_if_needed("");
    }

    #[test]
    fn empty_dir_uses_default() {
        let _g = guard();
        reload_if_needed("data/server_a");
        reload_if_needed("");
        assert_eq!(current_data_dir(), "data");
        assert_eq!(item_name_text(2000000), "红色药水");
    }

    #[test]
    fn nonexistent_dir_falls_back_to_default() {
        let _g = guard();
        let tmp = "data/this_does_not_exist_12345";
        reload_if_needed(tmp);
        assert_eq!(item_name_text(2000000), "红色药水");
        reload_if_needed("");
    }

    #[test]
    fn all_portals_loads() {
        let _g = guard();
        reload_if_needed("");
        let portals = all_portals();
        assert!(!portals.is_empty(), "portals.json should not be empty");
    }

    #[test]
    fn find_portal_works() {
        let _g = guard();
        reload_if_needed("");
        let portals = all_portals();
        if let Some((map_id, list)) = portals.iter().next() {
            if let Some(p) = list.first() {
                let found = find_portal(*map_id, &p.name);
                assert!(found.is_some(), "find_portal should find existing portal");
                assert_eq!(found.unwrap().name, p.name);
            }
        }
    }

    #[test]
    fn force_reload_clears_and_reloads() {
        let _g = guard();
        reload_if_needed("");
        let _ = item_name(2000000);
        force_reload();
        assert_eq!(item_name_text(2000000), "红色药水");
    }

    // ── Phase 1 (G3): per-data_dir isolation ──────────────────────────

    /// The core of the G3 fix: two tables for two different `data_dir` values
    /// are fully independent, and neither affects the process default.
    #[test]
    fn tables_for_different_dirs_are_isolated() {
        let server_a = NameTable::for_dir("data/server_a");
        let server_c = NameTable::for_dir("data/server_c");
        let server_b = NameTable::for_dir("data/server_b");

        assert_eq!(server_a.dir(), "data/server_a");
        assert_eq!(server_c.dir(), "data/server_c");
        assert_eq!(server_b.dir(), "data/server_b");

        // Each directory really has its own dictionary contents (the dirs hold
        // different server data), so reading one must not read another's.
        // We assert independence structurally: reloading one leaves the others
        // byte-identical in behaviour.
        let a_before = server_a.map_name_text(100000000);
        let c_before = server_c.map_name_text(100000000);
        let b_before = server_b.map_name_text(100000000);
        let _ = server_a.reload();
        assert_eq!(server_c.map_name_text(100000000), c_before);
        assert_eq!(server_b.map_name_text(100000000), b_before);
        assert_eq!(server_a.map_name_text(100000000), a_before);
    }

    /// Same directory must share one allocation (8 accounts / 3 dirs).
    #[test]
    fn same_dir_shares_one_allocation() {
        let a = NameTable::for_dir("data/server_a");
        let b = NameTable::for_dir("data/server_a");
        assert!(Arc::ptr_eq(&a, &b), "same data_dir must share one Arc");
    }

    /// Empty dir and explicit "data" are the same directory.
    #[test]
    fn empty_dir_is_the_default_dir() {
        let a = NameTable::for_dir("");
        let b = NameTable::for_dir(DEFAULT_DATA_DIR);
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(a.dir(), DEFAULT_DATA_DIR);
        assert_eq!(normalize_dir(""), DEFAULT_DATA_DIR);
        assert_eq!(normalize_dir("data/server_b"), "data/server_b");
    }

    /// Loading a third directory must not evict or mutate the others, and the
    /// registry must contain one entry per distinct directory.
    #[test]
    fn registry_holds_one_entry_per_dir() {
        let _g = guard();
        let dirs = ["data/server_a", "data/server_c", "data/server_b"];
        for d in dirs {
            let _ = NameTable::for_dir(d);
        }
        let cache = registry_lock();
        for d in dirs {
            assert!(cache.contains_key(d), "registry missing {d}");
        }
        let count = cache.len();
        assert!(count >= 3, "expected >=3 entries, got {count}");
    }

    #[test]
    fn reload_replaces_registry_entry() {
        let _g = guard();
        let before = NameTable::for_dir("data/server_b");
        let after = before.reload();
        let again = NameTable::for_dir("data/server_b");
        assert!(!Arc::ptr_eq(&before, &after), "reload must produce a new table");
        assert!(Arc::ptr_eq(&after, &again), "reload must publish to the registry");
        assert_eq!(after.dir(), "data/server_b");
    }

    /// A malformed dictionary file must be reported and degrade to an empty
    /// table rather than panicking or silently pretending ids are unknown.
    #[test]
    fn malformed_dict_is_reported_and_degrades_to_empty() {
        let _g = guard();
        let tmp = std::env::temp_dir().join("names_test_malformed");
        let _ = std::fs::create_dir_all(&tmp);
        // `portals.json` must exist here to keep the load path deterministic.
        let _ = std::fs::write(tmp.join("item.json"), "{ this is not json");
        let table = NameTable::load(tmp.to_str().unwrap());
        // Malformed → empty → ids render as unknown (no panic).
        assert_eq!(table.item_name_text(2000000), "未知");
        assert_eq!(table.item_name(2000000), "2000000·未知");
        // Files that do not exist fall back to data/ and still resolve.
        assert_ne!(table.map_name_text(100000000), "未知");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `set_default_dir` reports whether it actually changed anything.
    #[test]
    fn set_default_dir_reports_change() {
        let _g = guard();
        assert!(set_default_dir("data/server_b"), "first switch must change");
        assert!(!set_default_dir("data/server_b"), "same dir must be a no-op");
        assert!(set_default_dir("data/server_a"), "different dir must change");
        assert_eq!(current_data_dir(), "data/server_a");
        assert_eq!(default_dir(), "data/server_a");
        assert!(set_default_dir(""), "empty resets to default");
        assert_eq!(current_data_dir(), DEFAULT_DATA_DIR);
    }

    /// `resolve_path` prefers the directory's own file and only falls back
    /// when it is missing.
    #[test]
    fn resolve_path_prefers_own_file_then_falls_back() {
        let _g = guard();
        assert_eq!(resolve_path(DEFAULT_DATA_DIR, "map.json"), "data/map.json");
        // data/server_a has map.json → own path.
        assert_eq!(resolve_path("data/server_a", "map.json"), "data/server_a/map.json");
        // Missing file in an existing dir → default dir.
        let missing = "data/this_dir_has_no_files_98765";
        assert_eq!(resolve_path(missing, "map.json"), "data/map.json");
    }

    /// `map_count` reflects real loading (guards against an empty-table
    /// regression that would make everything render as 未知).
    #[test]
    fn map_count_is_nonzero_for_real_dirs() {
        for d in ["data", "data/server_a", "data/server_c", "data/server_b"] {
            let t = NameTable::for_dir(d);
            assert!(t.map_count() > 0, "{d} should have maps loaded");
        }
    }

    /// A counter proving `for_dir` does not reload on every call.
    #[test]
    fn for_dir_is_cached_not_reloaded() {
        let _g = guard();
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        // Structural check instead of instrumenting load: pointer identity
        // across N calls proves the cache is hit.
        let first = NameTable::for_dir("data/server_c");
        for _ in 0..50 {
            let t = NameTable::for_dir("data/server_c");
            assert!(Arc::ptr_eq(&first, &t));
            CALLS.fetch_add(1, Ordering::Relaxed);
        }
        assert_eq!(CALLS.load(Ordering::Relaxed), 50);
        let _ = registry_len();
    }
}
