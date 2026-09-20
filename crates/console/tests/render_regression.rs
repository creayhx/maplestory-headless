//! TUI 渲染回归测试 (阶段 1 起的等价性基线)。
//!
//! 驱动真实的 `App` + `ui::render` (同一个 `runtime::run` 事件管线) 打到本地服,
//! 把帧缓冲 dump 成文本并断言关键渲染结果。这条链路覆盖的正是「核心层改动是否
//! 改变了 TUI 画出来的东西」——阶段 3/4/5 的等价性验收都靠它守住。
//!
//! 需要本地服 127.0.0.1:8484; 连不上时打印 SKIP 并返回, 保证离线也能跑全套测试。
//!
//! 模块直接来自 `openstory-console` 这个 lib crate (实现全在 `src/lib.rs`)。
//!
//! 从前这里是 `#[path = "../src/app.rs"] mod app;` 一类的源码收纳 —— 那是
//! "纯 bin crate 没有 lib target" 时代的权宜。代价是这些模块在 test target 里
//! **被二次编译**, 而且测试只用到其中一部分, 于是刷出一片 "never used" 警告。
//! 现在两个可执行入口和测试编译的是同一份模块。

use openstory_console::{app, credentials, session, sessions, ui, watcher, wizard};

use std::sync::atomic::AtomicU8;
use std::sync::mpsc as smpsc;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use app::{App, STAGE_RUNNING};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tokio::sync::{mpsc, Mutex};

use openstory_bot::config::Config;
use openstory_bot::emit;
use openstory_bot::emit::Emitter as _;
use openstory_bot::runtime;
use openstory_bot::state::BotState;

const SERVER: &str = "127.0.0.1:8484";
const ACCOUNT: &str = "100000001";
const PASSWORD: &str = "test_password";
const PROFILE: &str = "profiles/本地服_100000001.json";

async fn server_up() -> bool {
    tokio::time::timeout(
        Duration::from_millis(800),
        tokio::net::TcpStream::connect(SERVER),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false)
}

/// 把终端帧缓冲转成可断言的文本 (行尾去空白)。
fn dump(term: &Terminal<TestBackend>) -> String {
    let buf = term.backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        for x in 0..buf.area.width {
            line.push_str(buf[(x, y)].symbol());
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// 全角/半角都要匹配: TUI 在 CJK 之间插入空格 (显示宽度对齐), 所以断言前把
/// 空白全部去掉再比较子串。
fn squish(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tui_renders_live_bot_frame() {
    // 档案里的 `data_dir` 与 `data/` 名字表都是相对仓库根目录的。
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/console -> workspace root")
        .to_path_buf();
    std::env::set_current_dir(&root).expect("chdir to workspace root");
    let profile = root.join(PROFILE);
    assert!(profile.exists(), "缺少档案 {PROFILE}");

    if !server_up().await {
        eprintln!("SKIP: 本地服 {SERVER} 未监听, 跳过 TUI 渲染回归");
        return;
    }

    let queue = Arc::new(openstory_console_lib::eventq::EventQueue::new(4000));
    // 单开会话走进程级 emitter, 与真实 TUI 一致 (多会话才用 task-local scope)。
    emit::set_emitter(queue.clone()).expect("register emitter");
    let state = Arc::new(Mutex::new(BotState::default()));
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(64);
    let cmd_slot = Arc::new(StdMutex::new(cmd_tx));
    let (wiz_tx, _wiz_rx) = smpsc::channel::<wizard::WizardResult>();
    let stage = Arc::new(AtomicU8::new(STAGE_RUNNING));
    let outcome = Arc::new(StdMutex::new(None));

    let mut app = App::new(
        queue.clone(),
        state.clone(),
        cmd_slot.clone(),
        Some(wiz_tx),
        stage.clone(),
        outcome.clone(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        false,
        "127.0.0.1".into(),
        8484,
        ACCOUNT.into(),
        PASSWORD.into(),
        String::new(),
    );

    let cfg = Config {
        ip: "127.0.0.1".into(),
        port: 8484,
        account: ACCOUNT.into(),
        password: PASSWORD.into(),
        char_index: 0,
        show_packets: false,
        config_paths: vec![profile.display().to_string()],
        duration: Some(40),
        auto: vec!["view status".into(), "hunt status".into()],
        ..Config::default()
    };

    let st = state.clone();
    let bot = tokio::spawn(async move { runtime::run(cfg, &mut cmd_rx, st).await });

    // 等进图, 再补一条 view portals 让日志有多行块事件。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    loop {
        if state.lock().await.mapid != 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "25s 内没有进图 (本地服/账号不可用?)"
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    cmd_slot
        .lock()
        .unwrap()
        .try_send("view portals".into())
        .ok();
    tokio::time::sleep(Duration::from_secs(2)).await;

    // 与渲染线程同样的顺序: 先吞事件, 再画一帧。
    app.poll_events();
    app.scroll = 0; // 0 = 贴底 (显示最新日志)

    let backend = TestBackend::new(150, 45);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| ui::render(f, &mut app)).unwrap();
    let text = dump(&term);
    let flat = squish(&text);

    let (mapid, phase, data_dir) = {
        let s = state.lock().await;
        (s.mapid, s.phase, s.cfg.data_dir.clone())
    };
    assert_eq!(phase, openstory_bot::state::Phase::InGame, "应处于游戏中");

    // 档案的 data_dir 必须真的被读到 (阶段 1 G2/G3 的守门断言)。
    assert_eq!(data_dir, "data/server_a", "档案的 data_dir 未被读取");

    // ── 1. 名字表按档案的 data_dir 生效 ──────────────────────────────
    // 会话自己的表解析出的地图名必须出现在画面上。这条曾经是真实 bug:
    // 渲染层读进程默认表, 而默认表晚一步才被设置 -> 地图名显示成「未知」。
    let session_table = openstory_bot::names::NameTable::for_dir(&data_dir);
    let map_name = session_table.map_name_text(mapid);
    assert_ne!(map_name, "未知", "地图 {mapid} 在该 data_dir 下应有中文名");
    assert!(
        flat.contains(&squish(&format!("{mapid}·{map_name}"))),
        "侧边栏/日志应显示 {mapid}·{map_name}\n{text}"
    );
    assert!(
        !flat.contains(&squish(&format!("{mapid}·未知"))),
        "不应把已知地图渲染成「未知」\n{text}"
    );

    // ── 2. 画面结构 ────────────────────────────────────────────────
    assert!(app.log.total_display_lines(None, 120) > 0, "日志缓冲为空");
    for must in ["角色信息", "地图", "场景", "挂机"] {
        assert!(flat.contains(must), "侧边栏缺少「{must}」\n{text}");
    }
    for tab in ["聊天", "公告", "NPC", "打怪", "封包", "错误"] {
        assert!(flat.contains(tab), "页签缺少「{tab}」\n{text}");
    }
    assert!(flat.contains("输入指令"), "缺少输入栏提示\n{text}");
    assert!(flat.contains("Ctrl+C"), "缺少快捷键提示行\n{text}");

    // ── 3. 事件真的进了队列并被渲染 ─────────────────────────────────
    let joined = squish(&text);
    assert!(
        joined.contains("phase=InGame"),
        "应渲染 view status 输出\n{text}"
    );
    // `view portals` 的中文渲染输出形如「18 hp00_1 → 100000100射手村集市」;
    // 其中一条目的目标地图就是本图, 可作为「块事件被完整渲染」的证据。
    assert!(
        joined.contains(&format!("→{mapid}{map_name}")),
        "应渲染 view portals 的传送门列表\n{text}"
    );

    // ── 4. 智能补全对着实时游戏状态 (F6, 本项目的核心资产) ───────────
    // 用真实 BotState 造 Ctx, 校验 Tab 候选真的来自"这一刻的游戏状态",
    // 而不只是静态命令表。
    let (npcs, mobs, reactors, rules, tasks, groups, portals, items, skills) = {
        let s = state.lock().await;
        (
            s.npcs
                .values()
                .map(|n| (n.oid, n.npcid))
                .collect::<Vec<_>>(),
            s.entities
                .values()
                .filter(|e| e.kind == openstory_bot::state::EntityKind::Mob)
                .map(|e| e.oid)
                .collect::<Vec<_>>(),
            s.reactors.keys().copied().collect::<Vec<_>>(),
            s.cfg.rules.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
            s.cfg.tasks.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
            s.cfg
                .groups
                .iter()
                .map(|g| g.id.clone())
                .collect::<Vec<_>>(),
            openstory_bot::names::map_portals(s.mapid)
                .into_iter()
                .map(|p| (p.name, p.target_map))
                .collect::<Vec<_>>(),
            s.inventory.values().map(|i| i.itemid).collect::<Vec<_>>(),
            s.skills.keys().copied().collect::<Vec<_>>(),
        )
    };
    let rule_refs: Vec<&str> = rules.iter().map(|s| s.as_str()).collect();
    let task_refs: Vec<&str> = tasks.iter().map(|s| s.as_str()).collect();
    let group_refs: Vec<&str> = groups.iter().map(|s| s.as_str()).collect();
    let ctx = openstory_console_lib::completion::Ctx {
        npcs: &npcs,
        mob_oids: &mobs,
        reactor_oids: &reactors,
        rule_ids: &rule_refs,
        task_ids: &task_refs,
        group_ids: &group_refs,
        portals: &portals,
        item_ids: &items,
        skill_ids: &skills,
        // 单会话: `@` 目标补全不该出现
        profiles: &[],
    };

    // 地图上应有 NPC (上面 dump 里可见): `npc ` + Tab 必须给出
    // 「oid -- 中文名」候选, 且描述不进入实际指令。
    assert!(!npcs.is_empty(), "进图后应已收到 NPC 生成包");
    let npc_c = openstory_console_lib::completion::complete_with("npc ", &ctx);
    assert!(
        npc_c.len() >= npcs.len(),
        "npc 候选 {} 个, 少于场上的 {} 个 NPC",
        npc_c.len(),
        npcs.len()
    );
    let (oid0, npcid0) = npcs[0];
    let want_cmd = format!("npc {oid0}");
    let hit = npc_c.iter().find(|c| c.cmd == want_cmd).unwrap_or_else(|| {
        panic!(
            "缺少候选 {want_cmd}: {:?}",
            npc_c.iter().map(|c| &c.cmd).collect::<Vec<_>>()
        )
    });
    let npc_name = openstory_bot::names::npc_name(npcid0);
    assert!(
        hit.display.contains(&npc_name),
        "候选展示应含中文 NPC 名 {npc_name}: {}",
        hit.display
    );
    assert_eq!(hit.cmd, want_cmd, "描述不能进入实际指令");

    // `warp ` + Tab 必须给出「传送点名 -- 目标地图名」。
    let warp_c = openstory_console_lib::completion::complete_with("warp ", &ctx);
    assert!(warp_c.len() >= portals.len(), "warp 候选数应覆盖全部传送门");
    let p0 = portals
        .iter()
        .find(|(_, tm)| openstory_bot::names::map_name_text(*tm) != "未知")
        .expect("至少有一个目标地图可解析名字");
    let warp_hit = warp_c
        .iter()
        .find(|c| c.cmd == format!("warp {}", p0.0))
        .unwrap_or_else(|| panic!("缺少 warp {} 候选", p0.0));
    assert!(
        warp_hit
            .display
            .contains(&openstory_bot::names::map_name_text(p0.1)),
        "warp 候选应展示目标地图名: {}",
        warp_hit.display
    );

    // 配置驱动的候选 (本档案有规则)
    if !rules.is_empty() {
        let c = openstory_console_lib::completion::complete_with(
            &format!("rule open {}", &rules[0][..1]),
            &ctx,
        );
        assert!(
            c.iter().any(|x| x.cmd == format!("rule open {}", rules[0])),
            "rule open 应补出配置里的规则 id {}",
            rules[0]
        );
    }

    // 背包道具候选: `sell <前缀>` 应补出背包里真实存在的 itemid
    if let Some(first) = items.first() {
        let digits = first.to_string();
        let c =
            openstory_console_lib::completion::complete_with(&format!("sell {}", &digits[..1]), &ctx);
        assert!(
            c.iter().any(|x| x.cmd == format!("sell {first}")),
            "sell 应补出背包里的 itemid {first}: {:?}",
            c.iter().map(|x| &x.cmd).collect::<Vec<_>>()
        );
    }

    cmd_slot.lock().unwrap().try_send("quit".into()).ok();
    let _ = tokio::time::timeout(Duration::from_secs(10), bot).await;
}

#[tokio::test]
async fn wizard_enter_in_multi_mode_actually_submits_and_closes() {
    // 现场症状: 多会话说"按回车没反应" —— 向导停在那里不动。
    //
    // 这条路要么提交成功 (主线程收到结果 → stage 转 RUNNING → 向导关闭),
    // 要么失败在某个校验上。**不能**是"按键被吃掉了什么也没发生"。
    let queue = Arc::new(openstory_console_lib::eventq::EventQueue::new(64));
    let state = Arc::new(Mutex::new(BotState::default()));
    let (cmd_tx, _rx) = mpsc::channel::<String>(4);
    let (wiz_tx, wiz_rx) = smpsc::channel::<wizard::WizardResult>();
    let stage = Arc::new(AtomicU8::new(app::STAGE_WIZARD));
    let outcome = Arc::new(StdMutex::new(None));
    // 复现现场: 账号已有 (从档案读出来的), 密码为空 → interactive
    let mut app = App::new(
        queue,
        state,
        Arc::new(StdMutex::new(cmd_tx)),
        Some(wiz_tx),
        stage.clone(),
        outcome,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        true,
        "127.0.0.1".into(),
        8484,
        "100000001".into(),
        String::new(),
        String::new(),
    );
    let specs: Vec<session::SessionSpec> = (0..2)
        .map(|i| session::SessionSpec::new(format!("profiles/acc{i}.json")))
        .collect();
    let mut set = sessions::SessionSet::from_specs(&specs);
    let _rxs = set.pull_receivers();
    app.attach_sessions(set, None, 0);
    // attach_sessions 会把 stage_flag 指向选中会话的 stage; 向导阶段要还原
    stage.store(app::STAGE_WIZARD, std::sync::atomic::Ordering::SeqCst);
    app.stage = app::STAGE_WIZARD;

    assert!(app.wizard.is_some(), "前置: 向导应该开着");
    // 密码留空 → 提交必须被拒 (不是静默无反应, 而是给出一条错误)
    app.on_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Enter,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    assert!(
        wiz_rx.try_recv().is_err(),
        "密码为空时不该提交成功"
    );
    assert!(
        app.wizard.as_ref().and_then(|w| w.error.as_deref()) == Some("密码不能为空"),
        "必须给出可见的错误原因, 而不是没反应: {:?}",
        app.wizard.as_ref().and_then(|w| w.error.clone())
    );

    // 填上密码 → Enter 必须真的提交出去
    app.wizard.as_mut().unwrap().password = "test_password".into();
    app.on_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Enter,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    let got = wiz_rx.try_recv().expect("Enter 必须把向导结果发出去");
    assert_eq!(got.account, "100000001");
    assert_eq!(got.password, "test_password");
}

#[tokio::test]
async fn multi_mode_does_not_let_a_session_stage_clobber_the_app_stage() {
    // 现场 bug 的**直接**成因 (比上面那条更底层):
    //
    // `attach_sessions` 把 `stage_flag` 指向**选中会话自己的** stage (单会话
    // 缓冲视图那一套)。手动模式下会话全停在 `STAGE_WIZARD` (未启动) —— 于是
    // `poll_events` 每帧把主线程刚设好的 `STAGE_RUNNING` 覆盖回 `WIZARD`,
    // 向导**永远关不掉**。用户看到的就是"按回车没反应"。
    //
    // 多会话下 `App.stage` 由主线程直接管理, 不能被会话的 stage 改写。
    let (mut app, _rxs) = multi_app(2);
    {
        let set = app.sessions.as_mut().unwrap();
        let s = set.iter_mut().next().unwrap();
        s.stage
            .store(session::STAGE_WIZARD, std::sync::atomic::Ordering::SeqCst);
    }
    // 主线程把 App 置为运行中 (向导提交后的正常动作)
    app.stage = STAGE_RUNNING;
    for _ in 0..3 {
        app.poll_events();
        assert_eq!(
            app.stage, STAGE_RUNNING,
            "多会话下 App.stage 不能被未启动会话的 WIZARD 覆盖 (否则向导关不掉)"
        );
    }
}

#[tokio::test]
async fn multi_session_has_no_wizard_to_get_stuck_on() {
    // 多开时压根不该有向导 —— 有它就意味着可能卡在上面。
    let queue = Arc::new(openstory_console_lib::eventq::EventQueue::new(64));
    let state = Arc::new(Mutex::new(BotState::default()));
    let (cmd_tx, _rx) = mpsc::channel::<String>(4);
    let stage = Arc::new(AtomicU8::new(STAGE_RUNNING));
    let outcome = Arc::new(StdMutex::new(None));
    // 多开时 main.rs 传 interactive=false (见 needs_login_wizard)
    let mut app = App::new(
        queue,
        state,
        Arc::new(StdMutex::new(cmd_tx)),
        None, // 多开不给 wiz_tx: 没有向导可提交
        stage,
        outcome,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        false,
        "127.0.0.1".into(),
        8484,
        String::new(),
        String::new(),
        String::new(),
    );
    let specs: Vec<session::SessionSpec> = (0..2)
        .map(|i| session::SessionSpec::new(format!("profiles/acc{i}.json")))
        .collect();
    let mut set = sessions::SessionSet::from_specs(&specs);
    let _rxs = set.pull_receivers();
    app.attach_sessions(set, None, 0);
    for _ in 0..3 {
        app.poll_events();
        assert!(app.wizard.is_none(), "多开会话不该冒出向导");
    }
}

#[tokio::test]
async fn f4_without_a_password_opens_the_login_wizard_for_that_session() {
    // 现场需求: 手动模式下按 F4 启动一个没密码的号 → **弹登录向导**
    // (账号/IP 从它自己的档案预填, 焦点落在密码上), 而不是只报一句错让用户
    // 退出程序去改档案。
    let (mut app, _rxs, _ctx) = manual_app(2, tokio::runtime::Handle::current());
    {
        // 抽掉密码 → 让第一个号"没有可用密码"
        let set = app.sessions.as_mut().unwrap();
        let s = set.iter_mut().next().unwrap();
        let mut cfg = s.template().unwrap();
        cfg.password = String::new();
        s.set_template(cfg);
    }
    app.toggle_selected_session();
    let w = app.wizard.as_ref().expect("F4 在缺密码时必须弹向导");
    assert_eq!(w.account, "acc0", "账号要从该会话的档案预填");
    assert_eq!(w.ip, "127.0.0.1");
    assert_eq!(w.port, "1", "端口也要预填 (来自同一份档案)");
    assert!(w.password.is_empty(), "密码当然是空的");
    assert_eq!(
        app.wizard_target.as_deref(),
        Some("acc0"),
        "要记住这次是给哪个会话开的"
    );
    assert_eq!(app.stage, app::STAGE_WIZARD, "向导阶段");
    // 关键: 不能因为弹了向导就顺手去启动 (那会先失败一次)
    assert_eq!(
        app.sessions.as_ref().unwrap().iter().next().unwrap().generation(),
        0,
        "还没提交就不该 spawn"
    );
}

/// 把一行单元格还原成 `(视觉文本, 每个字符的起始列)`。
///
/// 缓冲里**一个单元格 = 一列**; 宽字符 (CJK) 占一格, 紧随的续格内容是空格。
/// 直接拼接 `symbol()` 会在每个汉字后多出一个空格 —— 既让 `contains("档案")`
/// 失配, 也会让列号算错。这里按字符宽度跳过续格, 并记下每个字符真正落在第几列。
fn compact_cells(cells: &[String]) -> (String, Vec<usize>) {
    let mut text = String::new();
    let mut cols = Vec::new();
    let mut i = 0usize;
    while i < cells.len() {
        let sym = cells[i].as_str();
        if sym.is_empty() {
            i += 1;
            continue;
        }
        text.push_str(sym);
        cols.push(i);
        let w = sym
            .chars()
            .next()
            .and_then(unicode_width::UnicodeWidthChar::width)
            .unwrap_or(1);
        i += w.max(1);
    }
    (text, cols)
}

/// 取整屏全部行的原始单元格 (未去尾空白 —— 那正是对齐信息)。
fn screen_rows(app: &mut App) -> Vec<Vec<String>> {
    let backend = TestBackend::new(160, 40);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| ui::render(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        })
        .collect()
}

/// token 在该行里的 `(起始列, 末列)`。
///
/// 末列要把**最后一个字符自身的宽度**算进去: 汉字占 2 列, 它的起始格之后还有
/// 一个续格 —— 只报起始格会少算一列 (右对齐断言因此差 1)。
fn span_of(cells: &[String], token: &str) -> (usize, usize) {
    let (text, cols) = compact_cells(cells);
    let byte = text
        .find(token)
        .unwrap_or_else(|| panic!("行里找不到 {token:?}: {text:?}"));
    let ci = text[..byte].chars().count();
    let last = ci + token.chars().count() - 1;
    let last_w = token
        .chars()
        .next_back()
        .and_then(unicode_width::UnicodeWidthChar::width)
        .unwrap_or(1)
        .max(1);
    (cols[ci], cols[last] + last_w - 1)
}

#[tokio::test]
async fn board_columns_stay_aligned_with_cjk_content() {
    // 现场: F2 看板每一列都往右歪一点, 越往右越明显。
    //
    // 根因是 `format!("{:<22}", ..)` 按 **char 个数**补齐, 而汉字算 1 个 char
    // 却占 2 列: 表头 "档案" (2 字符/4 列) 与数据 "本地服_100000001"
    // (13 字符/16 列) 补到同一"字符宽度"后显示宽度并不相同, 整张表就此错开。
    let (mut app, _rxs, _ctx) = manual_app(2, tokio::runtime::Handle::current());
    app.board_open = true;
    // 给选中会话一个独特的金币数, 用来定位右对齐列
    {
        let set = app.sessions.as_ref().unwrap();
        let st = set.selected().unwrap().state.clone();
        st.try_lock().expect("状态锁没人占").meso = 987654321;
    }
    let rows = screen_rows(&mut app);
    // 用表头定位看板, 再取它的下一条数据行 —— 不能直接搜 "acc0": 左栏账号列表
    // 里也有它, 会匹配到侧栏那一行。
    let hy = rows
        .iter()
        .position(|r| compact_cells(r).0.contains("档案"))
        .expect("看板表头行");
    let hdr = rows[hy].clone();
    let row = rows[hy + 1].clone();
    let (row_text, _) = compact_cells(&row);
    assert!(row_text.contains("acc0"), "表头下一行应是首个账号: {row_text:?}");
    // 左对齐列: "档案" 与 "acc0" 的起始列必须一致
    assert_eq!(
        span_of(&hdr, "档案").0,
        span_of(&row, "acc0").0,
        "左对齐列 (档案) 没对齐"
    );
    // 右对齐列: "金币" 与数值的**末字符列**必须一致
    assert_eq!(
        span_of(&hdr, "金币").1,
        span_of(&row, "987654321").1,
        "右对齐列 (金币) 没对齐"
    );
    // 表头标题必须跟着数据走: 那一列是"日志丢弃条数", 不是重试次数
    let (hdr_text, _) = compact_cells(&hdr);
    assert!(hdr_text.contains("丢弃"), "看板表头应写「丢弃」: {hdr_text:?}");
    assert!(
        !hdr_text.contains("重试"),
        "「重试」与那一列的数据对不上: {hdr_text:?}"
    );
}

#[tokio::test]
async fn removing_a_profile_leaves_the_survivors_log_intact() {
    // 摘账号会让下标全体前移, 而"渲染位置"的那份日志属于某一个具体会话。
    // 处理不当的两种后果: 把那个号的日志**抹掉**, 或者让它**串**到下一个
    // 选中项身上。这里摘掉当前选中项, 检查幸存者的日志还是自己的。
    let (mut app, _rxs, _ctx) = manual_app(2, tokio::runtime::Handle::current());
    for (i, marker) in [(0usize, "MARK-ZERO"), (1usize, "MARK-ONE")] {
        let set = app.sessions.as_ref().unwrap();
        set.iter().nth(i).unwrap().queue.emit(emit::Event::new(
            emit::Level::Info,
            emit::Category::Cmd,
            marker.to_string(),
        ));
    }
    app.poll_events();
    // 选中 1 号并同步 —— 此刻 `app.log` 里装的是 1 号的日志
    app.sessions.as_mut().unwrap().select(1);
    app.sync_from_selected();
    app.input = "profiles remove acc1".into();
    app.submit_for_test();

    assert_eq!(app.session_position(), Some((0, 1)), "只剩 acc0 且选中它");
    let text = frame_of(&mut app);
    assert!(text.contains("MARK-ZERO"), "幸存者 acc0 的日志必须还在:\n{text}");
    assert!(
        !text.contains("MARK-ONE"),
        "被摘掉的 acc1 的日志不能留在屏幕上:\n{text}"
    );
}

#[tokio::test]
async fn live_events_of_the_selected_session_reach_the_screen() {
    // 现场 (用户报的"Alt+↑↓ 切换时日志偶尔空白, 再切一下又能切出来"):
    //
    // 多会话下屏幕渲染的是 `App.log`, 而事件 drain 进的是各会话自己的
    // `BotSession.log`。两者必须是**同一个缓冲** —— 否则新日志只会堆在会话里,
    // 屏幕内容要靠"切换会话时的交换"才刷新一次, 表现就是时不时停在旧内容或
    // 空白。
    //
    // 这条不切换会话, 只往**当前选中**会话的队列丢一条, 检验它是否立刻上屏。
    let (mut app, _rxs, _ctx) = manual_app(2, tokio::runtime::Handle::current());
    {
        let set = app.sessions.as_ref().unwrap();
        let s = set.selected().expect("有选中的会话");
        s.queue.emit(emit::Event::new(
            emit::Level::Info,
            emit::Category::Cmd,
            "LIVE-MARKER".to_string(),
        ));
    }
    app.poll_events();
    let text = frame_of(&mut app);
    assert!(
        text.contains("LIVE-MARKER"),
        "选中会话新到的日志必须立刻出现在屏幕上, 不能等下一次切换"
    );
}

#[tokio::test]
async fn rapid_session_switching_keeps_the_log_consistent() {
    // 快速来回切账号 (Alt+↑↓ 连按) 之后, 屏幕上的日志必须仍然是**当前选中**
    // 会话的。交换是自反的: 切走再切回来, 内容要一模一样 (不能变空、不能串台)。
    let (mut app, _rxs, _ctx) = manual_app(2, tokio::runtime::Handle::current());
    // 给两个会话各写一条可区分的日志
    for (i, marker) in [(0usize, "MARK-ZERO"), (1usize, "MARK-ONE")] {
        let set = app.sessions.as_ref().unwrap();
        let s = set.iter().nth(i).unwrap();
        s.queue.emit(emit::Event::new(
            emit::Level::Info,
            emit::Category::Cmd,
            marker.to_string(),
        ));
    }
    app.poll_events();

    // 连按 Alt+↓ / Alt+↑ 若干次, 每次都跟着一帧
    for _ in 0..4 {
        app.select_session_move(1);
        app.poll_events();
        app.select_session_move(-1);
        app.poll_events();
    }
    // 回到 0 号: 必须看得到它自己的那条
    assert_eq!(app.session_position(), Some((0, 2)), "前置: 停在 0 号");
    let text = frame_of(&mut app);
    assert!(text.contains("MARK-ZERO"), "0 号的日志不能丢:\n{text}");
}

#[tokio::test]
async fn f4_prefills_the_wizard_from_the_selected_session_not_the_first_one() {
    // 现场 (用户截图): 选了**第二个**号按 F4, 向导里却是**第一个**号的账号。
    // 提交之后两个会话登同一个账号, 服务器把两边来回踢 —— 用户描述成
    // "两个账号相互的重连, 不停的挤兑"。
    //
    // 这里钉住"向导的信息来自**选中**会话", 与侧栏 `账号 2/2` 的显示一致。
    let (mut app, _rxs, _ctx) = manual_app(2, tokio::runtime::Handle::current());
    {
        // 两个号都抽掉密码, 否则 F4 会直接启动而不是开向导
        let set = app.sessions.as_mut().unwrap();
        for i in 0..set.len() {
            let s = set.iter_mut().nth(i).unwrap();
            let mut cfg = s.template().unwrap();
            cfg.password = String::new();
            s.set_template(cfg);
        }
        set.select(1);
    }
    assert_eq!(
        app.session_position(),
        Some((1, 2)),
        "前置: 选中的是第二个号 (侧栏显示的也是 2/2)"
    );
    app.toggle_selected_session();
    let w = app.wizard.as_ref().expect("没密码 → 弹向导");
    assert_eq!(w.account, "acc1", "向导必须预填**选中**会话的账号");
    assert_eq!(
        app.wizard_target.as_deref(),
        Some("acc1"),
        "凭据要能回写到正确的那个会话"
    );
}

#[tokio::test]
async fn f4_still_offers_the_wizard_after_a_failed_start() {
    // 现场 (用户截图): 手动模式下号没密码, 结果左栏亮着「启动失败 — 需人工」,
    // 用户想知道"怎么手动输密码"。
    //
    // 关键保证: 这个状态**不是死路** —— 按 F4 必须仍然能弹出登录向导补密码。
    // 反例是 `is_running()` 把"启动失败"也算成在跑, 那样 F4 会走成 stop, 用户
    // 就再也进不去向导了 (界面亮红 + 没有任何入口), 只能退出程序改档案。
    let (mut app, _rxs, _ctx) = manual_app(1, tokio::runtime::Handle::current());
    {
        let set = app.sessions.as_mut().unwrap();
        let s = set.iter_mut().next().unwrap();
        let mut cfg = s.template().unwrap();
        cfg.password = String::new();
        s.set_template(cfg);
    }
    // 模拟用户敲 `start` (而不是按 F4): 这条路直接失败, 把会话打成"启动失败"。
    app.run_builtin_on_selected(sessions::Builtin::Start);
    {
        let set = app.sessions.as_ref().unwrap();
        let s = set.iter().next().unwrap();
        assert!(s.start_failed(), "前置: 已经落到「启动失败」");
        assert!(!s.is_running(), "启动失败 ≠ 在运行 (否则 F4 会走成 stop)");
        assert!(!s.is_waiting_restart(), "没密码不该排定自动拉起");
    }
    // 现在按 F4 —— 必须还能拿到向导。
    app.toggle_selected_session();
    assert!(
        app.wizard.is_some(),
        "「启动失败」的会话按 F4 必须还能弹出向导补密码 (否则是死路)"
    );
    assert_eq!(
        app.wizard_target.as_deref(),
        Some("acc0"),
        "向导要挂在那个会话上, 提交后凭据才能用回它"
    );
    assert_eq!(
        app.sessions.as_ref().unwrap().iter().next().unwrap().generation(),
        0,
        "只是弹向导, 不该顺手 spawn"
    );
}

#[tokio::test]
async fn typing_start_behaves_like_f4_and_asks_for_a_missing_password() {
    // 裸敲的 `start` 与 F4 必须等价 —— `App::submit` 里那句注释就是这么写的。
    //
    // 现场踩到过不一致: 敲 `start` 时直接调 `run_builtin_on_selected(Start)`,
    // 绕开了 `toggle_selected_session` 里"没密码就弹向导"的分支, 于是同一个
    // 动作按键能补密码、敲指令只会报一句错并把号打成「启动失败」。
    let (mut app, _rxs, _ctx) = manual_app(1, tokio::runtime::Handle::current());
    {
        let set = app.sessions.as_mut().unwrap();
        let s = set.iter_mut().next().unwrap();
        let mut cfg = s.template().unwrap();
        cfg.password = String::new();
        s.set_template(cfg);
    }
    app.input = "start".into();
    app.submit_for_test();
    assert!(
        app.wizard.is_some(),
        "敲 `start` 而号没密码时也要弹向导 (与 F4 一致), 而不是先失败一次"
    );
    let set = app.sessions.as_ref().unwrap();
    assert!(
        !set.iter().next().unwrap().start_failed(),
        "不该把会话打成「启动失败」"
    );
    assert!(
        app.input.is_empty(),
        "本地指令处理完要清空输入框, 不能把 `start` 发给 bot"
    );
}


#[tokio::test]
async fn submitting_the_wizard_applies_the_password_and_starts_that_session() {
    // 完整闭环: 向导里敲密码 + Enter → 凭据写回**那个会话**并启动它。
    let (mut app, _rxs, ctx) = manual_app(1, tokio::runtime::Handle::current());
    {
        let set = app.sessions.as_mut().unwrap();
        let s = set.iter_mut().next().unwrap();
        let mut cfg = s.template().unwrap();
        cfg.password = String::new();
        s.set_template(cfg);
    }
    app.toggle_selected_session();
    assert!(app.wizard.is_some(), "前置: 向导已开");
    // 模拟"敲了密码然后回车"
    app.wizard.as_mut().unwrap().password = "s3cret".into();
    app.on_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Enter,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));

    // 1. 向导关掉、回到控制台
    assert!(app.wizard.is_none(), "提交后向导要关掉");
    assert_eq!(app.stage, app::STAGE_RUNNING, "回到运行阶段");
    // 2. 凭据写回该会话的模板 (下一次拉起也要能用)
    {
        let set = app.sessions.as_ref().unwrap();
        let s = set.iter().next().unwrap();
        assert_eq!(s.template().unwrap().password, "s3cret");
        assert_eq!(s.generation(), 1, "必须真的 spawn 了");
        assert!(!s.start_failed(), "启动成功就不该还挂着'启动失败'");
    }
    // 3. 也记进了凭据存储 (自动拉起时从那里取密码)
    {
        let creds = ctx.creds.lock().unwrap();
        assert_eq!(creds.password_for("acc0"), Some("s3cret"));
    }
    // 4. 告诉主线程"这次向导提交已经处理了" —— 否则它会再 spawn 一个全局会话,
    //    同一个账号被登两次
    assert!(
        app.login_apply.load(std::sync::atomic::Ordering::SeqCst),
        "必须置上 login_apply, 否则主线程会重复 spawn"
    );
}

#[tokio::test]
async fn esc_in_the_password_wizard_cancels_without_quitting() {
    // 给某个号补凭据而打开的向导, Esc 应该只是取消 ——
    // 用户很可能只是改主意了, 不该顺手退出整个控制台。
    let (mut app, _rxs, _ctx) = manual_app(1, tokio::runtime::Handle::current());
    {
        // 同前: 抽掉密码才会走"弹向导"这条路
        let set = app.sessions.as_mut().unwrap();
        let s = set.iter_mut().next().unwrap();
        let mut cfg = s.template().unwrap();
        cfg.password = String::new();
        s.set_template(cfg);
    }
    app.toggle_selected_session();
    assert!(app.wizard.is_some(), "前置: 缺密码 → 弹向导");
    app.on_key(ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Esc,
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    assert!(app.wizard.is_none(), "Esc 关掉向导");
    assert!(!app.force_quit, "但**不能**退出程序");
    assert_eq!(app.stage, app::STAGE_RUNNING, "回到控制台");
    assert!(app.wizard_target.is_none(), "目标要清掉");
}

#[tokio::test]
async fn a_busy_session_does_not_blank_the_status_panel() {
    // "状态栏闪烁"的真回归 (用户报"右侧状态面板大概 100ms 闪一下")。
    //
    // 根因: `sync_from_selected` 从前**每帧**用
    // `try_lock().map(Snapshot::from).unwrap_or_default()` 重读快照。bot 在
    // `command::tick` 里持锁干活 (含网络 I/O), 那一帧 `try_lock` 失败 → 右侧
    // 面板被塞进一个**全 0 的默认快照**显示一帧 → 下一帧又恢复。
    //
    // 正确行为: 拿不到锁就**保留上一份好数据**, 绝不显示全 0。
    let (mut app, _rxs) = multi_app(2);
    // 先让面板有一份真实数据
    {
        let set = app.sessions.as_ref().unwrap();
        let s = set.iter().next().unwrap();
        let mut st = s.state.try_lock().unwrap();
        st.phase = openstory_bot::state::Phase::InGame;
        st.level = 69;
        st.hp = 3802;
        st.maxhp = 3802;
        st.mapid = 100000100;
        st.meso = 123456;
    }
    app.poll_events();
    assert_eq!(app.snap.level, 69, "前置: 面板已有真实数据");

    // 模拟"bot 正持锁干活": 手动占住锁, 然后跑几帧。
    //
    // 这就是每 50ms 一次的真实情形 (`runtime.rs` 的 tick 分支持锁调
    // `command::tick`)。
    {
        // 先把 state 的 Arc 取出来 —— 不能一边持有对 `app.sessions` 的不可变
        // 借用, 一边再 `app.poll_events()` (那要可变借用)。
        let st_arc = {
            let set = app.sessions.as_ref().unwrap();
            set.iter().next().unwrap().state.clone()
        };
        let guard = st_arc.lock().await;
        for _ in 0..3 {
            app.poll_events();
            assert_eq!(
                app.snap.level, 69,
                "锁被占住时面板必须保留上一份数据, 不能变成全 0 (那是闪烁)"
            );
            assert_eq!(app.snap.hp, 3802, "血量也不能闪成 0");
            assert_eq!(app.snap.meso, 123456, "金币同理");
            assert_eq!(app.snap.phase, openstory_bot::state::Phase::InGame);
        }
        drop(guard);
    }
    // 锁放开后继续正常更新
    app.poll_events();
    assert_eq!(app.snap.level, 69);
}

// ── 阶段 5: 多会话 UI (离线, 不需要服务器) ──────────────────────────────

/// 造一个挂在虚拟会话上的 App, 用于离线验证多会话 UI 的形状。
///
/// 不启动任何 bot —— 只验证"版面/键位/目标分发"这些纯 UI 逻辑。
fn multi_app(n: usize) -> (App, Vec<tokio::sync::mpsc::Receiver<String>>) {
    let queue = Arc::new(openstory_console_lib::eventq::EventQueue::new(64));
    let state = Arc::new(Mutex::new(BotState::default()));
    let (cmd_tx, _rx) = mpsc::channel::<String>(4);
    let cmd_slot = Arc::new(StdMutex::new(cmd_tx));
    let stage = Arc::new(AtomicU8::new(STAGE_RUNNING));
    let outcome = Arc::new(StdMutex::new(None));
    let app = App::new(
        queue,
        state,
        cmd_slot,
        None,
        stage,
        outcome,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        false,
        "127.0.0.1".into(),
        8484,
        String::new(),
        String::new(),
        String::new(),
    );
    let specs: Vec<session::SessionSpec> = (0..n)
        .map(|i| session::SessionSpec::new(format!("profiles/acc{i}.json")))
        .collect();
    let mut set = sessions::SessionSet::from_specs(&specs);
    let rxs = set.pull_receivers();
    let mut app = app;
    // 测试里的会话都由调用方自己 spawn (或干脆不 spawn), 所以"已启动数"按 0
    // 记账 —— `keepalive` 由各用例自己按需设置。
    app.attach_sessions(set, None, 0);
    app.enter_running_for_test();
    (app, rxs)
}

#[test]
fn single_session_hides_the_sidebar_but_is_still_a_session_set() {
    // 用户问的是这个: 只有一个账号时, 左栏是不是不显示、默认选中它,
    // 与"以前的单开 TUI"版面对齐?
    //
    // 答案分两种情况, 更容易混的是第二种:
    //
    // 1. **完全不带 --profiles** (走档案向导) → 单会话路径, `App.sessions` 是
    //    `None`。版面与改造前的 TUI 逐字一致 —— 这是阶段 4 的硬性验收。
    // 2. **--profiles 只给一个档案** → 走**多会话**路径, 集合里有 1 个会话。
    //    版面同样隐藏左栏 (`set.len() > 1` 为假), 默认选中第 0 个 —— 看起来
    //    一样, 但底下是会话集合, 所以 `F4` 这类会话操作是可用的 (情况 1 不可用)。
    //
    // 两种都对; 这条测试钉住"1 个会话也要隐藏左栏"这个版面保证。
    let (app, _rxs) = multi_app(1);
    assert!(!app.show_sidebar, "单会话不该显示左栏");
    assert_eq!(app.session_position(), Some((0, 1)), "默认选中第 1 个");
    assert!(app.sessions.is_some(), "走的是会话集合路径 (与纯单开不同)");

    // 版面证据: 左栏宽度按 0 分配 —— 日志区从第 0 列开始
    let mut app = app;
    let text = frame_of(&mut app);
    // 左栏渲染时会画「账号 1/1」的标题; 隐藏时整条不渲染
    assert!(
        !text.contains("账号1/1"),
        "单会话不该画出左栏标题:\n{text}"
    );
}

#[test]
fn single_session_has_no_sidebar_and_no_alt_arrow_switching() {
    // 只有 1 个账号时 Alt+↑↓ 没有意义 —— 不该改变选中 (也不该 panic)。
    let (mut app, _rxs) = multi_app(1);
    assert!(!app.select_session_move(-1), "1 个会话时切不动");
    assert!(!app.select_session_move(1), "1 个会话时切不动");
    assert_eq!(app.session_position(), Some((0, 1)));
}

#[test]
fn multi_session_shows_the_sidebar() {
    let (app, _rxs) = multi_app(3);
    assert!(app.show_sidebar, "多会话应显示左栏");
    assert_eq!(app.session_position(), Some((0, 3)));
    assert_eq!(app.board_rows().len(), 3);
}

#[test]
fn alt_arrows_cycle_sessions_and_wrap() {
    let (mut app, _rxs) = multi_app(3);
    let key = |code, mods| crossterm::event::KeyEvent::new(code, mods);
    // Alt+↓ 前进
    app.on_key(key(
        crossterm::event::KeyCode::Down,
        crossterm::event::KeyModifiers::ALT,
    ));
    assert_eq!(app.session_position(), Some((1, 3)));
    // Alt+↑ 后退
    app.on_key(key(
        crossterm::event::KeyCode::Up,
        crossterm::event::KeyModifiers::ALT,
    ));
    assert_eq!(app.session_position(), Some((0, 3)));
    // 向前越界回卷
    app.on_key(key(
        crossterm::event::KeyCode::Up,
        crossterm::event::KeyModifiers::ALT,
    ));
    assert_eq!(app.session_position(), Some((2, 3)));
}

#[test]
fn plain_arrows_still_scroll_the_log_not_the_selection() {
    // 裸 ↑↓ 必须仍是日志滚动/输入历史 —— 否则会破坏既有习惯
    let (mut app, _rxs) = multi_app(2);
    let sel_before = app.session_position().unwrap().0;
    app.on_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Up,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert_eq!(
        app.session_position().unwrap().0,
        sel_before,
        "裸 ↑ 不该切换账号"
    );
}

#[test]
fn f2_toggles_board_only_in_multi_mode() {
    let (mut app, _rxs) = multi_app(2);
    assert!(!app.board_open);
    app.on_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::F(2),
        crossterm::event::KeyModifiers::NONE,
    ));
    assert!(app.board_open, "多会话 F2 应打开看板");
    app.on_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Esc,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert!(!app.board_open, "Esc 应关闭看板");

    // 单会话: F2 不做事, 版面不变
    let (mut one, _rxs) = multi_app(1);
    one.on_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::F(2),
        crossterm::event::KeyModifiers::NONE,
    ));
    assert!(!one.board_open, "单会话 F2 不该打开看板");
}

#[test]
fn at_all_dispatches_to_every_session() {
    let (mut app, mut rxs) = multi_app(2);
    app.input = "@all hunt on".into();
    app.submit_for_test();
    for (i, rx) in rxs.iter_mut().enumerate() {
        assert_eq!(
            rx.try_recv().ok(),
            Some("hunt on".to_string()),
            "会话 {i} 应收到 @all 指令"
        );
    }
}

#[test]
fn at_prefix_dispatches_only_to_matching_profiles() {
    let (mut app, mut rxs) = multi_app(2);
    app.input = "@acc1 view status".into();
    app.submit_for_test();
    assert!(rxs[0].try_recv().is_err(), "acc0 不该收到");
    assert_eq!(rxs[1].try_recv().ok(), Some("view status".to_string()));
}

#[test]
fn at_unknown_prefix_reports_error_and_sends_nothing() {
    let (mut app, mut rxs) = multi_app(2);
    app.input = "@nope hunt on".into();
    app.submit_for_test();
    for (i, rx) in rxs.iter_mut().enumerate() {
        assert!(rx.try_recv().is_err(), "会话 {i} 不该收到任何指令");
    }
    // 错误必须落到日志里 (不能静默)
    let n = app.log.total_display_lines(None, 100);
    assert!(n > 0, "目标无命中必须写日志提示, 不能静默");
}

#[test]
fn bare_command_goes_to_selected_session_only() {
    let (mut app, mut rxs) = multi_app(2);
    app.input = "hunt on".into();
    app.submit_for_test();
    // 单会话路径走 cmd_tx (指向选中会话) —— 这里只断言"没发给别人"
    assert!(rxs[1].try_recv().is_err(), "非选中会话不该收到");
}

#[test]
fn at_target_completion_appears_only_in_multi_mode() {
    let (mut app, _rxs) = multi_app(3);
    app.input = "@acc".into();
    app.refresh_completion_for_test();
    let cmds: Vec<&str> = app.comp.candidates.iter().map(|c| c.cmd.as_str()).collect();
    assert!(cmds.contains(&"@acc0"), "应补出账号名: {cmds:?}");
    assert!(cmds.contains(&"@acc2"), "应补出账号名: {cmds:?}");

    let (mut one, _rxs) = multi_app(1);
    one.input = "@acc".into();
    one.refresh_completion_for_test();
    assert!(
        one.comp.candidates.is_empty(),
        "单会话不该出现 `@` 目标补全"
    );
}

// ── 阶段 6: 自动拉起在 App 里的接线 (离线) ──────────────────────────────

#[test]
fn the_log_filter_is_per_session() {
    // 过滤是"我在看哪个号"的一部分。全局一份的话, 在 A 上切到"聊天", 切到 B
    // 也变成"聊天" —— 而 B 的日志根本没被看过, 用户会以为 B 没有日志。
    let (mut app, _rxs) = multi_app(2);
    let chat = app::Filter::Cat(openstory_bot::emit::Category::Chat);
    // A 上切到「聊天」
    app.filter = chat;
    app.sync_from_selected();
    assert_eq!(app.filter, chat, "当前会话的过滤应生效");

    // 切到 B: 过滤应回到 B 自己的状态 (默认"全部"), 而不是跟着 A 走
    assert!(app.select_session_move(1));
    app.sync_from_selected();
    assert_eq!(app.filter, app::Filter::All, "B 的过滤不该被 A 改掉");
    // 在 B 上切到「公告」
    let notice = app::Filter::Cat(openstory_bot::emit::Category::Notice);
    app.filter = notice;
    app.sync_from_selected();

    // 切回 A: A 的"聊天"必须还在
    assert!(app.select_session_move(-1));
    app.sync_from_selected();
    assert_eq!(app.filter, chat, "A 的过滤必须被记住");
}

#[test]
fn filter_code_round_trips() {
    for f in app::Filter::ORDER {
        assert_eq!(app::Filter::from_code(f.code()), *f, "{f:?} 编码应可还原");
    }
    // 越界的下标回落到「全部」而不是 panic (文件格式变了 / 页签减少了)
    assert_eq!(app::Filter::from_code(200), app::Filter::All);
}

/// 造一个挂了 `RestartCtx` 的 App (需要 tokio handle, 所以是 async 测试)。
fn multi_app_with_restart(
    n: usize,
    handle: tokio::runtime::Handle,
) -> (
    App,
    Vec<tokio::sync::mpsc::Receiver<String>>,
    watcher::RestartCtx,
) {
    multi_app_with_restart_and_policy(
        n,
        handle,
        watcher::RestartPolicy {
            enabled: true,
            max_attempts: 3,
            backoff_secs: vec![0],
        },
    )
}

/// 同上, 但指定退避策略。
///
/// 渲染类的断言需要**足够长**的退避窗口: 用 `backoff_secs: [0]` 的话,
/// "5 秒后重连"在测试跑到渲染那一步时早就过期了, 左栏显示的是"正在重连…"
/// (那是对的行为, 只是断言不到倒计时)。给一个很长的窗口才能稳稳定住画面。
fn multi_app_with_restart_and_policy(
    n: usize,
    handle: tokio::runtime::Handle,
    policy: watcher::RestartPolicy,
) -> (
    App,
    Vec<tokio::sync::mpsc::Receiver<String>>,
    watcher::RestartCtx,
) {
    let (mut app, rxs) = multi_app(n);
    // multi_app 已经把集合挂上了 (无 restart), 这里重建一个带 restart 的
    let specs: Vec<session::SessionSpec> = (0..n)
        .map(|i| session::SessionSpec::new(format!("profiles/acc{i}.json")))
        .collect();
    let mut set = sessions::SessionSet::from_specs_with_policy(&specs, policy);
    let rxs2 = set.pull_receivers();
    let ctx = watcher::RestartCtx::new(handle, watcher::RestartPolicy::default());
    // 这些用例把会话当作"已经在跑" (它们自己造结束状态), 所以已启动数 = n。
    app.attach_sessions(set, Some(ctx.clone()), n);
    app.enter_running_for_test();
    let _ = rxs;
    (app, rxs2, ctx)
}

/// 一个"几乎不会到点"的退避策略 —— 让左边栏停在"还有 N 秒重连"的画面上。
fn long_backoff_policy() -> watcher::RestartPolicy {
    watcher::RestartPolicy {
        enabled: true,
        max_attempts: 3,
        backoff_secs: vec![3600],
    }
}

/// 把会话 `i` 拨到"刚以 `outcome` 结束"。
fn end_session(app: &mut App, i: usize, outcome: runtime::RunOutcome) {
    let set = app.sessions.as_mut().unwrap();
    let s = set.iter_mut().nth(i).unwrap();
    if let Ok(mut o) = s.outcome.lock() {
        *o = Some(outcome);
    }
    s.stage
        .store(session::STAGE_FINISHED, std::sync::atomic::Ordering::SeqCst);
}

#[tokio::test]
async fn a_clean_shutdown_sets_all_done_and_zeroes_keepalive() {
    // 退出条件: 所有会话都结束且不打算重连 → 主线程才能收尾。
    // 这条错了会有两种后果: 程序永远不退出 (keepalive 不降), 或者
    // 重连到一半就退出 (all_done 提前置位)。
    let (mut app, _rxs, ctx) = multi_app_with_restart(3, tokio::runtime::Handle::current());
    assert_eq!(ctx.keepalive(), 3, "attach 时按会话数记上");
    assert!(!ctx.is_all_done());
    for i in 0..3 {
        end_session(&mut app, i, runtime::RunOutcome::Quit);
    }
    app.drive_restarts(std::time::Instant::now());
    assert_eq!(ctx.keepalive(), 0, "全部正常退出后计数必须归零");
    assert!(ctx.is_all_done(), "全部结束必须通知主线程");
    assert!(!ctx.counters.has_errors(), "正常退出不是错误");
}

#[tokio::test]
async fn drive_restarts_keeps_a_disconnected_session_alive() {
    // 掉线的会话不能被算作"完事了" —— 它还会被拉起来
    let (mut app, _rxs, ctx) = multi_app_with_restart(2, tokio::runtime::Handle::current());
    end_session(&mut app, 0, runtime::RunOutcome::ConnectionClosed);
    end_session(&mut app, 1, runtime::RunOutcome::Quit);
    app.drive_restarts(std::time::Instant::now());
    assert_eq!(ctx.keepalive(), 1, "还在等重连的会话要算活着");
    assert!(!ctx.is_all_done(), "还有会话要重连, 不能收尾");
    // 掉线那条必须写进它自己的日志 (多开时才知道是哪个号出事)
    let set = app.sessions.as_ref().unwrap();
    let a = set.iter().next().unwrap();
    assert!(a.is_waiting_restart(), "断线必须排定拉起");
    let b = set.iter().nth(1).unwrap();
    assert!(!b.is_waiting_restart(), "正常退出的不该被排定");
}

#[tokio::test]
async fn a_session_without_a_template_disables_its_own_restart() {
    // 档案从未加载成功 → 拉不起来。必须**关掉**自动拉起并记一条错误,
    // 否则 UI 会永远显示一个不会发生的倒计时。
    let (mut app, _rxs, ctx) = multi_app_with_restart(1, tokio::runtime::Handle::current());
    end_session(&mut app, 0, runtime::RunOutcome::ConnectionClosed);
    app.drive_restarts(std::time::Instant::now());
    // 第一次只是排定 (还没到点), 第二次才到点尝试拉起
    app.drive_restarts(std::time::Instant::now() + Duration::from_secs(1));
    assert!(ctx.counters.has_errors(), "必须记下「拉不起来」这件事");
    let e = ctx.counters.first_error().unwrap();
    assert!(e.contains("档案未能加载"), "got {e}");
    let set = app.sessions.as_ref().unwrap();
    assert!(
        !set.iter().next().unwrap().restart_enabled(),
        "拉不起来之后必须关掉自动拉起"
    );
    assert_eq!(ctx.keepalive(), 0, "放弃之后也就不算活着了");
    assert!(ctx.is_all_done(), "没人再动了 → 可以收尾");
}

#[tokio::test]
async fn a_restartable_session_is_actually_respawned() {
    // 有模板 → 到点必须真的重新 spawn (代数 +1), 而不是只改了个状态。
    let (mut app, _rxs, ctx) = multi_app_with_restart(1, tokio::runtime::Handle::current());
    // 给会话一份"连不上"的模板: 端口 1 上不会有服务
    {
        let set = app.sessions.as_mut().unwrap();
        let s = set.iter_mut().next().unwrap();
        s.set_template(Config {
            ip: "127.0.0.1".into(),
            port: 1,
            account: "t".into(),
            password: "t".into(),
            ..Config::default()
        });
    }
    end_session(&mut app, 0, runtime::RunOutcome::ConnectionClosed);
    app.drive_restarts(std::time::Instant::now());
    app.drive_restarts(std::time::Instant::now() + Duration::from_secs(1));
    let set = app.sessions.as_ref().unwrap();
    let s = set.iter().next().unwrap();
    assert_eq!(s.generation(), 1, "必须真的重新拉起过");
    assert_eq!(ctx.counters.spawned_count(), 1);
    assert!(!ctx.counters.has_errors(), "拉起本身不该算错误");
}

#[tokio::test]
async fn f3_restarts_the_selected_session() {
    // F3 是本阶段新增的唯一键位 —— 按下必须真的拉起, 且不打扰别的会话。
    let (mut app, _rxs, _ctx) = multi_app_with_restart(2, tokio::runtime::Handle::current());
    {
        let set = app.sessions.as_mut().unwrap();
        for i in 0..2 {
            let s = set.iter_mut().nth(i).unwrap();
            s.set_template(Config {
                ip: "127.0.0.1".into(),
                port: 1,
                account: "t".into(),
                password: "t".into(),
                ..Config::default()
            });
        }
    }
    end_session(&mut app, 0, runtime::RunOutcome::ConnectionClosed);
    end_session(&mut app, 1, runtime::RunOutcome::ConnectionClosed);
    assert_eq!(
        app.stage,
        session::STAGE_RUNNING,
        "前置条件: 应用在运行阶段"
    );
    assert!(app.show_sidebar, "前置条件: 多会话面板已显示");
    app.on_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::F(3),
        crossterm::event::KeyModifiers::NONE,
    ));
    let set = app.sessions.as_ref().unwrap();
    let a = set.iter().next().unwrap();
    assert_eq!(a.generation(), 1, "选中会话应被真的拉起");
    assert!(
        a.outcome().is_none(),
        "拉起之后旧的结束原因必须清掉, 否则 UI 会一直显示上一次的失败"
    );
    assert_eq!(
        set.iter().nth(1).unwrap().generation(),
        0,
        "没选中的会话不该被重启"
    );
}

// ── 阶段 6: 左栏/看板的拉起状态渲染 (离线) ────────────────────────────

/// 把 App 渲染成一屏文本 (看板/左栏的断言用)。
///
/// 返回的文本已 `squish` (去掉全部空白) —— TUI 在 CJK 之间插空格对齐显示
/// 宽度, 不去掉的话子串匹配会莫名其妙失败。
fn frame_of(app: &mut App) -> String {
    let backend = TestBackend::new(140, 40);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| ui::render(f, app)).unwrap();
    squish(&dump(&term))
}

/// 只取左侧账号列表那一条 (宽 24, 与 `ui::render` 的 `side_w` 一致)。
///
/// 断言"某一行的详情属于哪个会话"时必须限定在左栏内 —— 右栏的角色信息面板
/// 里也会出现会话名, 整屏匹配会串到右栏去。
fn sidebar_of(app: &mut App) -> String {
    let backend = TestBackend::new(140, 40);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| ui::render(f, app)).unwrap();
    let buf = term.backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..24.min(buf.area.width) {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    squish(&out)
}

#[tokio::test]
async fn sidebar_shows_the_restart_countdown() {
    // 左栏必须显示"还有几秒重连" —— 这是用户判断"要不要过去看一眼"的唯一
    // 线索。数字要按到点时刻**现算**, 不能缓存 (缓存的会停在排定那一刻)。
    let (mut app, _rxs, _ctx) = multi_app_with_restart_and_policy(
        2,
        tokio::runtime::Handle::current(),
        long_backoff_policy(),
    );
    end_session(&mut app, 0, runtime::RunOutcome::ConnectionClosed);
    end_session(&mut app, 1, runtime::RunOutcome::Quit);
    // 排定退避 (3600 秒) 但**不**到点
    app.drive_restarts(std::time::Instant::now());
    let text = frame_of(&mut app);
    assert!(
        text.contains("3599s后重连") || text.contains("3600s后重连"),
        "左栏应显示倒计时:\n{text}"
    );
    assert!(text.contains('↻'), "等重连的会话应有 ↻ 标记:\n{text}");
    // 正常退出的那个不该被标成在重连
    assert!(text.contains("○acc1"), "正常退出的会话不该显示 ↻:\n{text}");
}

#[tokio::test]
async fn sidebar_countdown_is_recomputed_each_frame() {
    // 缓存的秒数会停在排定那一刻 —— 那是个一直不动的假数字。
    //
    // 注意用 2 个会话: 左栏只在多会话时出现 (单档版面必须与老 TUI 逐字一致)。
    let (mut app, _rxs, _ctx) = multi_app_with_restart_and_policy(
        2,
        tokio::runtime::Handle::current(),
        long_backoff_policy(),
    );
    end_session(&mut app, 0, runtime::RunOutcome::ConnectionClosed);
    end_session(&mut app, 1, runtime::RunOutcome::Quit);
    let t0 = std::time::Instant::now();
    app.drive_restarts(t0);
    let before = frame_of(&mut app);
    assert!(before.contains("3599s后重连"), "初始应是 3599s:\n{before}");
    // 让渲染时的 "现在" 变晚 —— 倒计时用的是 `Instant::now()`, 所以只能真等。
    // 等 2 秒足够把 3599 降到 3596..3597; 断言"变小了"而不是具体值 (防 flaky)。
    std::thread::sleep(Duration::from_millis(2100));
    let after = frame_of(&mut app);
    assert!(
        after.contains("3596s后重连") || after.contains("3597s后重连"),
        "倒计时必须按当前时刻现算, 而不是用缓存值:\n{after}"
    );
}

#[tokio::test]
async fn sidebar_marks_a_given_up_session() {
    // 拉不起来 (档案没加载成功) 之后必须有一条明确的"需人工"提示, 而不是
    // 继续显示一个不会发生的倒计时。
    let (mut app, _rxs, ctx) = multi_app_with_restart(2, tokio::runtime::Handle::current());
    // 不给模板 → 拉不起来 (会关掉自动拉起并记错误)
    end_session(&mut app, 0, runtime::RunOutcome::ConnectionClosed);
    end_session(&mut app, 1, runtime::RunOutcome::Quit);
    app.drive_restarts(std::time::Instant::now());
    app.drive_restarts(std::time::Instant::now() + Duration::from_secs(60));
    assert!(ctx.counters.has_errors(), "前置条件: 拉起失败");
    let set = app.sessions.as_ref().unwrap();
    assert!(!set.iter().next().unwrap().restart_enabled());
    let text = frame_of(&mut app);
    assert!(!text.contains("后重连"), "关掉之后不该再有倒计时:\n{text}");
    assert!(text.contains("需人工"), "应提示需要人工处理:\n{text}");
}

#[tokio::test]
async fn sidebar_marks_given_up_after_max_attempts() {
    // 试满 3 次之后必须显示「已放弃重连 — 需人工」
    let (mut app, _rxs, _ctx) = multi_app_with_restart(2, tokio::runtime::Handle::current());
    for i in 0..2 {
        let s = app.sessions.as_mut().unwrap().iter_mut().nth(i).unwrap();
        s.set_restart_enabled(true);
        s.set_template(Config {
            ip: "127.0.0.1".into(),
            port: 1,
            account: "t".into(),
            password: "t".into(),
            ..Config::default()
        });
    }
    // 三圈"掉线 → 到点 → 拉起" (只折腾 acc0)
    let mut t = std::time::Instant::now();
    for _ in 0..3 {
        end_session(&mut app, 0, runtime::RunOutcome::ConnectionClosed);
        app.drive_restarts(t);
        t += Duration::from_secs(60);
        app.drive_restarts(t);
        t += Duration::from_secs(60);
    }
    end_session(&mut app, 0, runtime::RunOutcome::ConnectionClosed);
    app.drive_restarts(t);
    end_session(&mut app, 1, runtime::RunOutcome::Quit);
    app.drive_restarts(t);
    let s = app.sessions.as_ref().unwrap().iter().next().unwrap();
    assert!(s.gave_up_restart(), "三轮之后必须放弃");
    let text = frame_of(&mut app);
    assert!(text.contains("已放弃重连"), "左栏应显示已放弃:\n{text}");
    assert!(text.contains("需人工"), "应提示需要人工处理:\n{text}");
}

#[tokio::test]
async fn board_shows_restart_state() {
    // F2 看板要把"几秒后重连"显示在阶段列里 (一眼扫全部账号的场合)
    let (mut app, _rxs, _ctx) = multi_app_with_restart(2, tokio::runtime::Handle::current());
    end_session(&mut app, 0, runtime::RunOutcome::ConnectionClosed);
    end_session(&mut app, 1, runtime::RunOutcome::Failed);
    app.drive_restarts(std::time::Instant::now());
    app.on_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::F(2),
        crossterm::event::KeyModifiers::NONE,
    ));
    let text = frame_of(&mut app);
    assert!(text.contains("s后重连"), "看板应显示倒计时:\n{text}");
    assert!(text.contains("需人工"), "登录失败应显示需人工:\n{text}");
}

// ── 阶段 6: 密码双模式 (凭据存储) 在 App 里的接线 ──────────────────────

#[tokio::test]
async fn automatic_restart_takes_the_password_from_the_store() {
    // 档案里没写密码时, 自动拉起必须从凭据存储补上 —— 否则新会话会在登录
    // 阶段失败并被判定为"认证问题, 不该重试", 自动拉起等于没生效。
    let (mut app, _rxs, ctx) = multi_app_with_restart(2, tokio::runtime::Handle::current());
    for i in 0..2 {
        let s = app.sessions.as_mut().unwrap().iter_mut().nth(i).unwrap();
        s.set_template(Config {
            ip: "127.0.0.1".into(),
            port: 1,
            account: "t".into(),
            password: String::new(), // 档案里没有
            ..Config::default()
        });
    }
    {
        let mut c = ctx.creds.lock().unwrap();
        c.record("acc0", "t", "FROM_STORE_0");
        c.record("acc1", "t", "FROM_STORE_1");
    }
    end_session(&mut app, 0, runtime::RunOutcome::ConnectionClosed);
    end_session(&mut app, 1, runtime::RunOutcome::ConnectionClosed);
    app.drive_restarts(std::time::Instant::now());
    app.drive_restarts(std::time::Instant::now() + Duration::from_secs(1));
    let set = app.sessions.as_ref().unwrap();
    for (i, want) in ["FROM_STORE_0", "FROM_STORE_1"].iter().enumerate() {
        let s = set.iter().nth(i).unwrap();
        assert_eq!(s.generation(), 1, "会话 {i} 应被拉起");
        assert_eq!(
            s.template().unwrap().password,
            *want,
            "会话 {i} 必须用存储里它自己的密码 (不能串到另一个档案)"
        );
    }
}

#[tokio::test]
async fn manual_restart_also_takes_the_password_from_the_store() {
    // 手动重启是另一条代码路径, 也得补密码
    let (mut app, _rxs, ctx) = multi_app_with_restart(1, tokio::runtime::Handle::current());
    {
        let s = app.sessions.as_mut().unwrap().iter_mut().next().unwrap();
        s.set_template(Config {
            ip: "127.0.0.1".into(),
            port: 1,
            account: "t".into(),
            password: String::new(),
            ..Config::default()
        });
    }
    ctx.creds.lock().unwrap().record("acc0", "t", "MANUAL_PW");
    end_session(&mut app, 0, runtime::RunOutcome::ConnectionClosed);
    app.manual_restart_selected();
    let s = app.sessions.as_ref().unwrap().iter().next().unwrap();
    assert_eq!(s.generation(), 1);
    assert_eq!(s.template().unwrap().password, "MANUAL_PW");
}

#[tokio::test]
async fn sidebar_shows_name_and_char_id_instead_of_level_stats() {
    // 左栏只回答"这是哪个角色": 角色名 + 角色 ID。等级/地图/血量属于详细信息,
    // 22 列装不下会被折成两行 ("Lv69 射手村集" / "3802/3802"), 把"一眼扫全部
    // 账号"的用途挤没了 —— 要看细节有 F2 看板与右侧角色面板。
    let (mut app, _rxs, _ctx) = multi_app_with_restart(2, tokio::runtime::Handle::current());
    {
        let set = app.sessions.as_mut().unwrap();
        let s = set.iter_mut().next().unwrap();
        s.stage
            .store(session::STAGE_RUNNING, std::sync::atomic::Ordering::SeqCst);
        let mut st = s.state.try_lock().unwrap();
        st.phase = openstory_bot::state::Phase::InGame;
        st.name = "Player".into();
        st.my_cid = 3953;
        st.level = 69;
        st.hp = 3802;
        st.maxhp = 3802;
        st.mapid = 100000100;
    }
    app.poll_events();
    let text = sidebar_of(&mut app);
    assert!(text.contains("Player"), "左栏要有角色名:\n{text}");
    assert!(text.contains("3953"), "左栏要有角色 ID:\n{text}");
    // 详细信息一律不再出现
    for gone in ["Lv69", "3802", "射手"] {
        assert!(
            !text.contains(gone),
            "左栏不该再显示详细信息 {gone:?} (那是看板/右栏的事):\n{text}"
        );
    }
}

#[tokio::test]
async fn each_sidebar_row_shows_its_own_session_phase_not_the_selected_one() {
    // 现场症状: 同一个 Lv69 角色在两行之间来回跳, 看着像"状态串台"。
    //
    // 左栏每一行的详情只能来自**那一行自己的会话状态**, 与"当前选中谁"无关。
    // 真串台的话这里会看到两行的详情在连续几帧里交替 —— 那是最坏的一类 bug
    // (用户会以为某个号还活着)。
    //
    // 用 4 帧连续渲染 + 每帧 poll_events 覆盖住"同步缓存"那条路径:
    // `sync_from_selected` 会改写 `app.snap`, 若它被误当成某一行的来源,
    // 两行就会一起变成选中会话的样子。
    let (mut app, _rxs, _ctx) = multi_app_with_restart(2, tokio::runtime::Handle::current());
    {
        let set = app.sessions.as_mut().unwrap();
        {
            let s = set.iter_mut().next().unwrap();
            // 会话必须在跑, 否则左栏走的是"未启动"分支, 测不到阶段渲染
            s.stage
                .store(session::STAGE_RUNNING, std::sync::atomic::Ordering::SeqCst);
            let mut st = s.state.try_lock().unwrap();
            st.phase = openstory_bot::state::Phase::InGame;
            // 左栏现在只显示角色名 + 角色 ID —— 用它当"这一行自己的数据"的标记。
            st.name = "hero0".into();
            st.my_cid = 3953;
            // 下面这些属于"详细信息", 左栏**不该**再渲染它们 (留给 F2 看板与右栏)。
            st.level = 69;
            st.hp = 3802;
            st.maxhp = 3802;
            st.mapid = 100000100;
        }
        {
            let s = set.iter_mut().nth(1).unwrap();
            s.stage
                .store(session::STAGE_RUNNING, std::sync::atomic::Ordering::SeqCst);
            let mut st = s.state.try_lock().unwrap();
            st.phase = openstory_bot::state::Phase::Disconnected;
        }
    }
    for round in 0..4 {
        app.poll_events();
        // 只看左栏那 24 列 —— 右栏的"角色信息"里也有 acc0/acc1 字样, 整屏
        // 匹配会把右栏的内容当成左栏的行。
        let text = sidebar_of(&mut app);
        let p0 = text
            .find("acc0")
            .unwrap_or_else(|| panic!("round {round}: 左栏缺 acc0:\n{text}"));
        let p1 = text
            .find("acc1")
            .unwrap_or_else(|| panic!("round {round}: 左栏缺 acc1:\n{text}"));
        assert!(p0 < p1, "round {round}: 左栏行序必须稳定:\n{text}");
        let seg0 = &text[p0..p1];
        let seg1 = &text[p1..];
        assert!(
            seg0.contains("hero0") && !seg0.contains("未连接"),
            "round {round}: acc0 必须一直显示自己的角色名 hero0:\n{seg0}"
        );
        assert!(
            seg1.contains("未连接") && !seg1.contains("hero0"),
            "round {round}: acc1 必须一直显示自己的未连接 (出现 hero0 = 状态串台):\n{seg1}"
        );
    }
}

#[tokio::test]
async fn the_store_never_writes_to_disk_by_default() {
    // 核心层的纪律是"密码永不落盘"; 控制台只在用户显式 `--remember-password`
    // 时才破例。这条测的是默认值那一边 —— 它错了会在用户毫不知情的情况下
    // 往磁盘写明文密码。
    let dir = std::env::temp_dir().join(format!("ost_rr_cred_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("manager.json");
    let mut creds = credentials::Credentials::load_from(&path, credentials::Remember::No);
    creds.record("acc0", "t", "pw");
    creds.save().unwrap();
    assert!(!path.exists(), "默认绝不能创建凭据文件");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_store_round_trips_when_remembering() {
    // 显式打开之后必须真的能跨进程留住 (否则"程序重启后自动拉起"是空话)
    let dir = std::env::temp_dir().join(format!("ost_rr_cred2_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("manager.json");
    let mut creds = credentials::Credentials::load_from(&path, credentials::Remember::Yes);
    creds.record("本地服_100000001", "100000001", "test_password");
    creds.save().unwrap();
    let back = credentials::Credentials::load_from(&path, credentials::Remember::Yes);
    assert_eq!(back.password_for("本地服_100000001"), Some("test_password"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ── 阶段 8: 手动启停 (手动模式) ───────────────────────────────────────

/// 手动模式的 App: 集合挂上, 但**一个都不启动**。
fn manual_app(
    n: usize,
    handle: tokio::runtime::Handle,
) -> (App, Vec<tokio::sync::mpsc::Receiver<String>>, watcher::RestartCtx) {
    let specs: Vec<session::SessionSpec> = (0..n)
        .map(|i| session::SessionSpec::new(format!("profiles/acc{i}.json")))
        .collect();
    let mut set =
        sessions::SessionSet::from_specs_with_policy(&specs, watcher::RestartPolicy::default());
    for i in 0..set.len() {
        let s = set.iter_mut().nth(i).unwrap();
        s.set_template(Config {
            ip: "127.0.0.1".into(),
            port: 1,
            account: format!("acc{i}"),
            password: "t".into(),
            ..Config::default()
        });
    }
    let rxs = set.pull_receivers();
    let ctx = watcher::RestartCtx::new(handle, watcher::RestartPolicy::default());
    let mut app = App::new(
        Arc::new(openstory_console_lib::eventq::EventQueue::new(64)),
        Arc::new(Mutex::new(BotState::default())),
        Arc::new(StdMutex::new(mpsc::channel::<String>(64).0)),
        None,
        Arc::new(AtomicU8::new(STAGE_RUNNING)),
        Arc::new(StdMutex::new(None)),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        false,
        "127.0.0.1".into(),
        8484,
        String::new(),
        String::new(),
        String::new(),
    );
    // started = 0: 手动模式什么都不启动
    app.attach_sessions(set, Some(ctx.clone()), 0);
    app.enter_running_for_test();
    // `enter_running_for_test` 会写 `stage_flag` —— 而 `attach_sessions` 把
    // 那个 Arc 指向**选中会话自己的** stage, 所以上面那一步顺带把会话也标成了
    // "运行中"。手动模式下一个号都没启动, 状态必须还原成"向导 (从未启动)",
    // 否则 `prepare_start` 会（正确地）拒绝启动它。
    {
        let set = app.sessions.as_mut().unwrap();
        for i in 0..set.len() {
            let s = set.iter_mut().nth(i).unwrap();
            s.stage
                .store(session::STAGE_WIZARD, std::sync::atomic::Ordering::SeqCst);
        }
    }
    (app, rxs, ctx)
}

#[tokio::test]
async fn manual_mode_does_not_let_the_program_finish_while_sessions_are_unstarted() {
    // 手动模式最危险的退出口: 用户按了一次失败的 start, 或启动了一个号让它跑完,
    // 剩下的号还没动过 —— 那一刻若 `all_done` 置位, 程序就退出了, 剩下的号根本
    // 没机会被启动。
    let (mut app, _rxs, ctx) = manual_app(2, tokio::runtime::Handle::current());
    assert_eq!(ctx.keepalive(), 0, "手动模式: 一个都没启动");
    app.drive_restarts(std::time::Instant::now());
    assert!(
        !ctx.is_all_done(),
        "还有号从没启动过 → 绝不能让主线程收尾"
    );
}

#[tokio::test]
async fn every_session_started_and_stopped_lets_the_program_finish() {
    // 反面: 用户启动过所有号, 又把它们都停掉 —— 这时没有任何东西会再动,
    // 程序必须肯退出。
    let (mut app, _rxs, ctx) = manual_app(2, tokio::runtime::Handle::current());
    let h = tokio::runtime::Handle::current();
    for i in 0..2 {
        let Some(set) = app.sessions.as_mut() else {
            panic!("有多会话")
        };
        assert!(set.start_session(&h, i, None).is_ok(), "启动第 {i} 个");
    }
    app.drive_restarts(std::time::Instant::now());
    assert!(!ctx.is_all_done(), "还在跑 → 不能收尾");
    for i in 0..2 {
        let set = app.sessions.as_mut().unwrap();
        assert!(set.stop_session(i).is_ok());
        // bot 收尾 (真实路径里由监督任务置位)
        let s = set.iter_mut().nth(i).unwrap();
        s.stage
            .store(session::STAGE_FINISHED, std::sync::atomic::Ordering::SeqCst);
    }
    app.drive_restarts(std::time::Instant::now());
    assert!(ctx.is_all_done(), "全都停掉了 → 必须肯收尾");
}

#[tokio::test]
async fn a_failed_start_does_not_wedge_the_exit_condition() {
    // 关键回归 (真实踩到过): 启动失败若仍算"从未启动", `has_never_started()`
    // 就永远为真 —— 用户按一次失败的 start, 程序再也不会自己退出。
    let (mut app, _rxs, ctx) = manual_app(1, tokio::runtime::Handle::current());
    {
        // 抽掉密码 → 启动必然失败
        let set = app.sessions.as_mut().unwrap();
        let s = set.iter_mut().next().unwrap();
        let mut cfg = s.template().unwrap();
        cfg.password = String::new();
        s.set_template(cfg);
        assert!(set.start_session(&tokio::runtime::Handle::current(), 0, None).is_err());
    }
    app.drive_restarts(std::time::Instant::now());
    {
        let set = app.sessions.as_ref().unwrap();
        let s = set.iter().next().unwrap();
        eprintln!(
            "DIAG never_started={} start_failed={} manual={} live={} all_stopped={} has_ns={} all_done={}",
            s.never_started(),
            s.start_failed(),
            s.is_manually_stopped(),
            set.live_count(),
            set.all_stopped(),
            set.has_never_started(),
            ctx.is_all_done()
        );
    }
    assert!(
        ctx.is_all_done(),
        "启动失败之后没有任何东西会再动它 → 必须肯收尾 (否则程序永远不退出)"
    );
}

#[tokio::test]
async fn sidebar_shows_unstarted_and_stopped_distinctly() {
    // 两种"没在跑"必须能被区分开:
    // - 从没启动 → 「未启动 — F4 启动」(用户下一步是去启动它)
    // - 用户停的 → 「已停止 (手动)」(用户下一步是别去管它)
    // 混成一句话的话, 用户看不出自己到底停过没有。
    let (mut app, _rxs, _ctx) = manual_app(2, tokio::runtime::Handle::current());
    let text = sidebar_of(&mut app);
    assert!(text.contains("未启动"), "从没启动的号要写清楚:\n{text}");
    assert!(text.contains("F4"), "要告诉用户按什么启动:\n{text}");
    assert!(!text.contains("已停止"), "还没停过就不该说已停止:\n{text}");

    // 启动并停掉第一个
    let h = tokio::runtime::Handle::current();
    {
        let set = app.sessions.as_mut().unwrap();
        assert!(set.start_session(&h, 0, None).is_ok());
        assert!(set.stop_session(0).is_ok());
        let s = set.iter_mut().next().unwrap();
        s.stage
            .store(session::STAGE_FINISHED, std::sync::atomic::Ordering::SeqCst);
    }
    let text = sidebar_of(&mut app);
    assert!(text.contains("已停止"), "停掉的号要写'已停止':\n{text}");
    assert!(!text.contains("需人工"), "用户自己停的不该报警:\n{text}");
}

#[tokio::test]
async fn sidebar_marks_a_failed_start_as_needing_attention() {
    // 启动失败必须明确报出来 —— 否则界面看起来和"从没启动过"一样, 用户会
    // 反复按同一个键而不知道少了密码。
    //
    // 用 2 个会话: 左栏只在多会话时存在 (单档版面必须与老 TUI 逐字一致)。
    let (mut app, _rxs, _ctx) = manual_app(2, tokio::runtime::Handle::current());
    {
        let set = app.sessions.as_mut().unwrap();
        let s = set.iter_mut().next().unwrap();
        let mut cfg = s.template().unwrap();
        cfg.password = String::new();
        s.set_template(cfg);
        assert!(set
            .start_session(&tokio::runtime::Handle::current(), 0, None)
            .is_err());
    }
    let text = sidebar_of(&mut app);
    assert!(text.contains("启动失败"), "必须报出启动失败:\n{text}");
    assert!(text.contains("需人工"), "并标记需人工:\n{text}");
    assert!(text.contains('✕'), "红叉状态点:\n{text}");
    // 另一个没被动过的号仍然显示"未启动"—— 两种状态不能混为一谈
    assert!(text.contains("未启动"), "另一个号该是'未启动':\n{text}");
}

#[tokio::test]
async fn f4_starts_a_stopped_session_and_stops_a_running_one() {
    // 一个键管两件事: 没跑就启动, 在跑就停掉。按键失效的话手动模式整个没用。
    let (mut app, _rxs, _ctx) = manual_app(1, tokio::runtime::Handle::current());
    assert!(app.sessions.as_ref().unwrap().iter().next().unwrap().never_started());
    app.toggle_selected_session();
    let s = app.sessions.as_ref().unwrap().iter().next().unwrap();
    assert_eq!(s.generation(), 1, "F4 必须真的把它启动起来");
    assert!(!s.never_started());
    assert!(s.is_running() || s.task_finished(), "spawn 之后阶段要动");

    // 再按一次 → 停掉
    app.toggle_selected_session();
    let s = app.sessions.as_ref().unwrap().iter().next().unwrap();
    assert!(s.is_manually_stopped(), "第二下 F4 必须停掉它");
    assert!(!s.restart_enabled());
}

#[tokio::test]
async fn profiles_add_makes_the_sidebar_appear_and_list_reports_status() {
    // 运行期增删账号的**集成点**: `profiles add` 之后左栏必须真的多一行。
    //
    // 单会话起手 → 加一个之後 `show_sidebar` 必须重算 —— 不重算的话左栏还是
    // 藏着, 用户会觉得"加了但界面没变"。
    let (mut app, _rxs, _ctx) = manual_app(1, tokio::runtime::Handle::current());
    assert!(!app.show_sidebar, "单会话时左栏是隐藏的 (版面保证)");

    let dir = std::env::temp_dir().join(format!("ost_rr_add_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("added_acc.json");
    std::fs::write(
        &p,
        r#"{"tick_ms":50,"login":{"account":"777","ip":"127.0.0.1","port":8484}}"#,
    )
    .unwrap();

    app.input = format!("profiles add {}", p.display());
    app.submit_for_test();
    assert!(app.show_sidebar, "加到 2 个之后左栏必须出现");
    {
        let set = app.sessions.as_ref().unwrap();
        assert_eq!(set.len(), 2);
        let added = set.iter().nth(1).unwrap();
        assert_eq!(added.profile, "added_acc");
        assert!(added.never_started(), "新挂上的账号是「未启动」");
    }
    let text = sidebar_of(&mut app);
    assert!(text.contains("added_acc"), "左栏要有新账号:\n{text}");
    assert!(text.contains("未启动"), "新账号标「未启动」:\n{text}");

    // `profiles list` 要能看到它
    app.input = "profiles list".into();
    app.submit_for_test();
    let n = app.log.total_display_lines(None, 200);
    assert!(n > 0, "list 必须写日志");

    // 摘掉它 → 又回到单会话版面
    app.input = "profiles remove added_acc".into();
    app.submit_for_test();
    assert_eq!(app.sessions.as_ref().unwrap().len(), 1);
    assert!(!app.show_sidebar, "回到 1 个之后左栏该收起来");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn profiles_add_rejects_a_missing_file_and_says_why() {
    // 静默失败最糟: 用户会以为挂上了。必须报错并说明原因。
    let (mut app, _rxs, _ctx) = manual_app(1, tokio::runtime::Handle::current());
    app.input = "profiles add profiles/no_such_file_abc123.json".into();
    app.submit_for_test();
    assert_eq!(app.sessions.as_ref().unwrap().len(), 1, "不该挂上");
    // 错误进日志 (总行数 > 0 说明确实写了一条)
    assert!(
        app.log.total_display_lines(None, 200) > 0,
        "失败必须有提示, 不能静默"
    );
}

#[tokio::test]
async fn builtin_command_lines_do_not_reach_the_bot() {
    // `@all stop` / `stop` 是控制台内建指令: 必须被拦下来, **不能**发给 bot。
    // 发过去的话 bot 会回一句"未知指令", 而用户要的是"把这些号断开"。
    let (mut app, _rxs, _ctx) = manual_app(2, tokio::runtime::Handle::current());
    let h = tokio::runtime::Handle::current();
    {
        let set = app.sessions.as_mut().unwrap();
        for i in 0..2 {
            let r = set.start_session(&h, i, None);
            assert!(r.is_ok(), "启动第 {i} 个: {r:?}");
        }
    }
    app.input = "@all stop".into();
    app.submit_for_test();
    {
        // 每个会话**自己的**日志里都要有这条 —— 多开时"这个号为什么停了"必须
        // 能在它自己的日志里读到, 而不是只在公共日志里有一行总数。
        //
        // 注意**选中会话的日志缓冲物理上在 `App.log`** (切换账号时用 swap 换到
        // 渲染位置, 它的新事件也写在那里 —— 见 `App::drain_all_sessions`)。
        // 所以先 flush 一次暂存, 再按"谁在哪"分别检查。
        app.drain_all_sessions();
        let set = app.sessions.as_ref().unwrap();
        assert!(
            set.iter().all(|s| s.is_manually_stopped()),
            "@all stop 必须停掉全部会话"
        );
        let sel = set.selected_index();
        for (i, s) in set.iter().enumerate() {
            let n = if i == sel { app.log.len() } else { s.log.len() };
            assert!(n > 0, "会话 {i} 的日志里应有停止说明 (选中项在渲染缓冲里)");
        }
    }
    // 控制台那一行要报出命中与成功数 (批量下发必须逐条可见)
    let text = squish(&frame_of(&mut app));
    assert!(
        text.contains("全部会话") && text.contains("成功2"),
        "日志要写明对谁做了什么、成没成:\n{text}"
    );
    // 关键: `stop` **不能**作为 bot 指令发下去 —— 那是控制台自己的动作
    // (bot 不认识它, 收到只会回一句"未知指令")。
    assert!(
        !text.contains("未知指令"),
        "`stop` 不该被当成 bot 指令:\n{text}"
    );
}
