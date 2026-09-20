//! Map change packets for CongMS 079.
//!
//! CHANGE_MAP (0x21) format (confirmed against live captures):
//! ```text
//! byte  died         0 = alive, 1 = dead (revive)
//! int   targetid     -1 = use portal's target, otherwise warp to this map
//! str   portal_name  MapleAsciiString, portal name on current map
//! byte  (padding)    skipped by server
//! short wheel        0 = normal, 1 = town scroll
//! ```
//!
//! CHANGE_MAP_SPECIAL (0x61) format from `PlayerHandler.ChangeMapSpecial`:
//! ```text
//! byte  (0x00)       skipped
//! str   portal_name  MapleAsciiString
//! ```

use crate::opcodes::send;
use crate::packet::Packet;

/// CHANGE_MAP (0x21) — request the server to warp the player to a different map.
///
/// When `targetid = -1`, the server looks up `portal_name` on the current map
/// and enters it via `portal.enterPortal(c)`, which uses the portal's target
/// map from WZ data. When `targetid != -1` and the player is dead, the server
/// warps to the map's return map.
///
/// The official client sends `byte 0 + int -1 + str portal + short x + short y
/// + short 0` (character position + wheel), 19 bytes for a 4-char portal
/// (verified from the private-server capture). Servers that read the trailing
/// fields unconditionally underflow on the truncated 9-byte form, so send the
/// full layout with the player's current position.
pub fn change_map(portal_name: &str, px: i16, py: i16) -> Packet {
    let mut p = Packet::new(send::CHANGE_MAP);
    p.write_u8(0); // died = false (alive)
    p.write_i32(-1); // targetid = -1 (use portal's target map)
    p.write_string_gb(portal_name);
    p.write_i16(px); // character x
    p.write_i16(py); // character y
    p.write_i16(0); // wheel = 0 (normal)
    p
}

/// CHANGE_MAP_SPECIAL (0x61) — request the server to enter a portal by name.
///
/// The server calls `chr.getMap().getPortal(portal_name)` then
/// `portal.enterPortal(c)` without any GM or map-id conditions.
///
/// The official client appends `short x + short y` after the portal string
/// (15 bytes, verified from the private-server capture); send the full layout
/// with the player's current position.
pub fn change_map_special(portal_name: &str, px: i16, py: i16) -> Packet {
    let mut p = Packet::new(send::CHANGE_MAP_SPECIAL);
    p.write_u8(0); // skipped byte
    p.write_string_gb(portal_name);
    p.write_i16(px);
    p.write_i16(py);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_map_matches_official_capture() {
        // (byte type + int -1 + "in03" + x=313 y=-95 + wheel 0); bot's died=0 is equivalent to the official client
        let bytes = change_map("in03", 313, -95).into_bytes();
        assert_eq!(bytes.len(), 19);
        assert_eq!(
            bytes,
            vec![
                0x21, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x04, 0x00, 0x69, 0x6E, 0x30,
                0x33, 0x39, 0x01, 0xA1, 0xFF, 0x00, 0x00,
            ]
        );
    }

    #[test]
    fn change_map_special_matches_official_capture() {
        // (byte type + "tuto01" + x=240 y=180)
        let bytes = change_map_special("tuto01", 240, 180).into_bytes();
        assert_eq!(bytes.len(), 15);
        assert_eq!(
            bytes,
            vec![
                0x61, 0x00, 0x00, 0x06, 0x00, 0x74, 0x75, 0x74, 0x6F, 0x30, 0x31, 0xF0,
                0x00, 0xB4, 0x00,
            ]
        );
    }
}