use crate::packet::Cursor;

/// One movement fragment from a movement broadcast, mirroring
/// `StaticLifeMovement.serialize` (the layout the server writes).
#[derive(Debug, Clone, Copy)]
pub struct Fragment {
    pub command: u8,
    pub x: i16,
    pub y: i16,
    pub has_position: bool,
    pub newstate: u8,
    pub duration: i16,
}

/// Parse a movement list (`serializeMovementList` layout): `byte count;
/// fragments...`. Returns the fragments.
pub fn parse_movement_list(recv: &mut Cursor<'_>) -> Vec<Fragment> {
    let count = recv.read_u8();
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let command = recv.read_u8();
        let (x, y, has_position) = match command {
            0 | 5 | 15 | 17 => {
                let p = recv.read_point();
                recv.skip_point(); // pixelsPerSecond
                recv.skip_short(); // unk
                if command == 15 {
                    recv.skip_short(); // fh
                }
                (p.0, p.1, true)
            }
            1 | 2 | 6 | 12 | 13 | 16 | 18 | 19 | 22 => {
                recv.skip_point(); // pixelsPerSecond
                (0, 0, false)
            }
            3 | 4 | 7 | 8 | 9 | 11 => {
                let p = recv.read_point();
                recv.skip_short(); // unk
                (p.0, p.1, true)
            }
            10 => {
                recv.skip_byte(); // wui
                (0, 0, false)
            }
            14 => {
                recv.skip_point(); // pixelsPerSecond
                recv.skip_short(); // fh
                (0, 0, false)
            }
            _ => (0, 0, false),
        };

        if command == 10 {
            // Only the wui byte is written for command 10.
            out.push(Fragment {
                command,
                x,
                y,
                has_position,
                newstate: 0,
                duration: 0,
            });
            continue;
        }

        let newstate = recv.read_u8();
        let duration = recv.read_i16();
        out.push(Fragment {
            command,
            x,
            y,
            has_position,
            newstate,
            duration,
        });
    }
    out
}

/// Parse MOVE_PLAYER broadcast (0xBB): `int cid; int 0; movement list`.
/// Returns (cid, final position, stance).
pub fn parse_move_player(recv: &mut Cursor<'_>) -> (i32, Option<(i16, i16)>, u8) {
    let cid = recv.read_i32();
    recv.skip_int(); // 0
    let frags = parse_movement_list(recv);

    let mut pos = None;
    let mut stance = 0;
    for f in frags {
        if f.has_position {
            pos = Some((f.x, f.y));
        }
        stance = f.newstate;
    }
    (cid, pos, stance)
}

/// Parse MOVE_MONSTER broadcast (0xF1), `MobPacket.moveMonster` boolean overload:
/// `int oid; byte 0; byte useskill; byte skill; int unk; short startX;
/// Parse MOVE_MONSTER (0xF1) broadcast. Layout per `MobPacket.moveMonster`
/// (boolean overload): `int oid; byte 0; byte useskill; byte skill; int unk;
///  short startX; short startY; movement list`. Returns (oid, final position)
/// where the final position is the last absolute-position fragment (the server
/// updates the monster via `MovementParse.updatePosition` the same way).
pub fn parse_mob_move(recv: &mut Cursor<'_>) -> (i32, Option<(i16, i16)>) {
    let oid = recv.read_i32();
    recv.skip(1); // 0
    recv.skip(1); // useskill
    recv.skip(1); // skill (byte)
    recv.skip_int(); // unk
    recv.skip_point(); // start position

    let mut pos = None;
    for f in parse_movement_list(recv) {
        if f.has_position {
            pos = Some((f.x, f.y));
        }
    }
    (oid, pos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::Packet;

    #[test]
    fn move_player_parses_position_and_stance() {
        // Server `movePlayer` layout: int cid + int 0 + movement list.
        let mut p = Packet::new(0);
        p.write_i32(3953);
        p.write_i32(0);
        p.write_u8(1); // command count
        p.write_u8(0); // command = absolute move
        p.write_i16(100);
        p.write_i16(214);
        p.write_i16(0); // xwobble
        p.write_i16(0); // ywobble
        p.write_i16(0); // unk
        p.write_u8(4); // newstate
        p.write_i16(100); // duration

        let mut c = Cursor::new(&p.as_bytes()[2..]);
        let (cid, pos, stance) = parse_move_player(&mut c);
        assert_eq!(cid, 3953);
        assert_eq!(pos, Some((100, 214)));
        assert_eq!(stance, 4);
    }

    #[test]
    fn mob_move_parses_final_position() {
        // Server `MobPacket.moveMonster` (boolean overload): int oid; byte 0;
        // byte useskill; byte skill; int unk; short startX; short startY;
        // movement list with one absolute fragment.
        let mut p = Packet::new(0);
        p.write_i32(50001); // oid
        p.write_u8(0);
        p.write_u8(1); // useskill
        p.write_u8(0); // skill (byte)
        p.write_i32(0); // unk
        p.write_i16(90); // startX
        p.write_i16(214); // startY
        p.write_u8(1); // fragment count
        p.write_u8(0); // command 0 = absolute
        p.write_i16(120); // x
        p.write_i16(150); // y (moved)
        p.write_i16(0); // xwobble
        p.write_i16(0); // ywobble
        p.write_i16(0); // unk
        p.write_u8(6); // newstate (fall)
        p.write_i16(100); // duration

        let mut c = Cursor::new(&p.as_bytes()[2..]);
        let (oid, pos) = parse_mob_move(&mut c);
        assert_eq!(oid, 50001);
        assert_eq!(pos, Some((120, 150)));
        assert!(!c.failed());
    }
}