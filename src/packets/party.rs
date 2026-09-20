//! Party packets for CongMS 079 — client→server = 0x78, server→client = 0x3B.
//!
//! Client sends (from live captures party.txt/party2.txt/party3.txt, the
//! server's [receive] view):
//! - Create:   `78 00 01` (no params, clicking the "create party" button)
//! - Invite:   `78 00 04 + string name` (invitee's name)
//! - Join:     `78 00 03 + int partyid + byte 0` (accept invite / join)
//! - Leave:    `78 00 02 00`
//! - Expel:    `78 00 05 + int cid`
//! Server replies (0x3B, [send]):
//! - Create ack: `3B 00 08 + int partyid + 0x3B9AC9FF + 0x3B9AC9FF + int 0` (partyid assigned by the server!)
//! - Invite/request: `3B 00 04 + int partyid + string name + byte 0`
//! - Full sync: `3B 00 0F + int partyid + ...` (member list)
//! - Update:   `3B 00 0C + int partyid + ...`
//! - Leave ack: `3B 00 02 + ...`
//! Accept invite (other side): DENY_PARTY_REQUEST 0x79 = byte 27 + int partyid

use crate::opcodes::send;
use crate::packet::Packet;

fn party(action: u8) -> Packet {
    let mut p = Packet::new(send::PARTY_OPERATION);
    p.write_u8(action);
    p
}

/// DENY_PARTY_REQUEST (0x79) with action 27 = accept the invite.
pub fn accept_invite(partyid: i32) -> Packet {
    let mut p = Packet::new(send::DENY_PARTY_REQUEST);
    p.write_u8(27);
    p.write_i32(partyid);
    p
}

/// Create a party: `78 00 01` (no params).
pub fn create() -> Packet {
    party(1)
}

/// Invite a player: `78 00 04 + name` (no partyid).
pub fn invite(name: &str) -> Packet {
    let mut p = party(4);
    p.write_string_gb(name);
    p
}

/// Join / accept an invite: `78 00 03 + partyid + 00`.
pub fn join(partyid: i32) -> Packet {
    let mut p = party(3);
    p.write_i32(partyid);
    p.write_u8(0);
    p
}

/// Leave / disband the party: `78 00 02 00`.
/// The client only sends this one packet; the server acts by the sender's
/// role: leader leaving = disband, member leaving = quit.
pub fn leave() -> Packet {
    let mut p = party(2);
    p.write_u8(0);
    p
}

/// Expel a member: `78 00 05 + cid`.
pub fn expel(cid: i32) -> Packet {
    let mut p = party(5);
    p.write_i32(cid);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_layout() {
        // capture: 78 00 01 (client creates)
        let bytes = create().into_bytes();
        assert_eq!(&bytes[0..2], &[0x78, 0x00]);
        assert_eq!(bytes[2], 1);
        assert_eq!(bytes.len(), 3);
    }

    #[test]
    fn invite_layout() {
        // capture: 78 00 04 06 00 7A 6A 6A 7A 6A 6A (invite Player)
        let bytes = invite("Player").into_bytes();
        assert_eq!(&bytes[0..2], &[0x78, 0x00]);
        assert_eq!(bytes[2], 4);
        assert_eq!(&bytes[3..5], &[6, 0]);
        assert_eq!(&bytes[5..11], b"Player");
        assert_eq!(bytes.len(), 11);
    }

    #[test]
    fn join_layout() {
        // capture: 78 00 03 02 00 00 00 00 (partyid=2)
        let bytes = join(2).into_bytes();
        assert_eq!(bytes[2], 3);
        assert_eq!(&bytes[3..7], &[2, 0, 0, 0]);
        assert_eq!(bytes[7], 0);
        assert_eq!(bytes.len(), 8);
    }

    #[test]
    fn leave_layout() {
        // capture: 78 00 02 00
        let bytes = leave().into_bytes();
        assert_eq!(bytes[2], 2);
        assert_eq!(bytes[3], 0);
        assert_eq!(bytes.len(), 4);
    }

    #[test]
    fn expel_layout() {
        // capture: 78 00 05 71 0F 00 00 (expel cid=3953)
        let bytes = expel(3953).into_bytes();
        assert_eq!(bytes[2], 5);
        assert_eq!(&bytes[3..7], &[0x71, 0x0F, 0x00, 0x00]);
        assert_eq!(bytes.len(), 7);
    }
}
