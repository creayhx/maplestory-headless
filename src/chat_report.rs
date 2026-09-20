//! Outbound report channel: push status/result/error lines to the whitelisted
//! chat admins as whispers (`[INFO]/[OK]/[WARN]/[ERR]` prefixed, one line each).
//!
//! The report *content* is decided per feature when it is implemented (buy
//! results, trade results, hunt statistics...); this module only provides the
//! channel and the severity levels.

use crate::packets::chat as pchat;
use crate::session::Session;
use crate::state::BotState;

/// Severity of a report line; rendered as a `[...]` prefix in the whisper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportLevel {
    /// general information (mode changes, map moves)
    Info,
    /// a successful operation result (bought X, traded, sold...)
    Ok,
    /// a recoverable warning (low HP, hunt stuck, inventory near full)
    Warn,
    /// an error / failure (send failed, disconnected)
    Err,
}

impl ReportLevel {
    fn tag(self) -> &'static str {
        match self {
            ReportLevel::Info => "INFO",
            ReportLevel::Ok => "OK",
            ReportLevel::Warn => "WARN",
            ReportLevel::Err => "ERR",
        }
    }
}

/// Render a report line with its severity tag.
pub fn format_line(level: ReportLevel, text: &str) -> String {
    format!("[{}] {}", level.tag(), text)
}

/// Push one report line to every whitelisted admin as a whisper. Falls back to
/// stdout when no `--chat-admin` was configured. Never fails the caller.
pub async fn notify(
    _state: &BotState,
    session: &mut Session,
    level: ReportLevel,
    text: &str,
    admins: &[String],
) {
    let line = format_line(level, text);
    if admins.is_empty() {
        crate::emit_f!([crate::emit::Field::Text => line] => "[report] {line}");
        return;
    }
    for name in admins {
        if let Err(e) = session.send_packet(pchat::whisper(name, &line)).await {
            crate::emit_err_f!([crate::emit::Field::Name => name, crate::emit::Field::Text => e] => "[report] whisper to '{name}' failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_tags() {
        assert_eq!(ReportLevel::Info.tag(), "INFO");
        assert_eq!(ReportLevel::Ok.tag(), "OK");
        assert_eq!(ReportLevel::Warn.tag(), "WARN");
        assert_eq!(ReportLevel::Err.tag(), "ERR");
    }

    #[test]
    fn format_line_renders_tagged_prefix() {
        assert_eq!(format_line(ReportLevel::Warn, "hunt stuck"), "[WARN] hunt stuck");
        assert_eq!(format_line(ReportLevel::Info, "上线"), "[INFO] 上线");
    }
}
