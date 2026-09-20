pub mod chat_report;
pub mod command;
pub mod config;
pub mod crypto;
pub mod emit;
pub mod exptable;
pub mod handlers;
pub mod login;
pub mod names;
pub mod opcodes;
pub mod packet;
pub mod packets;
pub mod parsers;
pub mod runtime;
pub mod runtime_config;
pub mod session;
pub mod state;

/// Make the console decode program output as UTF-8 (fixes garbled Chinese).
///
/// The program (CLI/TUI) always emits UTF-8 bytes; on a Windows console in the
/// GBK code page (936) Chinese displays as garbage. This sets the output code
/// page to 65001 (UTF-8) at startup, leaving it alone if already UTF-8
/// (adaptive); it only changes output, not input (stdin stays on the original
/// code page to avoid new garbage). When redirected to a file the bytes are
/// written verbatim, unaffected (still UTF-8).
pub fn setup_console_utf8() {
    #[cfg(windows)]
    {
        extern "system" {
            fn GetConsoleOutputCP() -> u32;
            fn SetConsoleOutputCP(cp: u32) -> i32;
        }
        unsafe {
            if GetConsoleOutputCP() != 65001 {
                SetConsoleOutputCP(65001);
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = ();
    }
}
