//! End-to-end smoke test of the tiny HTTP server (raw TCP, no client deps).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

fn request(port: u16, raw: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    s.write_all(raw.as_bytes()).unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).unwrap();
    buf
}

/// Minimal tiny_http probe: is the library itself bindable on this machine?
#[test]
fn minimal_tiny_http_bind() {
    let server = tiny_http::Server::http("127.0.0.1:18082").unwrap();
    std::thread::spawn(move || {
        for req in server.incoming_requests() {
            let resp = tiny_http::Response::from_string("minimal-ok").with_status_code(200);
            let _ = req.respond(resp);
        }
    });
    std::thread::sleep(Duration::from_millis(300));
    let out = request(18082, "GET /x HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(out.starts_with("HTTP/1.1 200"), "minimal: {out}");
    assert!(out.contains("minimal-ok"), "minimal: {out}");
}

#[test]
fn serves_page_and_config_api() {
    let cfg = openstory_webui::WebConfig {
        addr: "127.0.0.1:18081".into(),
        web_dir: None,
    };
    let handlers = openstory_webui::Handlers {
        list_configs: Arc::new(|| r#"[{"name":"config.json","path":"config.json"}]"#.to_string()),
        get_config: Arc::new(|file| {
            format!(r#"{{"file":{}}}"#, serde_json::to_string(&file).unwrap())
        }),
        save_config: Arc::new(|file, body| Ok(format!("accepted:{:?}:{}", file, body))),
    };
    std::thread::spawn(move || {
        let r = openstory_webui::serve(cfg, handlers);
        if let Err(e) = r {
            eprintln!("SERVE RETURNED ERR: {e}");
        }
    });
    std::thread::sleep(Duration::from_millis(400));

    // Requests use HTTP/1.0: tiny_http answers 1.0 requests with an identity
    // (non-chunked) body. The raw reader below decodes the body as one UTF-8
    // string, so HTTP/1.1 chunk framing (tiny_http chunks any response with
    // extra headers) would corrupt it — 1.0 keeps the test deterministic.
    // page
    let page = request(18081, "GET / HTTP/1.0\r\nHost: x\r\n\r\n");
    assert!(page.starts_with("HTTP/1.0 200"), "page: {page}");
    assert!(page.contains("openstory 配置"), "page body: {page}");

    // api list
    let list = request(18081, "GET /api/configs HTTP/1.0\r\nHost: x\r\n\r\n");
    assert!(list.starts_with("HTTP/1.0 200"), "list: {list}");
    assert!(list.contains("config.json"), "list body: {list}");

    // api get (no file)
    let api = request(18081, "GET /api/config HTTP/1.0\r\nHost: x\r\n\r\n");
    assert!(api.starts_with("HTTP/1.0 200"), "api: {api}");
    assert!(api.contains("\"file\":null"), "api body: {api}");

    // api get (?file=)
    let api_f = request(18081, "GET /api/config?file=profiles/x.json HTTP/1.0\r\nHost: x\r\n\r\n");
    assert!(api_f.starts_with("HTTP/1.0 200"), "api file: {api_f}");
    assert!(api_f.contains("\"file\":\"profiles/x.json\""), "api file body: {api_f}");

    // api put (?file=)
    let put = request(
        18081,
        "PUT /api/config?file=profiles/x.json HTTP/1.0\r\nHost: x\r\nContent-Length: 7\r\n\r\n{\"a\":1}",
    );
    assert!(put.starts_with("HTTP/1.0 200"), "put: {put}");
    assert!(put.contains("accepted:Some(\\\"profiles/x.json\\\")"), "put body: {put}");

    // 404
    let nf = request(18081, "GET /nope HTTP/1.0\r\nHost: x\r\n\r\n");
    assert!(nf.starts_with("HTTP/1.0 404"), "404: {nf}");
}
