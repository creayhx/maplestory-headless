use openstory_bot::command::{self, Command};
use openstory_bot::config::Config;
use openstory_bot::handlers;
use openstory_bot::names;
use openstory_bot::packets::login::login;
use openstory_bot::session::Session;
use openstory_bot::state::{BotState, Phase};

async fn connect() -> (BotState, Session) {
    let mut session = Session::connect_ex("127.0.0.1:8484", 79, false, false)
        .await
        .expect("connect");
    session
        .send(&login("100000001", "test_password").into_bytes())
        .await
        .expect("send login");
    let mut state = BotState::default();
    let config = Config {
        ip: "127.0.0.1".into(),
        port: 8484,
        account: "100000001".into(),
        password: "test_password".into(),
        char_index: 0,
        show_packets: false,
        ..Config::default()
    };
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let body = session.recv_packet().await.expect("recv").expect("closed");
            handlers::handle(&body, &mut state, &mut session, &config)
                .await
                .expect("handle");
            if state.phase == Phase::InGame {
                break;
            }
        }
    })
    .await
    .expect("login timeout");
    (state, session)
}

#[tokio::test]
async fn live_full_regression() {
    println!("\n=== 连接本地服 127.0.0.1:8484 ===");
    let (mut state, mut session) = connect().await;
    println!("✅ 登录成功, map={}, pos={:?}", state.mapid, state.position);

    // ── 1. view portals (地图名显示验证) ──
    println!("\n--- 测试1: view portals (地图名是否显示) ---");
    let portals = names::map_portals(state.mapid);
    println!("当前地图 {} ({}) 共 {} 个传送门:",
        state.mapid, names::map_name_text(state.mapid), portals.len());
    for (i, p) in portals.iter().enumerate() {
        let target_name = names::map_name_text(p.target_map);
        println!("  {:>3}  {:<16} → {:<10} {}", i + 1, p.name, p.target_map, target_name);
    }
    assert!(!portals.is_empty(), "出生地图应有传送门");
    // 验证地图名不是"未知"。
    // 排除哨兵/特殊传送门：目标地图 999999999 是"无效/特殊"占位（如 sp 出生点、
    // pachinkoDoor 等不可寻路入口），名称字典里本就没有条目，属预期行为。
    let named: Vec<_> = portals
        .iter()
        .filter(|p| p.target_map != 999999999 && !p.name.starts_with("sp"))
        .collect();
    assert!(!named.is_empty(), "应有可寻路的命名传送门");
    for p in &named {
        let name = names::map_name_text(p.target_map);
        assert_ne!(name, "未知", "传送门 {} 的目标地图 {} 名称不应是未知", p.name, p.target_map);
    }
    println!("✅ 所有可寻路传送门目标地图名正确显示 ({} 个, 跳过 {} 个哨兵入口)",
        named.len(), portals.len() - named.len());

    // ── 2. warp 发包验证 ──
    println!("\n--- 测试2: warp 命令 (发包验证) ---");
    let cross = named
        .iter()
        .find(|p| p.target_map != state.mapid)
        .expect("应有跨地图传送门");
    println!("执行: warp {} → {} ({})", cross.name, cross.target_map, names::map_name_text(cross.target_map));
    let quit = command::run(
        Command::ChangeMapSpecial(cross.name.clone()),
        &mut state,
        &mut session,
    )
    .await
    .expect("warp cmd");
    assert!(!quit);
    assert_eq!(state.phase, Phase::EnteringMap);
    println!("✅ warp 发包成功, phase=EnteringMap");

    // ── 3. bad portal 拒绝验证 ──
    println!("\n--- 测试3: warp 不存在的传送门 ---");
    let quit = command::run(
        Command::ChangeMapSpecial("nonexistent_xyz".to_string()),
        &mut state,
        &mut session,
    )
    .await
    .expect("warp bad");
    assert!(!quit);
    // EnteringMap 状态下无法进入游戏验证, 但命令本身不崩溃
    println!("✅ 不存在的传送门处理正常, quit={}", quit);

    // ── 4. 旧命令删除验证 ──
    println!("\n--- 测试4: 旧命令已删除 ---");
    assert!(command::parse("ports sp").is_err());
    println!("  ports sp → ✅ 报错");
    assert!(command::parse("warp_to sp").is_err());
    println!("  warp_to sp → ✅ 报错");
    assert!(command::parse("map 100000000").is_err());
    println!("  map 100000000 → ✅ 报错");

    // ── 5. town 发包验证 ──
    println!("\n--- 测试5: town 命令 (发包验证) ---");
    let quit = command::run(
        Command::ChangeMap(100000000, "sp".to_string()),
        &mut state,
        &mut session,
    )
    .await
    .expect("town cmd");
    assert!(!quit);
    assert_eq!(state.phase, Phase::EnteringMap);
    println!("✅ town 100000000 sp 发包成功, phase=EnteringMap");

    // ── 6. parse 验证 ──
    println!("\n--- 测试6: 命令解析验证 ---");
    let cmd = command::parse("warp east00").unwrap().unwrap();
    match cmd {
        Command::ChangeMapSpecial(name) => {
            assert_eq!(name, "east00");
            println!("  warp east00 → ChangeMapSpecial(\"{}\") ✅", name);
        }
        other => panic!("expected ChangeMapSpecial, got {other:?}"),
    }
    let cmd = command::parse("view portals").unwrap().unwrap();
    match cmd {
        Command::PortalList(None) => println!("  view portals → PortalList(None) ✅"),
        other => panic!("expected PortalList(None), got {other:?}"),
    }
    let cmd = command::parse("view portals 100000000").unwrap().unwrap();
    match cmd {
        Command::PortalList(Some(id)) => {
            assert_eq!(id, 100000000);
            println!("  view portals 100000000 → PortalList(Some({})) ✅", id);
        }
        other => panic!("expected PortalList(Some(100000000)), got {other:?}"),
    }

    println!("\n=== 全部测试通过 ===");
}
