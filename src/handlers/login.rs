//! Login-related packet handlers: auth, world select, char select, channel connect.

use crate::config::Config;
use crate::packet::Cursor;
use crate::packets::login as plogin;
use crate::parsers;
use crate::session::Session;
use crate::state::{BotState, Phase};

/// LOGIN_STATUS (0x00) — CongMS only sends this for login failures
/// (`byte reason; short 0`). A reason of 0 is treated as success.
pub(super) async fn handle_login_status(
    recv: &mut Cursor<'_>,
    state: &mut BotState,
    session: &mut Session,
) -> Result<(), String> {
    let reason = parsers::parse_login_status(recv);
    if reason == 0 {
        state.phase = Phase::WorldSelect;
        session.send_packet(plogin::server_request()).await?;
    } else {
        let desc = login_reason_desc(reason);
        crate::emit_f!([crate::emit::Field::Status => reason] => "[login] 失败 reason={reason} ({desc})");
    }
    Ok(())
}

fn login_reason_desc(reason: u8) -> &'static str {
    match reason {
        1 => "已封禁",
        2 => "密码错误",
        3 => "账号未注册",
        4 => "密码错误",
        5 => "账号不存在",
        6 => "已登录",
        7 => "服务器已满",
        8 => "已登录",
        9 => "频繁登录",
        10 => "密码错误(过期)",
        11 => "密码错误",
        12 => "IP限制",
        13 => "密码过期",
        14 => "账号封禁",
        15 => "硬件限制",
        _ => "未知错误",
    }
}

/// CHOOSE_GENDER (0x04) — the account needs a gender. Reply with SET_GENDER.
/// Part of the auto-register flow (register scope), no interactive entry: just reply with the configured gender.
pub(super) async fn handle_choose_gender(
    recv: &mut Cursor<'_>,
    state: &mut BotState,
    session: &mut Session,
    config: &Config,
) -> Result<(), String> {
    let account = recv.read_string();
    crate::emit_f!([crate::emit::Field::Name => account] => "[choose_gender] account '{account}' needs a gender");
    state.phase = Phase::GenderPick;
    session
        .send_packet(plogin::set_gender(config.gender == 1, &account)).await?;
    Ok(())
}

/// GENDER_SET (0x05) — gender was stored; log in again to finish auth.
/// The re-login will trigger LOGIN_STATUS, which requests the server list.
pub(super) async fn handle_gender_set(
    state: &mut BotState,
    session: &mut Session,
    config: &Config,
) -> Result<(), String> {
    crate::emit_f!([] => "[gender_set] gender stored, re-logging in");
    state.phase = Phase::LoggingIn;
    session
        .send_packet(plogin::login(&config.account, &config.password)).await?;
    Ok(())
}

pub(super) fn handle_server_status(recv: &mut Cursor<'_>) {
    let status = recv.read_i16();
    if status != 0 {
        crate::emit_f!([crate::emit::Field::Status => status] => "[serverstatus] population status={status}");
    }
}

pub(super) async fn handle_serverlist(
    recv: &mut Cursor<'_>,
    state: &mut BotState,
    session: &mut Session,
    config: &Config,
) -> Result<(), String> {
    let mut added = false;
    while recv.available() {
        match parsers::parse_world(recv) {
            Some(world) => {
                crate::emit_f!([
                        crate::emit::Field::Id => world.wid,
                        crate::emit::Field::World => world.name,
                        crate::emit::Field::Count => world.channelcount,
                    ] => 
                    "[world] id={} name={} channels={}",
                    world.wid, world.name, world.channelcount);
                // The server may repeat SERVERLIST; dedup by wid
                if !state.worlds.iter().any(|w| w.wid == world.wid) {
                    state.worlds.push(world);
                }
                added = true;
            }
            None => break,
        }
    }

    if !added {
        return Ok(());
    }

    if state.worlds.is_empty() {
        crate::emit_f!([] => "[serverlist] no worlds received");
        return Ok(());
    }

    if state.phase != Phase::WorldSelect {
        return Ok(());
    }

    // Only the first login (TUI interaction) waits for the user's pick; reconnects (relogin)
    // silently re-login with the last remembered world/charlist — a dropped-connection reconnect
    // is "returning to the game", not a fresh interaction.
    if config.interactive && !state.relogin {
        crate::emit_f!([] => 
            "[serverlist] interactive: pick with 'login world <index> [channel]'");
        return Ok(());
    }

    let wid = if state.relogin && state.selected_world >= 0 {
        state.selected_world as i8
    } else {
        state
            .worlds
            .iter()
            .find(|w| w.wid == config.world as i8)
            .map(|w| w.wid)
            .unwrap_or(state.worlds[0].wid)
    };

    state.selected_world = wid;
    // Reconnect (relogin) reuses the channel picked this session; the first
    // login falls back to config.channel (CLI --channel / default 1). The
    // TUI picker overwrites it via `login world <index> [channel]` before
    // charlist is requested. Channels are 1-based user-facing; the wire byte
    // is 0-based (chan - 1).
    let chan = if state.relogin && state.selected_channel >= 0 {
        state.selected_channel as u8
    } else {
        config.channel
    };
    state.selected_channel = chan as i32;
    state.channel = chan as i32;
    crate::emit_f!([
            crate::emit::Field::Id => wid,
            crate::emit::Field::Channel => chan,
        ] => 
        "[serverlist] selecting world={wid} channel={}",
        chan);

    session
        .send_packet(plogin::server_status_request()).await?;
    session
        .send_packet(plogin::charlist_request(wid as u8, chan.saturating_sub(1))).await?;

    state.phase = Phase::CharSelect;

    Ok(())
}

pub(super) async fn handle_charlist(
    recv: &mut Cursor<'_>,
    state: &mut BotState,
    session: &mut Session,
    config: &Config,
) -> Result<(), String> {
    recv.skip(1);
    recv.skip_int();
    let charcount = recv.read_u8();

    for _ in 0..charcount {
        let ce = parsers::parse_charentry(recv);
        crate::emit_f!([
                crate::emit::Field::Cid => ce.id,
                crate::emit::Field::Name => ce.stats.name,
                crate::emit::Field::Job => ce.stats.job,
                crate::emit::Field::Level => ce.stats.level,
            ] => 
            "[char] id={} name={} job={} level={}",
            ce.id, ce.stats.name, ce.stats.job, ce.stats.level);
        state.characters.push(ce);
    }

    recv.skip_short();
    let _slots = recv.read_i32();

    if state.characters.is_empty() {
        crate::emit_f!([] => "[charlist] no characters to select");
        return Ok(());
    }

    if state.phase != Phase::CharSelect {
        return Ok(());
    }

    // Only the first login (TUI interaction) waits for the user's pick; reconnects silently
    // re-login with the last remembered cid.
    if config.interactive && !state.relogin {
        crate::emit_f!([] => "[charlist] interactive: pick with 'login char <index>'");
        return Ok(());
    }

    let chosen = if state.relogin && state.my_cid != 0 {
        // Returning to the game: pick the character last used
        state
            .characters
            .iter()
            .find(|c| c.id == state.my_cid)
            .cloned()
            .unwrap_or_else(|| state.characters[0].clone())
    } else {
        match config.char_index {
            idx if idx >= 0 => state
                .characters
                .get(idx as usize)
                .cloned()
                .unwrap_or_else(|| state.characters[0].clone()),
            _ => state
                .characters
                .iter()
                .find(|c| c.id == config.cid)
                .cloned()
                .unwrap_or_else(|| state.characters[0].clone()),
        }
    };

    state.my_cid = chosen.id;
    state.mapid = chosen.stats.mapid;
    state.level = chosen.stats.level;
    state.exp = chosen.stats.exp;
    state.ap = chosen.stats.ap;
    state.hp = chosen.stats.hp;
    state.maxhp = chosen.stats.maxhp;
    state.mp = chosen.stats.mp;
    state.maxmp = chosen.stats.maxmp;
    state.str = chosen.stats.str;
    state.dex = chosen.stats.dex;
    state.int = chosen.stats.int;
    state.luk = chosen.stats.luk;
    state.phase = Phase::EnteringMap;
    crate::emit_f!([
            crate::emit::Field::Cid => chosen.id,
            crate::emit::Field::Name => chosen.stats.name,
            crate::emit::Field::MapId => chosen.stats.mapid,
        ] => 
        "[charlist] selecting cid={} ({}) map={}",
        chosen.id, chosen.stats.name, chosen.stats.mapid);

    if config.charlist_only {
        crate::emit_f!([] => 
            "[charlist] --charlist-only: stopping before map entry (no CHAR_SELECT sent)");
        return Ok(());
    }

    session
        .send_packet(plogin::select_char(chosen.id)).await?;

    Ok(())
}

pub(super) async fn handle_server_ip(
    recv: &mut Cursor<'_>,
    state: &mut BotState,
    session: &mut Session,
    config: &Config,
) -> Result<(), String> {
    let (ip, port, cid) = parsers::parse_server_ip(recv);
    state.my_cid = cid;
    reconnect_to_channel(state, session, config, &ip, port, Some(cid)).await
}

/// Channel-switch reply (0x13): same reconnect + PLAYER_LOGGEDIN flow as
/// SERVER_IP, but the packet carries no cid — reuse `my_cid`.
pub(super) async fn handle_channel_change(
    recv: &mut Cursor<'_>,
    state: &mut BotState,
    session: &mut Session,
    config: &Config,
) -> Result<(), String> {
    let (ip, port) = parsers::parse_channel_change(recv);
    crate::emit_f!([crate::emit::Field::Text => format!("{ip}:{port}")] => "[channel] switch -> {ip}:{port}");
    // 0x13 confirms the switch: remember the new channel for reconnects
    // (state.channel was set optimistically by the `channel` command).
    state.selected_channel = state.channel;
    reconnect_to_channel(state, session, config, &ip, port, Some(state.my_cid)).await
}

async fn reconnect_to_channel(
    state: &mut BotState,
    session: &mut Session,
    config: &Config,
    ip: &str,
    port: u16,
    cid: Option<i32>,
) -> Result<(), String> {
    state.phase = Phase::EnteringMap;
    state.map_enter_time = std::time::Instant::now();

    let channel_ip = config.channel_ip.as_deref().unwrap_or(ip);
    crate::emit_f!([crate::emit::Field::Text => format!("{channel_ip}:{port}")] => "[server_ip] channel at {channel_ip}:{port} (original {ip})");

    let addr = format!("{channel_ip}:{port}");
    session.reconnect(&addr).await?;
    crate::emit_f!([] => "[server_ip] reconnected to channel server");

    session
        .send_packet(plogin::player_login(cid.unwrap_or(state.my_cid))).await?;
    Ok(())
}