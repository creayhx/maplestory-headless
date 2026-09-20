//! Parse PLAYER_INTERACTION (0x14F) trade sub-packets.
//!
//! Formats from `PlayerInteractionHandler` / `MaplePacketCreator`:
//! - invite(2):     `byte 2 + byte 3 + string name + int 0`
//! - partner add(4):`byte 4 + byte 1 + addCharLook + string name`
//! - start(5):      `byte 5 + byte 3 + byte 2 + byte number + [number==1: byte 0 + addCharLook + name] + byte number + addCharLook + name + byte 255`
//! - message(10):   `byte 10 + byte slot + byte message`
//! - item add(14):  `byte 14 + byte number + addItemInfo(zeroPosition=false)`
//! - meso set(15):  `byte 15 + byte number + int meso`
//! - confirm(16):   `byte 16`

use crate::packet::Cursor;
use crate::parsers::inventory::parse_item_info_body;
use crate::state::Item;

/// Trade action sub-opcodes received from the server.
pub mod action {
    pub const INVITE: u8 = 2;
    pub const PARTNER_ADD: u8 = 4;
    pub const START: u8 = 5;
    pub const MESSAGE: u8 = 10;
    pub const ITEM_ADD: u8 = 14;
    pub const MESO_SET: u8 = 15;
    pub const CONFIRM: u8 = 16;
}

/// Trade result messages (PLAYER_INTERACTION action 10).
pub mod message {
    pub const CANCEL: u8 = 0;
    pub const SUCCESS: u8 = 1;
    pub const COMPLETE: u8 = 2;
    pub const FAILED_FULL: u8 = 9;
    pub const FAILED_PICKUP_RESTRICTED: u8 = 10;
    pub const DONE: u8 = 8;
}

/// Parse a trade invite: `byte 3 + string name + int 0`.
pub fn parse_trade_invite(recv: &mut Cursor<'_>) -> Option<String> {
    let _marker = recv.read_u8();
    let name = recv.read_string_gb();
    recv.skip(4);
    if recv.failed() {
        return None;
    }
    Some(name)
}

/// Parse the start of a trade window: `byte 3 + byte 2 + byte number`. Returns
/// our slot (0 = inviter / 1 = invitee). The charlook blocks are skipped.
pub fn parse_trade_start(recv: &mut Cursor<'_>) -> Option<u8> {
    let _type = recv.read_u8();
    let _mini = recv.read_u8();
    let number = recv.read_u8();
    if recv.failed() {
        return None;
    }
    Some(number)
}

/// Parse a trade item add: `byte number + addItemInfo(zeroPosition=false)`.
/// Returns (number, item). The leading position byte is consumed by the body
/// parser (mirrors the inventory block layout).
pub fn parse_trade_item_add(recv: &mut Cursor<'_>) -> Option<(u8, Item)> {
    let number = recv.read_u8();
    let _pos = recv.read_u8();
    if recv.failed() {
        return None;
    }
    let info = parse_item_info_body(recv)?;
    Some((
        number,
        Item {
            itemid: info.itemid,
            qty: info.qty,
            item_type: info.item_type,
            stats: info.stats,
            unique_id: info.unique_id,
        },
    ))
}

/// Parse a trade meso update: `byte number + int meso`.
pub fn parse_trade_meso(recv: &mut Cursor<'_>) -> Option<(u8, i32)> {
    let number = recv.read_u8();
    let meso = recv.read_i32();
    if recv.failed() {
        return None;
    }
    Some((number, meso))
}

/// Parse the trade result: `byte slot + byte message`.
pub fn parse_trade_message(recv: &mut Cursor<'_>) -> Option<(u8, u8)> {
    let slot = recv.read_u8();
    let message = recv.read_u8();
    if recv.failed() {
        return None;
    }
    Some((slot, message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::Packet;

    #[test]
    fn invite_parses() {
        let mut p = Packet::new(0);
        p.write_u8(3);
        p.write_string("playerA");
        p.write_i32(0);
        let mut c = Cursor::new(&p.as_bytes()[2..]);
        assert_eq!(parse_trade_invite(&mut c).as_deref(), Some("playerA"));
        assert!(!c.failed());
    }

    #[test]
    fn start_parses_slot() {
        let mut p = Packet::new(0);
        p.write_u8(3);
        p.write_u8(2);
        p.write_u8(1);
        let mut c = Cursor::new(&p.as_bytes()[2..]);
        assert_eq!(parse_trade_start(&mut c), Some(1));
        assert!(!c.failed());
    }

    #[test]
    fn meso_parses() {
        let mut p = Packet::new(0);
        p.write_u8(1);
        p.write_i32(1234567);
        let mut c = Cursor::new(&p.as_bytes()[2..]);
        assert_eq!(parse_trade_meso(&mut c), Some((1, 1234567)));
        assert!(!c.failed());
    }

    #[test]
    fn item_add_parses_use_item() {
        // number 1 + pos 9 + use item body (2000003, qty 5).
        let mut p = Packet::new(0);
        p.write_u8(1);
        p.write_u8(9);
        p.write_u8(2);
        p.write_i32(2000003);
        p.write_u8(0); // unique flag
        p.write_i64(0); // expiration
        p.write_i16(5); // qty
        p.write_string(""); // owner
        p.write_i16(0); // flag
        let mut c = Cursor::new(&p.as_bytes()[2..]);
        let (number, item) = parse_trade_item_add(&mut c).unwrap();
        assert_eq!(number, 1);
        assert_eq!(item.itemid, 2000003);
        assert_eq!(item.qty, 5);
        assert!(!c.failed());
    }

    #[test]
    fn message_parses() {
        let mut p = Packet::new(0);
        p.write_u8(0);
        p.write_u8(8);
        let mut c = Cursor::new(&p.as_bytes()[2..]);
        assert_eq!(parse_trade_message(&mut c), Some((0, 8)));
        assert!(!c.failed());
    }
}
