//! Web config editor: a tiny tiny_http server serving the static page and
//! two JSON endpoints backed by callbacks injected by the host.
//!
//! Kept dependency-light (tiny_http + serde_json only) and deliberately
//! independent of the bot's build: the host wires the callbacks, so this
//! crate is reusable. The `openstory-web` binary (src/main.rs) uses it in
//! standalone mode — reading/writing config.json directly — and the bot
//! itself never compiles this code.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tiny_http::{Header, Method, Request, Response, Server};

/// Where to listen + where to find an optional on-disk page override.
pub struct WebConfig {
    /// bind address, e.g. "127.0.0.1:8080"
    pub addr: String,
    /// optional directory containing index.html; when set, serves the file
    /// from disk instead of the compiled-in page (lets users restyle the
    /// page without recompiling)
    pub web_dir: Option<PathBuf>,
}

/// Host callbacks bridging this server to the config source (the standalone
/// `openstory-web` binary reads/writes config.json; nothing in-process).
pub struct Handlers {
    /// List available config files: returns a JSON array of
    /// `{"name": <filename>, "path": <relative path>}` entries.
    pub list_configs: Arc<dyn Fn() -> String + Send + Sync>,
    /// serialized config for `file` (relative path; `None` = default), as held
    /// on disk by the host.
    pub get_config: Arc<dyn Fn(Option<&str>) -> String + Send + Sync>,
    /// validate + atomically write + hot-reload; Ok(msg) / Err(reason).
    /// `file` is the relative path (`None` = default).
    pub save_config: Arc<dyn Fn(Option<&str>, &str) -> Result<String, String> + Send + Sync>,
}

/// Max accepted PUT body (config is small; guards against garbage uploads).
const MAX_BODY: usize = 4 * 1024 * 1024;

const INDEX_HTML: &str = include_str!("../web/index.html");

/// Serve until the server errors or the process dies. Blocking — call from a
/// dedicated thread. Returns the fatal error (bind failure etc.).
pub fn serve(
    cfg: WebConfig,
    handlers: Handlers,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = Server::http(&cfg.addr)?;
    for mut request in server.incoming_requests() {
    let raw_url = request.url().to_string();
    let path = raw_url.split('?').next().unwrap_or("/").to_string();
    let file = query_param(&raw_url, "file");
    let (code, content_type, body) = match (request.method(), path.as_str()) {
        (Method::Get, "/") | (Method::Get, "/index.html") => {
            serve_page(&mut request, &cfg)
        }
        (Method::Get, "/api/configs") => (200, "application/json", (handlers.list_configs)()),
        (Method::Get, "/api/config") => (200, "application/json", (handlers.get_config)(file.as_deref())),
        (Method::Put, "/api/config") => {
            match read_body(&mut request) {
                Ok(body) => match (handlers.save_config)(file.as_deref(), &body) {
                        Ok(msg) => (
                            200,
                            "application/json",
                            serde_json::json!({"ok": true, "msg": msg}).to_string(),
                        ),
                        Err(e) => (
                            400,
                            "application/json",
                            serde_json::json!({"ok": false, "error": e}).to_string(),
                        ),
                    },
                    Err(e) => (
                        400,
                        "application/json",
                        serde_json::json!({"ok": false, "error": format!("body read failed: {e}")})
                            .to_string(),
                    ),
                }
            }
            _ => (404, "text/plain", "not found".to_string()),
        };
        let response = Response::from_string(body)
            .with_header(Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()).unwrap())
            .with_header(Header::from_bytes(&b"Cache-Control"[..], b"no-store").unwrap())
            .with_status_code(code);
        if let Err(e) = request.respond(response) {
            eprintln!("[webui] response failed: {e}");
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        "webui server stopped",
    )
    .into())
}

/// Whether a directory-entry name is a config file the page may list/edit.
///
/// Deliberately excludes dot-prefixed names: `profiles/.manager.json` holds
/// saved passwords and is **not** a `RuntimeConfig`. Listing it would render
/// it as an all-defaults config (its keys are unknown to the form and to the
/// schema validator, which does not reject unknown fields), and saving would
/// then overwrite the passwords with that empty config.
pub fn is_listable_config(name: &str) -> bool {
    if name.starts_with('.') {
        return false;
    }
    Path::new(name)
        .extension()
        .map(|e| e == "json")
        .unwrap_or(false)
}

/// Whether a `?file=` value is a safe relative config path: non-empty, no
/// `..` traversal, and no dot-prefixed path segment (hidden files such as
/// `profiles/.manager.json` are not configs — see [`is_listable_config`]).
pub fn is_safe_config_path(file: &str) -> bool {
    if file.is_empty() || file.contains("..") {
        return false;
    }
    !file.split(|c| c == '/' || c == '\\').any(|seg| seg.starts_with('.'))
}

fn serve_page(_request: &mut Request, cfg: &WebConfig) -> (u16, &'static str, String) {
    if let Some(dir) = &cfg.web_dir {
        let path = dir.join("index.html");
        if path.is_file() {
            if let Ok(html) = std::fs::read_to_string(&path) {
                return (200, "text/html; charset=utf-8", html);
            }
        }
    }
    (200, "text/html; charset=utf-8", INDEX_HTML.to_string())
}

fn read_body(request: &mut Request) -> Result<String, String> {
    let len = request.body_length().unwrap_or(0).min(MAX_BODY);
    let mut buf = Vec::with_capacity(len);
    request
        .as_reader()
        .take(MAX_BODY as u64)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    String::from_utf8(buf).map_err(|e| e.to_string())
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Extract a single query parameter (URL-decoded) from a request URL.
fn query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split('?').nth(1)?;
    for pair in q.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next()?;
        if k == key {
            return Some(url_decode(it.next().unwrap_or("")));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_html_is_embedded_and_has_page_marker() {
        assert!(INDEX_HTML.contains("<html"));
        assert!(INDEX_HTML.contains("openstory"));
    }

    #[test]
    fn hidden_files_are_never_listable_configs() {
        // profiles/.manager.json holds saved passwords: it is NOT a
        // RuntimeConfig, yet its extension IS "json" — the old
        // `extension() == json` check listed it, and saving then overwrote
        // the passwords with an all-defaults config.
        assert!(is_listable_config("config.json"));
        assert!(is_listable_config("serverA.json"));
        assert!(!is_listable_config(".manager.json"));
        assert!(!is_listable_config(".env"));
        assert!(!is_listable_config("config.txt"));
        assert!(!is_listable_config("noext"));
    }

    #[test]
    fn hidden_paths_are_rejected_even_when_hand_typed() {
        // The listing hides them, but a hand-crafted URL must not slip past.
        assert!(is_safe_config_path("config.json"));
        assert!(is_safe_config_path("profiles/serverA.json"));
        assert!(!is_safe_config_path("profiles/.manager.json"));
        assert!(!is_safe_config_path(".manager.json"));
        assert!(!is_safe_config_path("profiles/../config.json"));
        assert!(!is_safe_config_path(""));
        // Windows separator form must be caught too.
        assert!(!is_safe_config_path("profiles\\.manager.json"));
    }

    #[test]
    fn index_html_lists_the_new_config_fields_and_commands() {
        // Guard against the form and the "命令参考" panel drifting from the
        // schema / command table again: every entry below is a field or
        // command that was missing from the page before.
        for id in ["gen-data_dir", "gen-include"] {
            assert!(
                INDEX_HTML.contains(&format!("id=\"{id}\"")),
                "page is missing the {id} field"
            );
        }
        for var in ["mobs", "drops", "npcs", "reactors", "teleport"] {
            assert!(
                INDEX_HTML.contains(var),
                "predicate var panel is missing {var}"
            );
        }
        for cmd in [
            "task start",
            "hunt filter",
            "gather ",
            "reactor ",
            "setvar",
            "clearvar",
            "while ",
            "rule open",
            "group run",
            "profiles add",
            "start",
            "stop",
        ] {
            assert!(
                INDEX_HTML.contains(cmd),
                "命令参考 is missing the `{cmd}` entry"
            );
        }
        // hunt is now merged over the loaded config instead of rebuilt from a
        // whitelist, so a new HuntConfig field can no longer be wiped silently.
        assert!(
            INDEX_HTML.contains("out.hunt = { ...(cfg.hunt || {}), ...huntForm }"),
            "hunt must be merged, not rebuilt"
        );
    }
}
