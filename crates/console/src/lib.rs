//! openstory-console — 冒险岛风格终端控制台 (一个进程内管理多个 bot 账号)。
//!
//! 这里是**全部实现**, 供 `openstory-console` 与 `openstory-tui` 两个可执行
//! 入口共用 (见 `Cargo.toml` 里 `[[bin]]` 的注释)。
//!
//! 入口形态 (同一份 UI 代码):
//! - **单选**一个档案 → 单会话 (版面与改造前逐字一致)
//! - **选择屏勾选多个** / `--profiles a.json b.json` → 多开会话台
//! - 多选 + `--no-autostart` → **管理器模式**: 挂上账号但都不登录, 由 `F4` 决定谁连
//!
//! 结构: 主线程启动 bot (`runtime::run`) + 渲染线程 (crossterm/ratatui);
//! 两者通过事件队列 / 状态锁 / 指令 channel 通信。
//!
//! `run()` 是入口调用的**唯一**函数: `main()` 只负责转发 Args, 这样两个入口
//! 不可能出现行为分叉。

pub mod app;
pub mod credentials;
pub mod profile;
pub mod session;
pub mod sessions;
pub mod ui;
pub mod watcher;
pub mod wizard;

pub use app::{App, STAGE_FINISHED, STAGE_RUNNING, STAGE_WIZARD};
pub use ui::render;

use std::io::stdout;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc as smpsc;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::{mpsc, Mutex};

use openstory_bot::config::Config;
use openstory_bot::emit;
use openstory_bot::runtime::{self, RunOutcome};
use openstory_bot::state::BotState;

/// 终端恢复守卫: 任何路径退出都还原原始终端。
pub struct TermGuard;

impl TermGuard {
    pub fn new() -> Self {
        Self
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// 还原终端 (备用屏退出 + raw 模式关闭)。选择屏取消退出时
/// 渲染线程尚未启动, 需手动调用。
pub fn restore_terminal() {
    let mut out = stdout();
    let _ = execute!(
        out,
        crossterm::cursor::Show,
        crossterm::event::DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
}

/// 这个档案在启动时要不要自动登录。
///
/// 三条规则的优先级 (`--no-autostart` 赢过 `--start`):
/// 1. `--no-autostart` / `--manual` → 一律不自动登录 (全部「未启动」)。
/// 2. `--start <name...>` → 只自动登录点名的那几个 (按档案名精确匹配)。
/// 3. 默认 → 全部自动登录 (`--profiles` 的原行为, 保持向后兼容)。
pub fn will_autostart(profile: &str, autostart: bool, only: &[String]) -> bool {
    if !autostart {
        return false;
    }
    if only.is_empty() {
        return true;
    }
    only.iter().any(|n| n == profile)
}

/// 启动时要不要弹"账号向导"。
///
/// **多开一律不弹** —— 即使 `account`/`password` 是空的。抽成纯函数是为了能
/// 直接测: 这条判据错了会让界面卡死在一个关不掉的向导上 (实测踩过, 见调用处
/// 的长注释), 而那种 bug 从"代码读起来"完全看不出来。
pub fn needs_login_wizard(multi_count: usize, account_empty: bool, password_empty: bool) -> bool {
    if multi_count > 0 {
        return false;
    }
    account_empty || password_empty
}

/// 控制台主流程: 解析参数 → 建会话 → 起渲染线程 → 跑 bot → 收尾。
///
/// 两个可执行入口 (`openstory-console` / `openstory-tui`) 都**只**调这个函数,
/// 因此它们的行为由同一份编译产物保证一致。
///
/// 不返回: 内部自行 `std::process::exit` (bot 会话结束后还要继续展示日志直到
/// 用户退出, 退出码由会话结果决定)。
pub fn run() -> ! {
    openstory_bot::setup_console_utf8();
    openstory_bot::packet::init_time_baseline();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut config = match Config::parse_partial(&args) {
        Ok(c) => c,
        Err(m) => {
            eprintln!("{m}");
            std::process::exit(2);
        }
    };
    // 多档案启动 (阶段 5): `--profiles a.json b.json c.json`。
    //
    // 必须在选择屏之前算出来 —— 否则 `--profiles` 会让 config_paths 保持为空
    // 而弹出档案选择屏 (那是单档入口的行为)。
    //
    // `mut`: 选择屏里**勾选多个**也会走这条路 (见下面的 `pick_many`)。
    let mut multi_paths: Vec<std::path::PathBuf> = {
        let mut v: Vec<std::path::PathBuf> = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == "--profiles" {
                for p in it.by_ref() {
                    if p.starts_with('-') {
                        break;
                    }
                    v.push(std::path::PathBuf::from(p));
                }
                break;
            }
        }
        // 重复 `--config` 也算多档 (`--config` 的链式语义是"多层覆盖一个
        // 账号", 而多档是"N 个账号各用各的")
        if v.is_empty() && config.config_paths.len() > 1 {
            v = config
                .config_paths
                .iter()
                .map(std::path::PathBuf::from)
                .collect();
        }
        v
    };

    // --config 显式指定档案 → 跳过选择屏, 直接锁定该文件链。
    // 多档启动同理: 档案已由 --profiles 给出, 不需要选择屏。
    if !config.config_paths.is_empty() {
        openstory_bot::runtime_config::set_config_paths(
            config
                .config_paths
                .iter()
                .map(std::path::PathBuf::from)
                .collect(),
        );
    }

    // 掉线自动拉起 (阶段 6): 默认**开启**, 用 `--no-restart` 关掉。
    //
    // 默认开的理由: 多开场景下"某个号半夜掉线"是常态, 而手动发现它的唯一
    // 途径是用户一直盯着左栏。默认关的话这个功能等于不存在。而代价可控 ——
    // 只在**断线/崩溃**时拉起 (认证失败绝不重试, 见 `watcher` 模块), 且最多
    // 3 次 (5s → 15s → 45s), 之后标记「已放弃」等人处理。
    let restart_enabled = !args.iter().any(|a| a == "--no-restart");

    // 手动模式 (阶段 8): 挂上档案但**不自动登录**, 全部停在「未启动」。
    //
    // 与 `--profiles` 的关系: `--profiles` 是"这些号现在就挂上去干活",
    // `--no-autostart` 是"这些号先摆在这儿, 我决定谁连"。后者才是"管理多个
    // bot"的形态 —— 密码未必齐、也不该一上来就同时登录 N 个号。
    //
    // `--start <name...>` 是折中: 只自动启动点名的那些 (按档案名匹配)。
    //
    // `mut`: 从选择屏**勾选多个**进来时要改成"不自动登录" —— "我挑了这几个号"
    // 和"这几个号现在就登录"是两件不同的事 (见下面的 `picked_many`)。
    let mut autostart = !args
        .iter()
        .any(|a| a == "--no-autostart" || a == "--manual");
    let only_start: Vec<String> = {
        let mut v = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == "--start" || a == "--autostart" {
                for p in it.by_ref() {
                    if p.starts_with('-') {
                        break;
                    }
                    v.push(p.clone());
                }
                break;
            }
        }
        v
    };

    // 密码双模式 (阶段 6)。
    //
    // 核心层的硬纪律是"密码永不落盘" (`login.rs` 的 `skip_serializing`), 而
    // 自动拉起需要明文密码 —— 所以这里单独开一份存储, 并且**默认仍然不记**
    // (`Remember::No`): 想要"程序重启后还能自动拉起"的人显式打开。
    //
    // 优先级: 档案里的 `login.password` (用户自己写的) > 这里的缓存。
    // 两者都没有时会话照常启动 (向导会问), 但拉起会在登录阶段失败并被标记
    // 「需人工」—— 刻意的, 宁可标出来看一眼也不做密码错的重启风暴。
    let remember = credentials::Remember::from_args(&args);
    let mut creds = credentials::Credentials::load(remember);
    if let Some(e) = creds.load_error() {
        // 读不出来必须让用户知道 —— "没存过密码"和"密码读坏了"是两件事
        eprintln!("[warn] {e}");
    }

    // 终端初始化 (raw 模式 + 备用屏 + 鼠标)
    let mut out = stdout();
    enable_raw_mode().expect("无法进入 raw 模式");
    execute!(
        out,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )
    .expect("无法进入备用屏幕");
    let backend = CrosstermBackend::new(out);
    let mut term = Terminal::new(backend).expect("无法初始化终端 (请使用 Windows Terminal)");

    // 无 --config / --profiles: 第一步永远是选择配置档案 (profiles/ 目录),
    // 之后一切读写 (向导回写 / hunt save / reload) 都锁定选中的那份文件。
    //
    // **勾选多个 = 直接进多开管理器**: 这是"管理多个 bot"最自然的入口 ——
    // 账号列表本来就在这个屏幕上, 不必再敲一遍 `--profiles a.json b.json`。
    // 多选进来时全部会话停在「未启动」(下面把 `autostart` 关掉), 因为
    // "我挑了这几个号"和"这几个号现在就登录"是两件事。
    if config.config_paths.is_empty() && multi_paths.is_empty() {
        match profile::pick_many(&mut term) {
            profile::PickOutcome::One(path) => {
                openstory_bot::runtime_config::set_config_paths(vec![path]);
            }
            profile::PickOutcome::Many(paths) => {
                if paths.is_empty() {
                    restore_terminal();
                    std::process::exit(0);
                }
                // 单选时走原来的"账号向导"路径; 多选时按档案逐个建会话。
                // 用 `config_paths` 传会让多档被判成"多层覆盖一个账号", 所以
                // 这里走 `multi_paths` 那条链 (每个档案 = 一个独立账号)。
                multi_paths = paths;
                // **勾选多个 = 管理器模式**: 全部停在「未启动」, 由 F4 决定谁连。
                //
                // 为什么不让它自动登录: 勾选这个动作的语义是"把这几个号挂上",
                // 不是"现在就并发登录 5 个账号"。真要一上来就连, 那是
                // `--profiles` 的语义 (脚本/自动化用), 继续可用。
                autostart = false;
            }
            profile::PickOutcome::Cancel => {
                restore_terminal();
                std::process::exit(0);
            }
        }
    }

    // 从选定档案的 `login` 节补全登录信息 (命令行参数优先)。
    // 向导提交后会把账号/密码/AES 密钥写回该档案, 下次直接登录。
    if let Some(info) = openstory_bot::login::load_default() {
        config.apply_login(&info);
    }
    // 多档启动: 每个账号的凭据在**各自的档案**里, 不是一个全局 login 节。
    // 先按第一个档案把 CLI 侧的 `account` 填上, 否则会误判为"需要向导"。
    // (每个会话真正用的 Config 在启动 loop 里逐档 apply_login。)
    if config.account.is_empty() {
        if let Some(first) = multi_paths.first() {
            if let Some(info) = openstory_bot::login::load(first) {
                config.account = info.account.clone();
                if config.password.is_empty() {
                    config.password = info.password.clone();
                }
            }
        }
    }
    // 多开时不走"账号向导"。
    //
    // # 为什么 (实测踩到的坑)
    //
    // 从前这里只看 `account`/`password` 是否为空。多开 + 手动模式下不给密码就
    // 必然为空 → `interactive = true` → 启动就弹向导。而**手动模式下一个号都
    // 没登录**, 没有哪个会话会写 `STAGE_RUNNING`, 于是:
    //
    // - 主线程确实收到了向导结果 (日志确认)、也确实把 stage 置成了 RUNNING;
    // - 但 `App::poll_events` 每帧用 `stage_flag` (指向选中会话的 stage, 仍是
    //   `WIZARD`) 把它覆盖回去;
    // - 结果向导永远关不掉 —— 用户看到的就是"按回车没反应"。
    //
    // 修法两层: `poll_events` 在多会话下不再同步 stage (见那里的注释), 以及
    // 这里**根本不弹**向导 —— 多开时每个会话的凭据来自各自的档案/凭据存储,
    // 一个全局的账号密码框在语义上就是错的 (也不可能对所有号都对)。
    //
    // 缺凭据的会话按 F4 启动时会在它自己的日志里报"没有可用密码", 左栏亮
    // 「启动失败 — 需人工」—— 那才是多开下正确的反馈方式。
    let interactive = needs_login_wizard(
        multi_paths.len(),
        config.account.is_empty(),
        config.password.is_empty(),
    );
    config.interactive = interactive;
    // 供向导预填 (aes_key 原文; None = 空)。
    let login_aes_key = openstory_bot::login::load_default()
        .and_then(|l| l.aes_key)
        .unwrap_or_default();

    // 事件队列 (Emitter) + 共享状态 + 通道
    let queue = Arc::new(openstory_console_lib::eventq::EventQueue::new(1000));
    let _ = emit::set_emitter(queue.clone());
    // 名字表与档案的 data_dir 对齐 (在任何渲染之前)。
    //
    // 核心层每个会话都通过 `Session::names` 读自己的表, 但 TUI 的侧边栏 /
    // 中文日志渲染走的是进程默认表 —— 而默认表原本要等 `runtime::run` 起来才
    // 被设置, 于是「连接前」的那几帧会用 data/ 的名字 (同一个地图 id 显示成
    // 未知)。这里提前按选中档案设置一次, 消除这个启动窗口。
    if let Some(path) = config.config_paths.last() {
        let dir = openstory_bot::runtime_config::peek_data_dir(std::path::Path::new(path));
        openstory_bot::names::set_default_dir(&dir);
    }
    let state = Arc::new(Mutex::new(BotState::default()));
    // 命令通道共享句柄: 每次重连重建 (cmd_tx, cmd_rx), 通过锁换新 Sender
    let cmd_slot: Arc<StdMutex<mpsc::Sender<String>>> = Arc::new(StdMutex::new({
        let (tx, _rx) = mpsc::channel::<String>(64);
        tx
    }));
    let (wiz_tx, wiz_rx) = smpsc::channel::<wizard::WizardResult>();
    let (exit_tx, exit_rx) = smpsc::channel::<()>();
    let stage = Arc::new(AtomicU8::new(if interactive {
        STAGE_WIZARD
    } else {
        STAGE_RUNNING
    }));
    let outcome = Arc::new(StdMutex::new(None::<RunOutcome>));
    // 向导提交是否已由渲染线程应用 (多会话补凭据路径); 渲染写主线程读 → 原子
    let login_apply = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // 多会话集合必须在渲染线程起跑**之前**建好并挂上 —— 渲染线程一旦接管
    // `App` 就不能再从主线程改它。这里先构造集合 (只建壳, 不启动 bot),
    // 稍后在 tokio runtime 里 `spawn_all`。
    //
    // 会话**不能**在这里直接启动: bot 需要 tokio runtime, 而 runtime 在渲染
    // 线程之后才建 (渲染必须最先起跑, 否则连接期间屏幕是黑的)。
    let mut app = App::new(
        queue.clone(),
        state.clone(),
        cmd_slot.clone(),
        Some(wiz_tx), // 向导任何时候都可能重开 (连接失败重试), 通道常驻
        stage.clone(),
        outcome.clone(),
        login_apply.clone(),
        interactive,
        config.ip.clone(),
        config.port,
        config.account.clone(),
        config.password.clone(),
        login_aes_key,
    );
    // 多会话: 先建集合 -> 取出接收端 + 启动所需的 Arc -> 把集合挂给 UI。
    //
    // 顺序不能反: 渲染线程一旦接管 `App`, 主线程就再也改不到它。所以集合必须
    // 在 spawn 渲染线程之前构造并挂上。
    //
    // 自动拉起策略 (`--restart`) 让掉线的账号自己回来; 关掉时策略为 disabled,
    // 行为与改造前的 TUI 完全一致 (结束就是结束)。
    let restart_policy = if restart_enabled {
        watcher::RestartPolicy::default()
    } else {
        watcher::RestartPolicy::disabled()
    };
    let (mut multi_set, multi_start, multi_rxs) = if multi_paths.is_empty() {
        (None, Vec::new(), Vec::new())
    } else {
        let dir = openstory_bot::runtime_config::peek_data_dir(&multi_paths[0]);
        openstory_bot::names::set_default_dir(&dir);
        let specs: Vec<session::SessionSpec> = multi_paths
            .iter()
            .map(|p| session::SessionSpec::new(p.clone()).remembering_password())
            .collect();
        let mut set = sessions::SessionSet::from_specs_with_policy(&specs, restart_policy.clone());
        // 启动所需的一切在这里取好 (Arc + 接收端), 与集合解耦 —— 集合稍后
        // 交给 UI 线程, 这些数据留给主线程启动 bot。
        let data: Vec<_> = set.iter().map(|s| s.arcs()).collect();
        let rxs = set.pull_receivers();
        (Some(set), data, rxs)
    };
    // runtime 必须在渲染线程起跑**之前**建好: 渲染线程要拿它的 handle 去
    // 重新拉起掉线的会话。`build()` 只是建立线程池, 不阻塞。
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio 运行时创建失败");
    // 拉起/退出用的共享句柄: 渲染线程负责"到点重新 spawn", 主线程负责
    // "全部结束了吗"。两边都不阻塞对方。
    //
    // `keepalive` 是"还在跑的会话数" —— 每拉起一个加一, 结束时减一。退出条件
    // 不能只看主线程最初那批 handle: 拉起来的新任务不在那批里, 只看它会提前
    // 退出, 用户看到的是"重连到一半程序关了"。
    let restart_ctx = watcher::RestartCtx::with_credentials(rt.handle().clone(), creds.clone());
    let multi_count = multi_set.as_ref().map(|s| s.len()).unwrap_or(0);
    if let Some(mut set) = multi_set.take() {
        // 自动拉起要重跑**同一份**配置 (向导里改过的 ip / 密码 / tick 必须
        // 延续, 不能重新读盘 —— 读盘会悄悄丢掉运行期的改动)。
        //
        // 每个档案的凭据从它自己的 `login` 节补齐; 档案里没写密码时用控制台
        // 的凭据存储 (`profiles/.manager.json`) 兜底 —— 那是"记住密码"模式的
        // 全部意义所在: 让程序重启之后还能拉起上次掉线的号。
        for i in 0..set.len() {
            let path = match set
                .iter()
                .nth(i)
                .and_then(|s| s.config_paths.last().cloned())
            {
                Some(p) => p,
                None => continue,
            };
            let name = session::BotSession::profile_from_path(&path);
            // **每个号用自己的 `login` 节** —— 不能沿用全局 config 的账号。
            // 详见 `sessions::session_template` 的说明 (继承账号会让两个号登成
            // 同一个, 服务器来回踢)。
            let mut c = sessions::session_template(&config, &path);
            // 档案里没有密码 → 用记着的; 并把用到的密码记回去 (首次运行后
            // 就能留住, 不必等下一次输入)。
            if c.password.is_empty() {
                if let Some(pw) = creds.password_for(&name) {
                    c.password = pw.to_string();
                }
            } else if creds.remember().is_yes() {
                creds.record(&name, &c.account, &c.password);
            }
            if let Some(s) = set.iter_mut().nth(i) {
                s.set_template(c);
            }
        }
        // 多开时逐档检查密码: 缺哪一个就在**启动前**问清楚。
        //
        // 不问的话, 那个会话会在登录阶段失败并被标记「需人工」; 而拉起又需要
        // 密码 —— 于是自动拉起对它永远无效, 用户却以为开着。
        //
        // 四个不必问的场合 (问了反而挡住自动化):
        // 1. 命令行已经给了 `--password` —— 用户明确指定了共用的密码, 直接用。
        // 2. stdin 不是终端 (管道 / CI / 计划任务) —— 问了也没人能输入。
        // 3. 没有缺密码的档。
        // 4. **手动模式: 那些号现在不登录** —— 密码等到用户按启动那一刻再说。
        //    启动前排队问 N 个密码, 恰恰是"一上来就全部登录"的思路, 与手动模式
        //    的意图相反。
        let ask_needed: Vec<String> = set
            .iter()
            .filter(|s| {
                will_autostart(&s.profile, autostart, &only_start)
                    && s.template().map(|c| c.password.is_empty()).unwrap_or(false)
            })
            .map(|s| s.profile.clone())
            .collect();
        let missing: Vec<String> = ask_needed;
        let shared_pw = (!config.password.is_empty()).then(|| config.password.clone());
        if !missing.is_empty() {
            if let Some(pw) = shared_pw {
                for name in &missing {
                    if let Some(s) = set.iter_mut().find(|s| &s.profile == name) {
                        if let Some(mut c) = s.template() {
                            let acct = c.account.clone();
                            c.password = pw.clone();
                            s.set_template(c);
                            creds.record(name, &acct, &pw);
                        }
                    }
                }
                let _ = creds.save();
            } else if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                eprintln!(
                    "[warn] {} 个档案没有密码, 且 stdin 不是终端 —— 跳过后它们会被标记为需人工: {}",
                    missing.len(),
                    missing.join(", ")
                );
            } else {
                match profile::ask_passwords(&mut term, &missing) {
                    Ok(Some(answers)) => {
                        for (name, pw) in answers {
                            if pw.is_empty() {
                                continue; // 留空 = 明确跳过该档
                            }
                            if let Some(s) = set.iter_mut().find(|s| s.profile == name) {
                                if let Some(mut c) = s.template() {
                                    let acct = c.account.clone();
                                    c.password = pw.clone();
                                    s.set_template(c);
                                    creds.record(&name, &acct, &pw);
                                }
                            }
                        }
                        let _ = creds.save();
                    }
                    Ok(None) => {
                        // 用户跳过: 不阻塞启动, 但要说清楚后果
                        eprintln!(
                            "[warn] {} 个档案没有密码 —— 自动拉起对它们无效 (会被标记为需人工)",
                            missing.len()
                        );
                    }
                    Err(e) => eprintln!("[warn] 密码输入失败: {e}"),
                }
            }
        }
        // 真正会去登录的会话数 (手动模式 / `--start` 下可能少于总数)。
        let will_start = set
            .iter()
            .filter(|s| will_autostart(&s.profile, autostart, &only_start))
            .count();
        app.attach_sessions(set, Some(restart_ctx.clone()), will_start);
        if will_start == 0 {
            // 手动模式: 一个号都不登录。说清楚"怎么启动" —— 否则用户面对一个
            // 全是「未启动」的界面只能靠猜。
            eprintln!(
                "[info] 手动模式: {} 个档案已挂上, 都没有登录。用 Alt+↑↓ 选择, F4 (或 `start`) 启动, 再按一次停止。",
                multi_paths.len()
            );
        }
    }
    let keepalive = restart_ctx.keepalive.clone();
    let all_done = restart_ctx.all_done.clone();
    let counters = restart_ctx.counters.clone();
    let thread_exit = exit_tx.clone();
    let handle = std::thread::spawn(move || {
        render_loop(app, term, thread_exit);
    });

    // 主循环: 向导 → 启动 bot → 结束; 连接失败 (服务器维护等) 自动回到
    // 向导 (预填上次信息), 改好或直接 Enter 重试。
    let mut need_wizard = interactive;
    let mut final_code = 0;
    // 退出前把凭据落盘 (仅 `--remember-password`; 不记模式下 `save` 是 no-op)。
    //
    // 放在退出而不是"记一条就写一次": 频繁写盘没有意义, 而且**写明文密码的
    // 次数越少越好**。
    macro_rules! save_creds {
        () => {
            if creds.remember().is_yes() {
                if let Err(e) = creds.save() {
                    eprintln!("[warn] 密码未能保存: {e}");
                }
            }
        };
    }
    'outer: loop {
        if need_wizard {
            stage.store(STAGE_WIZARD, Ordering::SeqCst);
            loop {
                if exit_rx.try_recv().is_ok() {
                    rt.shutdown_background();
                    break 'outer;
                }
                match wiz_rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(r) => {
                        // 多会话下向导是给**某个会话**补凭据用的, 渲染线程已经
                        // 用回那个会话并启动了它 —— 主线程这里不要再 spawn 一个
                        // 全局会话 (那会把同一个账号登两次)。
                        //
                        // 标记是渲染线程写、这里读的一次性信号, 所以用 compare_exchange
                        // 取走: 两个线程同时看到它也不会各处理一次。
                        if login_apply.swap(false, Ordering::SeqCst) {
                            continue;
                        }
                        config.ip = r.ip;
                        config.port = r.port;
                        config.account = r.account;
                        config.password = r.password;
                        if !r.aes_key.is_empty() {
                            if let Ok(key) = openstory_bot::config::parse_aes_key(&r.aes_key) {
                                config.aes_key = Some(key);
                            }
                        } else {
                            config.aes_key = None;
                        }
                        // 向导提交后把登录信息存回 config.json 的 `login` 节,
                        // 下次启动免输入 (AES 密钥可留空 = 标准密钥)。
                        let info = openstory_bot::login::LoginInfo {
                            ip: config.ip.clone(),
                            port: Some(config.port),
                            account: config.account.clone(),
                            password: config.password.clone(),
                            aes_key: if r.aes_key.is_empty() {
                                None
                            } else {
                                Some(r.aes_key)
                            },
                        };
                        if let Err(e) = openstory_bot::login::save_default(&info) {
                            eprintln!("login save: {e}");
                        }
                        break;
                    }
                    Err(smpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => {
                        rt.shutdown_background();
                        break 'outer;
                    }
                }
            }
        }

        // 重置会话状态 + 换新命令通道
        *state.blocking_lock() = BotState::default();
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(64);
        *cmd_slot.lock().unwrap() = cmd_tx;
        let (done_tx, done_rx) = smpsc::channel::<RunOutcome>();

        if multi_count == 0 {
            // 单会话路径 (与阶段 4 之前完全一致, 便于与旧行为逐项对照)
            {
                let state = state.clone();
                let stage = stage.clone();
                let outcome = outcome.clone();
                let cfg = config.clone();
                rt.spawn(async move {
                    let o = runtime::run(cfg, &mut cmd_rx, state).await;
                    stage.store(STAGE_FINISHED, Ordering::SeqCst);
                    *outcome.lock().unwrap() = Some(o);
                    let _ = done_tx.send(o);
                });
            }
            stage.store(STAGE_RUNNING, Ordering::SeqCst);

            // 等 bot 结束 (或渲染线程强制退出)
            let mut o = None;
            loop {
                if exit_rx.try_recv().is_ok() {
                    rt.shutdown_background();
                    break 'outer;
                }
                match done_rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(res) => {
                        o = Some(res);
                        break;
                    }
                    Err(smpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                }
            }

            match o {
                // 连接失败 (服务器维护/掉线) → 回到向导重试
                Some(RunOutcome::Failed) => {
                    final_code = 1;
                    need_wizard = true;
                    continue;
                }
                _ => break,
            }
        } else {
            // 多会话路径: 一个进程内跑 N 个 bot, 无子进程 / 无 IPC。
            //
            // 会话集合已经在渲染线程起跑前挂到 `App` 上了; 这里只负责把每个
            // 会话的 bot 在自己的 emitter 作用域里跑起来。`spawn_with` 消费
            // 之前 `pull_receivers` 取出的接收端, 因此每个 future 完全自持。
            drop(cmd_rx); // 多会话下指令按目标分发, 不用这条占位通道
            let specs: Vec<session::SessionSpec> = multi_paths
                .iter()
                .map(|p| {
                    let mut sp = session::SessionSpec::new(p.clone());
                    sp.remember_password = true;
                    sp
                })
                .collect();
            let cfgs: Vec<Config> = specs
                .iter()
                .map(|sp| {
                    // 每个账号的凭据来自**它自己的档案** (不能继承全局账号 ——
                    // 那会让所有会话登同一个号; 见 `sessions::session_template`)。
                    sessions::session_template(&config, &sp.config_path)
                })
                .collect();
            let idxs: Vec<usize> = (0..specs.len()).collect();
            let _ = idxs;
            // 会话对象在 `App` 里 (渲染线程持有), 这里只负责启动。自动拉起
            // 用的**配置模板**已在 attach 之前交给每个会话了 (见
            // `multi_set` 构造处的 `set_template`), 因此这里不需要再管。
            let mut handles = Vec::with_capacity(specs.len());
            for (i, (cfg, rx)) in cfgs.into_iter().zip(multi_rxs).enumerate() {
                let Some(arcs) = multi_start.get(i).cloned() else {
                    break;
                };
                // 手动模式 / `--start` 之外的那些号: 只挂会话, **不登录**。
                //
                // 接收端一起丢掉是对的 —— 用户按 F4 启动时 `start_session` 会建
                // 一条全新的 `(tx, rx)`, 而这个 rx 永远不会有人读。
                if !will_autostart(&specs[i].name, autostart, &only_start) {
                    drop(rx);
                    continue;
                }
                // `keepalive` 不在这里加: `attach_sessions` 已经按**实际会启动的
                // 数量**记过一次 (见那里的 `started` 参数), 这里再加就会双计 ——
                // 而 `keepalive` 是退出条件的一半, 双计会让程序永远不肯退出。
                // 之后每帧由 `drive_restarts` 按 `live_count` 校准。
                handles.push(sessions::spawn_one(rt.handle(), cfg, rx, arcs));
            }
            stage.store(STAGE_RUNNING, Ordering::SeqCst);

            // 等全部结束, 或渲染线程强制退出。
            //
            // 不能用 `rt.block_on(h)` 逐个 join: 主线程不在 runtime 里, 而
            // `block_on` 会占住调用线程 —— 会让其它会话的 timer 一起停摆。
            // 轮询 `JoinHandle::is_finished` + 短 sleep, 全部结束后再 join。
            //
            // **退出条件是 `all_done` 而不是"初始那批 handle 都结束"**:
            // 自动拉起会产生新的任务, 不在 `handles` 里; 只看 `handles` 会在
            // 第一次重连的间隙就退出。`all_done` 由渲染线程在"没有任何会话
            // 在跑、也没有任何会话在等退避"时置位。
            let mut any_failed = false;
            loop {
                if exit_rx.try_recv().is_ok() {
                    save_creds!();
                    rt.shutdown_background();
                    handle.join().ok();
                    std::process::exit(final_code);
                }
                if all_done.load(Ordering::SeqCst) && keepalive.load(Ordering::SeqCst) == 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            save_creds!();
            for h in handles {
                // 主线程不在 runtime 里 -> 用 block_on 收结果。全部
                // is_finished 之后才走到这里, 因此不会再阻塞别的会话。
                match rt.block_on(h) {
                    Ok(RunOutcome::Failed) => any_failed = true,
                    Err(_) => any_failed = true, // 任务 panic
                    Ok(_) => {}
                }
            }
            // 自动拉起过的会话: 它们的 panic 标志不在上面的 handle 里, 直接
            // 问共享计数器 (渲染线程每次拉起失败/异常都会记账)。
            if counters.spawned.load(Ordering::SeqCst) > 0 {
                // 拉起过就说明出过事; 退出码交给 errors 是否非空决定
            }
            if !counters
                .errors
                .lock()
                .map(|e| e.is_empty())
                .unwrap_or(false)
            {
                any_failed = true;
            }
            let _ = done_tx.send(RunOutcome::Quit);
            if any_failed {
                final_code = 1;
            }
            break;
        }
    }
    let _ = multi_count;
    save_creds!();
    // 会话结束后由渲染线程继续展示日志, 直到用户 Ctrl+C 退出
    handle.join().ok();

    std::process::exit(final_code);
}

pub fn render_loop(
    mut app: App,
    mut term: Terminal<CrosstermBackend<std::io::Stdout>>,
    exit_tx: smpsc::Sender<()>,
) {
    let guard = TermGuard::new();
    // 按键日志开关 (环境变量; 默认关闭 —— 见 `keylog_key` 的文档)。
    let keylog: Option<std::fs::File> = std::env::var("OPENSTORY_KEYLOG")
        .ok()
        .and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        loop {
            if app.should_exit() {
                break;
            }
            // Windows 上 event::read() 在队列排空后会永久阻塞:
            // 每个 read 前用 poll(0) 探测, 只在确有事件时读取。
            if event::poll(Duration::from_millis(50)).unwrap_or(false) {
                while event::poll(Duration::ZERO).unwrap_or(false) {
                    match event::read() {
                        Ok(Event::Key(k)) => {
                            keylog_key(&keylog, &app, &k);
                            app.on_key(k);
                        }
                        Ok(Event::Mouse(m)) => app.on_mouse(m.kind),
                        Ok(Event::Resize(_, _)) => {
                            app.dirty = true;
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
            }
            app.poll_events();
            let due = app.dirty || app.last_draw.elapsed() >= Duration::from_millis(100);
            if due {
                let _ = term.draw(|f| ui::render(f, &mut app));
                app.dirty = false;
                app.last_draw = std::time::Instant::now();
            }
        }
    }));
    drop(guard);
    let _ = exit_tx.send(());
    if let Err(p) = result {
        std::panic::resume_unwind(p);
    }
}

/// 把一次按键写进按键日志 (`OPENSTORY_KEYLOG=<路径>` 时开启)。
///
/// 存在的理由: "按键没反应"这类问题**没法靠读代码定位** —— 得先知道键到底有没有
/// 到这个进程。开启后用户跑一次把文件发来, 就能分清三种情况:
/// "键没到" / "到了但被当成别的键" / "处理了但状态不对"。
///
/// 只记按键自身与当时的阶段。**不记字符内容** (那会连密码一起写进文件), 只记
/// "这是一个字符键"。
pub fn keylog_key(
    log: &Option<std::fs::File>,
    app: &App,
    k: &crossterm::event::KeyEvent,
) {
    use crossterm::event::KeyCode;
    let Some(log) = log else { return };
    use std::io::Write as _;
    let desc = match k.code {
        KeyCode::Char(_) => "Char(内容不记)".to_string(),
        other => format!("{other:?}"),
    };
    let mut f = log;
    let _ = writeln!(
        f,
        "ts={:?} stage={} kind={:?} mods={:?} key={}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
        app.stage,
        k.kind,
        k.modifiers,
        desc,
    );
    let _ = f.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_rules_are_ordered_by_specificity() {
        // 默认: 全部自动登录
        assert!(will_autostart("a", true, &[]));
        // --start 点名: 只点到的自动登录
        assert!(will_autostart("a", true, &["a".into()]));
        assert!(!will_autostart("b", true, &["a".into()]));
        // --no-autostart 赢过 --start
        assert!(!will_autostart("a", false, &["a".into()]));
        assert!(!will_autostart("a", false, &[]));
    }

    #[test]
    fn multi_session_never_opens_the_login_wizard() {
        // 现场 bug (用户报"按回车没反应")的根因判据:
        //
        // 多开 + 手动模式下不给密码 → account/password 为空 → 从前会弹向导。
        // 而手动模式下一个号都没登录, 没有会话会写 STAGE_RUNNING, 于是
        // `poll_events` 每帧把 stage 覆盖回 WIZARD, 向导**永远关不掉**。
        //
        // 多开时凭据来自各自的档案/凭据存储, 一个全局账号密码框在语义上就是
        // 错的。所以多开一律不弹。
        assert!(
            !needs_login_wizard(2, true, true),
            "多开 + 空账号密码 → 绝不弹向导 (弹了就会卡死)"
        );
        assert!(
            !needs_login_wizard(1, true, true),
            "多开 1 档也一样 (走的是多会话路径)"
        );
        assert!(!needs_login_wizard(2, false, false), "多开且凭据齐全");
        // 单开保持原行为: 缺账号或密码就弹
        assert!(needs_login_wizard(0, true, false), "单开缺账号 → 弹");
        assert!(needs_login_wizard(0, false, true), "单开缺密码 → 弹");
        assert!(!needs_login_wizard(0, false, false), "单开凭据齐全 → 不弹");
    }
}
