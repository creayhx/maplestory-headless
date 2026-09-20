//! Chat message parsing.
//!
//! Receive-side layouts (server -> client, from MaplePacketCreator / the
//! private server's packet maps in properties/send.ini):
//! - CHATTEXT (0xA4): `int cidfrom + byte whiteBG + maple-string text + byte show`
//!   (getChatText - map broadcast chat)
//! - MULTICHAT (0x8A): `byte mode + maple-string name + maple-string text`
//!   (multiChat - mode 0 friend / 1 party / 2 guild / 3 alliance, cross-map)
//! - WHISPER (0x8B): incoming `byte 18 + string sender + short channel-1 +
//!   string text`; reply `byte 10 + string target + byte reply` (getWhisper /
//!   getWhisperReply)
//! - SET_WEEK_EVENT_MESSAGE (0x4E): `byte -1 + string text` (yellow GM chat)
//! - SERVERMESSAGE (0x41): `byte type + [type==4: byte bool] + string text
//!   + type-specific trailer` (serverNotice/serverMessage). We only consume
//!   type/text and skip the trailer.
//!
//! All maple strings in this server are written with `MapleType.China`'s ANSI
//! charset = GBK (ServerConstants.MAPLE_TYPE), so text is decoded with
//! `read_string_gb`.

use crate::packet::Cursor;
use crate::state::Entity;

/// A parsed chat message from the map (CHATTEXT).
#[derive(Debug, Clone)]
pub struct MapChat {
    pub sender: String,
    pub text: String,
}

/// A parsed multi-channel chat message (MULTICHAT: party/guild/buddy/alliance).
#[derive(Debug, Clone)]
pub struct MultiChat {
    pub mode: u8,
    pub name: String,
    pub text: String,
}

/// A parsed whisper (WHISPER).
#[derive(Debug, Clone)]
pub struct Whisper {
    pub sender: String,
    pub channel: i16,
    pub text: String,
}

/// A parsed server message (SERVERMESSAGE).
#[derive(Debug, Clone)]
pub struct ServerMsg {
    pub kind: u8,
    pub text: String,
}

/// Parse CHATTEXT. `entities` maps character ids to player names so the
/// sender id can be resolved. Returns None on malformed data.
pub fn parse_chattext(
    recv: &mut Cursor<'_>,
    entities: &std::collections::BTreeMap<i32, Entity>,
) -> Option<MapChat> {
    let cid = recv.read_i32();
    recv.skip(1); // whiteBG
    let text = recv.read_string_gb();
    recv.skip(1); // show
    if recv.failed() {
        return None;
    }
    let sender = entities
        .get(&cid)
        .map(|e| e.charname.clone())
        .unwrap_or_else(|| format!("{}", cid));
    Some(MapChat { sender, text })
}

/// Parse MULTICHAT. Returns None on malformed data.
pub fn parse_multichat(recv: &mut Cursor<'_>) -> Option<MultiChat> {
    let mode = recv.read_u8();
    let name = recv.read_string_gb();
    let text = recv.read_string_gb();
    if recv.failed() {
        return None;
    }
    Some(MultiChat { mode, name, text })
}

/// Parse WHISPER (incoming message or reply). Returns None on malformed data
/// or for unhandled whisper sub-modes.
pub fn parse_whisper(recv: &mut Cursor<'_>) -> Option<Whisper> {
    let mode = recv.read_u8();
    match mode {
        18 => {
            let sender = recv.read_string_gb();
            let channel = recv.read_i16();
            let text = recv.read_string_gb();
            if recv.failed() {
                return None;
            }
            Some(Whisper { sender, channel, text })
        }
        _ => None,
    }
}

/// Parse SET_WEEK_EVENT_MESSAGE (yellow GM chat). Returns None on malformed
/// data or when the byte before the string is not 0xFF.
pub fn parse_yellow_chat(recv: &mut Cursor<'_>) -> Option<String> {
    let marker = recv.read_i8();
    if marker != -1 {
        return None;
    }
    let text = recv.read_string_gb();
    if recv.failed() {
        return None;
    }
    Some(text)
}

/// Parse SERVERMESSAGE, consuming only the type byte and the text; the
/// type-specific trailer (channel bytes / item info / closing int) is
/// skipped. Returns None on malformed data.
pub fn parse_server_message(recv: &mut Cursor<'_>) -> Option<ServerMsg> {
    let kind = recv.read_u8();
    if kind == 4 {
        let empty = recv.read_u8();
        if empty == 0 {
            // type 4 with bool false has no text
            return Some(ServerMsg { kind, text: String::new() });
        }
    }
    let text = recv.read_string_gb();
    if recv.failed() {
        return None;
    }
    Some(ServerMsg { kind, text })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Entity, EntityKind};

    fn packet(bytes: &[u8]) -> Vec<u8> {
        let mut p = Vec::with_capacity(bytes.len() + 2);
        p.extend_from_slice(&0x00u16.to_le_bytes());
        p.extend_from_slice(bytes);
        p
    }

    fn entity(cid: i32, name: &str) -> (i32, Entity) {
        (
            cid,
            Entity {
                oid: cid,
                kind: EntityKind::Player,
                charname: name.to_string(),
                ..Entity::default()
            },
        )
    }

    #[test]
    fn chattext_ascii() {
        let mut data = Vec::new();
        data.extend_from_slice(&1001i32.to_le_bytes());
        data.push(0);
        data.extend_from_slice(&7u16.to_le_bytes());
        data.extend_from_slice(b"hi mom!");
        data.push(1);
        let bytes = packet(&data);
        let mut c = Cursor::new(&bytes[2..]);
        let chat = parse_chattext(&mut c, &std::collections::BTreeMap::new()).unwrap();
        assert_eq!(chat.sender, "1001");
        assert_eq!(chat.text, "hi mom!");
        assert!(!c.failed());
    }

    #[test]
    fn chattext_sender_lookup() {
        let mut data = Vec::new();
        data.extend_from_slice(&7i32.to_le_bytes());
        data.push(0);
        data.extend_from_slice(&2u16.to_le_bytes());
        data.extend_from_slice(b"hi");
        data.push(0);
        let bytes = packet(&data);
        let mut c = Cursor::new(&bytes[2..]);
        let mut entities = std::collections::BTreeMap::new();
        entities.insert(7, entity(7, "Player").1);
        let chat = parse_chattext(&mut c, &entities).unwrap();
        assert_eq!(chat.sender, "Player");
    }

    #[test]
    fn chattext_gbk() {
        // "你好" in GBK = C4 E3 BA C3
        let mut data = Vec::new();
        data.extend_from_slice(&1i32.to_le_bytes());
        data.push(0);
        data.extend_from_slice(&4u16.to_le_bytes());
        data.extend_from_slice(&[0xC4, 0xE3, 0xBA, 0xC3]);
        data.push(0);
        let bytes = packet(&data);
        let mut c = Cursor::new(&bytes[2..]);
        let chat = parse_chattext(&mut c, &std::collections::BTreeMap::new()).unwrap();
        assert_eq!(chat.text, "你好");
    }

    #[test]
    fn multichat_party() {
        let mut data = Vec::new();
        data.push(1); // mode: party
        data.extend_from_slice(&3u16.to_le_bytes());
        data.extend_from_slice(b"bob");
        data.extend_from_slice(&2u16.to_le_bytes());
        data.extend_from_slice(b"go");
        let bytes = packet(&data);
        let mut c = Cursor::new(&bytes[2..]);
        let m = parse_multichat(&mut c).unwrap();
        assert_eq!((m.mode, m.name.as_str(), m.text.as_str()), (1, "bob", "go"));
    }

    #[test]
    fn whisper_incoming() {
        let mut data = Vec::new();
        data.push(18);
        data.extend_from_slice(&5u16.to_le_bytes());
        data.extend_from_slice(b"alice");
        data.extend_from_slice(&2i16.to_le_bytes());
        data.extend_from_slice(&5u16.to_le_bytes());
        data.extend_from_slice(b"hello");
        let bytes = packet(&data);
        let mut c = Cursor::new(&bytes[2..]);
        let w = parse_whisper(&mut c).unwrap();
        assert_eq!((w.sender.as_str(), w.channel, w.text.as_str()), ("alice", 2, "hello"));
    }

    #[test]
    fn whisper_other_mode_ignored() {
        let mut data = Vec::new();
        data.push(10);
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(b"x");
        let bytes = packet(&data);
        let mut c = Cursor::new(&bytes[2..]);
        assert!(parse_whisper(&mut c).is_none());
    }

    #[test]
    fn yellow_chat() {
        let mut data = Vec::new();
        data.push(0xFF);
        data.extend_from_slice(&8u16.to_le_bytes());
        data.extend_from_slice(b"GM alert");
        let bytes = packet(&data);
        let mut c = Cursor::new(&bytes[2..]);
        assert_eq!(parse_yellow_chat(&mut c).unwrap(), "GM alert");
    }

    #[test]
    fn server_message_notice() {
        let mut data = Vec::new();
        data.push(4);
        data.push(1); // bool true -> text present
        data.extend_from_slice(&3u16.to_le_bytes());
        data.extend_from_slice(b"ok!");
        let bytes = packet(&data);
        let mut c = Cursor::new(&bytes[2..]);
        let m = parse_server_message(&mut c).unwrap();
        assert_eq!((m.kind, m.text.as_str()), (4, "ok!"));
    }

    #[test]
    fn server_message_no_text() {
        let mut data = Vec::new();
        data.push(4);
        data.push(0); // bool false -> no text
        let bytes = packet(&data);
        let mut c = Cursor::new(&bytes[2..]);
        let m = parse_server_message(&mut c).unwrap();
        assert_eq!((m.kind, m.text.as_str()), (4, ""));
    }

    #[test]
    fn server_message_plain() {
        let mut data = Vec::new();
        data.push(5);
        data.extend_from_slice(&5u16.to_le_bytes());
        data.extend_from_slice(b"error");
        let bytes = packet(&data);
        let mut c = Cursor::new(&bytes[2..]);
        let m = parse_server_message(&mut c).unwrap();
        assert_eq!((m.kind, m.text.as_str()), (5, "error"));
    }
}
