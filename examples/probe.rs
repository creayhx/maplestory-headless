//! Probe: connect to a login server and try LOGIN packet variants, reporting
//! which (if any) gets a response. Build with `cargo run --example probe --
//! <ip> <port> <account> <password> [aes-key]`.
//!
//! Each variant uses a fresh TCP connection + handshake so a dropped packet
//! never desyncs the next attempt. The optional last argument is the custom
//! AES key for modified-key servers: an 8-byte key (16 hex chars, expanded at
//! runtime) or a 256-byte pre-expanded table (512 hex chars).

use openstory_bot::config::parse_aes_key;
use openstory_bot::crypto::Cryptography;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn s(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut v = vec![0; 2];
    v[0] = (b.len() & 0xFF) as u8;
    v[1] = (b.len() >> 8) as u8;
    v.extend_from_slice(b);
    v
}

fn body_from(parts: &[Vec<u8>]) -> Vec<u8> {
    let mut v = Vec::new();
    for p in parts {
        v.extend_from_slice(p);
    }
    v
}

async fn try_variant(
    label: &str,
    addr: &str,
    opcode: u8,
    parts: &[Vec<u8>],
    send_iv_off: usize,
    recv_iv_off: usize,
    version: u16,
    wait_ms: u64,
    key: &[u8; 256],
) {
    // IV windows must stay inside the 16-byte handshake
    if recv_iv_off + 4 > 16 {
        println!("{label}: skipped (recv IV window out of bounds)");
        return;
    }
    let mut stream = match TcpStream::connect(addr).await {
        Ok(s) => s,
        Err(e) => {
            println!("{label}: connect failed: {e}");
            return;
        }
    };
    let mut hs = [0u8; 16];
    if stream.read_exact(&mut hs).await.is_err() {
        println!("{label}: handshake read failed");
        return;
    }
    let mut sendiv = [0u8; 4];
    let mut recviv = [0u8; 4];
    for i in 0..4 {
        sendiv[i] = hs[i + send_iv_off];
        recviv[i] = hs[i + recv_iv_off];
    }
    let mut crypto = Cryptography::with_ivs_keyed(sendiv, recviv, version, *key);

    let mut pkt = vec![opcode];
    pkt.extend_from_slice(&body_from(parts));
    let header = crypto.create_header(pkt.len());
    crypto.encrypt(&mut pkt);
    if stream.write_all(&header).await.is_err() || stream.write_all(&pkt).await.is_err() {
        println!("{label}: send failed");
        return;
    }

    let mut buf = [0u8; 8192];
    match tokio::time::timeout(
        std::time::Duration::from_millis(wait_ms),
        stream.read(&mut buf),
    )
    .await
    {
        Ok(Ok(0)) => println!("{label}: server closed connection"),
        Ok(Ok(n)) => {
            let hex: Vec<String> = buf[..n].iter().map(|b| format!("{b:02X}")).collect();
            println!("{label}: RESPONSE {n} bytes: {}", hex.join(" "));
        }
        Ok(Err(e)) => println!("{label}: read error: {e}"),
        Err(_) => println!("{label}: no response in {wait_ms}ms"),
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ip = args.get(0).cloned().unwrap_or_else(|| "127.0.0.1".into());
    let port: u16 = args
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8484);
    let acc = args.get(2).cloned().unwrap_or_else(|| "test".into());
    let pass = args.get(3).cloned().unwrap_or_else(|| "test".into());
    let key = match args.get(4) {
        Some(k) => match parse_aes_key(k) {
            Ok(k) => {
                println!("using custom AES key (16-hex key or 512-hex table)");
                k
            }
            Err(e) => {
                eprintln!("bad --key: {e}");
                std::process::exit(2);
            }
        },
        None => {
            eprintln!("note: no custom key given, using standard v79 table");
            openstory_bot::crypto::standard_key()
        }
    };
    let addr = format!("{ip}:{port}");
    let six = vec![0u8; 6];
    let six_nz = vec![1u8, 2, 3, 4, 5, 6];
    let four = vec![0u8; 4];
    let eight = vec![0u8; 8];
    let one = vec![0u8; 1];
    let ver_pre = {
        let mut v = Vec::new();
        v.extend_from_slice(&79u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v
    };

    println!("probing {addr} acc={acc} pass={pass}");

    // opcode 0x01, IV layout [7..11]/[11..15] (CongMS), version 79
    try_variant("A: 0x01 acc+pass+6B", &addr, 0x01, &[s(&acc), s(&pass), six.clone()], 7, 11, 79, 4000, &key).await;
    try_variant("B: 0x01 acc+pass", &addr, 0x01, &[s(&acc), s(&pass)], 7, 11, 79, 4000, &key).await;
    try_variant("C: 0x01 acc+pass+4B", &addr, 0x01, &[s(&acc), s(&pass), four.clone()], 7, 11, 79, 4000, &key).await;
    try_variant("D: 0x01 acc+pass+8B", &addr, 0x01, &[s(&acc), s(&pass), eight], 7, 11, 79, 4000, &key).await;
    // swapped IVs: send = hs[11..15], recv = hs[7..11]
    try_variant("J: 0x01 acc+pass+6B iv swapped", &addr, 0x01, &[s(&acc), s(&pass), six.clone()], 11, 7, 79, 4000, &key).await;
    // IVs at [3..7]/[7..11] and [4..8]/[8..12]
    try_variant("G: 0x01 acc+pass+6B iv[3..11]", &addr, 0x01, &[s(&acc), s(&pass), six.clone()], 3, 7, 79, 4000, &key).await;
    try_variant("K: 0x01 acc+pass+6B iv[4..12]", &addr, 0x01, &[s(&acc), s(&pass), six.clone()], 4, 8, 79, 4000, &key).await;
    // version prefix inside body (some 079 private servers)
    try_variant("L: 0x01 ver+sub+acc+pass+6B", &addr, 0x01, &[ver_pre, s(&acc), s(&pass), six.clone()], 7, 11, 79, 4000, &key).await;
    // leading byte
    try_variant("M: 0x01 0x00+acc+pass+6B", &addr, 0x01, &[one, s(&acc), s(&pass), six.clone()], 7, 11, 79, 4000, &key).await;
    // non-zero MAC bytes
    try_variant("N: 0x01 acc+pass+6B(nonzero)", &addr, 0x01, &[s(&acc), s(&pass), six_nz], 7, 11, 79, 4000, &key).await;
    // header XOR version 83
    try_variant("H: 0x01 acc+pass+6B ver83", &addr, 0x01, &[s(&acc), s(&pass), six.clone()], 7, 11, 83, 4000, &key).await;

    // brute-force opcodes with the correct IV/version: any response means the
    // crypto is right and that opcode is what the server wants first
    for op in 0u8..=0x1F {
        try_variant(
            &format!("OP: 0x{op:02X} acc+pass+6B"),
            &addr,
            op,
            &[s(&acc), s(&pass), six.clone()],
            7,
            11,
            79,
            1500,
            &key,
        )
        .await;
    }

    // definitive sweep: (version x IV window position) with the current key.
    // ANY response proves version+IV+key are all correct together.
    let login = [s(&acc), s(&pass), six.clone()];
    for ver in [79u16, 83] {
        for (so, ro) in [
            (3, 7),
            (4, 8),
            (5, 9),
            (6, 10),
            (7, 11),
            (8, 12),
            (9, 13),
            (10, 14),
            (7, 3),
            (8, 4),
            (9, 5),
            (10, 6),
            (11, 7),
            (12, 8),
        ] {
            try_variant(
                &format!("SW v{ver} send@{so} recv@{ro}"),
                &addr,
                0x01,
                &login,
                so,
                ro,
                ver,
                2500,
                &key,
            )
            .await;
        }
    }
}
