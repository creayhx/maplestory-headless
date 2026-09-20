//! 解密假服务器抓到的 MapleStory 真实登录包, 验证密钥/扩展表
//! 用法: cargo run --example decrypt_real_pkt
use openstory_bot::crypto::Cryptography;

fn hex_decode(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[tokio::main]
async fn main() {
    // 假服务器抓包: 头4字节 + payload 41 字节
    let pkt = hex_decode("bd0b940bafd3e2d2527f200e77bbc2bcd78fd7e330334bfcec50c16dcca4682a9ba00bea2105abd730a9e8655c");
    let header = &pkt[..4];
    let payload = &pkt[4..];
    println!("头: {:02X?}", header);
    println!("payload {} 字节", payload.len());

    // 握手 sendIV = [7..11] = 64 cc f2 0b (客户端加密用), recvIV = [11..15] = 90 50 2d 57
    // 解密客户端发出的包: 用客户端加密用的 sendiv 当 decrypt 的 recviv
    let mut crypto = Cryptography::with_ivs(
        [0x90, 0x50, 0x2D, 0x57], // sendiv(解密侧不用)
        [0x64, 0xCC, 0xF2, 0x0B], // recviv = 客户端 sendiv
        79,
    );

    // 头长度校验 (客户端加密时用 sendiv 生成头)
    let len = Cryptography::check_length(header);
    println!("check_length = {}", len);
    println!("与 payload 长度一致: {}", len == payload.len());

    // 解密 payload (客户端用 sendiv 加密 -> 用 sendiv 解密)
    let mut body = payload.to_vec();
    crypto.decrypt(&mut body);
    println!("解密后 {} 字节: {:02X?}", body.len(), body);
    let ascii: String = body.iter().map(|b| if (32..127).contains(b) { *b as char } else { '.' }).collect();
    println!("ASCII: {}", ascii);
    println!("opcode: 0x{:02X}", body[0]);
}
