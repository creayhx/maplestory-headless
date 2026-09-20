//! Item packets for CongMS 079.
//!
//! `ITEM_PICKUP` (0xC6) format verified from `InventoryHandler.PlayerPickup`:
//! `int tick; byte 0; short x; short y; int oid`. The server sanity-checks the
//! reported position against the drop's real position, so report the drop's
//! coordinates.

use crate::opcodes::send;
use crate::packet::Packet;

/// ITEM_PICKUP (0xC6) — pick up a map drop by oid at (x, y).
pub fn pickup(oid: i32, x: i16, y: i16) -> Packet {
    let mut p = Packet::new(send::ITEM_PICKUP);
    p.write_time(); // tick
    p.write_u8(0);
    p.write_point(x, y);
    p.write_i32(oid);
    p
}

/// USE_ITEM (0x45) — consume a USE-tab item. `slot` is the inventory slot,
/// `itemid` its id (server validates both, `InventoryHandler.UseItem`).
pub fn use_item(slot: i16, itemid: i32) -> Packet {
    let mut p = Packet::new(send::USE_ITEM);
    p.write_time(); // tick
    p.write_i16(slot);
    p.write_i32(itemid);
    p
}

/// REWARD_ITEM (0x70) — open a reward box / special item (e.g. auction box 2022552).
/// Format: `short slot; int itemId` (from `InventoryHandler.UseRewardItem`).
pub fn use_reward_item(slot: i16, itemid: i32) -> Packet {
    let mut p = Packet::new(send::REWARD_ITEM);
    p.write_i16(slot);
    p.write_i32(itemid);
    p
}

/// USE_UPGRADE_SCROLL (0x53) — apply an upgrade scroll to an equip.
/// Layout per `MapleServerHandler`: `int tick + short slot + short dst + short ws`.
/// Layout: `tick + slot (scroll's USE slot) + dst (target equip slot,
/// negative = worn / positive = backpack) + ws`.
/// `dst` negative = worn equip slot, positive = backpack slot; `ws` bit2 = white scroll protection.
pub fn use_upgrade_scroll(slot: i16, dst: i16, ws: i16) -> Packet {
    let mut p = Packet::new(send::USE_UPGRADE_SCROLL);
    p.write_time(); // tick
    p.write_i16(slot);
    p.write_i16(dst);
    p.write_i16(ws);
    p
}

/// ITEM_MOVE (0x44) with dst = 0 — drop an item from `inv_type` at `slot`.
/// The server drops it at the player's position (`MapleInventoryManipulator.drop`).
/// `inv_type`: 2=USE 3=SETUP 4=ETC (equips can't be dropped in bulk).
pub fn drop_item(inv_type: u8, slot: i16, quantity: i16) -> Packet {
    let mut p = Packet::new(send::ITEM_MOVE);
    p.write_time(); // tick
    p.write_u8(inv_type);
    p.write_i16(slot); // src
    p.write_i16(0); // dst = 0 -> drop
    p.write_i16(quantity);
    p
}

/// ITEM_MOVE (0x44) — equip (`src` backpack slot, `dst` negative body slot)
/// or unequip (`src` negative body slot, `dst` backpack slot) an equip from
/// the EQUIP tab (`InventoryHandler.ItemMove`). The server decides equip vs
/// unequip purely from the sign of src/dst.
/// qty is written as -1 (0xFFFF): the fixed quantity for equip moves
/// (third-party servers validate/branch on -1; writing 0 is ignored).
pub fn move_item(src: i16, dst: i16) -> Packet {
    let mut p = Packet::new(send::ITEM_MOVE);
    p.write_time(); // tick
    p.write_u8(1); // invType = EQUIP
    p.write_i16(src);
    p.write_i16(dst);
    p.write_i16(-1); // qty = -1 (fixed value for equip moves)
    p
}

/// MESO_DROP (0x5B) — drop `amount` meso (server limits 10..=50000).
pub fn drop_meso(amount: i32) -> Packet {
    let mut p = Packet::new(send::MESO_DROP);
    p.write_time(); // tick
    p.write_i32(amount);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_item_layout() {
        // int tick + short slot + int itemId
        let bytes = use_item(9, 2000003).into_bytes();
        assert_eq!(&bytes[0..2], &[0x45, 0x00]);
        assert_eq!(&bytes[6..8], &[9, 0]); // slot
        assert_eq!(&bytes[8..12], &[0x83, 0x84, 0x1E, 0x00]); // 2000003
        assert_eq!(bytes.len(), 2 + 4 + 2 + 4);
    }

    #[test]
    fn drop_item_layout() {
        // int tick + byte invType + short src + short 0 + short qty
        let bytes = drop_item(2, 9, 5).into_bytes();
        assert_eq!(&bytes[0..2], &[0x44, 0x00]);
        assert_eq!(bytes[6], 2); // invType
        assert_eq!(&bytes[7..9], &[9, 0]); // src
        assert_eq!(&bytes[9..11], &[0, 0]); // dst = 0 (drop)
        assert_eq!(&bytes[11..13], &[5, 0]); // qty
    }

    #[test]
    fn drop_meso_layout() {
        let bytes = drop_meso(1000).into_bytes();
        assert_eq!(&bytes[0..2], &[0x5B, 0x00]);
        assert_eq!(&bytes[6..10], &[0xE8, 0x03, 0x00, 0x00]); // 1000
    }

    #[test]
    fn use_upgrade_scroll_layout() {
        // scroll in USE slot 7 onto worn weapon slot -11, ws=1
        let bytes = use_upgrade_scroll(7, -11, 1).into_bytes();
        assert_eq!(&bytes[0..2], &[0x53, 0x00]);
        assert_eq!(&bytes[6..8], &[7, 0]); // slot
        assert_eq!(&bytes[8..10], &[0xF5, 0xFF]); // dst -11
        assert_eq!(&bytes[10..12], &[1, 0]); // ws
        assert_eq!(bytes.len(), 2 + 4 + 2 + 2 + 2);
    }

    #[test]
    fn move_item_equip_layout() {
        // equip sword from backpack slot 3 to body slot -11
        let bytes = move_item(3, -11).into_bytes();
        assert_eq!(&bytes[0..2], &[0x44, 0x00]);
        assert_eq!(bytes[6], 1); // invType EQUIP
        assert_eq!(&bytes[7..9], &[3, 0]); // src
        assert_eq!(&bytes[9..11], &[0xF5, 0xFF]); // dst -11
        assert_eq!(&bytes[11..13], &[0xFF, 0xFF]); // qty -1
    }

    #[test]
    fn move_item_unequip_layout() {
        // unequip cap from body slot -1 to backpack slot 9
        let bytes = move_item(-1, 9).into_bytes();
        assert_eq!(&bytes[0..2], &[0x44, 0x00]);
        assert_eq!(&bytes[7..9], &[0xFF, 0xFF]); // src -1
        assert_eq!(&bytes[9..11], &[9, 0]); // dst
    }
}
