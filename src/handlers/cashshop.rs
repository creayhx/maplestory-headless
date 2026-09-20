//! Cash shop packet handlers (CongMS 079).
//!
//! Formats from `tools/packet/MTSCSPacket.java` + `CashShopOperation.java`:
//! - SET_CASH_SHOP 0x83: warpCS entrance (huge, not parsed)
//! - CS_UPDATE 0x161: `int nxCredit + int rewardPoints`
//! - CS_USE 0x15: cash shop enabled (ignored)
//! - CS_OPERATION 0x162 sub-ops: 66 inventory, 68 gifts, 70 wishlist,
//!   76 bought, 79 bought-fail, 93 taken out, 95 stored, 123 generic fail

use crate::packet::Cursor;
use crate::state::{BotState, CashItem, Phase};

/// One cash item entry (addCashItemInfo):
/// `long uniqueId + long accId + int itemid + int sn + short qty +
///  ascii(13) sender + expire(8) + long 0`.
fn parse_cash_item(recv: &mut Cursor<'_>) -> CashItem {
    let unique_id = recv.read_i64();
    let accid = recv.read_i64();
    let itemid = recv.read_i32();
    let sn = recv.read_i32();
    let qty = recv.read_i16();
    recv.skip(13); // giftFrom
    recv.skip(8); // expiration
    recv.skip_long();
    CashItem {
        unique_id,
        accid,
        itemid,
        sn,
        qty,
    }
}

/// Human-readable cash shop error (sendCSFail / sendShowBoughtCashItemFail).
pub fn cs_error_name(err: i32) -> &'static str {
    match err {
        162 => "need a second password / bad password",
        163 => "wrong gender for recipient",
        164 => "not enough points",
        165 => "invalid coupon",
        167 => "coupon already used",
        168 => "not enough NX",
        171 => "cannot gift yourself",
        172 => "character not found",
        175 => "cash inventory full (100)",
        176 => "wrong gender for recipient",
        177 => "inventory full / cannot move",
        184 => "not enough mesos",
        186 => "item gender does not match",
        193 | 194 => "transaction error",
        225 => "item not on sale / invalid",
        _ => "unknown error",
    }
}

/// SET_CASH_SHOP (0x83) — the warpCS entrance packet: character info (CS
/// variant) + the sale list. Transition into the cash shop; the server follows
/// up with 66/68/70 + 0x161 + 0x15 packets.
pub(super) fn handle_set_cash_shop(recv: &mut Cursor<'_>, state: &mut BotState) {
    state.phase = Phase::CashShop;
    match crate::parsers::parse_cash_shop(recv) {
        Some((_info, items)) => {
            state.cs_items = items;
            crate::emit_f!([crate::emit::Field::Count => state.cs_items.len()] =>
                "[cashshop] entered: {} item(s) on sale",
                state.cs_items.len());
        }
        None => {
            state.cs_items.clear();
            let total = recv.remaining();
            crate::emit_f!([crate::emit::Field::Count => total] =>
                "[cashshop] entered (sale list unparsed, {} bytes left)",
                total);
        }
    }
}

/// CS_UPDATE (0x161) — NX credit + reward point balances.
pub(super) fn handle_cs_update(recv: &mut Cursor<'_>, state: &mut BotState) {
    state.cs_nx = recv.read_i32();
    state.cs_points = recv.read_i32();
    crate::emit_f!([
            crate::emit::Field::Value => state.cs_nx,
            crate::emit::Field::Value => state.cs_points,
        ] => 
        "[cashshop] balance nx={} points={}",
        state.cs_nx, state.cs_points);
}

/// CS_OPERATION (0x162) — sub-operation dispatcher.
pub(super) fn handle_cs_operation(recv: &mut Cursor<'_>, state: &mut BotState) {
    let sub = recv.read_u8();
    match sub {
        66 => {
            // showCashInventory: count, items, storageSlots, charSlots
            let count = recv.read_i16();
            state.cs_inventory.clear();
            for _ in 0..count {
                state.cs_inventory.push(parse_cash_item(recv));
            }
            recv.skip_short(); // storage slots
            recv.skip_short(); // character slots
            crate::emit_f!([crate::emit::Field::Count => state.cs_inventory.len()] => 
                "[cashshop] inventory {} item(s)",
                state.cs_inventory.len());
            for it in &state.cs_inventory {
                crate::emit_f!([
                        crate::emit::Field::Value => it.unique_id,
                        crate::emit::Field::ItemId => it.itemid,
                        crate::emit::Field::Value => it.sn,
                        crate::emit::Field::Qty => it.qty,
                    ] => 
                    "[cashshop]   unique={} item={} sn={} qty={}",
                    it.unique_id, it.itemid, it.sn, it.qty);
            }
        }
        68 => {
            // showGifts: count, then fixed blocks — ignored
        }
        70 => {
            // sendShowWishList: 10 ints — ignored
        }
        76 => {
            // showBoughtCashItem: purchase ok
            let item = parse_cash_item(recv);
            crate::emit_f!([
                    crate::emit::Field::ItemId => item.itemid,
                    crate::emit::Field::Value => item.sn,
                    crate::emit::Field::Qty => item.qty,
                    crate::emit::Field::Value => item.unique_id,
                ] => 
                "[cashshop] BOUGHT item={} sn={} qty={} unique={}",
                item.itemid, item.sn, item.qty, item.unique_id);
            state.cs_inventory.push(item);
        }
        79 => {
            // sendShowBoughtCashItemFail
            let err = recv.read_i16() as i32;
            crate::emit_f!([
                    crate::emit::Field::Status => err,
                    crate::emit::Field::Text => cs_error_name(err),
                ] => 
                "[cashshop] buy failed: {} ({err})",
                cs_error_name(err));
        }
        93 => {
            // confirmFromCSInventory: short pos + addItemInfo — item moved to bag
            let pos = recv.read_i16();
            crate::emit_f!([crate::emit::Field::Slot => pos] => "[cashshop] taken out to inventory slot {pos}");
        }
        95 => {
            // confirmToCSInventory: addCashItemInfo — item stored
            let item = parse_cash_item(recv);
            crate::emit_f!([
                    crate::emit::Field::ItemId => item.itemid,
                    crate::emit::Field::Value => item.unique_id,
                ] => 
                "[cashshop] stored item={} unique={}",
                item.itemid, item.unique_id);
        }
        123 => {
            // sendCSFail (generic)
            let err = recv.read_i16() as i32;
            crate::emit_f!([
                    crate::emit::Field::Status => err,
                    crate::emit::Field::Text => cs_error_name(err),
                ] => 
                "[cashshop] fail: {} ({err})",
                cs_error_name(err));
        }
        other => {
            crate::emit_f!([crate::emit::Field::Value => other] => "[cashshop] unhandled sub-op {other}");
        }
    }
}