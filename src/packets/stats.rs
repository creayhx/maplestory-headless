//! Stats packet builders for CongMS 079.
//!
//! `DISTRIBUTE_AP` (0x54) format verified from `StatsHandling.DistributeAP`:
//! `int tick; int statValue(MapleStat.getValue())`, adding one point per packet.
//! Stat values: STR=256, DEX=512, INT=1024, LUK=2048, MAXHP=8192, MAXMP=32768.

use crate::opcodes::send;
use crate::packet::Packet;

/// MapleStat values accepted by AP distribution (see `MapleStat.getValue`).
pub mod stat {
    pub const STR: i32 = 256;
    pub const DEX: i32 = 512;
    pub const INT: i32 = 1024;
    pub const LUK: i32 = 2048;
    pub const MAXHP: i32 = 8192;
    pub const MAXMP: i32 = 32768;
}

/// DISTRIBUTE_AP (0x54) — spend one ability point on `stat_value`.
pub fn distribute_ap(stat_value: i32) -> Packet {
    let mut p = Packet::new(send::DISTRIBUTE_AP);
    p.write_time(); // tick
    p.write_i32(stat_value);
    p
}

/// DISTRIBUTE_SP (0x57) — spend one skill point on `skillid` (from
/// `StatsHandling.DistributeSP`, read as `int tick; int skillid`).
pub fn distribute_sp(skillid: i32) -> Packet {
    let mut p = Packet::new(send::DISTRIBUTE_SP);
    p.write_time(); // tick
    p.write_i32(skillid);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribute_ap_layout() {
        let bytes = distribute_ap(stat::STR).into_bytes();
        // opcode 0x54 LE + int tick + int STR(256)
        assert_eq!(&bytes[0..2], &[0x54, 0x00]);
        assert_eq!(&bytes[6..10], &[0x00, 0x01, 0x00, 0x00]); // 256 LE
        assert_eq!(bytes.len(), 2 + 4 + 4);
    }

    #[test]
    fn distribute_sp_layout() {
        let bytes = distribute_sp(1001003).into_bytes();
        // opcode 0x57 LE + int tick + int skillid
        assert_eq!(&bytes[0..2], &[0x57, 0x00]);
        assert_eq!(&bytes[6..10], &[0x2b, 0x46, 0x0f, 0x00]); // 1001003 LE
        assert_eq!(bytes.len(), 2 + 4 + 4);
    }
}
