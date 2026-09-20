//! Trade packets for CongMS 079 — PLAYER_INTERACTION (0x014F).
//!
//! Formats verified against `PlayerInteractionHandler` + live captures (packet/trade.txt):
//! - start:    `byte 0 + byte 3`                     (capture: 00 03)
//! - invite:   `byte 2 + byte 3 + string name + int 0` (capture: 02 03 06 00 CCCCCW 00 00 00 00)
//! - decline:  `byte 3`
//! - accept:   `byte 4` (the client also sends 02 03 name 0 + 05 sync + extended
//!   look fields, but the server's case 4 trade path doesn't read the body, so
//!   an empty packet works)
//! - put item: `byte 14 + byte invType + short slot + short qty + byte tradeSlot`
//!   (the client also carries 17B of item info; the server only reads the first 6B)
//! - put meso: `byte 15 + int meso` (the client also carries 1B of side fields;
//!   the server only reads the int)
//! - confirm:  `byte 16` (capture: 4F 01 10)
//! - quit:     `byte 10` (the client sends 01 08 / 00 08; the server doesn't read the body)

use crate::opcodes::send;
use crate::packet::Packet;

fn interaction(action: u8) -> Packet {
    let mut p = Packet::new(send::PLAYER_INTERACTION);
    p.write_u8(action);
    p
}

/// Open a trade window with ourselves (CREATE, type 3).
pub fn start() -> Packet {
    let mut p = interaction(0);
    p.write_u8(3);
    p
}

/// Invite a character by name (INVITE_TRADE).
/// Actual client format: `02 + 03 + name (ascii string) + int 0`.
pub fn invite(name: &str) -> Packet {
    let mut p = interaction(2);
    p.write_u8(3);
    p.write_string_gb(name);
    p.write_i32(0);
    p
}

/// Decline a pending invite (DENY_TRADE).
pub fn decline() -> Packet {
    interaction(3)
}

/// Accept a pending invite and enter the trade window (VISIT).
pub fn accept() -> Packet {
    interaction(4)
}

/// Put an item into the trade (SET_ITEMS).
pub fn put_item(inv_type: u8, slot: i16, quantity: i16, trade_slot: u8) -> Packet {
    let mut p = interaction(14);
    p.write_u8(inv_type);
    p.write_i16(slot);
    p.write_i16(quantity);
    p.write_u8(trade_slot);
    p
}

/// Put meso into the trade (SET_MESO).
pub fn put_meso(meso: i32) -> Packet {
    let mut p = interaction(15);
    p.write_i32(meso);
    p
}

/// Confirm / lock the trade (CONFIRM_TRADE).
pub fn confirm() -> Packet {
    interaction(16)
}

/// Exit / cancel the trade (EXIT).
pub fn quit() -> Packet {
    interaction(10)
}
