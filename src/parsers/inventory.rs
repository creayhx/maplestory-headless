use crate::packet::Cursor;
use crate::state::{EquipStats, Item};

use std::collections::BTreeMap;

/// A parsed item header (itemId / quantity / type / unique id), enough to
/// track inventory.
#[derive(Debug, Clone, Copy)]
pub struct ItemInfo {
    pub itemid: i32,
    pub qty: i16,
    pub item_type: u8,
    pub stats: Option<EquipStats>,
    /// 8-byte unique id (cash/point items, pets); 0 when absent
    pub unique_id: i64,
}

/// Parse the item body of `PacketHelper.addItemInfo` *after* the leading
/// position byte has been consumed (charinfo blocks) or the mod header
/// (0x20 ADD). Layout verified against `PacketHelper.addItemInfo`:
/// `byte type; int itemId; byte hasUnique[+8]; long expiration;` then for
/// equips `addEquipStats` (upgrade, level, 12 shorts, hands, speed, jump,
/// owner, flag, incSkill, baseLevel, exp%, vicious) plus unique/tail; for
/// non-equips `short qty + string owner + short flag` (+ 8 bytes for
/// rechargable arrows/stars). Returns None on a short/failed read so callers
/// can bail out of a block.
pub(crate) fn parse_item_info_body(recv: &mut Cursor<'_>) -> Option<ItemInfo> {
    let item_type = recv.read_u8();
    let itemid = recv.read_i32();
    let has_unique = recv.read_u8() != 0;
    let unique_id = if has_unique { recv.read_i64() } else { 0 };
    recv.skip(8); // expiration
    if recv.failed() {
        return None;
    }
    if item_type == 1 {
        // addEquipStats block
        let upgrade_slots = recv.read_u8();
        let level = recv.read_u8();
        let str = recv.read_i16();
        let dex = recv.read_i16();
        let int = recv.read_i16();
        let luk = recv.read_i16();
        let hp = recv.read_i16();
        let mp = recv.read_i16();
        let watk = recv.read_i16();
        let matk = recv.read_i16();
        let wdef = recv.read_i16();
        let mdef = recv.read_i16();
        let acc = recv.read_i16();
        let avoid = recv.read_i16();
        let hands = recv.read_i16();
        let speed = recv.read_i16();
        let jump = recv.read_i16();
        recv.skip_string(); // owner
        let flag = recv.read_u16();
        let inc_skill = recv.read_u8();
        let base_level = recv.read_u8();
        let exp_percent = recv.read_i32();
        let vicious = recv.read_i32();
        if !has_unique {
            recv.skip(8); // unique id
        }
        recv.skip(8 + 4); // getTime(-2), int -1
        if recv.failed() {
            return None;
        }
        return Some(ItemInfo {
            itemid,
            qty: 1,
            item_type,
            stats: Some(EquipStats {
                upgrade_slots,
                level,
                str,
                dex,
                int,
                luk,
                hp,
                mp,
                watk,
                matk,
                wdef,
                mdef,
                acc,
                avoid,
                hands,
                speed,
                jump,
                flag,
                inc_skill,
                base_level,
                exp_percent,
                vicious,
            }),
            unique_id,
        });
    }
    // Pet items: the type byte is written as 3 and the body is
    // `addPetItemInfo` (no qty/owner/flag). Item ids are 500xxxx.
    if itemid / 10000 == 500 {
        // 43-byte pet body minus the expiration long already read above.
        recv.skip(35);
        if recv.failed() {
            return None;
        }
        return Some(ItemInfo {
            itemid,
            qty: 1,
            item_type: 5,
            stats: None,
            unique_id,
        });
    }
    let qty = recv.read_i16();
    recv.skip_string(); // owner
    recv.skip(2); // flag
    // Rechargable arrows/stars append `int 2; short 84; byte 0; byte 52`.
    if itemid / 10000 == 207 || itemid / 10000 == 233 {
        recv.skip(8);
    }
    if recv.failed() {
        return None;
    }
    Some(ItemInfo {
        itemid,
        qty,
        item_type,
        stats: None,
        unique_id,
    })
}

/// Parse a MODIFY_INVENTORY_ITEM ADD-mode item (`addItemInfo` with
/// zeroPosition=true — no leading position byte; the slot came from the mod
/// header).
pub fn parse_item_info_add(recv: &mut Cursor<'_>) -> Option<ItemInfo> {
    parse_item_info_body(recv)
}

/// Walk one inventory block: items each prefixed by a position byte, terminated
/// by byte 0 (mirrors the per-tab loop in `PacketHelper.addInventoryInfo`). If
/// `out` is Some the items are inserted into the map keyed by `(tab, slot)`.
pub fn parse_inventory_block(
    recv: &mut Cursor<'_>,
    tab: u8,
    mut out: Option<&mut BTreeMap<(u8, i16), Item>>,
) {
    loop {
        let pos = recv.read_u8();
        if pos == 0 || recv.failed() {
            return;
        }
        if let Some(info) = parse_item_info_body(recv) {
            if let Some(map) = out.as_deref_mut() {
                map.insert(
                    (tab, pos as i16),
                    Item {
                        itemid: info.itemid,
                        qty: info.qty,
                        item_type: info.item_type,
                        stats: info.stats,
                        unique_id: info.unique_id,
                    },
                );
            }
        } else {
            return;
        }
    }
}

/// Walk one *worn equipment* block from `addInventoryInfo`. Each item is
/// prefixed by a position byte that encodes the absolute body slot:
/// `pos < 0 → -pos; write(pos > 100 ? pos - 100 : pos)`. `base` is 0 for the
/// first block (-1..-99) and 100 for the cash-equip block (-100..-999), which
/// lets us recover the real negative slot. Items go into `out` keyed by that
/// negative slot. Terminated by byte 0.
pub fn parse_equipped_block(recv: &mut Cursor<'_>, base: i16, out: &mut BTreeMap<i16, Item>) {
    loop {
        let p = recv.read_u8();
        if p == 0 || recv.failed() {
            return;
        }
        if let Some(info) = parse_item_info_body(recv) {
            let slot = if base == 0 {
                -(p as i16)
            } else if p < 100 {
                -(100 + p as i16)
            } else if p == 100 {
                -100
            } else {
                -(p as i16)
            };
            out.insert(
                slot,
                Item {
                    itemid: info.itemid,
                    qty: info.qty,
                    item_type: info.item_type,
                    stats: info.stats,
                    unique_id: info.unique_id,
                },
            );
        } else {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::Packet;

    #[test]
    fn inventory_block_parses_use_items() {
        // addInventoryInfo USE block: two potions then terminator 0.
        let mut p = Packet::new(0);
        // item 1: pos 9, type 2, itemid 2000003, no unique, expiration long,
        // qty 5, owner "", flag 0.
        p.write_u8(9);
        p.write_u8(2);
        p.write_i32(2000003);
        p.write_u8(0); // unique flag
        p.write_i64(0); // expiration
        p.write_i16(5); // qty
        p.write_string(""); // owner
        p.write_i16(0); // flag
        // item 2: pos 8, type 2, itemid 2000001, qty 3.
        p.write_u8(8);
        p.write_u8(2);
        p.write_i32(2000001);
        p.write_u8(0);
        p.write_i64(0);
        p.write_i16(3);
        p.write_string("");
        p.write_i16(0);
        p.write_u8(0); // block terminator

        let mut c = Cursor::new(&p.as_bytes()[2..]);
        let mut map = BTreeMap::new();
        parse_inventory_block(&mut c, 2, Some(&mut map));
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&(2, 9)).map(|i| (i.itemid, i.qty)), Some((2000003, 5)));
        assert_eq!(map.get(&(2, 8)).map(|i| (i.itemid, i.qty)), Some((2000001, 3)));
        assert_eq!(map.get(&(2, 9)).unwrap().stats, None);
        assert!(!c.failed());
    }

    fn equip_body(p: &mut Packet, itemid: i32, str: i16, watk: i16, level: u8) {
        // addEquipStats body with a distinctive STR/WATK and 0 elsewhere.
        p.write_u8(1); // type
        p.write_i32(itemid);
        p.write_u8(0); // unique flag
        p.write_i64(0); // expiration
        p.write_u8(7); // upgrade slots
        p.write_u8(level); // level
        p.write_i16(str); // str
        p.write_i16(0); // dex
        p.write_i16(0); // int
        p.write_i16(0); // luk
        p.write_i16(0); // hp
        p.write_i16(0); // mp
        p.write_i16(watk); // watk
        p.write_i16(0); // matk
        p.write_i16(0); // wdef
        p.write_i16(0); // mdef
        p.write_i16(0); // acc
        p.write_i16(0); // avoid
        p.write_i16(0); // hands
        p.write_i16(0); // speed
        p.write_i16(0); // jump
        p.write_string(""); // owner
        p.write_i16(0); // flag
        p.write_u8(0); // inc skill
        p.write_u8(30); // base level
        p.write_i32(0); // exp%
        p.write_i32(0); // vicious
        p.write_i64(0); // unique (no unique flag)
        p.write_i64(-2); // getTime(-2)
        p.write_i32(-1);
    }

    #[test]
    fn item_info_body_parses_pet() {
        // CASH block pet: pos 1, type 3, itemid 5000000, unique flag + unique,
        // then addPetItemInfo body (43 bytes: expiration long + 13 name +
        // level + closeness + fullness + expiration long + 3 shorts + 4 bytes).
        let mut p = Packet::new(0);
        p.write_u8(1); // pos
        p.write_u8(3); // pet type byte
        p.write_i32(5000000); // pet itemid
        p.write_u8(1); // has unique
        p.write_i64(0x1122_3344_5566_7788); // unique id
        p.write_i64(0); // expiration
        p.write_bytes(b"fluffy"); // pet name (padded to 13)
        p.write_bytes(&[0u8; 7]);
        p.write_u8(3); // level
        p.write_i16(50); // closeness
        p.write_u8(100); // fullness
        p.write_i64(0); // expiration
        p.write_i16(0); // short 0
        p.write_i16(0); // flags
        p.write_i16(0); // short 0
        p.write_i32(0); // 4 bytes
        p.write_u8(2); // next item pos
        p.write_u8(5); // type cash
        p.write_i32(5130000); // some cash item
        p.write_u8(0);
        p.write_i64(0); // expiration
        p.write_i16(1); // qty
        p.write_string(""); // owner
        p.write_i16(0); // flag
        p.write_u8(0); // block terminator

        let mut c = Cursor::new(&p.as_bytes()[2..]);
        let mut map = BTreeMap::new();
        parse_inventory_block(&mut c, 5, Some(&mut map));
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&(5, 1)).map(|i| (i.itemid, i.item_type, i.qty)), Some((5000000, 5, 1)));
        assert_eq!(map.get(&(5, 2)).map(|i| (i.itemid, i.qty)), Some((5130000, 1)));
        assert!(!c.failed());
    }

    #[test]
    fn item_info_body_parses_equip_stats() {        let mut p = Packet::new(0);
        equip_body(&mut p, 1302000, 5, 8, 2);
        let mut c = Cursor::new(&p.as_bytes()[2..]);
        let info = parse_item_info_body(&mut c).expect("parse equip");
        assert_eq!(info.itemid, 1302000);
        assert_eq!(info.item_type, 1);
        assert_eq!(info.qty, 1);
        let st = info.stats.expect("equip stats present");
        assert_eq!(st.str, 5);
        assert_eq!(st.watk, 8);
        assert_eq!(st.upgrade_slots, 7);
        assert_eq!(st.level, 2);
        assert_eq!(st.base_level, 30);
        assert!(!c.failed());
    }

    #[test]
    fn equipped_block_recovers_body_slots() {
        // Block 1: worn sword at -11, cap at -1; block 2: cash hat at -103.
        let mut p = Packet::new(0);
        p.write_u8(11); // -11 weapon
        equip_body(&mut p, 1302000, 3, 15, 1);
        p.write_u8(1); // -1 cap
        equip_body(&mut p, 1002140, 2, 0, 1);
        p.write_u8(0); // terminator
        p.write_u8(3); // -103 cash equip (103 > 100 -> p-100 = 3)
        equip_body(&mut p, 1002929, 0, 0, 1);
        p.write_u8(0); // terminator

        let mut c = Cursor::new(&p.as_bytes()[2..]);
        let mut eq = BTreeMap::new();
        parse_equipped_block(&mut c, 0, &mut eq);
        assert_eq!(eq.get(&-11).map(|i| i.itemid), Some(1302000));
        assert_eq!(eq.get(&-11).and_then(|i| i.stats).map(|s| s.watk), Some(15));
        assert_eq!(eq.get(&-1).map(|i| i.itemid), Some(1002140));
        parse_equipped_block(&mut c, 100, &mut eq);
        assert_eq!(eq.get(&-103).map(|i| i.itemid), Some(1002929));
        assert!(!c.failed());
    }
}
