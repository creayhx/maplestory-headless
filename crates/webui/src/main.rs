//! Standalone `openstory-web`: the config.json web editor as its own process.
//!
//! Serves the same config page + `/api/config` endpoints as the former
//! embedded `--web` mode, but completely decoupled from the bot: it reads and
//! writes config.json directly, validating the payload against the bot's
//! `RuntimeConfig` schema before touching the file. The bot itself never
//! compiles this code (no `web` feature, no tiny_http) — changes take effect
//! when the bot next runs `reload` or restarts.
//!
//! Usage:
//!   openstory-web [--port 8080] [--config config.json] [--dir <web_dir>]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use openstory_bot::runtime_config::RuntimeConfig;

const USAGE: &str = "\
openstory-web — config.json 网页编辑器（独立程序，与 bot 无进程内关联）

USAGE:
  openstory-web [--port <port>] [--config <path>] [--dir <path>]

OPTIONS:
  --port <n>      listen on 127.0.0.1:<n> (default 8080)
  --config <path> config file to read/write (default config.json, same as the bot)
  --configs <path> base dir to scan for config files (default: --config's parent)
  --dir <path>    serve <path>/index.html instead of the embedded page
  --help          show this help
";

/// Parse CLI args; `Err` = usage/argument problem (printed + exit 2).
fn parse_args(args: &[String]) -> Result<(u16, PathBuf, Option<PathBuf>, Option<PathBuf>), String> {
    let mut port: u16 = 8080;
    let mut config_path = PathBuf::from("config.json");
    let mut web_dir: Option<PathBuf> = None;
    let mut configs_dir: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let val = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            args.get(*i)
                .cloned()
                .ok_or_else(|| format!("missing value for {}", args[*i - 1]))
        };
        match arg.as_str() {
            "--port" => {
                port = val(&mut i)?.parse().map_err(|e| format!("bad port: {e}"))?;
            }
            "--config" => config_path = PathBuf::from(val(&mut i)?),
            "--dir" => web_dir = Some(PathBuf::from(val(&mut i)?)),
            "--configs" => configs_dir = Some(PathBuf::from(val(&mut i)?)),
            "--help" | "-h" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg: {other}")),
        }
        i += 1;
    }
    Ok((port, config_path, web_dir, configs_dir))
}

/// Resolve a `file` (relative path from the listing) against `base`, rejecting
/// absolute paths, `..` traversal, dot-prefixed segments and non-`.json` files.
/// Symlinks are collapsed via canonicalization so the result always lives under
/// `base`.
fn resolve(base: &Path, file: &str) -> Option<PathBuf> {
    if !openstory_webui::is_safe_config_path(file) {
        return None;
    }
    let cand = base.join(file);
    if cand.extension().map(|e| e != "json").unwrap_or(true) {
        return None;
    }
    // Anchor on the parent directory (which must exist and stay within `base`),
    // so saves may target a not-yet-existing file while GETs still resolve the
    // real path. This avoids the relative-vs-absolute `starts_with` mismatch.
    // Empty segments canonicalize as "." (the current dir) on Windows.
    let parent = cand.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let parent_canon = std::fs::canonicalize(parent).ok()?;
    let base_arg: &Path = if base.as_os_str().is_empty() { Path::new(".") } else { base };
    let base_canon = std::fs::canonicalize(base_arg).ok()?;
    if !parent_canon.starts_with(&base_canon) {
        return None;
    }
    let name = cand.file_name()?;
    Some(parent_canon.join(name))
}

fn main() {
    openstory_bot::setup_console_utf8();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let (port, config_path, web_dir, configs_dir) = match parse_args(&args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    // Same as the bot's first launch: no config.json → write defaults so the
    // page always has something to show.
    if !config_path.exists() {
        match std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&RuntimeConfig::default())
                .map_err(|e| e.to_string())
                .unwrap(),
        ) {
            Ok(()) => println!("[web] no {} — wrote defaults", config_path.display()),
            Err(e) => {
                eprintln!("[web] cannot write default {}: {e}", config_path.display());
                std::process::exit(2);
            }
        }
    }

    let base_dir = configs_dir.unwrap_or_else(|| {
        let p = config_path.parent().unwrap_or_else(|| Path::new("."));
        if p.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            p.to_path_buf()
        }
    });

    let list_base = base_dir.clone();
    let list_cfg_path = config_path.clone();
    let list_configs = Arc::new(move || {
        let mut entries: Vec<serde_json::Value> = Vec::new();
        let mut add_dir = |dir: &Path, prefix: &str| {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if openstory_webui::is_listable_config(&name) {
                        let rel = if prefix.is_empty() {
                            name.clone()
                        } else {
                            format!("{}/{}", prefix, name)
                        };
                        entries.push(serde_json::json!({ "name": name, "path": rel }));
                    }
                }
            }
        };
        add_dir(&list_base, "");
        add_dir(&list_base.join("profiles"), "profiles");
        let cfg_rel = list_cfg_path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        if !entries
            .iter()
            .any(|v| v["path"].as_str() == Some(cfg_rel.as_str()))
        {
            entries.push(serde_json::json!({ "name": cfg_rel, "path": cfg_rel }));
        }
        entries.sort_by(|a, b| {
            let ap = a["path"].as_str().unwrap_or("");
            let bp = b["path"].as_str().unwrap_or("");
            let ao = if ap == "config.json" { 0u8 } else { 1u8 };
            let bo = if bp == "config.json" { 0u8 } else { 1u8 };
            ao.cmp(&bo).then_with(|| ap.cmp(bp))
        });
        serde_json::json!(entries).to_string()
    });

    let get_base = base_dir.clone();
    let get_path = config_path.clone();
    let get_config = Arc::new(move |file: Option<&str>| {
        let path = match file {
            Some(f) => match resolve(&get_base, f) {
                Some(p) => p,
                None => return "{\"error\":\"invalid config path\"}".to_string(),
            },
            None => get_path.clone(),
        };
        std::fs::read_to_string(&path).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
    });

    let save_base = base_dir.clone();
    let save_path = config_path.clone();
    let save_config = Arc::new(
        move |file: Option<&str>, body: &str| -> Result<String, String> {
            let path = match file {
                Some(f) => resolve(&save_base, f).ok_or_else(|| "非法配置路径".to_string())?,
                None => save_path.clone(),
            };
            // Validate against the real schema before touching the file.
            serde_json::from_str::<RuntimeConfig>(body)
                .map_err(|e| format!("配置校验失败: {e}"))?;
            // Atomic write: tmp file in the same directory, then rename.
            let tmp = format!("{}.tmp", path.display());
            std::fs::write(&tmp, body).map_err(|e| format!("写入失败: {e}"))?;
            std::fs::rename(&tmp, &path)
                .map_err(|e| format!("替换 {} 失败: {e}", path.display()))?;
            Ok(format!(
                "已写入 {}（运行中的 bot 需 reload 或重启后生效）",
                path.display()
            ))
        },
    );

    let addr = format!("127.0.0.1:{port}");
    let cfg = openstory_webui::WebConfig { addr, web_dir };
    let handlers = openstory_webui::Handlers {
        list_configs,
        get_config,
        save_config,
    };

    println!(
        "[web] openstory-web serving http://127.0.0.1:{port} (config: {})",
        config_path.display()
    );
    println!("[web] Ctrl+C to stop");
    if let Err(e) = openstory_webui::serve(cfg, handlers) {
        eprintln!("[web] server stopped: {e}");
        std::process::exit(1);
    }
}
