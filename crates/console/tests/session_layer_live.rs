//! 阶段 4 验收: 用**会话层** (`session.rs` / `sessions.rs`) 跑真实账号。
//!
//! 与 `tests/multi_session_isolation.rs` 的分工:
//! - 那个测**核心层**隔离 (两个 `runtime::run` 不串台)
//! - 这个测**UI 面向的会话层**: 两个会话同时进游戏, 日志各进各的 `LogBuf`;
//!   `@all` / `@<前缀>` 精确命中; 单会话路径与直接 `runtime::run` 等价
//!
//! 需要本地服 127.0.0.1:8484; 连不上时打印 SKIP 并返回。

//! 模块直接来自 `openstory-console` 的 lib crate —— 与两个可执行入口编译的是
//! 同一份实现 (见 `render_regression.rs` 顶部关于 `#[path]` 的说明)。

use openstory_console::{session, sessions, watcher};

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use session::{SessionSpec, STAGE_RUNNING};
use sessions::{SessionSet, Target};

use openstory_bot::config::Config;
use openstory_bot::runtime::RunOutcome;
use openstory_bot::state::Phase;

const SERVER: &str = "127.0.0.1:8484";

/// 联调测试串行闸门。
///
/// 本地游戏服对同时登录的账号数有限 (3 个并发连接就会让其中一个卡在登录阶段),
/// 而 `cargo test` 默认多线程跑同一二进制里的测试。两个联调测试并行时会随机
/// 超时 —— 那是环境限制而不是代码问题, 所以用一把进程内锁把它们串起来,
/// 而不是放宽超时 (放宽会把真实的回归一起掩盖掉)。
fn live_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

async fn server_up() -> bool {
    tokio::time::timeout(
        Duration::from_millis(800),
        tokio::net::TcpStream::connect(SERVER),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false)
}

fn workspace_root() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/console -> workspace root")
        .to_path_buf()
}

/// 在临时目录写一份档案 (只改 data_dir / tick_ms), 不动仓库里的档案。
fn write_profile(dir: &std::path::Path, name: &str, source: &PathBuf, data_dir: &str) -> PathBuf {
    let raw = std::fs::read_to_string(source).unwrap_or_else(|e| panic!("read {source:?}: {e}"));
    let mut v: serde_json::Value = serde_json::from_str(&raw).expect("parse profile");
    let obj = v.as_object_mut().expect("object");
    obj.insert(
        "data_dir".into(),
        serde_json::Value::String(data_dir.into()),
    );
    obj.insert("tick_ms".into(), serde_json::Value::from(50));
    let p = dir.join(format!("{name}.json"));
    std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    p
}

fn cfg_for(profile: &std::path::Path, account: &str) -> Config {
    Config {
        ip: "127.0.0.1".into(),
        port: 8484,
        account: account.into(),
        password: "test_password".into(),
        char_index: 0,
        show_packets: false,
        config_paths: vec![profile.display().to_string()],
        duration: Some(90),
        auto: vec!["view status".into()],
        ..Config::default()
    }
}

/// 等全部指定会话进图。
async fn wait_all_in_game(set: &SessionSet, idxs: &[usize], secs: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let all = idxs.iter().all(|&i| {
            set.iter()
                .nth(i)
                .and_then(|s| s.state.try_lock().ok().map(|st| st.phase == Phase::InGame))
                .unwrap_or(false)
        });
        if all {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

fn log_lines(set: &mut SessionSet, i: usize) -> usize {
    set.iter_mut()
        .nth(i)
        .map(|s| s.log.total_display_lines(None, 200))
        .unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_layer_runs_two_accounts_independently() {
    if !server_up().await {
        eprintln!("SKIP: 本地服务器 {SERVER} 未监听, 跳过会话层验收");
        return;
    }
    let _serial = live_guard();
    let root = workspace_root();
    // 档案里的 data_dir 与 `data/` 名字表都是相对仓库根的路径, 而 cargo 跑
    // 成员 crate 的测试时工作目录是 crate 目录 —— 必须先切回仓库根。
    std::env::set_current_dir(&root).expect("chdir to workspace root");
    let tmp = std::env::temp_dir().join(format!("openstory_sess_layer_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // 两个不同前缀的档案名 -> 既能测 @all 也能测 @前缀
    let p1 = write_profile(
        &tmp,
        "serverA_1",
        &root.join("profiles/本地服_100000001.json"),
        "data/server_a",
    );
    let p2 = write_profile(
        &tmp,
        "serverB_1",
        &root.join("profiles/本地服_100000002.json"),
        "data/server_b",
    );

    let specs = vec![SessionSpec::new(p1.clone()), SessionSpec::new(p2.clone())];
    assert_eq!(specs[0].name, "serverA_1");
    assert_eq!(specs[1].name, "serverB_1");

    let mut set = SessionSet::from_specs(&specs);
    assert_eq!(set.len(), 2);
    assert_eq!(set.profile_names(), vec!["serverA_1", "serverB_1"]);

    let cfgs = vec![cfg_for(&p1, "100000001"), cfg_for(&p2, "100000002")];
    let arcs: Vec<_> = set.iter().map(|s| s.arcs()).collect();
    let rxs = set.pull_receivers();
    let mut handles = Vec::new();
    for (i, (cfg, rx)) in cfgs.into_iter().zip(rxs).enumerate() {
        handles.push(sessions::spawn_one(
            &tokio::runtime::Handle::current(),
            cfg,
            rx,
            arcs[i].clone(),
        ));
    }
    assert_eq!(handles.len(), 2);

    assert!(
        wait_all_in_game(&set, &[0, 1], 30).await,
        "30s 内两个会话未全部进图"
    );
    assert_eq!(set.running(), vec![0, 1], "两个会话都应在运行中");
    tokio::time::sleep(Duration::from_secs(2)).await;

    // ── 1. 日志按会话分离 ─────────────────────────────────────────
    set.drain_all();
    let counts = [log_lines(&mut set, 0), log_lines(&mut set, 1)];
    assert!(counts[0] > 0, "会话 0 的日志为空");
    assert!(counts[1] > 0, "会话 1 的日志为空");

    // ── 2. @all 命中两个, 两边都真的收到 ───────────────────────────
    let before = [log_lines(&mut set, 0), log_lines(&mut set, 1)];
    let (hit, ok, errs) = set.dispatch_line("@all hunt status").unwrap();
    assert_eq!((hit, ok), (2, 2), "errs={errs:?}");
    assert!(errs.is_empty(), "errs={errs:?}");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    set.drain_all();
    for i in 0..2 {
        assert!(
            log_lines(&mut set, i) > before[i],
            "会话 {i} 日志没增长 (@all 没送到?): {before:?} -> {:?}",
            [log_lines(&mut set, 0), log_lines(&mut set, 1)]
        );
    }

    // ── 3. @前缀 只命中匹配的档案 ─────────────────────────────────
    assert_eq!(set.resolve(&Target::Prefix("serverA".into())), vec![0]);
    assert_eq!(set.resolve(&Target::Prefix("serverB".into())), vec![1]);
    let before = [log_lines(&mut set, 0), log_lines(&mut set, 1)];
    let (hit, ok, _) = set.dispatch_line("@serverA view status").unwrap();
    assert_eq!((hit, ok), (1, 1));
    tokio::time::sleep(Duration::from_millis(1500)).await;
    set.drain_all();
    assert!(log_lines(&mut set, 0) > before[0], "被命中的会话应收到指令");
    assert_eq!(
        log_lines(&mut set, 1),
        before[1],
        "未被命中的会话不该收到任何东西"
    );

    // ── 4. 未知前缀必须报错而不是误发 ──────────────────────────────
    let before = [log_lines(&mut set, 0), log_lines(&mut set, 1)];
    let err = set.dispatch_line("@nope hunt on").unwrap_err();
    assert!(err.contains("没有匹配"), "got {err}");
    tokio::time::sleep(Duration::from_millis(800)).await;
    set.drain_all();
    assert_eq!(
        [log_lines(&mut set, 0), log_lines(&mut set, 1)],
        before,
        "无命中时任何人都不该收到"
    );

    // ── 5. 干净收尾 ───────────────────────────────────────────────
    let (hit, ok, _) = set.dispatch_line("@all quit").unwrap();
    assert_eq!((hit, ok), (2, 2));
    for h in handles {
        let o = tokio::time::timeout(Duration::from_secs(10), h)
            .await
            .expect("join timeout")
            .expect("task panic");
        assert_eq!(o, RunOutcome::Quit, "应正常退出");
    }
    assert!(
        set.iter().all(|s| s.outcome().is_some()),
        "结束后 outcome 应有值"
    );
    assert!(
        !set.iter()
            .any(|s| s.stage.load(Ordering::SeqCst) == STAGE_RUNNING),
        "结束后不应还是运行中"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// 单会话等价性: 一个会话走会话层, 状态/日志/指令路径都要正常
/// (阶段 4 的硬性验收 "一档 == 今天的 TUI" 的可自动化部分)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_session_matches_direct_run() {
    if !server_up().await {
        eprintln!("SKIP: 本地服务器 {SERVER} 未监听, 跳过单会话等价性");
        return;
    }
    let _serial = live_guard();
    let root = workspace_root();
    std::env::set_current_dir(&root).expect("chdir to workspace root");
    // 用另一个账号: 两个测试在同一个二进制里并行跑, 同账号会被服务器顶掉
    // (同账号重复登录 -> 前一个连接被踢)。
    let profile = root.join("profiles").join("本地服_100000002.json");
    assert!(profile.exists(), "缺少档案 profiles/本地服_100000002.json");

    let spec = SessionSpec::new(profile.clone());
    let mut set = SessionSet::from_specs(std::slice::from_ref(&spec));
    let arcs: Vec<_> = set.iter().map(|s| s.arcs()).collect();
    let rxs = set.pull_receivers();
    let mut handles = Vec::new();
    for (i, (cfg, rx)) in vec![cfg_for(&profile, "100000002")]
        .into_iter()
        .zip(rxs)
        .enumerate()
    {
        handles.push(sessions::spawn_one(
            &tokio::runtime::Handle::current(),
            cfg,
            rx,
            arcs[i].clone(),
        ));
    }

    assert!(wait_all_in_game(&set, &[0], 25).await, "25s 内未进图");

    // 单会话下 dispatch_line("cmd") 必须发给当前选中会话
    set.drain_all();
    let before = log_lines(&mut set, 0);
    let (hit, ok, errs) = set.dispatch_line("view status").unwrap();
    assert_eq!((hit, ok), (1, 1));
    assert!(errs.is_empty());
    tokio::time::sleep(Duration::from_millis(1500)).await;
    set.drain_all();
    assert!(log_lines(&mut set, 0) > before, "指令输出应进入会话日志");

    // 快照字段可用 (阶段 5 的多会话 UI 直接读它)
    let snap = set.selected().unwrap().snapshot().await;
    assert_eq!(snap.phase, Phase::InGame);
    assert_ne!(snap.mapid, 0, "快照应有地图");
    assert!(!snap.name.is_empty(), "快照应有角色名");
    // 中文名也解析出来了 (阶段 1 的 G3 在会话层依然成立)
    let names = openstory_bot::names::NameTable::for_dir(&set.selected().unwrap().data_dir());
    assert_ne!(
        names.map_name_text(snap.mapid),
        "未知",
        "地图 {} 应能解析出中文名",
        snap.mapid
    );

    set.dispatch_line("quit").unwrap();
    let o = tokio::time::timeout(Duration::from_secs(10), handles.into_iter().next().unwrap())
        .await
        .expect("join timeout")
        .expect("task panic");
    assert_eq!(o, RunOutcome::Quit);
}

/// 阶段 6 端到端: 真实会话 → `--duration` 到点结束 → 退避到点 → 真的重新拉起。
///
/// # 为什么用 `--duration` 而不是"断开连接"
///
/// 想造一次**真的断线**，只有两条路：
/// 1. 杀服务器 —— 不可接受 (会打断同机上其它人的测试)。
/// 2. 让核心层把连接关掉 —— 而核心层没有"主动断开"这个命令。
///
/// 而这条测试要验的**不是**断线检测 (那是 `watcher` 单测的事)，是那条
/// **拉起链路**: 结束 → 登记 → 退避到点 → 真的重新 spawn 一个能进游戏的
/// 会话。用 `--duration` 到点可以完整走通这条链路，且完全不依赖服务器的
/// 任何行为。
///
/// **但有个细节必须处理**: `DurationElapsed` 属于"用户要它停"那一类，默认
/// **不**拉起。所以要显式把它改造成"值得拉起"的结局 —— 这里直接调
/// `note_exit` 之前先把 `outcome` 换成 `ConnectionClosed`，模拟"核心层判定
/// 为断线"的那一步。这样测的仍然是控制台层真实的决策 + 真实的重新拉起。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_finished_session_is_really_respawned_and_re_enters_the_game() {
    if !server_up().await {
        eprintln!("SKIP: 本地服务器 {SERVER} 未监听, 跳过拉起端到端");
        return;
    }
    let _serial = live_guard();
    let root = workspace_root();
    std::env::set_current_dir(&root).expect("chdir to workspace root");
    let profile = root.join("profiles").join("本地服_100000001.json");
    assert!(profile.exists(), "缺少档案 profiles/本地服_100000001.json");

    // 退避几乎为 0: 测试不该等 5 秒
    let policy = watcher::RestartPolicy {
        enabled: true,
        max_attempts: 3,
        backoff_secs: vec![0],
    };
    let spec = SessionSpec::new(profile.clone());
    let mut set = SessionSet::from_specs_with_policy(std::slice::from_ref(&spec), policy);
    let sess = set.iter_mut().next().unwrap();
    let mut cfg = cfg_for(&profile, "100000001");
    cfg.duration = Some(8); // 8 秒后自己结束
    sess.set_template(cfg.clone());

    let arcs: Vec<_> = set.iter().map(|s| s.arcs()).collect();
    let rxs = set.pull_receivers();
    let mut handles = vec![sessions::spawn_one(
        &tokio::runtime::Handle::current(),
        cfg,
        rxs.into_iter().next().unwrap(),
        arcs[0].clone(),
    )];

    assert!(wait_all_in_game(&set, &[0], 30).await, "30s 内未进图");
    assert_eq!(set.iter().next().unwrap().generation(), 0, "首跑是第 0 代");

    // 等它自己结束 (duration 到点)
    let mut finished = false;
    for _ in 0..400 {
        if set.iter().next().unwrap().task_finished() {
            finished = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(finished, "40s 内没有因 --duration 结束");

    // 控制台层看到的结局: 这里改造成"断线" —— 见上面的说明
    {
        let sess = set.iter_mut().next().unwrap();
        if let Ok(mut o) = sess.outcome.lock() {
            *o = Some(RunOutcome::ConnectionClosed);
        }
    }
    let t = std::time::Instant::now();
    let noted = set.note_exits(t);
    assert_eq!(noted.len(), 1, "结束必须被登记");
    assert!(set.iter().next().unwrap().is_waiting_restart());

    // 到点 → 真的重新拉起 (走 `advance_watchers`, 即 App 每帧调的那个)
    let later = std::time::Instant::now() + Duration::from_secs(1);
    let (spawned, errs) = set.advance_watchers(&tokio::runtime::Handle::current(), later, None);
    assert_eq!(spawned, vec![0], "到点必须真的拉起 ({errs:?})");
    let g = set.iter().next().unwrap().generation();
    assert_eq!(g, 1, "代数应加一");

    // 而且新的会话真的能再进游戏 (这才是"拉起"的意义)
    assert!(
        wait_all_in_game(&set, &[0], 30).await,
        "拉起后的会话 30s 内没再进图"
    );
    let snap = set.selected().unwrap().snapshot().await;
    assert_eq!(snap.phase, Phase::InGame);
    assert!(!snap.name.is_empty(), "拉起后角色名应正常");

    // 收尾 (它还会因 duration 再结束一次, 但这里只关心拉起成功)
    set.dispatch_line("quit").ok();
    for h in handles.drain(..) {
        let _ = tokio::time::timeout(Duration::from_secs(15), h).await;
    }
}
