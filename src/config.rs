//! Bot configuration from CLI arguments.

use crate::crypto::MAPLEVERSION;

#[derive(Debug, Clone)]
pub struct Config {
    pub ip: String,
    pub port: u16,
    /// Channel server address override (from SERVER_IP packet auto-detected;
    /// use when the server's CustomIP is not reachable from this machine).
    pub channel_ip: Option<String>,
    pub version: u16,
    pub account: String,
    pub password: String,
    /// Gender to pick if the account needs one (0 = male, 1 = female).
    pub gender: u8,
    pub world: u8,
    pub channel: u8,
    /// Index into the char list, or -1 to use `cid`.
    pub char_index: i32,
    /// Explicit character id to select (when char_index is -1).
    pub cid: i32,
    /// Stop after the char list arrives: print characters, send nothing,
    /// never enter a map (server probing / account checks).
    pub charlist_only: bool,
    pub show_packets: bool,
    /// Dump received packets as raw hex.
    pub dump_packets: bool,
    /// Commands to run automatically once the bot enters a map.
    pub auto: Vec<String>,
    /// Reported attack damage override (`--damage`). None = auto formula by level.
    pub damage: Option<i32>,
    /// Auto-exit after this many seconds (for testing/scripting). None = run
    /// until stdin says quit or the connection drops.
    pub duration: Option<u64>,
    /// Character names allowed to control the bot via in-game whispers
    /// (`--chat-admin <name>`, repeatable). Empty = feature disabled.
    pub chat_admins: Vec<String>,
    /// Custom 256-byte pre-expanded AES round-key table for servers that
    /// replaced the standard MapleStory key. None = standard. Built from
    /// `--aes-key`: an 8-byte key (16 hex chars, expanded at runtime) or a
    /// 256-byte table (512 hex chars).
    pub aes_key: Option<[u8; 256]>,
    /// Interactive login: the login handlers stop at each phase
    /// (GenderPick / WorldSelect / CharSelect) instead of auto-advancing from
    /// CLI flags, and the frontend drives the flow with `login ...` commands.
    /// Always false for the headless CLI.
    pub interactive: bool,
    /// Login-related args explicitly given on the command line
    /// (--ip/--port/--account/--password/--aes-key).
    /// `apply_login` uses this so the CLI takes precedence over login.json.
    pub given_login_args: Vec<&'static str>,
    /// `--config <path>` chain (repeatable, merged in order; later files
    /// override earlier ones). Empty = legacy default `config.json` in the
    /// working directory. Credentials still come from CLI flags only.
    pub config_paths: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            channel_ip: None,
            ip: "127.0.0.1".into(),
            port: 8484,
            version: MAPLEVERSION,
            account: String::new(),
            password: String::new(),
            gender: 0,
            world: 0,
            channel: 1,
            char_index: 0,
            cid: 0,
            charlist_only: false,
            show_packets: false,
            dump_packets: false,
            auto: Vec::new(),
            damage: None,
            duration: None,
            chat_admins: Vec::new(),
            aes_key: None,
            interactive: false,
            given_login_args: Vec::new(),
            config_paths: Vec::new(),
        }
    }
}

/// Parse an AES key from `--aes-key`: 16 hex chars = the 8-byte MapleStory
/// key (expanded at runtime via `crypto::expand_key`), 512 hex chars = a
/// 256-byte pre-expanded round-key table (240 expanded bytes + 16 zero pad).
pub fn parse_aes_key(s: &str) -> Result<[u8; 256], String> {
    let clean: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    match clean.len() {
        16 => {
            let mut key8 = [0u8; 8];
            for i in 0..8 {
                key8[i] = u8::from_str_radix(&clean[i * 2..i * 2 + 2], 16)
                    .map_err(|_| "bad hex in --aes-key".to_string())?;
            }
            Ok(crate::crypto::expand_key(key8))
        }
        512 => {
            let mut key = [0u8; 256];
            for i in 0..256 {
                key[i] = u8::from_str_radix(&clean[i * 2..i * 2 + 2], 16)
                    .map_err(|_| "bad hex in --aes-key".to_string())?;
            }
            Ok(key)
        }
        n => Err(format!(
            "--aes-key must be 16 hex chars (8-byte key) or 512 hex chars (256-byte AES round-key table), got {n}"
        )),
    }
}

impl Config {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let c = Self::parse_args(args)?;
        if c.account.is_empty() {
            return Err(usage());
        }
        Ok(c)
    }

    /// Like `parse`, but does not require `--account` — the TUI collects
    /// credentials interactively when they are missing.
    pub fn parse_partial(args: &[String]) -> Result<Self, String> {
        Self::parse_args(args)
    }

    fn parse_args(args: &[String]) -> Result<Self, String> {
        let mut c = Self::default();

        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            let val = |i: &mut usize| -> Result<String, String> {
                *i += 1;
                if *i >= args.len() {
                    Err(format!("missing value for {}", args[*i - 1]))
                } else {
                    Ok(args[*i].clone())
                }
            };

            match arg.as_str() {
                "--ip" => {
                    c.ip = val(&mut i)?;
                    c.given_login_args.push("--ip");
                }
                "--channel-ip" => c.channel_ip = Some(val(&mut i)?),
                "--port" => {
                    c.port = val(&mut i)?.parse().map_err(|e| format!("bad port: {e}"))?;
                    c.given_login_args.push("--port");
                }
                "--version" => c.version = val(&mut i)?.parse().map_err(|e| format!("bad version: {e}"))?,
                "--account" | "--acc" => {
                    c.account = val(&mut i)?;
                    c.given_login_args.push("--account");
                }
                "--password" | "--pass" => {
                    c.password = val(&mut i)?;
                    c.given_login_args.push("--password");
                }
                "--gender" => {
                    c.gender = val(&mut i)?.parse().map_err(|e| format!("bad gender: {e}"))?
                }
                "--world" => c.world = val(&mut i)?.parse().map_err(|e| format!("bad world: {e}"))?,
                "--channel" => {
                    c.channel = val(&mut i)?.parse().map_err(|e| format!("bad channel: {e}"))?;
                    if c.channel < 1 {
                        return Err("channel must be >= 1 (1-based)".into());
                    }
                }
                "--char" => {
                    let v = val(&mut i)?;
                    match v.parse::<i32>() {
                        Ok(n) if n < 0 => c.char_index = -1,
                        Ok(n) => c.char_index = n,
                        Err(_) => return Err(format!("bad char: {v}")),
                    }
                }
                "--cid" => c.cid = val(&mut i)?.parse().map_err(|e| format!("bad cid: {e}"))?,
                "--charlist-only" => c.charlist_only = true,
                "--show-packets" => c.show_packets = true,
                "--dump" => c.dump_packets = true,
                "--auto" => c.auto.push(val(&mut i)?),
                "--damage" => {
                    c.damage = Some(
                        val(&mut i)?
                            .parse()
                            .map_err(|e| format!("bad damage: {e}"))?,
                    )
                }
                "--duration" => {
                    c.duration = Some(
                        val(&mut i)?
                            .parse()
                            .map_err(|e| format!("bad duration: {e}"))?,
                    )
                }
                "--chat-admin" => c.chat_admins.push(val(&mut i)?),
                "--aes-key" => {
                    c.aes_key = Some(parse_aes_key(&val(&mut i)?)?);
                    c.given_login_args.push("--aes-key");
                }
                "--interactive" => c.interactive = true,
                "--config" => c.config_paths.push(val(&mut i)?),
                // 多档案启动 (多开控制台): `--profiles a.json b.json ...`。
                // 消费掉其后所有非选项 token 并**忽略** —— 多会话的档案列表由
                // 前端自己解释 (它需要的是"N 个独立账号", 而不是 `--config` 的
                // "多层覆盖同一个账号"链式语义)。
                "--profiles" => {
                    while i + 1 < args.len() && !args[i + 1].starts_with('-') {
                        i += 1;
                    }
                }
                // 多开控制台: 关掉掉线自动拉起。默认开启, 由前端解释
                // (核心层不认识"拉起"这件事 —— 它只跑一次就结束)。
                "--no-restart" => {}
                // 多开控制台 (手动模式): 启动时**不**自动登录任何账号, 全部停在
                // 「未启动」, 由用户逐个启动 (F4 / `start`)。同样只由前端解释。
                "--no-autostart" | "--manual" => {}
                // 多开控制台: 只自动启动这些档案 (其余停在未启动)。
                // 消费掉其后所有非选项 token (与 `--profiles` 同样的解析方式)。
                "--start" | "--autostart" => {
                    while i + 1 < args.len() && !args[i + 1].starts_with('-') {
                        i += 1;
                    }
                }
                // 多开控制台: 密码是否落盘 (`profiles/.manager.json`)。
                // **默认不记** —— 核心层的纪律是"密码永不落盘", 这里只在用户
                // 显式要求时才破例 (为了跨程序重启的自动拉起)。核心层不读它。
                "--remember-password" | "--no-remember-password" => {}
                "--help" | "-h" => {
                    return Err(usage());
                }
                other => return Err(format!("unknown arg: {other}")),
            }
            i += 1;
        }

        Ok(c)
    }

    /// 多开: 用**这个档案自己的** `login` 节覆盖从全局继承来的登录字段。
    ///
    /// 与 [`Self::apply_login`] 的区别是"覆盖"而不是"只填空缺"。填空缺的语义
    /// 适用于单账号 (命令行 > login.json), 但多开时那份全局 config 的账号属于
    /// **另一个**号 —— 继承过来就是拿 A 的账号去登 B。
    ///
    /// # 现场 (用户报的"两个号互相挤兑")
    ///
    /// `--profiles a.json b.json`, a 的档案账号是 A、b 的是 B。两个会话的模板都
    /// 从全局 config (账号 = A) 克隆, 而 `apply_login` 见到 `account` 非空就跳过,
    /// 于是**两个会话都登 A**:
    ///
    /// - 给第二个号按 F4, 向导预填的是 A 的账号 (用户看到的第一手症状);
    /// - 提交/启动后同一个账号被登录两次, 服务器把两边来回踢 ——
    ///   "相互重连、不停挤兑"。
    ///
    /// # 仍然尊重显式命令行参数
    ///
    /// `--password` 是"这些号共用这个密码"的明确意图, 不被档案里的值盖掉;
    /// `--ip` / `--port` / `--aes-key` 沿用 [`Self::apply_login`] 原有的
    /// "命令行优先"规则。
    ///
    /// **账号例外**: 即使显式给了 `--account` 也以档案为准 —— 一个全局 account
    /// 分给 N 个档案在语义上就是错的, 那正是这个 bug 本身。
    pub fn apply_login_for_profile(&mut self, info: &crate::login::LoginInfo) {
        if !info.account.is_empty() {
            self.account = info.account.clone();
        }
        // 密码**不**沿用继承来的那份: 档案没写就是"没有密码", 交给凭据存储 /
        // 向导去问。把别的账号的密码塞给这个号只会登错号。
        if !self.given_login_args.iter().any(|a| *a == "--password") {
            self.password = info.password.clone();
        }
        // ip / 端口 / aes 密钥: 沿用"命令行优先"的既有规则。
        self.apply_login(info);
    }

    /// Fill in login fields from the `login` section of login.json/config.json.
    /// Explicit CLI args take precedence (never overwritten); `--aes-key`,
    /// ip and port use the login section only when not given on the CLI.
    pub fn apply_login(&mut self, info: &crate::login::LoginInfo) {
        if self.account.is_empty() {
            self.account = info.account.clone();
        }
        if self.password.is_empty() {
            self.password = info.password.clone();
        }
        if !self.given_login_args.iter().any(|a| *a == "--ip") && !info.ip.is_empty() {
            self.ip = info.ip.clone();
        }
        if !self.given_login_args.iter().any(|a| *a == "--port") {
            if let Some(p) = info.port {
                self.port = p;
            }
        }
        if self.aes_key.is_none() {
            if let Some(k) = &info.aes_key {
                if !k.is_empty() {
                    if let Ok(key) = parse_aes_key(k) {
                        self.aes_key = Some(key);
                    }
                }
            }
        }
    }
}

fn usage() -> String {
    let mut u = String::from(
        "openstory-bot\n\nUSAGE:\n  openstory-bot --ip <ip> --port <port> --account <acc> --password <pass> [options]\n\nOPTIONS:\n  --ip <ip>               server address (default 127.0.0.1)\n  --channel-ip <ip>       channel server address override (default: from SERVER_IP)\n  --port <port>           server port (default 8484)\n  --version <n>           MapleStory version for header XOR (default 79)\n  --account <acc>         account name\n  --password <pass>       account password\n  --gender <0|1>          gender to pick if the account needs one (default 0)\n  --world <n>             world id (default 0)\n  --channel <n>           channel id, 1-based (default 1)\n  --char <n>              character index in the char list (default 0)\n  --cid <n>               explicit character id (uses --cid over --char if both given)\n  --charlist-only         stop after the char list (never enter a map; probe only)\n  --show-packets          print every sent/received opcode\n  --dump                  print received packets as raw hex\n  --auto <cmd>            run <cmd> once the bot enters a map (repeatable)\n  --damage <n>            reported attack damage per hit (default: auto by level)\n  --duration <sec>        auto-exit after <sec> seconds (no stdin needed)\n  --chat-admin <name>     allow <name> to control the bot via whisper (repeatable)\n  --aes-key <hex16|hex512> custom AES key for modified-key servers: 8-byte key (16 hex, expanded) or 256-byte table (512 hex)\n  --config <path>         config file for rules/tasks/groups/hunt (repeatable, merged in order, later wins; write-back target = the last one). Credentials still come from CLI flags only. Default: config.json in the working directory\n",
    );
    u.push_str("  --help                  show this help\n");
    // Multi-open console options: parsed here only so that `--help` documents
    // them and `parse_partial` accepts them; their meaning is implemented by
    // the frontend (the core layer runs exactly one session and knows nothing
    // about "restarting" or "N profiles").
    u.push_str("  --profiles <p...>       multi-open console: run one bot per profile file\n");
    u.push_str("  --no-autostart          multi-open console (MANAGER mode): load the profiles but do NOT log in; every account starts as 「not started」 and you start it yourself (F4 / `start`)\n");
    u.push_str("  --start <name...>       multi-open console: auto-start only these profiles (by name); the rest stay 「not started」\n");
    u.push_str("  --no-restart            multi-open console: do NOT auto-restart dropped sessions\n");
    u.push_str("  --remember-password     multi-open console: persist passwords to profiles/.manager.json (PLAINTEXT) so auto-restart survives a program restart; default is NOT to remember\n");
    u.push_str("  --no-remember-password  multi-open console: never write passwords to disk (wins if both are given)\n");
    u
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_aes_key_8byte_expands() {
        // 16 hex chars = the 8-byte key, expanded at runtime.
        let key = parse_aes_key("130806B41B0F3352").unwrap();
        assert_eq!(&key[..], &crate::crypto::standard_key()[..]);
    }

    #[test]
    fn parse_aes_key_table_unchanged() {
        // 512 hex chars = the pre-expanded 256-byte table, passed through.
        let table_hex = crate::crypto::standard_key()
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<String>();
        assert_eq!(table_hex.len(), 512);
        let key = parse_aes_key(&table_hex).unwrap();
        assert_eq!(&key[..], &crate::crypto::standard_key()[..]);
    }

    #[test]
    fn parse_aes_key_rejects_bad_input() {
        assert!(parse_aes_key("130806B41B0F335").is_err()); // 15 hex
        assert!(parse_aes_key("130806B41B0F3352AB").is_err()); // 17 hex
        assert!(parse_aes_key("zz0806B41B0F3352").is_err()); // bad chars
        assert!(parse_aes_key("").is_err());
        // 64 hex (32-byte raw) is intentionally unsupported — no ambiguity.
        assert!(parse_aes_key("130000000900000006000000B40000001B0000000F0000003300000052000000").is_err());
    }

    #[test]
    fn cli_aes_key_8byte_end_to_end() {
        // `--aes-key <hex16>` flows into Config.aes_key as an expanded table.
        let args = vec![
            "--ip".into(), "127.0.0.1".into(),
            "--account".into(), "acc".into(),
            "--password".into(), "pass".into(),
            "--aes-key".into(), "130806B41B0F3352".into(),
        ];
        let c = Config::parse(&args).unwrap();
        assert_eq!(&c.aes_key.unwrap()[..], &crate::crypto::standard_key()[..]);
    }

    #[test]
    fn apply_login_fills_missing_fields() {
        let mut c = Config::parse_partial(&[]).unwrap();
        let info = crate::login::LoginInfo {
            ip: "127.0.0.1".into(),
            port: Some(8484),
            account: "100000003".into(),
            password: "123123".into(),
            aes_key: Some("130A06B41B0F3352".into()),
        };
        c.apply_login(&info);
        assert_eq!(c.ip, "127.0.0.1");
        assert_eq!(c.port, 8484);
        assert_eq!(c.account, "100000003");
        assert_eq!(c.password, "123123");
        // aes_key is a custom expanded key table (4th byte = 0x09; the standard table is 0x08)
        let expanded = parse_aes_key("130A06B41B0F3352").unwrap();
        assert_eq!(&c.aes_key.unwrap()[..], &expanded[..]);
    }

    #[test]
    fn apply_login_does_not_override_explicit_args() {
        let args = vec![
            "--ip".into(), "1.2.3.4".into(),
            "--account".into(), "cli_acc".into(),
            "--aes-key".into(), "130A06B41B0F3352".into(),
        ];
        let mut c = Config::parse_partial(&args).unwrap();
        let info = crate::login::LoginInfo {
            ip: "127.0.0.1".into(),
            port: Some(9999),
            account: "100000003".into(),
            password: "123123".into(),
            aes_key: Some("130806B41B0F3352".into()),
        };
        c.apply_login(&info);
        assert_eq!(c.ip, "1.2.3.4"); // CLI takes precedence
        assert_eq!(c.port, 9999); // no CLI port → login fills it
        assert_eq!(c.account, "cli_acc"); // CLI takes precedence
        assert_eq!(c.password, "123123"); // missing → login fills it
        // CLI aes-key given → not overridden by the login section
        let expanded = parse_aes_key("130A06B41B0F3352").unwrap();
        assert_eq!(&c.aes_key.unwrap()[..], &expanded[..]);
    }

    #[test]
    fn apply_login_for_profile_overrides_an_inherited_account() {
        // 现场 bug: 多开时每个会话的模板都是从全局 config 克隆的, 而全局那份
        // 带着**上一个**账号。`apply_login` 见 account 非空就跳过, 于是所有
        // 会话都登同一个号 —— 服务器把两边来回踢。
        // 全局那份已经带着账号 A 和密码 "inherited" (模拟 config.json / 上一个档案)。
        let mut c = Config::parse_partial(&["--account".into(), "100000001".into()]).unwrap();
        c.password = "inherited".into();
        let info = crate::login::LoginInfo {
            ip: "127.0.0.1".into(),
            port: Some(8484),
            account: "100000002".into(),
            password: String::new(),
            aes_key: None,
        };
        c.apply_login_for_profile(&info);
        assert_eq!(c.account, "100000002", "账号必须以档案为准, 不能继承上一个号");
        // 档案没写密码 → 清空, 交给凭据存储/向导; 绝不沿用别的账号的密码。
        // (显式 --password 是"共用密码"的意图, 那个例外见下一个用例。)
        assert_eq!(c.password, "", "不能把上一个号的密码带过来");
        assert_eq!(c.ip, "127.0.0.1", "ip 仍由档案填");
        assert_eq!(c.port, 8484);
    }

    #[test]
    fn apply_login_for_profile_keeps_an_explicit_shared_password() {
        // `--password` 是"这些号共用这个密码"的明确意图, 不该被档案盖掉。
        let mut c = Config::parse_partial(&["--password".into(), "shared".into()]).unwrap();
        let info = crate::login::LoginInfo {
            ip: "127.0.0.1".into(),
            port: Some(8484),
            account: "100000002".into(),
            password: "profile_pw".into(),
            aes_key: None,
        };
        c.apply_login_for_profile(&info);
        assert_eq!(c.account, "100000002", "账号以档案为准");
        assert_eq!(c.password, "shared", "显式 --password 优先");
    }

    #[test]
    fn apply_login_for_profile_beats_an_explicit_account_flag() {
        // 一个全局 `--account` 分给 N 个档案在语义上就是错的 (那正是 bug 本身),
        // 所以账号这一项连命令行也不让 —— 档案即身份。
        let mut c = Config::parse_partial(&["--account".into(), "cli_acc".into()]).unwrap();
        let info = crate::login::LoginInfo {
            ip: String::new(),
            port: None,
            account: "100000002".into(),
            password: "p".into(),
            aes_key: None,
        };
        c.apply_login_for_profile(&info);
        assert_eq!(c.account, "100000002");
    }

    #[test]
    fn apply_login_for_profile_keeps_the_inherited_account_when_the_profile_has_none() {
        // 档案里没有 login 节 (或账号为空) → 保持继承值, 不做无谓的清空。
        let mut c = Config::parse_partial(&["--account".into(), "solo".into()]).unwrap();
        let info = crate::login::LoginInfo {
            ip: "127.0.0.1".into(),
            port: Some(8484),
            account: String::new(),
            password: String::new(),
            aes_key: None,
        };
        c.apply_login_for_profile(&info);
        assert_eq!(c.account, "solo");
        assert_eq!(c.port, 8484, "其他字段照常由档案补");
    }

    #[test]
    fn config_flag_repeatable_and_ordered() {
        let args = vec![
            "--account".into(), "a".into(),
            "--password".into(), "p".into(),
            "--config".into(), "profiles/one.json".into(),
            "--auto".into(), "view attr".into(),
            "--config".into(), "profiles/two.json".into(),
        ];
        let c = Config::parse(&args).unwrap();
        assert_eq!(
            c.config_paths,
            vec!["profiles/one.json".to_string(), "profiles/two.json".to_string()]
        );
        assert_eq!(c.auto, vec!["view attr".to_string()]);
    }

    #[test]
    fn config_flag_missing_value_errors() {
        let args = vec![
            "--account".into(), "a".into(),
            "--password".into(), "p".into(),
            "--config".into(),
        ];
        assert!(Config::parse(&args).is_err());
    }

    #[test]
    fn config_paths_default_empty() {
        let c = Config::parse_partial(&[]).unwrap();
        assert!(c.config_paths.is_empty());
    }
}

