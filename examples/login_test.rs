//! 用修正后的登录包直连真实服务器测试
//! 用法: cargo run --example login_test -- <ip> <port> <account> <password>
use openstory_bot::crypto::Cryptography;
use openstory_bot::packets::login;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ip = args.get(0).cloned().unwrap_or("127.0.0.1".into());
    let port: u16 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(8484);
    let acc = args.get(2).cloned().unwrap_or("100000003".into());
    let pass = args.get(3).cloned().unwrap_or("test_password".into());
    let addr = format!("{ip}:{port}");
    println!("连接 {addr} acc={acc} pass={pass}");

    let mut stream = match TcpStream::connect(&addr).await {
        Ok(s) => s,
        Err(e) => { println!("连接失败: {e}"); return; }
    };
    let mut hs = [0u8; 16];
    if stream.read_exact(&mut hs).await.is_err() {
        println!("握手读取失败");
        return;
    }
    println!("握手: {:02X?}", hs);
    let mut crypto = Cryptography::with_version(&hs, 79);
    println!("sendIV: {:02X?}", &hs[7..11]);

    // 登录包
    let pkt = login::login(&acc, &pass);
    let body = pkt.into_bytes();
    println!("明文: {:02X?}", body);

    let header = crypto.create_header(body.len());
    let mut enc = body.clone();
    crypto.encrypt(&mut enc);
    println!("密文头: {:02X?} 体: {:02X?}", header, enc);
    if stream.write_all(&header).await.is_err() || stream.write_all(&enc).await.is_err() {
        println!("发送失败");
        return;
    }
    println!("已发送登录包");

    // 等待响应 (最多 8 秒)
    let mut buf = [0u8; 8192];
    match tokio::time::timeout(std::time::Duration::from_secs(8), stream.read(&mut buf)).await {
        Ok(Ok(0)) => println!("服务器关闭连接"),
        Ok(Ok(n)) => {
            println!("响应 {} 字节: {:02X?}", n, &buf[..n]);
            // 尝试解密响应
            if n >= 4 {
                let len = Cryptography::check_length(&buf[..4]);
                println!("响应长度字段 = {}", len);
                if n >= 4 + len {
                    let mut rbody = buf[4..4 + len].to_vec();
                    crypto.decrypt(&mut rbody);
                    println!("响应明文: {:02X?}", rbody);
                    let ascii: String = rbody.iter().map(|b| if (32..127).contains(b) { *b as char } else { '.' }).collect();
                    println!("ASCII: {}", ascii);
                }
            }
        }
        Ok(Err(e)) => println!("读取错误: {e}"),
        Err(_) => println!("8 秒无响应"),
    }
}
