//! SET_CASH_SHOP (0x83) parsing — the cash shop entrance packet.
//!
//! Layout (verified against `MTSCSPacket.warpCS` + `addModCashItemInfo`):
//! `addCharacterInfo(chr, isCs=true); accountName; int 70; 70×int sn;
//!  short count + count×addModCashItemInfo; <tail>` where the isCs=true
//! variant replaces skill/cooldown/quest/monster-book blocks with short 0s.
//! The tail (short 0, byte 0, 120 bytes, hot-sell ints) is not parsed.

use crate::packet::Cursor;
use crate::parsers::{parse_character_info_body, CharInfo};
use crate::state::CashShopItem;

/// One `addModCashItemInfo` entry: `int sn; int flags;` then fields driven by
/// the flag bits (1=itemid, 2=count, 4=discountPrice, 8=unk_1-1,
/// 0x10=priority, 0x20=period, 0x40=int 0, 0x80=meso, 0x100=unk_2-1,
/// 0x200=gender, 0x400=showUp, 0x800=mark, 0x1000=unk_3-1, 0x2000/0x4000/
/// 0x8000=short 0, 0x10000=package items).
fn parse_mod_item(recv: &mut Cursor<'_>) -> Option<CashShopItem> {
    let sn = recv.read_i32();
    let flags = recv.read_i32();
    if recv.failed() {
        return None;
    }
    let mut item = CashShopItem { sn, ..Default::default() };
    if flags & 1 != 0 {
        item.itemid = recv.read_i32();
    }
    if flags & 2 != 0 {
        item.count = recv.read_i16();
    }
    if flags & 4 != 0 {
        item.price = recv.read_i32();
    }
    if flags & 8 != 0 {
        recv.skip(1); // unk_1 - 1
    }
    if flags & 0x10 != 0 {
        item.priority = recv.read_u8();
    }
    if flags & 0x20 != 0 {
        item.period = recv.read_i16();
    }
    if flags & 0x40 != 0 {
        recv.skip(4); // int 0
    }
    if flags & 0x80 != 0 {
        item.meso = recv.read_i32();
    }
    if flags & 0x100 != 0 {
        recv.skip(1); // unk_2 - 1
    }
    if flags & 0x200 != 0 {
        item.gender = recv.read_u8();
    }
    if flags & 0x400 != 0 {
        item.show_up = recv.read_u8() != 0;
    }
    if flags & 0x800 != 0 {
        item.mark = recv.read_u8();
    }
    if flags & 0x1000 != 0 {
        recv.skip(1); // unk_3 - 1
    }
    if flags & 0x2000 != 0 {
        recv.skip(2);
    }
    if flags & 0x4000 != 0 {
        recv.skip(2);
    }
    if flags & 0x8000 != 0 {
        recv.skip(2);
    }
    if flags & 0x10000 != 0 {
        let n = recv.read_u8();
        recv.skip(n as usize * 4); // package member sns
    }
    if recv.failed() {
        return None;
    }
    Some(item)
}

/// Parse SET_CASH_SHOP (0x83). Returns the character info block (CS variant,
/// skills empty) and the sale list. `None` when the fixed blocks don't line up.
pub fn parse_cash_shop(recv: &mut Cursor<'_>) -> Option<(CharInfo, Vec<CashShopItem>)> {
    recv.skip(8); // long -1
    recv.skip(1); // byte 0
    let info = parse_character_info_body(recv)?;
    skip_cs_character_tail(recv)?;
    recv.skip_string(); // account name
    let n = recv.read_i32();
    recv.skip(n as usize * 4); // fixed sn list (server writes 70)
    let count = recv.read_i16();
    if recv.failed() {
        return None;
    }
    let mut items = Vec::with_capacity(count.max(0) as usize);
    for _ in 0..count {
        match parse_mod_item(recv) {
            Some(it) => items.push(it),
            None => return None,
        }
    }
    Some((info, items))
}

/// The isCs=true variant of `PacketHelper.addCharacterInfo` replaces the
/// skill/cooldown/quest/monster-book blocks with fixed short 0s; ring info is
/// still variable-length.
fn skip_cs_character_tail(recv: &mut Cursor<'_>) -> Option<()> {
    recv.skip(2); // addSkillInfo (isCs: short 0)
    recv.skip(2); // addCoolDownInfo (isCs: short 0)
    recv.skip(4); // addQuestInfo (isCs: short 0 + short 0)
    // addRingInfo: short 0; short count + count×(int + ascii13 + long + long);
    // short count + count×(int + ascii13 + long + long + int); short 0/1 [+
    // int 0 + ascii13 + int + int]
    recv.skip(2);
    let crush = recv.read_i16();
    recv.skip(crush.max(0) as usize * (4 + 13 + 8 + 8));
    let friend = recv.read_i16();
    recv.skip(friend.max(0) as usize * (4 + 13 + 8 + 8 + 4));
    let has_marriage = recv.read_i16();
    if has_marriage != 0 {
        recv.skip(4 + 13 + 4 + 4);
    }
    recv.skip(60); // addRocksInfo: 5×int + 10×int (fixed)
    recv.skip(7); // addMonsterBookInfo (isCs: int 1 + byte 0 + short 0)
    recv.skip(2); // QuestInfoPacket (isCs: short 0)
    recv.skip(6); // short 0 × 3
    if recv.failed() {
        return None;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::Packet;

    #[test]
    fn parses_mod_item_flags() {
        // sn=10000000, flags=1|2|4 (itemid+count+price)
        let mut p = Packet::new(0);
        p.write_i32(10000000);
        p.write_i32(7);
        p.write_i32(5050000);
        p.write_i16(2);
        p.write_i32(99);
        let mut cur = Cursor::new(&p.as_bytes()[2..]);
        let it = parse_mod_item(&mut cur).expect("parse");
        assert!(!cur.failed());
        assert_eq!(it.sn, 10000000);
        assert_eq!(it.itemid, 5050000);
        assert_eq!(it.count, 2);
        assert_eq!(it.price, 99);
    }

    #[test]
    fn parses_cash_shop_list() {
        // character core (long -1, byte 0, addCharStats, buddy, bless=0,
        // addInventoryInfo all empty), short 0 skills, account name,
        // 1 fixed sn, 1 mod item.
        let mut p = Packet::new(0);
        p.write_i64(-1);
        p.write_u8(0);
        // addCharStats
        p.write_i32(3953); // id
        let mut name = [0u8; 13];
        name[..6].copy_from_slice(b"Player");
        p.write_bytes(&name);
        p.write_u8(0); // gender
        p.write_u8(3); // skin
        p.write_i32(20000); // face
        p.write_i32(30000); // hair
        p.write_bytes(&[0u8; 24]); // pets
        p.write_u8(60); // level
        p.write_i16(130); // job
        p.write_i16(4); // str
        p.write_i16(5); // dex
        p.write_i16(4); // int
        p.write_i16(4); // luk
        p.write_i16(100); // hp
        p.write_i16(100); // maxhp
        p.write_i16(50); // mp
        p.write_i16(50); // maxmp
        p.write_i16(0); // ap
        p.write_i16(0); // sp
        p.write_i32(0); // exp
        p.write_i16(0); // fame
        p.write_i32(0); // int 0
        p.write_i64(0); // time
        p.write_i32(100000000); // mapid
        p.write_u8(0); // spawnpoint
        p.write_u8(4); // buddylist
        p.write_u8(0); // bless (isCs)
        // addInventoryInfo (empty)
        p.write_i32(0); // meso
        p.write_i32(0); // charid
        p.write_i32(0); // beans
        p.write_i32(0); // int 0
        p.write_bytes(&[0u8; 5]); // slot limits
        p.write_i64(0); // long
        p.write_u8(0); // equipped block end
        p.write_u8(0); // cash-equipped end
        for _ in 0..5 {
            p.write_u8(0); // inventory tab end
        }
        p.write_i16(0); // skills (isCs: short 0)
        p.write_i16(0); // cooldown (isCs)
        p.write_i16(0); // quest a
        p.write_i16(0); // quest b
        // addRingInfo: all empty
        p.write_i16(0);
        p.write_i16(0);
        p.write_i16(0);
        p.write_i16(0);
        // addRocksInfo: 15 ints
        for _ in 0..15 {
            p.write_i32(0);
        }
        // monster book (isCs)
        p.write_i32(1);
        p.write_u8(0);
        p.write_i16(0);
        p.write_i16(0); // questinfo
        p.write_i16(0); // short 0
        p.write_i16(0);
        p.write_i16(0);
        // account name
        p.write_string("ccc");
        // 1 fixed sn + 1 mod item (flags=1: itemid only)
        p.write_i32(1);
        p.write_i32(10199999);
        p.write_i16(1);
        p.write_i32(5550000);
        p.write_i32(1);
        p.write_i32(5000000);
        let mut cur = Cursor::new(&p.as_bytes()[2..]);
        let (info, items) = parse_cash_shop(&mut cur).expect("parse");
        assert!(!cur.failed());
        assert_eq!(info.id, 3953);
        assert_eq!(info.name, "Player");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].sn, 5550000);
        assert_eq!(items[0].itemid, 5000000);
    }
}
