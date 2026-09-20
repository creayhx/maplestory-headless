//! 多开控制台的凭据存储 (`profiles/.manager.json`)。
//!
//! # 为什么需要它
//!
//! 核心层有一条硬纪律: **密码永不落盘** (`login.rs` 的 `skip_serializing`)。
//! 那条纪律是对的 (单开时重连复用内存里的密码, 重启程序重新输入), 但它与
//! 自动拉起直接冲突:
//!
//! > 拉起一个会话 = 重新登录一次 = 需要明文密码。
//!
//! 单开时这不成问题 —— 进程还活着, 密码还在内存里。但**程序重启之后**
//! 要拉起上次掉线的号, 就必须有地方存住它。所以这里单独开一个文件, 并且:
//!
//! 1. **默认不记** (`Remember::No`) —— 与今天的行为完全一致, 不引入新的
//!    落盘面。想要自动拉起跨重启生效的人显式打开 (`Remember::Yes`)。
//! 2. 存储位置与权限都明确: `profiles/.manager.json`, 明文, 只有该用户可读
//!    (Windows 上依赖用户目录 ACL)。文件里**写明**这一点, 不做假加密 ——
//!    假加密会让人误以为它是安全的。
//! 3. 密码只按**档案名**索引, 不按账号 —— 同一个账号在两个档案里可以是不同
//!    的密码 (不同服/不同 key), 按账号索引会在那种情况下取错。
//!
//! # 与档案里 `login.password` 的关系
//!
//! 那份是用户自己写在档案里的 (核心层读得到), 优先级**高于**这里: 档案是
//! 用户显式的意图, 这个文件只是控制台为了拉起而做的缓存。两者都缺时, 会话
//! 会在拉起后以 `RunOutcome::Failed` 结束并被标记「需人工」—— 那个行为是
//! 刻意的 (宁可标出来让人看一眼, 也不做密码错的重启风暴)。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 存储文件名 (在 `profiles/` 下, 与档案并列)。
pub const STORE_FILE: &str = ".manager.json";

/// 当前文件格式版本。将来改格式时用它做迁移判断。
pub const STORE_VERSION: u32 = 1;

/// 一个档案的凭据。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Credential {
    /// 账号 (诊断用; 真正的索引是档案名)。
    #[serde(default)]
    pub account: String,
    /// 明文密码。
    ///
    /// 刻意**不做**假加密: 解码脚本就在旁边, 只会让人误以为安全。真要安全
    /// 就用操作系统凭据管理器, 那是另一件事 (见模块文档)。
    #[serde(default)]
    pub password: String,
}

/// 磁盘上的存储结构。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreFile {
    #[serde(default = "default_version")]
    pub version: u32,
    /// 给人看的警告 —— 打开文件的人应该立刻知道这里面是明文密码。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub warning: String,
    /// 档案名 -> 凭据。
    #[serde(default)]
    pub profiles: BTreeMap<String, Credential>,
}

fn default_version() -> u32 {
    STORE_VERSION
}

impl Default for StoreFile {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            warning: String::new(),
            profiles: BTreeMap::new(),
        }
    }
}

/// 是否把密码落盘 (两种模式)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remember {
    /// 不落盘: 密码只在本次进程的内存里, 程序退出就没了。
    /// 单开时代的行为, **默认值**。
    No,
    /// 落盘到 `profiles/.manager.json`, 让自动拉起能跨程序重启生效。
    Yes,
}

impl Remember {
    /// 从命令行参数判定 (`--remember-password` / `--no-remember-password`)。
    ///
    /// 两个都给时取**后者**? 不 —— 取 `--no-remember-password`, 即"不记"优先。
    /// 涉及密码落盘时, 保守的一侧必须赢 (命令别名/脚本拼接很容易同时带上两个)。
    pub fn from_args(args: &[String]) -> Self {
        if args.iter().any(|a| a == "--no-remember-password") {
            return Remember::No;
        }
        if args.iter().any(|a| a == "--remember-password") {
            return Remember::Yes;
        }
        Remember::No
    }

    pub fn is_yes(self) -> bool {
        matches!(self, Remember::Yes)
    }
}

/// 凭据存储 (内存视图 + 落盘策略)。
#[derive(Debug, Clone)]
pub struct Credentials {
    path: PathBuf,
    remember: Remember,
    entries: BTreeMap<String, Credential>,
    /// 加载时遇到的问题 (损坏 / 不可读)。**不吞掉** —— 调用方要能把它显示给
    /// 用户: "你的密码没能读出来" 和 "你没存过密码" 是两件不同的事。
    load_error: Option<String>,
}

impl Credentials {
    /// 默认位置: 工作目录下的 `profiles/.manager.json`。
    pub fn default_path() -> PathBuf {
        PathBuf::from("profiles").join(STORE_FILE)
    }

    /// 从默认位置加载。
    pub fn load(remember: Remember) -> Self {
        Self::load_from(Self::default_path(), remember)
    }

    /// 从指定文件加载。文件不存在是**正常情况** (第一次跑), 不算错误。
    pub fn load_from(path: impl Into<PathBuf>, remember: Remember) -> Self {
        let path = path.into();
        let mut me = Self {
            path,
            remember,
            entries: BTreeMap::new(),
            load_error: None,
        };
        let text = match std::fs::read_to_string(&me.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return me,
            Err(e) => {
                me.load_error = Some(format!("无法读取 {}: {e}", me.path.display()));
                return me;
            }
        };
        match serde_json::from_str::<StoreFile>(&text) {
            Ok(f) => me.entries = f.profiles,
            Err(e) => {
                // 损坏时**不**清空文件 (那会把用户其它档案的密码一起丢掉),
                // 只在内存里当空的并记下原因。
                me.load_error = Some(format!("{} 不是有效的 JSON: {e}", me.path.display()));
            }
        }
        me
    }

    /// 纯内存存储 (测试 / 从不落盘的场合)。
    pub fn in_memory(remember: Remember) -> Self {
        Self {
            path: PathBuf::new(),
            remember,
            entries: BTreeMap::new(),
            load_error: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn remember(&self) -> Remember {
        self.remember
    }

    /// 加载时出的问题 (None = 一切正常)。
    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    /// 记下一个档案的凭据 (只在内存里; 落盘要显式 [`Self::save`])。
    pub fn record(&mut self, profile: &str, account: &str, password: &str) {
        if profile.is_empty() {
            return;
        }
        self.entries.insert(
            profile.to_string(),
            Credential {
                account: account.to_string(),
                password: password.to_string(),
            },
        );
    }

    /// 取一个档案的密码。
    ///
    /// 返回 `None` 或空串都表示"没有可用密码" —— 调用方按 `None` 处理即可,
    /// 这里统一把空密码也归为 `None` (空密码登录必然失败, 拿去重试只会浪费
    /// 一次请求并很可能触发风控)。
    pub fn password_for(&self, profile: &str) -> Option<&str> {
        let p = self.entries.get(profile)?.password.as_str();
        if p.is_empty() {
            None
        } else {
            Some(p)
        }
    }

    /// 有没有这个档案的密码。
    pub fn has(&self, profile: &str) -> bool {
        self.password_for(profile).is_some()
    }

    /// 已记录的档案名 (有序)。
    pub fn profiles(&self) -> Vec<&str> {
        self.entries.keys().map(|s| s.as_str()).collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 落盘。`Remember::No` 时是 **no-op** (返回 `Ok(())`) —— 调用方不需要
    /// 在每处都判断模式, 由这里统一兜住"不许写"的纪律。
    pub fn save(&self) -> Result<(), String> {
        if !self.remember.is_yes() {
            return Ok(());
        }
        if self.path.as_os_str().is_empty() {
            return Ok(()); // 纯内存存储
        }
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("无法创建 {}: {e}", parent.display()))?;
            }
        }
        let file = StoreFile {
            version: STORE_VERSION,
            warning: "明文密码, 仅本用户可读。删除本文件即清除记住的密码。".to_string(),
            profiles: self.entries.clone(),
        };
        let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
        std::fs::write(&self.path, text)
            .map_err(|e| format!("无法写入 {}: {e}", self.path.display()))?;
        restrict_permissions(&self.path);
        Ok(())
    }

    /// 清空 (用户要求"别再记住"时调用), 并删除文件。
    pub fn clear(&mut self) -> Result<(), String> {
        self.entries.clear();
        if self.path.as_os_str().is_empty() || !self.path.exists() {
            return Ok(());
        }
        std::fs::remove_file(&self.path)
            .map_err(|e| format!("无法删除 {}: {e}", self.path.display()))
    }
}

/// 把文件权限收到"只有本人可读"。
///
/// Windows 上依赖用户目录本身的 ACL (不做额外动作 —— 用 icacls 改权限会失败
/// 在没有权限的机器上, 而且用户目录默认已经只有本人可读)。Unix 上显式 `0600`
/// 是必须的: 目录里的 `0644` 会被同机其它用户读到。
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut p = meta.permissions();
        p.set_mode(0o600);
        let _ = std::fs::set_permissions(path, p);
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {
    // Windows: 见上面的说明。
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ost_cred_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(STORE_FILE)
    }

    #[test]
    fn remember_defaults_to_no() {
        // 默认**不**落盘 —— 与今天的行为一致, 不引入新的落盘面
        assert_eq!(Remember::from_args(&[]), Remember::No);
        assert_eq!(
            Remember::from_args(&["--profiles".into(), "a.json".into()]),
            Remember::No
        );
        assert_eq!(
            Remember::from_args(&["--remember-password".into()]),
            Remember::Yes
        );
    }

    #[test]
    fn no_wins_when_both_flags_are_given() {
        // 涉及密码落盘时保守的一侧必须赢
        let args = vec![
            "--remember-password".to_string(),
            "--no-remember-password".to_string(),
        ];
        assert_eq!(Remember::from_args(&args), Remember::No);
        // 反过来也一样
        let args = vec![
            "--no-remember-password".to_string(),
            "--remember-password".to_string(),
        ];
        assert_eq!(Remember::from_args(&args), Remember::No);
    }

    #[test]
    fn record_and_read_back() {
        let mut c = Credentials::in_memory(Remember::No);
        assert!(!c.has("acc1"));
        assert_eq!(c.password_for("acc1"), None, "没记过必须返回 None");
        c.record("acc1", "100000001", "test_password");
        assert!(c.has("acc1"));
        assert_eq!(c.password_for("acc1"), Some("test_password"));
        assert_eq!(c.len(), 1);
        assert_eq!(c.profiles(), vec!["acc1"]);
    }

    #[test]
    fn empty_password_is_treated_as_missing() {
        // 空密码登录必然失败 —— 拿去重试只会浪费一次请求并可能触发风控
        let mut c = Credentials::in_memory(Remember::No);
        c.record("acc1", "100000001", "");
        assert!(!c.has("acc1"));
        assert_eq!(c.password_for("acc1"), None);
    }

    #[test]
    fn recording_an_empty_profile_name_is_ignored() {
        let mut c = Credentials::in_memory(Remember::No);
        c.record("", "acc", "pw");
        assert!(c.is_empty(), "空档案名不该写进存储");
    }

    #[test]
    fn save_is_a_noop_when_not_remembering() {
        // 这是"密码永不落盘"纪律的机械保证: 调用方不用每处都判断模式
        let p = tmp("nofile");
        let mut c = Credentials::load_from(&p, Remember::No);
        c.record("acc1", "a", "pw");
        c.save().unwrap();
        assert!(!p.exists(), "不记模式绝不能创建文件");
    }

    #[test]
    fn save_and_reload_roundtrip() {
        let p = tmp("rt");
        let mut c = Credentials::load_from(&p, Remember::Yes);
        c.record("本地服_100000001", "100000001", "test_password");
        c.record("serverA_1", "100000002", "hunter2");
        c.save().unwrap();
        assert!(p.exists());

        let back = Credentials::load_from(&p, Remember::Yes);
        assert!(back.load_error().is_none());
        assert_eq!(back.password_for("本地服_100000001"), Some("test_password"));
        assert_eq!(back.password_for("serverA_1"), Some("hunter2"));
        assert_eq!(back.profiles(), vec!["serverA_1", "本地服_100000001"]);
        // 文件里要有明确警告 (它不是"加密过的")
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("明文"), "文件里必须有明文警告: {text}");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn missing_file_is_not_an_error() {
        // 第一次跑 (还没存过密码) 与"文件读坏了"必须能区分
        let p = tmp("missing").join("nope").join(STORE_FILE);
        let c = Credentials::load_from(&p, Remember::Yes);
        assert!(c.is_empty());
        assert!(c.load_error().is_none(), "文件不存在不是错误");
    }

    #[test]
    fn corrupt_file_reports_an_error_and_keeps_nothing() {
        let p = tmp("corrupt");
        std::fs::write(&p, "{oops").unwrap();
        let c = Credentials::load_from(&p, Remember::Yes);
        assert!(c.is_empty());
        let e = c.load_error().expect("损坏必须报错");
        assert!(e.contains("JSON"), "got {e}");
        // 而且不得动原文件 (下次还能人工修)
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{oops");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn save_over_a_corrupt_file_replaces_it() {
        // 与 `login.rs` 的策略不同: 那个文件里还有规则/任务, 覆盖会丢配置;
        // 这里整个文件只存密码, 坏了就没有可保留的信息。
        let p = tmp("overwrite");
        std::fs::write(&p, "{oops").unwrap();
        let mut c = Credentials::load_from(&p, Remember::Yes);
        assert!(c.load_error().is_some());
        c.record("acc1", "a", "pw");
        c.save().unwrap();
        let back = Credentials::load_from(&p, Remember::Yes);
        assert_eq!(back.password_for("acc1"), Some("pw"));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn save_creates_the_parent_directory() {
        let p = tmp("mkdir").join("sub").join(STORE_FILE);
        let mut c = Credentials::load_from(&p, Remember::Yes);
        c.record("acc1", "a", "pw");
        c.save().unwrap();
        assert!(p.exists(), "父目录应被自动创建");
        let _ = std::fs::remove_dir_all(p.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn clear_removes_the_file() {
        let p = tmp("clear");
        let mut c = Credentials::load_from(&p, Remember::Yes);
        c.record("acc1", "a", "pw");
        c.save().unwrap();
        assert!(p.exists());
        c.clear().unwrap();
        assert!(!p.exists(), "clear 必须删掉文件");
        assert!(c.is_empty());
        // 再 clear 一次不该报错 (幂等)
        c.clear().unwrap();
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn accounts_are_keyed_by_profile_not_by_account() {
        // 同一个账号名在两个档案里可以是不同密码 (不同服 / 不同 key)
        let mut c = Credentials::in_memory(Remember::No);
        c.record("serverA", "100000001", "pw_a");
        c.record("serverB", "100000001", "pw_b");
        assert_eq!(c.password_for("serverA"), Some("pw_a"));
        assert_eq!(c.password_for("serverB"), Some("pw_b"));
    }

    #[test]
    fn unknown_version_still_loads_known_fields() {
        // 将来版本升级时不能因为多了一个字段就整个读不出来
        let p = tmp("fwd");
        std::fs::write(
            &p,
            r#"{"version":99,"future_field":true,"profiles":{"acc1":{"account":"a","password":"pw","extra":1}}}"#,
        )
        .unwrap();
        let c = Credentials::load_from(&p, Remember::Yes);
        assert!(c.load_error().is_none(), "多出的字段不该让加载失败");
        assert_eq!(c.password_for("acc1"), Some("pw"));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn empty_file_is_an_error_not_a_silent_empty_store() {
        let p = tmp("empty");
        std::fs::write(&p, "").unwrap();
        let c = Credentials::load_from(&p, Remember::Yes);
        assert!(c.load_error().is_some(), "空文件不是合法 JSON, 应报错");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn saved_file_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let p = tmp("perm");
        let mut c = Credentials::load_from(&p, Remember::Yes);
        c.record("acc1", "a", "pw");
        c.save().unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "同机其它用户不该能读到密码");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
