//! Headless packet handlers for CongMS 079 — a pure-state login/field machine
//! with no UI.

pub mod cashshop;
pub mod field;
pub mod login;
pub mod party;
pub mod trade;

use self::cashshop::*;
use self::field::*;
use self::login::*;
use self::party::*;
use self::trade::*;

use crate::config::Config;
use crate::opcodes::recv;
use crate::packet::Cursor;
use crate::packets::login as plogin;
use crate::session::Session;
use crate::state::BotState;

/// Dispatch one decrypted packet body to the matching handler. Malformed
/// packets set the cursor's `failed` flag instead of crashing; we log and skip.
pub async fn handle(
    body: &[u8],
    state: &mut BotState,
    session: &mut Session,
    config: &Config,
) -> Result<(), String> {
    if body.len() < 2 {
        return Ok(());
    }

    let opcode = u16::from_le_bytes([body[0], body[1]]);
    let payload = &body[2..];
    let mut recv = Cursor::new(payload);

    match opcode {
        recv::PING => {
            session.send_packet(plogin::pong()).await?;
        }
        recv::LOGIN_STATUS => handle_login_status(&mut recv, state, session).await?,
        recv::CHOOSE_GENDER => handle_choose_gender(&mut recv, state, session, config).await?,
        recv::GENDER_SET => handle_gender_set(state, session, config).await?,
        recv::SERVERSTATUS => handle_server_status(&mut recv),
        recv::SERVERLIST => handle_serverlist(&mut recv, state, session, config).await?,
        recv::CHARLIST => handle_charlist(&mut recv, state, session, config).await?,
        recv::SERVER_IP => handle_server_ip(&mut recv, state, session, config).await?,
        recv::CHANGE_CHANNEL => handle_channel_change(&mut recv, state, session, config).await?,
        recv::SET_FIELD => {
            // Arc clone (cheap refcount bump, not a table copy) so the borrow of
            // session.names ends before the call; SET_FIELD is a rare packet.
            let names = session.names.clone();
            handle_set_field(&mut recv, state, &names);
        }
        recv::SET_ITC => { /* MTS transition, ignored */ }
        recv::SET_CASH_SHOP => handle_set_cash_shop(&mut recv, state),
        recv::CS_OPERATION => handle_cs_operation(&mut recv, state),
        recv::CS_UPDATE => handle_cs_update(&mut recv, state),
        recv::CS_USE => { /* cash shop enabled, ignored */ }
        recv::SERVERMESSAGE => handle_server_message(&mut recv),
        recv::MULTICHAT => handle_multichat(&mut recv),
        recv::WHISPER => handle_whisper(&mut recv, state, session).await,
        recv::CHATTEXT => handle_chattext(&mut recv, state),
        recv::SET_WEEK_EVENT_MESSAGE => handle_yellow_chat(&mut recv),
        recv::SPAWN_PLAYER => handle_spawn_player(&mut recv, state),
        recv::REMOVE_PLAYER_FROM_MAP => {
            let oid = recv.read_i32();
            state.entities.remove(&oid);
        }
        recv::CLOSE_RANGE_ATTACK => handle_attack_broadcast(&mut recv, state),
        recv::MOVE_PLAYER => handle_move_player(&mut recv, state),
        recv::PARTY_OPERATION => handle_party_operation(&mut recv, state, session).await?,
        recv::SPAWN_MONSTER_CONTROL => handle_spawn_mob(&mut recv, state, true),
        recv::SPAWN_MONSTER => handle_spawn_mob(&mut recv, state, false),
        recv::MOVE_MONSTER => handle_move_life(&mut recv, state),
        recv::DAMAGE_MONSTER => handle_damage_monster(&mut recv, state),
        recv::SHOW_MONSTER_HP => handle_player_hp(&mut recv, state),
        recv::KILL_MONSTER => handle_kill_monster(&mut recv, state),
        recv::DROP_ITEM_FROM_MAPOBJECT => handle_drop(&mut recv, state),
        recv::REMOVE_ITEM_FROM_MAP => handle_drop_removal(&mut recv, state),
        recv::UPDATE_STATS => handle_update_stats(&mut recv, state),
        recv::UPDATE_SKILLS => handle_skill_update(&mut recv, state),
        recv::MODIFY_INVENTORY_ITEM => handle_inventory_update(&mut recv, state),
        recv::SPAWN_NPC => handle_spawn_npc(&mut recv, state),
        recv::REMOVE_NPC => handle_remove_npc(&mut recv, state),
        recv::REACTOR_SPAWN => handle_spawn_reactor(&mut recv, state),
        recv::REACTOR_HIT => handle_reactor_hit(&mut recv, state),
        recv::REACTOR_DESTROY => handle_reactor_destroy(&mut recv, state),
        recv::KEYMAP => handle_keymap(&mut recv, state),
        recv::NPC_TALK => handle_npc_talk(&mut recv, state),
        recv::OPEN_NPC_SHOP => handle_open_npc_shop(&mut recv, state),
        recv::CONFIRM_SHOP_TRANSACTION => handle_confirm_shop(&mut recv, state),
        recv::PLAYER_INTERACTION => handle_player_interaction(&mut recv, state),
        _ => {
            if session.show_packets {
                crate::emit_f!([] => "UNHANDLED [{opcode:02X}] {} bytes", payload.len());
            }
        }
    }

    if recv.failed() {
        crate::emit_err_f!([crate::emit::Field::Value => opcode, crate::emit::Field::Count => payload.len()] => "[warn] malformed {opcode:02X} packet ({} bytes)", payload.len());
    }

    Ok(())
}