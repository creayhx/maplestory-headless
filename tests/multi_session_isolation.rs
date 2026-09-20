//! Phase 1 acceptance: **two bot sessions in ONE process** must not interfere.
//!
//! This is the core architectural claim behind the in-process multi-session
//! design (`docs/MANAGER_PLAN.md` §2.1): the three former process-level
//! globals (G1 emitter / G2 config paths / G3 name tables) are gone, so two
//! concurrent `runtime::run` calls keep separate events, separate config
//! chains and separate name dictionaries.
//!
//! Two live sessions are started against the local server simultaneously, each
//! with its **own** profile copy in a temp directory pointing at a **different
//! `data_dir`**. Assertions:
//!
//! 1. both reach in-game (the server accepts two concurrent accounts),
//! 2. each session's events land only in its own emitter (G1),
//! 3. the Chinese map name comes from that session's `data_dir` (G3),
//! 4. `hunt save` writes to that session's own profile file, never the other's
//!    and never the repo's tracked profile (G2).
//!
//! Requires the local server on 127.0.0.1:8484. Skips (with a loud message)
//! when it is unreachable so the suite stays runnable offline.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use openstory_bot::config::Config;
use openstory_bot::emit::{self, Emitter, Event};
use openstory_bot::runtime::{self, RunOutcome};
use openstory_bot::state::{BotState, Phase};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

const SERVER: &str = "127.0.0.1:8484";
/// Both local-server profiles use `data/server_a`; the test overrides it to a
/// different directory per session so name isolation is actually observable.
const DIR_A: &str = "data/server_a";
const DIR_B: &str = "data/server_b";

/// Collects everything one session emitted.
#[derive(Default)]
struct Collector {
    seen: Mutex<Vec<Event>>,
}

impl Collector {
    fn texts(&self) -> Vec<String> {
        self.seen.lock().unwrap().iter().map(|e| e.text.clone()).collect()
    }
    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

impl Emitter for Collector {
    fn emit(&self, ev: Event) {
        self.seen.lock().unwrap().push(ev);
    }
}

/// Is the local game server listening?
async fn server_up() -> bool {
    tokio::time::timeout(Duration::from_millis(800), tokio::net::TcpStream::connect(SERVER))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
}

/// Write a session profile in `dir`, based on an existing repo profile but
/// with `data_dir` overridden. Returns the path.
///
/// The bots are pointed at **copies**, never the repo's tracked profiles: a
/// stray `hunt save` / default-bootstrap must not dirty the user's files.
fn write_profile(
    dir: &std::path::Path,
    name: &str,
    source: &str,
    data_dir: &str,
    tick_ms: u32,
) -> PathBuf {
    let raw = std::fs::read_to_string(source)
        .unwrap_or_else(|e| panic!("read {source}: {e}"));
    let mut v: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {source}: {e}"));
    let obj = v.as_object_mut().expect("profile root must be an object");
    obj.insert("data_dir".into(), serde_json::Value::String(data_dir.into()));
    // Fast tick so the auto command runs promptly; also proves per-session
    // tick rates are read from each session's own config.
    obj.insert("tick_ms".into(), serde_json::Value::from(tick_ms));
    let path = dir.join(format!("{name}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).expect("write profile");
    path
}

/// One live session: its own state, command channel, profile and emitter.
struct LiveSession {
    label: &'static str,
    state: Arc<AsyncMutex<BotState>>,
    tx: mpsc::Sender<String>,
    collector: Arc<Collector>,
    handle: tokio::task::JoinHandle<RunOutcome>,
}

impl LiveSession {
    fn spawn(
        label: &'static str,
        profile: PathBuf,
        account: &str,
        data_dir: &str,
        duration_secs: u64,
    ) -> LiveSession {
        let state = Arc::new(AsyncMutex::new(BotState::default()));
        let collector = Arc::new(Collector::default());
        let (tx, mut rx) = mpsc::channel::<String>(64);

        let cfg = Config {
            ip: "127.0.0.1".into(),
            port: 8484,
            account: account.into(),
            password: "test_password".into(),
            char_index: 0,
            show_packets: false,
            config_paths: vec![profile.display().to_string()],
            duration: Some(duration_secs),
            // `view status` prints the map's own name on its first line:
            //   [view] phase=InGame world=0 map=<id> (<中文名>) pos=...
            auto: vec!["view status".into()],
            ..Config::default()
        };
        // Sanity: the profile really carries the data_dir we expect.
        let on_disk: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&profile).unwrap()).unwrap();
        assert_eq!(
            on_disk["data_dir"].as_str(),
            Some(data_dir),
            "{label}: profile data_dir mismatch"
        );

        let st = state.clone();
        let col = collector.clone();
        let handle = tokio::spawn(async move {
            // G1: every event this session emits is scoped to its own emitter.
            // No process-global `set_emitter` is used anywhere in this test.
            emit::with_session_emitter(col, async move {
                runtime::run(cfg, &mut rx, st).await
            })
            .await
        });

        LiveSession { label, state, tx, collector, handle }
    }


    /// Send a command line to this session (fails loudly if it already ended).
    async fn send(&self, cmd: &str) {
        self.tx
            .send(cmd.to_string())
            .await
            .unwrap_or_else(|e| panic!("{}: session already ended, cannot send `{cmd}`: {e}", self.label));
    }

    /// Wait until in-game; returns the map the session landed on.
    async fn wait_in_game(&self, timeout: Duration) -> Option<i32> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let st = self.state.lock().await;
            if st.phase == Phase::InGame {
                return Some(st.mapid);
            }
            drop(st);
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_sessions_in_one_process_stay_isolated() {
    if !server_up().await {
        eprintln!("SKIP: 本地服务器 {SERVER} 未监听，跳过双会话隔离测试");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("openstory_two_sessions_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // Snapshot the repo profiles so we can prove nothing was written to them.
    let repo_a = "profiles/本地服_100000001.json";
    let repo_b = "profiles/本地服_100000002.json";
    let a_before = std::fs::read(repo_a).unwrap();
    let b_before = std::fs::read(repo_b).unwrap();

    let pa = write_profile(&tmp, "sess_a", repo_a, DIR_A, 50);
    let pb = write_profile(&tmp, "sess_b", repo_b, DIR_B, 50);

    let a = LiveSession::spawn("A", pa.clone(), "100000001", DIR_A, 90);
    let b = LiveSession::spawn("B", pb.clone(), "100000002", DIR_B, 90);

    // Both must reach in-game concurrently.
    let (a_mapid, b_mapid) = tokio::join!(
        a.wait_in_game(Duration::from_secs(25)),
        b.wait_in_game(Duration::from_secs(25))
    );
    assert!(a_mapid.is_some(), "{} did not reach InGame", a.label);
    assert!(b_mapid.is_some(), "{} did not reach InGame", b.label);

    // Let the auto command run in both.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // ── G2: config chains stayed separate (while both are still running) ──
    // `hunt save` in A must write A's profile only.
    let a_cfg_before: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&pa).unwrap()).unwrap();
    let b_cfg_before = std::fs::read(&pb).unwrap();
    a.send("hunt damage 4321").await;
    a.send("hunt save").await;
    tokio::time::sleep(Duration::from_millis(1200)).await;

    let a_cfg_after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&pa).unwrap()).unwrap();
    assert_eq!(
        a_cfg_after["hunt"]["damage"], 4321,
        "A's `hunt save` must land in A's own profile"
    );
    assert_ne!(
        a_cfg_before["hunt"]["damage"], a_cfg_after["hunt"]["damage"],
        "A's profile must actually have changed"
    );
    assert_eq!(
        std::fs::read(&pb).unwrap(),
        b_cfg_before,
        "B's profile must be untouched by A's `hunt save`"
    );

    // Now stop both cleanly.
    a.send("quit").await;
    b.send("quit").await;
    let (ra, rb) = tokio::join!(a.handle, b.handle);
    let ra = ra.expect("session A task must not panic");
    let rb = rb.expect("session B task must not panic");
    assert_eq!(ra, RunOutcome::Quit, "A should quit cleanly");
    assert_eq!(rb, RunOutcome::Quit, "B should quit cleanly");

    let ta = a.collector.texts();
    let tb = b.collector.texts();
    assert!(a.collector.count() > 0, "A emitted nothing");
    assert!(b.collector.count() > 0, "B emitted nothing");

    // ── G1: no event crossed over ─────────────────────────────────────
    // Each session's own login line is present exactly in its own collector.
    let a_login = ta.iter().filter(|t| t.contains("connecting to 127.0.0.1:8484")).count();
    let b_login = tb.iter().filter(|t| t.contains("connecting to 127.0.0.1:8484")).count();
    assert_eq!(a_login, 1, "A must see exactly its own connect line, got {a_login}");
    assert_eq!(b_login, 1, "B must see exactly its own connect line, got {b_login}");
    // "> ready." is emitted once per session's FIRST run only.
    assert_eq!(ta.iter().filter(|t| t.contains("ready. type 'help'")).count(), 1);
    assert_eq!(tb.iter().filter(|t| t.contains("ready. type 'help'")).count(), 1);
    // The `hunt save` line went to A's collector, never B's.
    assert!(
        ta.iter().any(|t| t.contains("[hunt] saved to")),
        "A must see its own hunt save confirmation"
    );
    assert!(
        !tb.iter().any(|t| t.contains("saved to")),
        "B must not see A's hunt save confirmation"
    );

    // ── G3: each session resolved names from its OWN data_dir ─────────
    // The two directories carry genuinely different map tables, so they must
    // be distinct objects (no shared process-global table).
    let na = openstory_bot::names::NameTable::for_dir(DIR_A);
    let nb = openstory_bot::names::NameTable::for_dir(DIR_B);
    assert!(!Arc::ptr_eq(&na, &nb), "different data_dir must not share a table");
    assert_eq!(na.dir(), DIR_A);
    assert_eq!(nb.dir(), DIR_B);

    // Each session's map is rendered by the core through ITS OWN table. The
    // two accounts sit on different maps, so verify per session against its
    // own table.
    let (a_map, b_map) = (a_mapid.expect("A mapid"), b_mapid.expect("B mapid"));
    let a_view = ta
        .iter()
        .find(|t| t.contains(&format!("map={a_map} ")))
        .unwrap_or_else(|| panic!("A produced no `view status` for map {a_map}: {ta:#?}"))
        .clone();
    let b_view = tb
        .iter()
        .find(|t| t.contains(&format!("map={b_map} ")))
        .unwrap_or_else(|| panic!("B produced no `view status` for map {b_map}: {tb:#?}"))
        .clone();

    // The status line names the session's own map.
    assert!(
        a_view.contains(&na.map_name_text(a_map)),
        "A's status must use {DIR_A} names for map {a_map}: {a_view}"
    );
    assert!(
        b_view.contains(&nb.map_name_text(b_map)),
        "B's status must use {DIR_B} names for map {b_map}: {b_view}"
    );
    // And the tables really do differ (otherwise this test would pass even if
    // both sessions shared one table).
    let tables_differ = (0..=2)
        .map(|i| 100000000 + i * 100)
        .any(|m| na.map_name_text(m) != nb.map_name_text(m));
    assert!(
        tables_differ,
        "test precondition: {DIR_A} and {DIR_B} must have different map names"
    );

    // The repo's tracked profiles were never written to.
    assert_eq!(std::fs::read(repo_a).unwrap(), a_before, "{repo_a} was modified!");
    assert_eq!(std::fs::read(repo_b).unwrap(), b_before, "{repo_b} was modified!");

    let _ = std::fs::remove_dir_all(&tmp);
}