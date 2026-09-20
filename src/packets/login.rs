//! Login-flow packet builders for CongMS 079.
//!
//! Formats verified against `CharLoginHandler` / `MapleServerHandler` in the
//! decompiled `079.jar`:
//! - LOGIN_PASSWORD reads `string acc; string pass; 6 bytes` (MAC).
//! - SET_GENDER reads `byte gender; string accountName`.
//! - CHARLIST_REQUEST reads `byte world; byte channel`.
//! - CHAR_SELECT reads `int charId` only (no PIC, no hardware block).

use crate::opcodes::send;
use crate::packet::Packet;

/// LOGIN_PASSWORD (0x01) — account + password + MAC(6B) + extra8B + extra6B.
/// Layout: 01 00 | str(acc) | str(pass) | MAC 6B | 8B | 6B.
pub fn login(account: &str, password: &str) -> Packet {
    let mut p = Packet::new(send::LOGIN_PASSWORD);
    p.write_string_gb(account);
    p.write_string_gb(password);
    // fixed MAC/session fields
    p.write_bytes(&[0x2E, 0x74, 0x23, 0xC3, 0x23, 0x40]); // MAC 6B
    p.write_bytes(&[0x4F, 0x29, 0x4C, 0xA7, 0x00, 0x00, 0x00, 0x00]); // 8B
    p.write_bytes(&[0xB3, 0x34, 0x00, 0x00, 0x00, 0x00]); // 6B
    p
}

/// SERVERLIST_REQUEST (0x02) — request the world/channel list.
pub fn server_request() -> Packet {
    Packet::new(send::SERVERLIST_REQUEST)
}

/// SET_GENDER (0x04) — pick a gender for an account that needs one.
pub fn set_gender(female: bool, account: &str) -> Packet {
    let mut p = Packet::new(send::SET_GENDER);
    p.write_u8(female as u8);
    p.write_string_gb(account);
    p
}

/// SERVERSTATUS_REQUEST (0x05).
pub fn server_status_request() -> Packet {
    Packet::new(send::SERVERSTATUS_REQUEST)
}

/// CHARLIST_REQUEST (0x09) — `byte world; byte channel`.
pub fn charlist_request(world: u8, channel: u8) -> Packet {
    let mut p = Packet::new(send::CHARLIST_REQUEST);
    p.write_u8(world);
    p.write_u8(channel);
    p
}

/// CHAR_SELECT (0x0A) — select a character by id.
pub fn select_char(cid: i32) -> Packet {
    let mut p = Packet::new(send::CHAR_SELECT);
    p.write_i32(cid);
    p
}

/// PLAYER_LOGGEDIN (0x0B) — enter the channel server as the given character.
pub fn player_login(cid: i32) -> Packet {
    let mut p = Packet::new(send::PLAYER_LOGGEDIN);
    p.write_i32(cid);
    p
}

/// CHANGE_CHANNEL (0x22) — switch channels. `channel` is 0-based; the server
/// adds 1 (`InterServerHandler.ChangeChannel`: `slea.readByte() + 1`).
/// Layout: `22 00 + channel(1) + tick(4)` (read by the server in this order).
pub fn change_channel(channel: u8) -> Packet {
    let mut p = Packet::new(send::CHANGE_CHANNEL);
    p.write_u8(channel);
    p.write_time(); // tick
    p
}

/// PONG (0x13) — reply to a server PING.
pub fn pong() -> Packet {
    Packet::new(send::PONG)
}
