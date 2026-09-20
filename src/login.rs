//! Login info (the `login` section of config.json): account / optional AES key.
//!
//! Shares config.json with the game config; no separate file needed. `aes_key`
//! is only required for servers with a custom key; missing/empty = the standard
//! MapleStory key.
//! **The password is never persisted** (skip_serializing): only ip / port /
//! account / aes_key are written back; the password lives in memory (Config /
//! state) for the session — reconnects reuse it, restarts re-enter it.
//! If a password already exists in the config file it is still read on load.

use serde::{Deserialize, Serialize};

pub const LOGIN_KEY: &str = "login";

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LoginInfo {
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub account: String,
    /// password: skipped when serializing (never written to config.json);
    /// session-only, in-memory (reconnects reuse it; wizard re-enters it).
    #[serde(default, skip_serializing)]
    pub password: String,
    /// custom AES key (16 hex = 8-byte key, 512 hex = pre-expanded table);
    /// None/empty = the standard key (servers that don't change it, e.g. test servers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aes_key: Option<String>,
}

/// Read the `login` section from config.json; returns None if the file is
/// missing / has no such section / fails to parse (optional config, silent).
pub fn load(path: &std::path::Path) -> Option<LoginInfo> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    serde_json::from_value(v.get(LOGIN_KEY)?.clone()).ok()
}

pub fn load_default() -> Option<LoginInfo> {
    load(&crate::runtime_config::write_path())
}

/// Write the `login` section back into the file, preserving all other
/// fields. Creates a minimal file with just the login section if it doesn't
/// exist. Refuses to touch a corrupt target (parse error) instead of
/// silently replacing it.
pub fn save(path: &std::path::Path, info: &LoginInfo) -> Result<(), String> {
    let mut v: serde_json::Value = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| {
            format!(
                "{} is not valid JSON ({e}) — login save refused to overwrite",
                path.display()
            )
        })?,
        Err(_) => serde_json::Value::Object(serde_json::Map::new()),
    };
    v.as_object_mut()
        .ok_or("config.json root must be an object")?
        .insert(
            LOGIN_KEY.into(),
            serde_json::to_value(info).map_err(|e| e.to_string())?,
        );
    let text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn save_default(info: &LoginInfo) -> Result<(), String> {
    save(&crate::runtime_config::write_path(), info)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ost_login_{}_{}", name, std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("config.json")
    }

    #[test]
    fn save_load_roundtrip_keeps_other_fields() {
        let p = tmp_path("rt1");
        std::fs::write(
            &p,
            r#"{"tick_ms":1,"hunt":{"attack_mode":"area"},"rules":[]}"#,
        )
        .unwrap();
        let info = LoginInfo {
            ip: "127.0.0.1".into(),
            port: Some(8484),
            account: "100000001".into(),
            password: "test_password".into(),
            aes_key: Some("130A06B41B0F3352".into()),
        };
save(&p, &info).unwrap();
        // other fields preserved
        let text = std::fs::read_to_string(&p).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["tick_ms"], 1);
        assert_eq!(v["hunt"]["attack_mode"], "area");
        // password never persisted (only ip/port/account/aes_key saved)
        assert!(v["login"].get("password").is_none(), "password must not be saved");
        // login section reads back (password empty)
        let back = load(&p).unwrap();
        assert_eq!(back.ip, "127.0.0.1");
        assert_eq!(back.port, Some(8484));
        assert_eq!(back.account, "100000001");
        assert_eq!(back.password, "", "password never persisted");
        assert_eq!(back.aes_key.as_deref(), Some("130A06B41B0F3352"));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn password_in_file_still_loads() {
        let p = tmp_path("rt4");
        std::fs::write(
            &p,
            r#"{"login":{"account":"a","password":"oldpass","aes_key":"130A06B41B0F3352"}}"#,
        )
        .unwrap();
        let back = load(&p).unwrap();
        assert_eq!(back.account, "a");
        assert_eq!(back.password, "oldpass", "password still readable");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn save_creates_file_when_missing() {
        let p = tmp_path("rt2");
        let _ = std::fs::remove_file(&p);
        let info = LoginInfo {
            account: "a".into(),
            password: "b".into(),
            ..Default::default()
        };
        save(&p, &info).unwrap();
        assert!(p.exists());
        let back = load(&p).unwrap();
        assert_eq!(back.account, "a");
        assert_eq!(back.aes_key, None);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn save_refuses_to_wipe_corrupt_target() {
        // 回归: 旧实现对解析失败的文件 unwrap_or(空对象), 向导保存会把整个
        // 配置静默替换成只剩 login 一节 — 现在必须报错且原文件不动。
        let p = tmp_path("rt5");
        let corrupt = "{oops";
        std::fs::write(&p, corrupt).unwrap();
        let info = LoginInfo::default();
        assert!(save(&p, &info).is_err(), "损坏目标必须拒绝写入");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), corrupt, "原文件不得被改动");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn no_login_section_returns_none() {
        let p = tmp_path("rt3");
        std::fs::write(&p, r#"{"tick_ms":5}"#).unwrap();
        assert!(load(&p).is_none());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn missing_file_returns_none() {
        assert!(load(std::path::Path::new("C:\\nope\\config.json")).is_none());
    }
}


