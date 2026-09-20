//! 登录向导表单 (ip / 端口 / 账号 / 密码 / AES 密钥)。

use crossterm::event::{KeyCode, KeyEvent};

#[derive(Debug, Clone)]
pub struct WizardResult {
    pub ip: String,
    pub port: u16,
    pub account: String,
    pub password: String,
    /// 自定义 AES 密钥 (16/512 hex); 空 = 标准密钥 (不改密钥的服务端)。
    pub aes_key: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Ip,
    Port,
    Account,
    Password,
    AesKey,
}

impl Field {
    fn next(self) -> Field {
        match self {
            Field::Ip => Field::Port,
            Field::Port => Field::Account,
            Field::Account => Field::Password,
            Field::Password => Field::AesKey,
            Field::AesKey => Field::Ip,
        }
    }
    fn prev(self) -> Field {
        match self {
            Field::Ip => Field::AesKey,
            Field::Port => Field::Ip,
            Field::Account => Field::Port,
            Field::Password => Field::Account,
            Field::AesKey => Field::Password,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Field::Ip => "服务器IP",
            Field::Port => "端  口",
            Field::Account => "账  号",
            Field::Password => "密  码",
            Field::AesKey => "AES密钥",
        }
    }
}

pub struct Wizard {
    pub ip: String,
    pub port: String,
    pub account: String,
    pub password: String,
    pub aes_key: String,
    pub focus: Field,
    /// 提交结果 (供主线程取走)
    pub result: Option<WizardResult>,
    pub error: Option<String>,
}

impl Wizard {
    pub fn new(ip: String, port: u16, account: String, password: String, aes_key: String) -> Self {
        let focus = if account.is_empty() && password.is_empty() {
            Field::Account
        } else if password.is_empty() {
            Field::Password
        } else {
            Field::Ip
        };
        Self {
            ip,
            port: port.to_string(),
            account,
            password,
            aes_key,
            focus,
            result: None,
            error: None,
        }
    }

    pub fn value(&self, f: Field) -> &str {
        match f {
            Field::Ip => &self.ip,
            Field::Port => &self.port,
            Field::Account => &self.account,
            Field::Password => &self.password,
            Field::AesKey => &self.aes_key,
        }
    }

    pub fn value_mut(&mut self, f: Field) -> &mut String {
        match f {
            Field::Ip => &mut self.ip,
            Field::Port => &mut self.port,
            Field::Account => &mut self.account,
            Field::Password => &mut self.password,
            Field::AesKey => &mut self.aes_key,
        }
    }

    /// 处理按键; 返回 true 表示已提交。
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        self.error = None;
        match key.code {
            KeyCode::Esc => {
                // 留空: 由上层决定退出 (Esc 提交不了即退出应用)
                return false;
            }
            KeyCode::Tab => {
                self.focus = self.focus.next();
                return false;
            }
            KeyCode::BackTab => {
                self.focus = self.focus.prev();
                return false;
            }
            KeyCode::Up => {
                self.focus = self.focus.prev();
                return false;
            }
            KeyCode::Down => {
                self.focus = self.focus.next();
                return false;
            }
            // Enter 提交。`Ctrl+M` 是终端里的 Enter (CR), 有些输入法/终端会把
            // 裸 Enter 吃掉 (中文输入法用它上屏候选词), 那时这是唯一的出路。
            KeyCode::Enter => {
                return self.submit();
            }
            KeyCode::Char('m') | KeyCode::Char('M')
                if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                return self.submit();
            }
            KeyCode::Char(c) => {
                let is_port = self.focus == Field::Port;
                let v = self.value_mut(self.focus);
                v.push(c);
                if is_port {
                    v.retain(|ch| ch.is_ascii_digit());
                    if v.len() > 5 {
                        v.truncate(5);
                    }
                }
                return false;
            }
            KeyCode::Backspace => {
                let v = self.value_mut(self.focus);
                v.pop();
                return false;
            }
            _ => return false,
        }
    }

    pub fn submit(&mut self) -> bool {
        let port: u16 = match self.port.trim().parse() {
            Ok(p) if p > 0 => p,
            _ => {
                self.error = Some("端口无效 (1-65535)".into());
                return false;
            }
        };
        if self.ip.trim().is_empty() {
            self.error = Some("服务器 IP 不能为空".into());
            return false;
        }
        if self.account.trim().is_empty() {
            self.error = Some("账号不能为空".into());
            return false;
        }
        if self.password.is_empty() {
            self.error = Some("密码不能为空".into());
            return false;
        }
        self.result = Some(WizardResult {
            ip: self.ip.trim().to_string(),
            port,
            account: self.account.trim().to_string(),
            password: self.password.clone(),
            aes_key: self.aes_key.trim().to_string(),
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_char(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), crossterm::event::KeyModifiers::NONE)
    }

    #[test]
    fn typing_and_submit() {
        let mut w = Wizard::new(
            "127.0.0.1".into(),
            8484,
            "acc".into(),
            "pass".into(),
            String::new(),
        );
        w.focus = Field::Account;
        w.on_key(key_char('1'));
        assert_eq!(w.account, "acc1");
        assert!(w.submit());
        let r = w.result.unwrap();
        assert_eq!(r.account, "acc1");
        assert_eq!(r.port, 8484);
        assert_eq!(r.aes_key, "");
    }

    #[test]
    fn aes_key_field_roundtrips() {
        let mut w = Wizard::new(
            "127.0.0.1".into(),
            8484,
            "acc".into(),
            "pass".into(),
            String::new(),
        );
        w.focus = Field::AesKey;
        for ch in "130A06B41B0F3352".chars() {
            w.on_key(key_char(ch));
        }
        assert_eq!(w.aes_key, "130A06B41B0F3352");
        assert!(w.submit());
        assert_eq!(w.result.unwrap().aes_key, "130A06B41B0F3352");
    }

    #[test]
    fn tab_wraps_through_aes_key() {
        let mut w = Wizard::new(
            "127.0.0.1".into(),
            8484,
            "acc".into(),
            "pass".into(),
            String::new(),
        );
        w.focus = Field::Password;
        w.on_key(KeyEvent::new(
            KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(w.focus, Field::AesKey);
        w.on_key(KeyEvent::new(
            KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(w.focus, Field::Ip);
    }

    #[test]
    fn bad_port_rejected() {
        let mut w = Wizard::new(
            "127.0.0.1".into(),
            0,
            "acc".into(),
            "pass".into(),
            String::new(),
        );
        assert!(!w.submit());
        assert!(w.error.is_some());
    }

    #[test]
    fn missing_account_rejected() {
        let mut w = Wizard::new(
            "127.0.0.1".into(),
            8484,
            "".into(),
            "pass".into(),
            String::new(),
        );
        assert!(!w.submit());
        assert!(w.error.is_some());
    }

    #[test]
    fn port_only_digits() {
        let mut w = Wizard::new(
            "127.0.0.1".into(),
            8484,
            "a".into(),
            "b".into(),
            String::new(),
        );
        w.focus = Field::Port;
        w.on_key(key_char('x'));
        w.on_key(key_char('9'));
        assert_eq!(w.port, "84849");
    }
}
