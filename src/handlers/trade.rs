//! Player trade handlers — PLAYER_INTERACTION (0x14F) sub-packets.

use crate::packet::Cursor;
use crate::parsers::trade as ptrade;
use crate::parsers::trade::action;
use crate::parsers::trade::message;
use crate::state::BotState;

pub fn handle_player_interaction(recv: &mut Cursor<'_>, state: &mut BotState) {
    let act = recv.read_u8();
    match act {
        action::INVITE => {
            if let Some(name) = ptrade::parse_trade_invite(recv) {
                state.trade.invite = Some(crate::state::TradeInvite {
                    name: name.clone(),
                    time: std::time::Instant::now(),
                });
                crate::emit_f!([crate::emit::Field::Name => name] => "[trade] invite from '{name}' (use 'trade accept' to join)");
            }
        }
        action::PARTNER_ADD => {
            let _marker = recv.read_u8();
            crate::emit_f!([] => "[trade] partner joined the trade");
        }
        action::START => {
            if let Some(slot) = ptrade::parse_trade_start(recv) {
                if let Some(inv) = state.trade.invite.take() {
                    state.trade.partner = inv.name.clone();
                }
                if state.trade.partner.is_empty() {
                    state.trade.partner = "?".to_string();
                }
                state.trade.active = true;
                state.trade.my_slot = slot;
                state.trade.meso_partner = 0;
                state.trade.items_partner.clear();
                state.trade.partner_locked = false;
                state.trade.locked = false;
                crate::emit_f!([
                        crate::emit::Field::Name => state.trade.partner,
                        crate::emit::Field::Slot => slot,
                    ] => 
                    "[trade] window open with '{}' slot={} (put items then 'trade confirm')",
                    state.trade.partner, slot);
            }
        }
        action::ITEM_ADD => {
            if let Some((number, item)) = ptrade::parse_trade_item_add(recv) {
                if number == 1 {
                    state.trade.items_partner.push((item.itemid, item.qty));
                    crate::emit_f!([
                            crate::emit::Field::ItemId => item.itemid,
                            crate::emit::Field::Qty => item.qty.max(1),
                            crate::emit::Field::Count => state.trade.items_partner.len(),
                        ] => 
                        "[trade] partner added item {} x{} (total {})",
                        item.itemid,
                        item.qty.max(1),
                        state.trade.items_partner.len());
                }
            }
        }
        action::MESO_SET => {
            if let Some((number, meso)) = ptrade::parse_trade_meso(recv) {
                if number == 1 {
                    state.trade.meso_partner = meso;
                    crate::emit_f!([crate::emit::Field::Meso => meso] => "[trade] partner added meso {meso}");
                }
            }
        }
        action::CONFIRM => {
            state.trade.partner_locked = true;
            crate::emit_f!([] => "[trade] partner locked the trade");
        }
        action::MESSAGE => {
            if let Some((_slot, msg)) = ptrade::parse_trade_message(recv) {
                match msg {
                    message::COMPLETE | message::DONE | message::SUCCESS => {
                        crate::emit_f!([
                                crate::emit::Field::Meso => state.trade.meso_partner,
                                crate::emit::Field::Count => state.trade.items_partner.len(),
                            ] => 
                            "[trade] completed — received {} meso + {} item(s)",
                            state.trade.meso_partner,
                            state.trade.items_partner.len());
                    }
                    message::FAILED_FULL => crate::emit_f!([] => "[trade] failed: inventory full"),
                    message::FAILED_PICKUP_RESTRICTED => {
                        crate::emit_f!([] => "[trade] failed: item cannot be picked up")
                    }
                    _ => crate::emit_f!([crate::emit::Field::Value => msg] => "[trade] closed (result {msg})"),
                }
                state.trade.active = false;
                state.trade.invite = None;
                state.trade.items_partner.clear();
                state.trade.meso_partner = 0;
                state.trade.partner_locked = false;
                state.trade.locked = false;
                state.trade.partner.clear();
            }
        }
        other => {
            crate::emit_f!([crate::emit::Field::Value => other] => "[trade] unhandled action {other}");
        }
    }
}
