//! NPC interaction packets for CongMS 079.
//!
//! NPC_TALK (0x36) format from `NPCHandler.handleNPCTalk`:
//! ```text
//! int   oid    NPC object ID
//! ```
//!
//! NPC_TALK_MORE (0x38) from `NPCHandler.NPCMoreTalk`:
//! ```text
//! byte  lastMsg    last message type
//! byte  action     1 = continue
//! [int] selection  only for msgType 4 (sendSimple)
//! ```
//!
//! NPC_SHOP (0x3A) from `NPCHandler.handleNPCShop`:
//! ```text
//! byte  mode     0=buy, 1=sell, 2=recharge, 3=leave
//! // buy/sell: short 0/slot + int itemId + short qty
//! ```

use crate::opcodes::send;
use crate::packet::Packet;

/// NPC_TALK (0x36) — initiate conversation with an NPC.
pub fn npc_talk(oid: i32) -> Packet {
    let mut p = Packet::new(send::NPC_TALK);
    p.write_i32(oid);
    p
}

/// NPC_TALK_MORE (0x38) — continue NPC dialog with a selection.
pub fn npc_talk_more(last_msg: u8, selection: i32) -> Packet {
    let mut p = Packet::new(send::NPC_TALK_MORE);
    p.write_u8(last_msg);
    p.write_u8(1); // action = continue
    p.write_i32(selection);
    p
}

/// NPC_TALK_MORE (0x38) — acknowledge a simple dialog (no selection).
pub fn npc_talk_ack(last_msg: u8) -> Packet {
    let mut p = Packet::new(send::NPC_TALK_MORE);
    p.write_u8(last_msg);
    p.write_u8(1); // action = continue
    p
}

/// NPC_TALK_MORE (0x38) — send "Prev" action for type=1 dialogs (go back).
pub fn npc_talk_prev(last_msg: u8) -> Packet {
    let mut p = Packet::new(send::NPC_TALK_MORE);
    p.write_u8(last_msg);
    p.write_u8(0); // action = 0 (prev/cancel)
    p
}

/// NPC_TALK_MORE (0x38) — send text input for type=2 dialogs.
pub fn npc_talk_text(text: &str) -> Packet {
    let mut p = Packet::new(send::NPC_TALK_MORE);
    p.write_u8(2);
    p.write_u8(1); // action = continue
    p.write_string_gb(text);
    p
}
pub fn npc_talk_cancel() -> Packet {
    let mut p = Packet::new(send::NPC_TALK_MORE);
    p.write_u8(0);
    p.write_u8(0); // action = 0 (cancel)
    p
}

/// NPC_SHOP (0x3A) — buy from or sell to an NPC shop.
pub fn npc_shop_buy(item_id: i32, quantity: i16) -> Packet {
    let mut p = Packet::new(send::NPC_SHOP);
    p.write_u8(0); // mode = buy
    p.write_i16(0); // slot (unused for buy)
    p.write_i32(item_id);
    p.write_i16(quantity);
    p
}

/// NPC_SHOP (0x3A) — sell an item to an NPC shop.
pub fn npc_shop_sell(slot: i16, item_id: i32, quantity: i16) -> Packet {
    let mut p = Packet::new(send::NPC_SHOP);
    p.write_u8(1); // mode = sell
    p.write_i16(slot);
    p.write_i32(item_id);
    p.write_i16(quantity);
    p
}

/// NPC_SHOP (0x3A) — leave the NPC shop.
pub fn npc_shop_leave() -> Packet {
    let mut p = Packet::new(send::NPC_SHOP);
    p.write_u8(3); // mode = leave
    p
}