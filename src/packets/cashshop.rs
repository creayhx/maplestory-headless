//! Cash shop packets for CongMS 079.
//!
//! Formats verified against `CashShopOperation` in the decompiled `079.jar`:
//! - enter:   `ENTER_CASH_SHOP (0x23)` — the server never reads the body
//! - buy:     `CASHSHOP_OPERATION (0xE9) = byte 3 + byte currency + int sn`
//!   currency: 0 = NX credit, 1 = reward points (server does `+1`)
//! - takeout: `CASHSHOP_OPERATION (0xE9) = byte 13 + long uniqueId`
//! - leave:   `CHANGE_MAP (0x21)` — on the cash shop server this is handled
//!   as `LeaveCashShop`, which also never reads the body

use crate::opcodes::send;
use crate::packet::Packet;

/// Enter the cash shop from the channel server (0x23, empty body).
pub fn enter() -> Packet {
    Packet::new(send::ENTER_CASH_SHOP)
}

/// Open the auction (0x8D, empty body) — the client's "auction" button.
/// Used when REWARD_ITEM (0x70) is blocked: `auction` → dialog → `npc reply` flow.
pub fn enter_mts() -> Packet {
    Packet::new(send::ENTER_MTS)
}

/// Leave the cash shop back to the channel server (0x21, empty body).
pub fn leave() -> Packet {
    Packet::new(send::CHANGE_MAP)
}

/// Buy a cash item by its SN (0xE9 action 3).
/// `points = true` pays with reward points instead of NX credit.
pub fn buy(sn: i32, points: bool) -> Packet {
    let mut p = Packet::new(send::CASHSHOP_OPERATION);
    p.write_u8(3);
    p.write_u8(points as u8);
    p.write_i32(sn);
    p
}

/// Move a bought item from the cash inventory into the backpack (0xE9 action 13).
pub fn take_out(unique_id: i64) -> Packet {
    let mut p = Packet::new(send::CASHSHOP_OPERATION);
    p.write_u8(13);
    p.write_i64(unique_id);
    p
}

/// Store a cash item from the backpack back into the cash shop inventory
/// (0xE9 action 14): `byte 14 + long uniqueId + byte invType` where invType
/// is the item's inventory tab (1=equip, 2=use, 3=setup, 4=etc). The server
/// only stores items that carry a unique id.
pub fn store(unique_id: i64, inv_type: u8) -> Packet {
    let mut p = Packet::new(send::CASHSHOP_OPERATION);
    p.write_u8(14);
    p.write_i64(unique_id);
    p.write_u8(inv_type);
    p
}