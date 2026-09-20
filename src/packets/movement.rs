//! Movement packets for CongMS 079.
//!
//! `PlayerHandler.MovePlayer` does `slea.skip(33)` then
//! `MovementParse.parseMovement(slea, 1)` then requires exactly 8 bytes left.
//! The movement section is `byte count; fragments...; byte X; ceil(X/2) bytes`.
//! The 33-byte prefix and trailing 8 bytes are not validated by the server, so
//! zeros are acceptable.

use crate::opcodes::send;
use crate::packet::Packet;

/// One movement fragment (command 0 = normal absolute move, 14 bytes).
#[derive(Debug, Clone, Copy)]
pub struct Movement {
    pub xpos: i16,
    pub ypos: i16,
    pub xwobble: i16,
    pub ywobble: i16,
    pub unk: i16,
    pub fh: i16,
    pub newstate: u8,
    pub duration: i16,
}

impl Movement {
    pub fn absolute(x: i16, y: i16, state: u8) -> Self {
        Self {
            xpos: x,
            ypos: y,
            xwobble: 0,
            ywobble: 0,
            unk: 0,
            fh: 0,
            newstate: state,
            duration: 1,
        }
    }

    /// A walking move that animates smoothly on other clients. The duration
    /// (60ms) matches the bot's ~60ms measured movement tick period (50ms target
    /// + overhead), and the step (7px) equals duration × speed, so the receiving
    /// client glides at full speed with no idle gap. `facing` is the direction
    /// (0 = right, 1 = left) and lands in the stance low bit.
    pub fn walk(x: i16, y: i16, facing: u8) -> Self {
        const SPEED: i16 = 125; // px/sec, MapleStory default walk speed
        const DURATION: i16 = 56; // 7px @ 125px/s = 56ms
        Self {
            xpos: x,
            ypos: y,
            xwobble: SPEED,
            ywobble: 0,
            unk: 0,
            fh: 0,
            newstate: 2 | (facing & 1), // walking + facing
            duration: DURATION,
        }
    }

    /// A standing move: resets the animation state to idle so other clients
    /// don't keep showing the character walking in place. The character stance
    /// is WALK=2 / STAND=4 (with the facing bit in the low bit: odd = left), so
    /// standing is newstate 4 (right) or 5 (left).
    pub fn stand(x: i16, y: i16, facing: u8) -> Self {
        Self {
            xpos: x,
            ypos: y,
            xwobble: 0,
            ywobble: 0,
            unk: 0,
            fh: 0,
            newstate: 4 | (facing & 1), // standing + facing
            duration: 1,
        }
    }
}

/// Periodic client report packet (0x15): `byte 1 + int random + int 0`,
/// about 3~4 times per second. On standard servers 0x15 is CS_USE (cash shop);
/// its content is ignored there. The server may use it to confirm client
/// liveness / controller state.
pub fn strange_report() -> Packet {
    // On standard 079, 0x15 is recv (CS_USE); some servers repurpose it as a client→server report packet.
    let mut p = Packet::new(0x15);
    p.write_u8(1);
    p.write_i32(rand_u32());
    p.write_i32(0);
    p
}

fn rand_u32() -> i32 {
    use std::time::Instant;
    static START: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);
    // xorshift64-derived, avoids depending on the rand crate
    let t = START.elapsed().as_nanos() as u64;
    let mut x = t.wrapping_add(0x9E3779B97F4A7C15);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x >> 32) as i32
}

/// MOVE_PLAYER (0x24) — send the player's movement.
///
/// Layout (after the opcode): 33-byte prefix (position header, unused by the
/// server), `byte 1` command count, one command-0 fragment (14 bytes), `byte 0`
/// X (ceil(0/2)=0 skipped bytes), then exactly 8 trailing bytes.
pub fn move_player(m: &Movement) -> Packet {
    let mut p = Packet::new(send::MOVE_PLAYER);
    p.skip(33);
    p.write_u8(1); // command count
    p.write_u8(0); // command = normal move
    p.write_i16(m.xpos);
    p.write_i16(m.ypos);
    p.write_i16(m.xwobble);
    p.write_i16(m.ywobble);
    p.write_i16(m.unk);
    p.write_u8(m.newstate);
    p.write_i16(m.duration);
    p.write_u8(0); // X -> ceil(0/2) = 0 skipped bytes
    p.skip(8); // trailing block (exactly 8 bytes)
    p
}

/// MOVE_LIFE (0xB7) — client→server: pull a mob toward a point ("gather").
///
/// Layout per `MobHandler.MoveMonster`:
/// `int oid; short moveid; byte useSkill; byte skill; int unk2;
///  [when useSkill: a skill block the server randomizes itself; the client may omit it];
///  byte; int unk3; int; int; [when unk3==18: short n + n bytes];
///  short sx; short sy (startPos = the mob's current position as the server has it);
///  movement: byte count; fragments...; byte X; ceil(X/2) bytes`.
///
/// The server computes the displacement from `startPos` to the move target
/// (MOB_VAC detection: reduce_x > 200 || reduce_y > 150 counts as a gather
/// violation), so gathering must move in small steps.
///
/// Fixed field values (to avoid script-pattern detection):
/// moveid increments, skill=0xFF, unk3=1, int1=int2=0x00FFDDCC;
/// these fields have no protocol function, but always-0 values are a bot tell.
pub fn move_monster(oid: i32, moveid: i16, sx: i16, sy: i16, tx: i16, ty: i16) -> Packet {
    let mut p = Packet::new(send::MOVE_LIFE);
    p.write_i32(oid);
    p.write_i16(moveid);
    p.write_u8(0); // useSkill
    p.write_u8(0xFF); // skill (fixed value)
    p.write_i32(0); // unk2
    p.write_u8(0); // byte
    p.write_i32(1); // unk3 (fixed value)
    p.write_i32(0x00FFDDCC); // fixed magic value
    p.write_i32(0x00FFDDCC);
    p.write_i16(sx); // startPos = the mob's current coords (server-side)
    p.write_i16(sy);
    p.write_u8(1); // movement count
    p.write_u8(0); // command 0 = normal move
    p.write_i16(tx);
    p.write_i16(ty);
    p.write_i16(0); // xwobble
    p.write_i16(0); // ywobble
    p.write_i16(0); // unk
    p.write_u8(2); // newstate: walk
    p.write_i16(1); // duration
    p.write_u8(0); // X -> ceil(0/2) = 0
    p.skip(8); // trailing block (server checks available()==8, same as the player move packet)
    p
}

