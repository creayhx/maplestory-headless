//! Chat packets for CongMS 079.
//!
//! `ChatHandler.GeneralChat(String text, byte unk, ...)` -> `string + byte`.

use crate::opcodes::send;
use crate::packet::Packet;

/// GENERAL_CHAT (0x2D) — public chat. The message is GBK-encoded because the
/// server reads map strings with `MapleType.China`'s ANSI charset.
pub fn general_chat(message: &str, show: bool) -> Packet {
    let mut p = Packet::new(send::GENERAL_CHAT);
    p.write_string_gb(message);
    p.write_u8(show as u8);
    p
}

/// WHISPER (0x75) — send a whisper. Layout per `ChatHandler.WhisperFind`
/// mode 6: `byte 6 + string recipient + string text`.
pub fn whisper(recipient: &str, message: &str) -> Packet {
    let mut p = Packet::new(send::WHISPER);
    p.write_u8(6);
    p.write_string_gb(recipient);
    p.write_string_gb(message);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_packet_layout() {
        let p = general_chat("hi", false);
        let bytes = p.into_bytes();
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 0x2D);
        assert_eq!(u16::from_le_bytes([bytes[2], bytes[3]]), 2);
        assert_eq!(&bytes[4..6], b"hi");
        assert_eq!(bytes[6], 0);
    }

    #[test]
    fn chat_gbk_encoding() {
        let p = general_chat("你好", false);
        let bytes = p.into_bytes();
        assert_eq!(u16::from_le_bytes([bytes[2], bytes[3]]), 4);
        assert_eq!(&bytes[4..8], &[0xC4, 0xE3, 0xBA, 0xC3]);
    }

    #[test]
    fn whisper_layout_matches_whisper_find_mode_6() {
        let p = whisper("CCCCCW", "hi");
        let bytes = p.into_bytes();
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 0x75);
        assert_eq!(bytes[2], 6); // mode
        assert_eq!(u16::from_le_bytes([bytes[3], bytes[4]]), 6); // recipient len
        assert_eq!(&bytes[5..11], b"CCCCCW");
        assert_eq!(u16::from_le_bytes([bytes[11], bytes[12]]), 2); // text len
        assert_eq!(&bytes[13..15], b"hi");
    }
}

