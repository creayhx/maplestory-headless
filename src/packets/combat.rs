//! Combat packets for CongMS 079.
//!
//! The client->server attack layout is from `DamageParse.parseDmgM`; the server
//! recomputes damage authoritatively, so the bot can send placeholder damage.

use crate::opcodes::send;
use crate::packet::Packet;

/// CLOSE_RANGE_ATTACK (0x28) — melee attack against one target, one hit.
///
/// Layout (after the opcode, per `DamageParse.parseDmgM`):
/// `skip(1) + skip(8) + byte tbyte(targets<<4|hits) + skip(8) + int skill
///  + skip(12) + byte unk + byte display + byte animation + skip(1)
///  + byte speed + int lastAttackTick + int oid + skip(14) + hits × int damage
///  + skip(4) + short x + short y`.
///
/// The server relays `display` and `animation` verbatim in the 0xBC broadcast;
/// receiving clients parse them as `toleft` (facing) and `stance` (attack
/// animation selector) respectively. `stance` must be a valid `Stance::Id`
/// (1..36, e.g. 23 = swingO1 one-handed sword swing), otherwise clients fall
/// back to the default action and the attack animation/effects don't play.
pub fn close_attack(oid: i32, x: i16, y: i16, damage: i32, facing: u8) -> Packet {
    let mut p = Packet::new(send::CLOSE_RANGE_ATTACK);
    p.skip(1);
    p.skip(8);
    p.write_u8(0x11); // tbyte: 1 target, 1 hit
    p.skip(8);
    p.write_i32(0); // skill
    p.skip(12);
    p.write_u8(0); // unk
    p.write_u8(facing & 1); // display: toleft (0 = right, 1 = left)
    p.write_u8(23); // animation: stance 23 = swingO1 (valid Stance::Id)
    p.skip(1);
    p.write_u8(4); // speed
    p.write_time(); // lastAttackTick
    p.write_i32(oid);
    p.skip(14);
    p.write_i32(damage);
    p.skip(4);
    p.write_point(x, y);
    p
}

/// CLOSE_RANGE_ATTACK (0x28) — multi-target skill attack (e.g. Slash Blast
/// 1001005), `hits` hits per target (stages: e.g. a 2-stage AoE sends 2 damage
/// values per target). `targets` is a list of `(oid, per-hit damages)` pairs:
/// each stage's damage is picked independently by the caller within the damage
/// range (e.g. a 2-stage AoE capped at 1200 = 1080~1200 per stage, each under
/// the per-stage validation cap).
///
/// Same layout as `close_attack`, with `skill` set and one `oid + skip(14) +
/// hits × int damage + skip(4)` block per target (matches `DamageParse.parseDmgM`'s
/// per-target loop; the tbyte low nibble tells the parser how many damage ints
/// to read per target).
pub fn skill_attack(
    skill: i32,
    hits: u8,
    targets: &[(i32, Vec<i32>)],
    x: i16,
    y: i16,
    facing: u8,
) -> Packet {
    let mut p = Packet::new(send::CLOSE_RANGE_ATTACK);
    p.skip(1);
    p.skip(8);
    let n = targets.len().min(0xF) as u8;
    let hits = hits.clamp(1, 0xF);
    p.write_u8((n << 4) | hits); // tbyte: n targets, hits per target
    p.skip(8);
    p.write_i32(skill);
    p.skip(12);
    p.write_u8(0); // unk
    p.write_u8(facing & 1); // display: toleft (0 = right, 1 = left)
    p.write_u8(23); // animation: stance 23 = swingO1 (valid Stance::Id)
    p.skip(1);
    p.write_u8(4); // speed
    p.write_time(); // lastAttackTick
    // tbyte caps targets at 15: the body must match the header count, or the
    // server's parseDmgM reads misaligned bytes and the whole packet is rejected.
    for &(oid, ref dmgs) in targets.iter().take(n as usize) {
        p.write_i32(oid);
        p.skip(14);
        for i in 0..hits {
            p.write_i32(dmgs.get(i as usize).copied().unwrap_or_default());
        }
        p.skip(4);
    }
    p.write_point(x, y);
    p
}

/// DAMAGE_REACTOR (0xC9) — hit a map reactor.
///
/// Layout (per `PlayersHandler.HitReactor`): `int oid + int charPos + short
/// stance`. The server only checks the reactor exists and is alive — no
/// distance/cooldown/skill validation, so this works from anywhere on the map.
/// `charPos` is the client-side player-side marker (only checked for type-2
/// reactors, where 0/2 are accepted); 0 is safe for normal reactors. `stance`
/// is the attack stance used in the broadcast animation (4 = swing).
///
/// The official client appends `short 0 + short playerX + int 0` after stance
/// (verified from the private-server capture: 20-byte packet, the middle short
/// tracks the character's local x). Servers that read past the base fields
/// underflow on the truncated 12-byte form and drop the connection, so send the
/// full layout with the player's current x.
pub fn damage_reactor(oid: i32, char_pos: i32, stance: i16, px: i16) -> Packet {
    let mut p = Packet::new(send::DAMAGE_REACTOR);
    p.write_i32(oid);
    p.write_i32(char_pos);
    p.write_i16(stance);
    p.write_i16(0);
    p.write_i16(px);
    p.write_i32(0);
    p
}

/// SPECIAL_MOVE (0x58) — cast a buff/assist skill (no target).
///
/// Layout (per `PlayerHandler.SpecialMove`): `skip(4) + int skillid + byte
/// skillLevel + [short x + short y + byte faceLeft]`. The server validates
/// `skillLevel` against the character's real skill level, so callers must
/// pass the level from the skills table. `faceLeft` is 0 = facing left,
/// 1 = facing right (bot facing: 0 = right, 1 = left).
pub fn special_move(skillid: i32, level: u8, x: i16, y: i16, facing: u8) -> Packet {
    let mut p = Packet::new(send::SPECIAL_MOVE);
    p.skip(4);
    p.write_i32(skillid);
    p.write_u8(level);
    p.write_point(x, y);
    p.write_u8(if facing & 1 == 1 { 0 } else { 1 }); // faceLeft: 0 = left
    p
}

/// CANCEL_BUFF (0x59) — cancel a buff by skill id (per `PlayerHandler.
/// CancelBuffHandler`): `int sourceid`.
pub fn cancel_buff(skillid: i32) -> Packet {
    let mut p = Packet::new(send::CANCEL_BUFF);
    p.write_i32(skillid);
    p
}

/// CHANGE_KEYMAP (0x83) — rebind keys (per `PlayerHandler.ChangeKeymap`):
/// `int tick + int count + [int key + byte type + int action] × count`.
/// The server does not validate bindings, so any key/type/action is accepted.
pub fn change_keymap(bindings: &[(i32, u8, i32)]) -> Packet {
    let mut p = Packet::new(send::CHANGE_KEYMAP);
    p.write_time(); // tick
    p.write_i32(bindings.len() as i32);
    for &(key, ty, action) in bindings {
        p.write_i32(key);
        p.write_u8(ty);
        p.write_i32(action);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_buff_layout_matches_cancel_buff_handler() {
        let bytes = cancel_buff(1001004).into_bytes();
        assert_eq!(&bytes[0..2], &[0x59, 0x00]); // opcode
        assert_eq!(&bytes[2..6], &[0x2C, 0x46, 0x0F, 0x00]); // 1001004
        assert_eq!(bytes.len(), 2 + 4);
    }

    #[test]
    fn change_keymap_layout_matches_change_keymap_handler() {
        let bindings = [(2, 1u8, 1001004), (31, 2u8, 2000002)];
        let bytes = change_keymap(&bindings).into_bytes();
        assert_eq!(&bytes[0..2], &[0x83, 0x00]); // opcode
        assert_eq!(&bytes[6..10], &[2, 0, 0, 0]); // count
        // binding 1: key=2 type=1 action=1001004
        assert_eq!(&bytes[10..14], &[2, 0, 0, 0]);
        assert_eq!(bytes[14], 1);
        assert_eq!(&bytes[15..19], &[0x2C, 0x46, 0x0F, 0x00]);
        // binding 2: key=31 type=2 action=2000002
        assert_eq!(&bytes[19..23], &[31, 0, 0, 0]);
        assert_eq!(bytes[23], 2);
        assert_eq!(&bytes[24..28], &[0x82, 0x84, 0x1E, 0x00]);
        assert_eq!(bytes.len(), 2 + 4 + 4 + 2 * 9);
    }

    #[test]
    fn special_move_layout_matches_skill_effect_handler() {
        let bytes = special_move(1001004, 5, 100, 214, 0).into_bytes();
        assert_eq!(&bytes[0..2], &[0x58, 0x00]); // opcode
        assert_eq!(&bytes[6..10], &[0x2C, 0x46, 0x0F, 0x00]); // 1001004
        assert_eq!(bytes[10], 5); // level
        assert_eq!(&bytes[11..13], &[100, 0]); // x
        assert_eq!(&bytes[13..15], &[214, 0]); // y
        assert_eq!(bytes[15], 1); // faceLeft: facing 0 (right) -> 1
    }

    #[test]
    fn special_move_facing_left_writes_zero() {
        let bytes = special_move(1001004, 5, 0, 0, 1).into_bytes();
        assert_eq!(bytes[15], 0); // facing 1 (left) -> faceLeft byte 0
    }

    #[test]
    fn damage_reactor_layout_matches_hit_reactor() {
        let bytes = damage_reactor(50001, 0, 4, 100).into_bytes();
        assert_eq!(&bytes[0..2], &[0xC9, 0x00]); // opcode
        assert_eq!(&bytes[2..6], &[0x51, 0xC3, 0x00, 0x00]); // oid
        assert_eq!(&bytes[6..10], &[0, 0, 0, 0]); // charPos
        assert_eq!(&bytes[10..12], &[4, 0]); // stance
        // append short 0 + short playerX + int 0 after stance (20B total);
        // the server reads the full block — a truncated packet drops the connection.
        assert_eq!(&bytes[12..14], &[0, 0]); // short 0
        assert_eq!(&bytes[14..16], &[100, 0]); // playerX
        assert_eq!(&bytes[16..20], &[0, 0, 0, 0]); // int 0
        assert_eq!(bytes.len(), 20);
    }

    #[test]
    fn damage_reactor_matches_official_capture() {
        // (oid=100005 charPos=0 stance=2 short0 x=449 int0)
        let bytes = damage_reactor(100005, 0, 2, 449).into_bytes();
        assert_eq!(
            bytes,
            vec![
                0xC9, 0x00, 0xA5, 0x86, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00,
                0x00, 0x00, 0xC1, 0x01, 0x00, 0x00, 0x00, 0x00,
            ]
        );
    }

    #[test]
    fn skill_attack_layout_matches_parse_dmg_m() {
        // Two targets, skill 1001005 (Slash Blast), one hit each.
        let targets = [(50001, vec![2400]), (50002, vec![2400])];
        let bytes = skill_attack(1001005, 1, &targets, 100, 214, 0).into_bytes();

        assert_eq!(&bytes[0..2], &[0x28, 0x00]); // opcode
        // skip(1)+skip(8) = 9 bytes after opcode -> tbyte at offset 11
        assert_eq!(bytes[11], 0x21); // 2 targets << 4 | 1 hit
        // skill after skip(8): offset 20..24 == 0xF462D LE
        assert_eq!(&bytes[20..24], &[0x2D, 0x46, 0x0F, 0x00]); // 1001005
        // target oids at 45..49 and 71..75 (skip(12)+4 control+time then per-target)
        assert_eq!(&bytes[45..49], &[0x51, 0xC3, 0x00, 0x00]); // 50001
        assert_eq!(&bytes[71..75], &[0x52, 0xC3, 0x00, 0x00]); // 50002
        // position written last
        assert_eq!(&bytes[bytes.len() - 4..], &[100u8, 0, 214, 0]);
    }

    #[test]
    fn skill_attack_survives_full_parse_dmg_m_simulation() {
        // Simulate DamageParse.parseDmgM field-by-field over our packet to prove
        // the server reads the exact values we intend (this is the crux of the
        // "skill deals no damage" investigation).
        let targets = [(50001, vec![180]), (50002, vec![180]), (50003, vec![180])];
        let bytes = skill_attack(1001005, 1, &targets, 300, 214, 0).into_bytes();
        let b = &bytes[2..]; // skip opcode

        let mut p = 0;
        let skip = |n: &mut usize, c: usize| *n += c;
        skip(&mut p, 1);
        skip(&mut p, 8);
        let tbyte = b[p];
        p += 1;
        skip(&mut p, 8);
        let skill = i32::from_le_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]]);
        p += 4;
        skip(&mut p, 12);
        let unk = b[p];
        p += 1;
        let display = b[p];
        p += 1;
        let animation = b[p];
        p += 1;
        p += 1; // skip(1)
        let speed = b[p];
        p += 1;
        let _tick = i32::from_le_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]]);
        p += 4;

        assert_eq!(tbyte >> 4, 3, "targets");
        assert_eq!(tbyte & 0xF, 1, "hits");
        assert_eq!(skill, 1001005, "skill");
        assert_eq!(unk, 0);
        assert_eq!(display, 0, "facing 0 = toleft 0 (right)");
        assert_eq!(animation, 23, "stance = swingO1 (valid Stance::Id)");
        assert_eq!(speed, 4);

        let mut oids = Vec::new();
        for _ in 0..(tbyte >> 4) {
            let oid = i32::from_le_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]]);
            p += 4;
            p += 14;
            let dmg = i32::from_le_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]]);
            p += 4;
            p += 4;
            oids.push((oid, dmg));
        }
        assert_eq!(oids, vec![(50001, 180), (50002, 180), (50003, 180)]);
        // position tail
        assert_eq!(b.len() - p, 4);
        let x = i16::from_le_bytes([b[p], b[p + 1]]);
        let y = i16::from_le_bytes([b[p + 2], b[p + 3]]);
        assert_eq!((x, y), (300, 214));
    }

    #[test]
    fn skill_attack_multi_hit_writes_per_hit_damage() {
        // 2-stage AoE: tbyte low nibble = 2, 2 int damages per target
        // (each stage picked independently, all within the damage range).
        let targets = [(50001, vec![1190, 1080]), (50002, vec![1150, 1110])];
        let bytes = skill_attack(2001004, 2, &targets, 100, 214, 0).into_bytes();

        assert_eq!(bytes[11], 0x22, "tbyte: 2 targets, 2 hits");
        // per-target block: oid(4) + skip(14) + hits×int + skip(4)
        // target 1 damages at 63..71: 1190, 1080 (written as-is, not split)
        assert_eq!(i32::from_le_bytes([bytes[63], bytes[64], bytes[65], bytes[66]]), 1190);
        assert_eq!(i32::from_le_bytes([bytes[67], bytes[68], bytes[69], bytes[70]]), 1080);
        // target 2 damages: 1150, 1110
        assert_eq!(i32::from_le_bytes([bytes[93], bytes[94], bytes[95], bytes[96]]), 1150);
        assert_eq!(i32::from_le_bytes([bytes[97], bytes[98], bytes[99], bytes[100]]), 1110);
    }

    #[test]
    fn facing_goes_to_display_not_stance() {
        // clients parse the broadcast display byte as `toleft` and the
        // animation byte as `stance`; facing must land in display so the
        // attack mirrors correctly and the stance byte stays valid.
        let bytes = skill_attack(1001005, 1, &[(50001, vec![100])], 100, 214, 1).into_bytes();
        let b = &bytes[2..];
        // display offset: skip(1)+skip(8)+tbyte+skip(8)+skill+skip(12)+unk = 35
        assert_eq!(b[35], 1, "facing 1 (left) -> display/toleft 1");
        assert_eq!(b[36], 23, "stance stays swingO1");
        let bytes = skill_attack(1001005, 1, &[(50001, vec![100])], 100, 214, 0).into_bytes();
        let b = &bytes[2..];
        assert_eq!(b[35], 0, "facing 0 (right) -> display/toleft 0");
    }
}



