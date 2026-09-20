//! End-to-end login-flow test against a fake CongMS 079 server.
//!
//! Exercises the full client state machine offline against the CongMS packet
//! formats: handshake, LOGIN + SERVERLIST_REQUEST, CHOOSE_GENDER/SET_GENDER,
//! SERVERLIST, CHARLIST, CHAR_SELECT, SERVER_IP, channel reconnect,
//! PLAYER_LOGGEDIN, SET_FIELD, and finally a MOVE_PLAYER command.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use openstory_bot::command;
use openstory_bot::config::Config;
use openstory_bot::crypto::Cryptography;
use openstory_bot::handlers;
use openstory_bot::opcodes::{recv, send};
use openstory_bot::packet::Packet;
use openstory_bot::packets::login::login;
use openstory_bot::session::Session;
use openstory_bot::state::{BotState, Phase};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn seeded_rng() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
        | 0x9E3779B97F4A7C15
}

struct Rng(u64);
impl Rng {
    fn next_u8(&mut self) -> u8 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 & 0xFF) as u8
    }
}

/// Build a random CongMS handshake + server-perspective crypto.
fn server_pair(version: u16) -> ([u8; 16], Cryptography) {
    server_pair_keyed(version, None)
}

/// Like `server_pair`, but with a custom AES key (modified-key server).
fn server_pair_keyed(version: u16, key: Option<[u8; 256]>) -> ([u8; 16], Cryptography) {
    let mut rng = Rng(seeded_rng());
    let mut server_send = [0u8; 4];
    let mut server_recv = [0u8; 4];
    for i in 0..4 {
        server_send[i] = rng.next_u8();
        server_recv[i] = rng.next_u8();
    }

    // CongMS LoginPacket.getHello: writeShort(14); writeShort(version);
    // writeMapleAsciiString("1"); write(recvIv); write(sendIv); write(4)
    let mut handshake = [0u8; 16];
    handshake[0] = 0x0E;
    handshake[1] = 0x00;
    handshake[2] = (version & 0xFF) as u8;
    handshake[3] = (version >> 8) as u8;
    handshake[4] = 0x01;
    handshake[5] = 0x00;
    handshake[6] = b'1';
    for i in 0..4 {
        handshake[7 + i] = server_recv[i]; // client's send IV
        handshake[11 + i] = server_send[i]; // client's recv IV
    }
    handshake[15] = 0x04;

    let mut sendiv = [0u8; 4];
    let mut recviv = [0u8; 4];
    for i in 0..4 {
        sendiv[i] = handshake[i + 11];
        recviv[i] = handshake[i + 7];
    }
    let crypto = match key {
        Some(key) => Cryptography::with_ivs_keyed(sendiv, recviv, version, key),
        None => Cryptography::with_ivs(sendiv, recviv, version),
    };
    (handshake, crypto)
}

async fn server_send(sock: &mut TcpStream, crypto: &mut Cryptography, body: &[u8]) {
    let mut payload = body.to_vec();
    let header = crypto.create_header(payload.len());
    crypto.encrypt(&mut payload);
    sock.write_all(&header).await.unwrap();
    sock.write_all(&payload).await.unwrap();
}

async fn server_recv_opcode(sock: &mut TcpStream, crypto: &mut Cryptography) -> u16 {
    let mut header = [0u8; 4];
    sock.read_exact(&mut header).await.unwrap();
    let len = Cryptography::check_length(&header);
    assert!(len <= 0x20000, "server-side packet too large: {len}");
    let mut body = vec![0u8; len];
    sock.read_exact(&mut body).await.unwrap();
    crypto.decrypt(&mut body);
    let op = u16::from_le_bytes([body[0], body[1]]);
    eprintln!("[fake-server] recv opcode 0x{op:02X}");
    op
}

fn padded_name13(name: &str) -> Vec<u8> {
    let mut v = name.as_bytes().to_vec();
    v.resize(13, 0);
    v
}

fn choose_gender(account: &str) -> Vec<u8> {
    let mut p = Packet::new(recv::CHOOSE_GENDER);
    p.write_string(account);
    p.into_bytes()
}

fn gender_set() -> Vec<u8> {
    let mut p = Packet::new(recv::GENDER_SET);
    p.write_u8(0);
    p.write_string("botaccount");
    p.write_string("10001");
    p.into_bytes()
}

/// LOGIN_STATUS success (LoginPacket.getAuthSuccessRequest layout).
fn login_status_success() -> Vec<u8> {
    let mut p = Packet::new(recv::LOGIN_STATUS);
    p.write_u8(0); // reason = success
    p.write_i32(10001); // accid
    p.write_u8(0); // gender
    p.write_u8(0);
    p.write_u8(0);
    p.write_string("botaccount");
    p.write_bytes(&[0x00, 0x00, 0x00, 0x03, 0x01, 0x00, 0x00, 0x00, 0xE2, 0xED, 0xA3, 0x7A, 0xFA, 0xC9, 0x01]);
    p.write_u8(0);
    p.write_i64(0);
    p.write_u16(0);
    p.write_u8(0);
    p.write_string("10001");
    p.write_string("botaccount");
    p.write_u8(1);
    p.into_bytes()
}

fn serverlist() -> Vec<u8> {
    let mut p = Packet::new(recv::SERVERLIST);
    p.write_u8(0); // serverId
    p.write_string("TestWorld");
    p.write_u8(0); // flag
    p.write_string("tip");
    p.write_u16(100);
    p.write_u16(100);
    p.write_u8(1); // lastChannel
    p.write_i32(500);
    p.write_string("TestWorld-1"); // channel name
    p.write_i32(10); // load
    p.write_u8(0); // serverId
    p.write_u16(0); // index
    p.write_u16(0); // trailing short
    p.into_bytes()
}

fn end_of_serverlist() -> Vec<u8> {
    let mut p = Packet::new(recv::SERVERLIST);
    p.write_u8(255);
    p.into_bytes()
}

fn serverstatus_ok() -> Vec<u8> {
    let mut p = Packet::new(recv::SERVERSTATUS);
    p.write_i16(0);
    p.into_bytes()
}

fn charlist() -> Vec<u8> {
    let mut p = Packet::new(recv::CHARLIST);
    p.write_u8(0); // byte 0
    p.write_i32(0); // int 0
    p.write_u8(1); // count

    // addCharStats
    p.write_i32(500); // id
    p.write_bytes(&padded_name13("BotMan"));
    p.write_u8(0); // gender
    p.write_u8(0); // skin
    p.write_i32(20000); // face
    p.write_i32(30000); // hair
    for _ in 0..3 {
        p.write_i64(0); // pet ids
    }
    p.write_u8(30); // level
    p.write_i16(100); // job
    p.write_i16(5); // str
    p.write_i16(5); // dex
    p.write_i16(5); // int
    p.write_i16(5); // luk
    p.write_i16(1000); // hp
    p.write_i16(1000); // maxhp
    p.write_i16(500); // mp
    p.write_i16(500); // maxmp
    p.write_i16(0); // ap
    p.write_i16(0); // sp
    p.write_i32(0); // exp
    p.write_i16(0); // fame
    p.write_i32(0); // gachaexp
    p.write_i64(0); // timestamp
    p.write_i32(100000000); // mapid
    p.write_u8(0); // portal

    // addCharLook
    p.write_u8(0); // gender
    p.write_u8(0); // skin
    p.write_i32(20000); // face
    p.write_u8(0); // mega
    p.write_i32(30000); // hair
    p.write_u8(0xFF); // equip terminator
    p.write_u8(0xFF); // masked terminator
    p.write_i32(0); // weapon
    p.write_i32(0);
    p.write_i64(0);

    // addCharEntry tail
    p.write_u8(0); // rankinfo

    p.write_u16(3); // short 3
    p.write_i32(6); // slots
    p.into_bytes()
}

fn server_ip(port: u16, cid: i32) -> Vec<u8> {
    let mut p = Packet::new(recv::SERVER_IP);
    p.write_u16(0); // short 0
    p.write_u8(127);
    p.write_u8(0);
    p.write_u8(0);
    p.write_u8(1);
    p.write_u16(port);
    p.write_i32(cid);
    p.write_bytes(&[1, 0, 0, 0, 0]);
    p.into_bytes()
}

fn set_field_warp(mapid: i32) -> Vec<u8> {
    let mut p = Packet::new(recv::SET_FIELD);
    p.write_i32(0); // channel - 1
    p.write_u8(0);
    p.write_u8(3); // mode2 = warp
    p.write_u8(0);
    p.write_u16(0);
    p.write_i32(mapid);
    p.write_u8(0); // portal
    p.write_i16(1000); // hp
    p.write_i64(0); // time
    p.into_bytes()
}

#[tokio::test]
async fn full_login_flow_and_move() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let channel_port = listener.local_addr().unwrap().port();

    let (done_tx, done_rx) = tokio::sync::oneshot::channel();

    let server = tokio::spawn(async move {
        // --- connection 1: login server ---
        let (mut s1, _) = listener.accept().await.unwrap();
        let (hs1, mut c1) = server_pair(79);
        s1.write_all(&hs1).await.unwrap();

        // Bot sends LOGIN_PASSWORD.
        assert_eq!(server_recv_opcode(&mut s1, &mut c1).await, send::LOGIN_PASSWORD);

        // Account needs a gender -> CHOOSE_GENDER.
        server_send(&mut s1, &mut c1, &choose_gender("botaccount")).await;

        // Bot replies SET_GENDER (byte gender + string account).
        assert_eq!(server_recv_opcode(&mut s1, &mut c1).await, send::SET_GENDER);
        server_send(&mut s1, &mut c1, &gender_set()).await;

        // Bot re-logs in; the server confirms with LOGIN_STATUS.
        assert_eq!(server_recv_opcode(&mut s1, &mut c1).await, send::LOGIN_PASSWORD);
        server_send(&mut s1, &mut c1, &login_status_success()).await;

        // Bot now requests the server list.
        assert_eq!(
            server_recv_opcode(&mut s1, &mut c1).await,
            send::SERVERLIST_REQUEST
        );

        // Server sends the world list + terminator.
        server_send(&mut s1, &mut c1, &serverlist()).await;
        server_send(&mut s1, &mut c1, &end_of_serverlist()).await;

        // Bot asks for status + charlist.
        let a = server_recv_opcode(&mut s1, &mut c1).await;
        let b = server_recv_opcode(&mut s1, &mut c1).await;
        assert!(
            (a == send::SERVERSTATUS_REQUEST && b == send::CHARLIST_REQUEST)
                || (a == send::CHARLIST_REQUEST && b == send::SERVERSTATUS_REQUEST),
            "expected status+charlist requests, got {a} {b}"
        );
        server_send(&mut s1, &mut c1, &serverstatus_ok()).await;
        server_send(&mut s1, &mut c1, &charlist()).await;

        // Bot selects a character by id (no PIC on CongMS).
        assert_eq!(server_recv_opcode(&mut s1, &mut c1).await, send::CHAR_SELECT);

        // Send the channel address; the bot reconnects.
        server_send(&mut s1, &mut c1, &server_ip(channel_port, 500)).await;

        // --- connection 2: channel server ---
        let (mut s2, _) = listener.accept().await.unwrap();
        let (hs2, mut c2) = server_pair(79);
        s2.write_all(&hs2).await.unwrap();

        assert_eq!(
            server_recv_opcode(&mut s2, &mut c2).await,
            send::PLAYER_LOGGEDIN
        );
        server_send(&mut s2, &mut c2, &set_field_warp(100000000)).await;

        // Wait for the bot's move command.
        let op = server_recv_opcode(&mut s2, &mut c2).await;
        assert_eq!(op, send::MOVE_PLAYER, "expected MOVE_PLAYER");
        let _ = done_tx.send(());
    });

    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let config = Config {
            ip: "127.0.0.1".into(),
            port: channel_port,
            account: "botaccount".into(),
            password: "pw".into(),
            char_index: 0,
            show_packets: false,
            ..Config::default()
        };

        let addr = format!("127.0.0.1:{channel_port}");
        let mut session = Session::connect_ex(&addr, 79, false, false).await.unwrap();
        let mut state = BotState::default();

        session
            .send(&login("botaccount", "pw").into_bytes())
            .await
            .unwrap();

        // Drive until we are in game.
        loop {
            let body = session
                .recv_packet()
                .await
                .expect("recv")
                .expect("connection closed");
            handlers::handle(&body, &mut state, &mut session, &config)
                .await
                .unwrap();
            if state.phase == Phase::InGame {
                break;
            }
        }

        assert_eq!(state.mapid, 100000000);
        assert_eq!(state.my_cid, 500);

        // Issue a move command; the fake server verifies MOVE_PLAYER arrives.
        let quit = command::run(command::Command::Move(84, 92), &mut state, &mut session)
            .await
            .unwrap();
        assert!(!quit);
        assert_eq!(state.position, (84, 92));

        done_rx.await.unwrap();
        server.await.unwrap();
    })
    .await;

    assert!(result.is_ok(), "login flow test timed out or failed");
}

#[tokio::test]
async fn custom_key_survives_channel_reconnect() {
    // Modified-key server (custom AES key): both the login and the channel
    // server only accept the custom key. The channel reconnect must re-apply
    // the custom key (Session::reconnect), otherwise PLAYER_LOGGEDIN decrypts
    // to garbage and this test fails at the opcode assertion.
    let custom_key = openstory_bot::crypto::expand_key([
        0x13, 0x09, 0x06, 0xB4, 0x1B, 0x0F, 0x33, 0x52,
    ]);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let channel_port = listener.local_addr().unwrap().port();

    let (done_tx, done_rx) = tokio::sync::oneshot::channel();

    let server = tokio::spawn(async move {
        // --- connection 1: login server (custom key) ---
        let (mut s1, _) = listener.accept().await.unwrap();
        let (hs1, mut c1) = server_pair_keyed(79, Some(custom_key));
        s1.write_all(&hs1).await.unwrap();

        assert_eq!(server_recv_opcode(&mut s1, &mut c1).await, send::LOGIN_PASSWORD);
        server_send(&mut s1, &mut c1, &login_status_success()).await;

        assert_eq!(
            server_recv_opcode(&mut s1, &mut c1).await,
            send::SERVERLIST_REQUEST
        );
        server_send(&mut s1, &mut c1, &serverlist()).await;
        server_send(&mut s1, &mut c1, &end_of_serverlist()).await;

        let a = server_recv_opcode(&mut s1, &mut c1).await;
        let b = server_recv_opcode(&mut s1, &mut c1).await;
        assert!(
            (a == send::SERVERSTATUS_REQUEST && b == send::CHARLIST_REQUEST)
                || (a == send::CHARLIST_REQUEST && b == send::SERVERSTATUS_REQUEST),
            "expected status+charlist requests, got {a} {b}"
        );
        server_send(&mut s1, &mut c1, &serverstatus_ok()).await;
        server_send(&mut s1, &mut c1, &charlist()).await;

        assert_eq!(server_recv_opcode(&mut s1, &mut c1).await, send::CHAR_SELECT);
        server_send(&mut s1, &mut c1, &server_ip(channel_port, 500)).await;

        // --- connection 2: channel server (custom key) ---
        // If reconnect() dropped the custom key, the client would encrypt
        // PLAYER_LOGGEDIN with the STANDARD key and this decrypt + opcode
        // assertion fails.
        let (mut s2, _) = listener.accept().await.unwrap();
        let (hs2, mut c2) = server_pair_keyed(79, Some(custom_key));
        s2.write_all(&hs2).await.unwrap();

        assert_eq!(
            server_recv_opcode(&mut s2, &mut c2).await,
            send::PLAYER_LOGGEDIN
        );
        server_send(&mut s2, &mut c2, &set_field_warp(100000000)).await;
        let _ = done_tx.send(());
    });

    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let config = Config {
            ip: "127.0.0.1".into(),
            port: channel_port,
            account: "botaccount".into(),
            password: "pw".into(),
            char_index: 0,
            show_packets: false,
            ..Config::default()
        };

        let addr = format!("127.0.0.1:{channel_port}");
        let mut session = Session::connect_ex_keyed(&addr, 79, false, false, Some(custom_key))
            .await
            .unwrap();
        let mut state = BotState::default();

        session
            .send(&login("botaccount", "pw").into_bytes())
            .await
            .unwrap();

        loop {
            let body = session
                .recv_packet()
                .await
                .expect("recv")
                .expect("connection closed");
            handlers::handle(&body, &mut state, &mut session, &config)
                .await
                .unwrap();
            if state.phase == Phase::InGame {
                break;
            }
        }

        assert_eq!(state.mapid, 100000000);
        assert_eq!(state.my_cid, 500);

        done_rx.await.unwrap();
        server.await.unwrap();
    })
    .await;

    assert!(result.is_ok(), "custom-key reconnect test timed out or failed");
}
