//! Packet parsers for CongMS 079.
//!
//! Formats verified against the decompiled `079.jar`:
//! - `LoginPacket.getServerList` (SERVERLIST)
//! - `LoginPacket.getCharList` + `PacketHelper.addCharStats/addCharLook` (CHARLIST)
//! - `LoginPacket.getLoginFailed` (LOGIN_STATUS failure: byte reason + short 0)
//! - `MaplePacketCreator.getServerIP` (SERVER_IP)

use crate::packet::Cursor;
use crate::state::{CharEntry, LookEntry, World};

use super::field::parse_stats;

/// Parse the login failure reason from LOGIN_STATUS.
/// Returns the reason byte; a reason of 0 would mean "success" but CongMS never
/// sends a success LOGIN_STATUS (it just accepts the login silently).
pub fn parse_login_status(recv: &mut Cursor<'_>) -> u8 {
    recv.read_u8()
}

/// Parse one world from a SERVERLIST packet. Returns None for the end marker
/// (byte 255).
pub fn parse_world(recv: &mut Cursor<'_>) -> Option<World> {
    let wid = recv.read_i8();

    if wid == -1 {
        return None;
    }

    let name = recv.read_string_gb();
    let flag = recv.read_u8();
    let message = recv.read_string_gb();

    recv.skip(4); // short 100, short 100
    let channelcount = recv.read_u8(); // lastChannel
    recv.skip(4); // int 500

    let mut chloads = Vec::with_capacity(channelcount as usize);
    for _ in 0..channelcount {
        recv.skip_string(); // channel name
        chloads.push(recv.read_i32()); // load
        recv.skip(1); // serverId
        recv.skip(2); // index
    }

    recv.skip(2); // trailing short 0

    Some(World {
        wid,
        name,
        flag,
        message,
        channelcount,
        chloads,
    })
}

/// Parse one character entry from a CHARLIST packet.
pub fn parse_charentry(recv: &mut Cursor<'_>) -> CharEntry {
    let id = recv.read_i32();
    let stats = parse_stats(recv);
    let look = parse_look(recv);

    // addCharEntry tail: write(0), plus write(2) if job is 900 (GM).
    recv.skip(1);
    if stats.job == 900 {
        recv.skip(1);
    }

    CharEntry { id, stats, look }
}

/// Parse the look block (`PacketHelper.addCharLook`).
pub fn parse_look(recv: &mut Cursor<'_>) -> LookEntry {
    let mut look = LookEntry::default();

    look.female = recv.read_bool();
    look.skin = recv.read_u8();
    look.faceid = recv.read_i32();
    recv.skip(1); // mega
    look.hairid = recv.read_i32();

    let mut eqslot = recv.read_u8();
    while eqslot != 0xFF {
        let itemid = recv.read_i32();
        look.equips.insert(eqslot as i8, itemid);
        eqslot = recv.read_u8();
    }

    let mut mskeqslot = recv.read_u8();
    while mskeqslot != 0xFF {
        let itemid = recv.read_i32();
        look.maskedequips.insert(mskeqslot as i8, itemid);
        mskeqslot = recv.read_u8();
    }

    look.maskedequips.insert(-111, recv.read_i32());
    recv.skip(4); // int 0
    recv.skip(8); // long 0

    look
}

/// Parse the channel server address from SERVER_IP. Returns (ip, port, cid).
pub fn parse_server_ip(recv: &mut Cursor<'_>) -> (String, u16, i32) {
    recv.skip_short(); // short 0

    let mut parts = Vec::with_capacity(4);
    for _ in 0..4 {
        parts.push(recv.read_u8().to_string());
    }
    let ip = parts.join(".");
    let port = recv.read_u16();
    let cid = recv.read_i32();

    (ip, port, cid)
}

/// Parse the channel-switch reply (0x13, `MaplePacketCreator.getChannelChange`):
/// `byte 1 + byte ip×4 + short port + byte 0`. No cid — the client reconnects
/// with its own PLAYER_LOGGEDIN(cid).
pub fn parse_channel_change(recv: &mut Cursor<'_>) -> (String, u16) {
    recv.skip(1); // byte 1 (mode)

    let mut parts = Vec::with_capacity(4);
    for _ in 0..4 {
        parts.push(recv.read_u8().to_string());
    }
    let ip = parts.join(".");
    let port = recv.read_u16();

    (ip, port)
}