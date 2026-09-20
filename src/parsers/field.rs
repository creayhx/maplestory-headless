use crate::packet::Cursor;
use crate::state::{Entity, EntityKind, Item, SkillEntry};

use std::collections::BTreeMap;

use super::inventory::{parse_equipped_block, parse_inventory_block};
use super::login::parse_look;
use super::skill::parse_skill_info;

/// Parse the stats block (`PacketHelper.addCharStats`).
pub fn parse_stats(recv: &mut Cursor<'_>) -> crate::state::StatsEntry {
    let mut s = crate::state::StatsEntry::default();

    s.name = recv.read_padded_string_gb(13);
    s.female = recv.read_bool();
    let skin = recv.read_u8();
    let face = recv.read_i32();
    let hair = recv.read_i32();

    for _ in 0..3 {
        recv.skip_long(); // pet ids
    }

    s.level = recv.read_u8();
    s.job = recv.read_i16();
    s.str = recv.read_i16();
    s.dex = recv.read_i16();
    s.int = recv.read_i16();
    s.luk = recv.read_i16();
    s.hp = recv.read_i16();
    s.maxhp = recv.read_i16();
    s.mp = recv.read_i16();
    s.maxmp = recv.read_i16();
    s.ap = recv.read_i16();
    s.sp = recv.read_i16();
    s.exp = recv.read_i32();
    s.fame = recv.read_i16();

    recv.skip_int(); // gachaexp
    recv.skip_long(); // timestamp (getTime)

    s.mapid = recv.read_i32();
    s.portal = recv.read_u8();

    let _ = (skin, face, hair);
    s
}

/// Skip a monster-status block (`MobPacket.addMonsterStatus`). Returns false
/// when the mob has non-EMPTY statuses whose payload length we cannot
/// determine, so the caller should not trust the position parse.
/// Status block parse result (single-pass, no backtracking).
enum StatusKind {
    /// Standard format:  mask all zeros, no payload bytes
    Plain,
    /// Standard format:  mask has only the EMPTY bit (1<<27), followed by 4 bytes
    Empty,
    /// Variant server: mask = [0,0,0,0x88000000]
    /// (status area: 15 zero bytes + 0x88 value in 16-byte LE form)
    ThirdParty,
}

/// Read the 4×u32 status mask and decide the layout directly, no backtracking:
///
/// - Standard format with no statuses: mask all zeros (Plain).
/// - Standard format with EMPTY (`addEmpty`): mask = 1<<27, followed by 4 bytes (Empty).
/// - Variant server: mask = [0,0,0,0x88000000] — this layout writes the status
///   area as 15 zero bytes + a single `0x88` value, whose 16-byte LE form is
///   exactly EMPTY|bit31. bit31 has no definition in the standard format, so a
///   standard server can never emit it — it's a deterministic signature of
///   this layout (no length probing or second parse needed).
/// - Any other mask = real statuses (variable payload length), return None.
fn skip_monster_status(recv: &mut Cursor<'_>) -> Option<StatusKind> {
    let mut mask = [0u32; 4];
    for m in mask.iter_mut() {
        *m = recv.read_u32();
    }

    // Variant server signature: 15×00 + 0x88 → LE u32 = 0x88000000
    if mask == [0, 0, 0, 0x8800_0000] {
        return Some(StatusKind::ThirdParty);
    }

    // MonsterStatus.EMPTY: i=27 -> pos = 3 - floor(27/32) = 3, value = 1 << 27.
    // `addEmpty` is always applied to mobs without statuses, so the EMPTY bit
    // is normally set and is followed by a single int (4 bytes).
    const EMPTY_BIT: u32 = 1 << 27;
    let has_empty = mask[3] & EMPTY_BIT != 0;
    let mut rest = mask;
    if has_empty {
        rest[3] &= !EMPTY_BIT;
    }

    if rest.iter().any(|m| *m != 0) {
        // Real statuses present; their payload length varies by status type.
        return None;
    }

    if has_empty {
        recv.skip_int();
        return Some(StatusKind::Empty);
    }
    Some(StatusKind::Plain)
}

/// Parse SPAWN_MONSTER_CONTROL (0xF0):
/// `byte aggro; int oid; byte 1; int mobid; [status]; short x; short y;
///  byte stance; short fh; short fh; byte fake; byte team; int 0`.
/// Returns None if the mob has statuses we can't size.
/// Special case: aggro=0 means the server released control of the mob
/// (`byte 0 + int oid`, e.g. stopControllingMonster / makeMonsterInvisible).
/// That is signalled back with a zero-mobid Entity so the caller can drop it.
pub fn parse_spawn_monster_control(recv: &mut Cursor<'_>) -> Option<Entity> {
    let aggro = recv.read_u8(); // 1/2 = spawn with control, 0 = release control
    let oid = recv.read_i32();
    if aggro == 0 {
        return Some(Entity {
            oid,
            kind: EntityKind::Mob,
            mobid: 0,
            x: 0,
            y: 0,
            fh: 0,
            stance: 0,
            charname: String::new(),
            hp_pct: None,
            controlled: false,
        });
    }
    recv.skip_byte(); // 1
    let mobid = recv.read_i32();
    let mut e = parse_spawn_tail(recv, oid, mobid)?;
    // aggro=1/2 → the server assigns controller to the bot; only these mobs can be moved/attacked
    e.controlled = true;
    Some(e)
}

/// Parse SPAWN_MONSTER (0xEE):
/// `int oid; byte 1; int mobid; [status]; short x; short y; byte stance;
///  short 0; short fh; ...`.
pub fn parse_spawn_monster(recv: &mut Cursor<'_>) -> Option<Entity> {
    let oid = recv.read_i32();
    recv.skip_byte(); // 1
    let mobid = recv.read_i32();
    parse_spawn_tail(recv, oid, mobid)
}

/// Common tail parsing after mobid, handling both server layouts in one pass:
///
/// 1. Standard format:  status block = 4×u32 mask (Plain = no statuses; the EMPTY
///    bit 1<<27 is followed by 4 bytes), then `short x; short y; byte stance;
///    short 0; short fh`.
/// 2. Variant server 079: status block = 15 zero bytes + a `0x88` value
///    (recognized directly by the mask shape [0,0,0,0x88000000]), then
///    `00 00 00; short x; short y; byte stance; ...` (tail fields not read in detail).
///
/// No backtracking, no length probing: the layout is decided once from the
/// mask's deterministic shape.
fn parse_spawn_tail(recv: &mut Cursor<'_>, oid: i32, mobid: i32) -> Option<Entity> {
    match skip_monster_status(recv) {
        Some(StatusKind::Plain) | Some(StatusKind::Empty) => {
            // standard layout
            let x = recv.read_i16();
            let y = recv.read_i16();
            let stance = recv.read_u8();
            recv.skip_short(); // 0
            let fh = recv.read_i16();
            Some(Entity {
                oid,
                kind: EntityKind::Mob,
                mobid,
                x,
                y,
                fh,
                stance,
                charname: String::new(),
                hp_pct: None,
                controlled: false
            })
        }
        Some(StatusKind::ThirdParty) => {
            // mask consumed 15×0 + 0x88 (first byte); the 0x88 value leaves 3 more bytes + a 3-byte gap
            recv.skip(6);
            let x = recv.read_i16();
            let y = recv.read_i16();
            let stance = recv.read_u8();
            Some(Entity {
                oid,
                kind: EntityKind::Mob,
                mobid,
                x,
                y,
                fh: 0, // fh position varies in this layout; not parsed
                stance,
                charname: String::new(),
                hp_pct: None,
                controlled: false
            })
        }
        None => None,
    }
}

/// Parse the head of SPAWN_PLAYER (0xA2): `int charid; byte level; string name`.
pub fn parse_spawn_player_head(recv: &mut Cursor<'_>) -> (i32, String) {
    let charid = recv.read_i32();
    recv.skip_byte(); // level
    let name = recv.read_string_gb();
    (charid, name)
}

/// Parse DROP_ITEM_FROM_MAPOBJECT (0x110):
/// `byte mod; int oid; byte isMeso; int itemId; int owner; byte dropType;
///  short x; short y; [int owner]; [short fx; short fy; short 0]; [long exp];
///  short playerDrop`. Returns (oid, itemId, isMeso, x, y). For meso drops
/// `itemId` is the meso amount.
pub fn parse_drop(recv: &mut Cursor<'_>) -> Option<(i32, i32, bool, i16, i16)> {
    recv.skip_byte(); // mod
    let oid = recv.read_i32();
    let is_meso = recv.read_u8() != 0;
    let itemid = recv.read_i32();
    recv.skip_int(); // owner
    recv.skip_byte(); // dropType
    let (x, y) = recv.read_point();
    recv.skip(4); // int (dropType == 0 ? owner : 0)
    Some((oid, itemid, is_meso, x, y))
}

/// Parse REMOVE_ITEM_FROM_MAP (0x111): `byte animation; int oid; ...`.
/// Returns the removed drop's oid.
pub fn parse_remove_drop(recv: &mut Cursor<'_>) -> i32 {
    recv.skip_byte(); // animation
    recv.read_i32()
}

/// Best-effort parse of the bot's OWN spawn position from SPAWN_PLAYER (0xA2),
/// mirroring `MaplePacketCreator.spawnPlayerMapobject` up to
/// `writePos(getTruePosition)`. The cursor must already be past the head
/// (`parse_spawn_player_head`).
///
/// Verified against a live capture (268-byte packet): the buff mask is 16 bytes
/// (no payload for an unbuffed character), the fixed block from the mask to the
/// char look is 127 bytes, the look matches `addCharLook` (== `parse_look`),
/// followed by 20 fixed bytes, then `x y stance`. Returns (x, y, stance), or
/// None if the layout doesn't line up (the cursor is non-panicking, so a bad
/// parse just yields garbage or a short read).
pub fn parse_spawn_player_self(recv: &mut Cursor<'_>) -> Option<(i16, i16, u8)> {
    // guild: string + 6 bytes (logo block or zeros)
    recv.skip_string();
    recv.skip(6);
    // buff mask: 4 ints; assume no buff payloads follow (unbuffed character)
    recv.skip(16);
    // fixed block (mask -> addCharLook)
    recv.skip(127);
    // char look
    let _look = parse_look(recv);
    // 20 fixed bytes (cash, itemEffect, 2 strings, 2 shorts, chair)
    recv.skip(20);
    let (x, y) = recv.read_point();
    let stance = recv.read_u8();
    Some((x, y, stance))
}

/// Parse an NPC sendSimple menu (msgType=4): extract each option's id and text.
///
/// Standard format:  option = `#L<id>#<text>#l`, options concatenated,
/// text may contain `\r\n` newlines. Some servers omit the closing `#l`
/// (the next `#L` starts right after the option), so read until the next `#L`
/// or the end of the string.
/// Returns a list of (id, text); empty when there is no menu.
pub fn parse_simple_options(msg: &str) -> Vec<(i32, String)> {
    let mut out = Vec::new();
    let mut rest = msg;
    while let Some(start) = rest.find("#L") {
        rest = &rest[start + 2..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            // not an option (e.g. a #L in plain text); skip and keep looking
            rest = &rest[1..];
            continue;
        }
        let id: i32 = digits.parse().unwrap_or(0);
        rest = &rest[digits.len()..];
        if !rest.starts_with('#') {
            continue;
        }
        rest = &rest[1..];
        // Text ends at "#l" (case-insensitive; the next option's "#L" also matches);
        // if there's no "#l" at all, the text runs to the end of the string.
        let lower = rest.to_ascii_lowercase();
        match lower.find("#l") {
            Some(end) => {
                out.push((id, rest[..end].to_string()));
                // end points at the '#' of "#L" (the next option): keep it, re-parse next round
                rest = &rest[end..];
            }
            None => {
                out.push((id, rest.to_string()));
                break;
            }
        }
    }
    out
}

/// Clean NPC dialog text: strip format tags (keeping only `#L<id>#` menu
/// options), collapse tabs/CR, truncate overlong text — otherwise screens of
/// `#fEffect/...#` `#e#r` tags and huge newline-heavy strings wreck the
/// log/TUI layout.
pub fn clean_npc_text(msg: &str, max_len: usize) -> String {
    let b: Vec<char> = msg.chars().collect();
    let mut out = String::with_capacity(b.len().min(max_len + 8));
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            '\t' => {
                out.push(' ');
                i += 1;
            }
            '\r' => {
                i += 1; // drop \r, keep \n newlines
            }
            '#' => {
                if i + 1 >= b.len() {
                    i += 1;
                    continue;
                }
                let tag = b[i + 1];
                // Keep menu options `#L<id>#` (the option id is the argument for npc reply)
                if tag == 'L' || tag == 'l' {
                    let mut j = i + 2;
                    while j < b.len() && b[j].is_ascii_digit() {
                        j += 1;
                    }
                    if j > i + 2 && j < b.len() && b[j] == '#' {
                        out.push_str("#L");
                        out.extend(&b[i + 2..j]);
                        out.push('#');
                        i = j + 1;
                        continue;
                    }
                    // Incomplete #L/#l (incl. the menu terminator "#l") → drop the whole tag
                    i += 2;
                    continue;
                }
                // Delete image/item tags `#fpath#` / `#v<id>#` / `#t<id>#` entirely
                if tag == 'f' || tag == 'v' || tag == 't' {
                    match (i + 2..b.len()).find(|&k| b[k] == '#') {
                        Some(k) => {
                            i = k + 1;
                            continue;
                        }
                        None => break,
                    }
                }
                // Single-char format tags (#e #b #r #k #n #d #p #g #z etc.) → delete
                i += 2;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
        if out.chars().count() > max_len {
            break;
        }
    }
    let count = out.chars().count();
    if count > max_len {
        out = out.chars().take(max_len).collect();
        out.push('…');
    }
    out
}

/// A target (and its damage numbers) from an attack broadcast.
#[derive(Debug, Clone)]
pub struct AttackTarget {
    pub oid: i32,
    pub hits: Vec<i32>,
}

/// A parsed attack broadcast.
#[derive(Debug, Clone)]
pub struct AttackBroadcast {
    pub cid: i32,
    pub skill: i32,
    pub targets: Vec<AttackTarget>,
}

/// Parse CLOSE_RANGE_ATTACK broadcast (0xBC):
/// `int cid; byte tbyte; byte lvl; [byte level; int skill] | byte 0;
///  byte unk; byte display; byte animation; byte speed; byte mastery; int 0;
///  per target: int oid; byte 7; hits × int damage`.
pub fn parse_attack_broadcast(recv: &mut Cursor<'_>) -> AttackBroadcast {
    let cid = recv.read_i32();
    let tbyte = recv.read_u8();
    let targets = (tbyte >> 4) as usize;
    let hits = (tbyte & 0xF) as usize;

    recv.skip(1); // lvl
    let mut skill = 0;
    let level_byte = recv.read_u8();
    if level_byte != 0 {
        skill = recv.read_i32();
    }

    recv.skip(1); // unk
    recv.skip(1); // display
    recv.skip(1); // animation
    recv.skip(1); // speed
    recv.skip(1); // mastery
    recv.skip_int(); // 0

    let mut targets_out = Vec::with_capacity(targets);
    for _ in 0..targets {
        let oid = recv.read_i32();
        recv.skip(1); // 7
        let mut dmg = Vec::with_capacity(hits);
        for _ in 0..hits {
            dmg.push(recv.read_i32());
        }
        targets_out.push(AttackTarget { oid, hits: dmg });
    }

    AttackBroadcast {
        cid,
        skill,
        targets: targets_out,
    }
}

/// HP/MP/EXP/level changes parsed from UPDATE_STATS (0x22).
#[derive(Debug, Clone, Copy, Default)]
pub struct StatsUpdate {
    pub hp: Option<i16>,
    pub maxhp: Option<i16>,
    pub mp: Option<i16>,
    pub maxmp: Option<i16>,
    pub exp: Option<i32>,
    pub level: Option<u8>,
    pub str: Option<i16>,
    pub dex: Option<i16>,
    pub int: Option<i16>,
    pub luk: Option<i16>,
    /// remaining ability points (AVAILABLEAP bit)
    pub ap: Option<i16>,
    /// remaining skill points (AVAILABLESP bit)
    pub sp: Option<i16>,
    /// current meso (MESO bit)
    pub meso: Option<i32>,
}

/// Parse UPDATE_STATS (0x22): `byte itemReaction; int mask; values...`.
/// Values follow `MaplePacketCreator.updatePlayerStats`, iterated in the
/// `MapleStat` enum declaration order (EnumMap iteration order).
pub fn parse_stats_update(recv: &mut Cursor<'_>) -> StatsUpdate {
    recv.skip_byte(); // itemReaction
    parse_stats_update_after_reaction(recv)
}

/// Parse stats update without skipping the itemReaction byte (already read).
pub fn parse_stats_update_after_reaction(recv: &mut Cursor<'_>) -> StatsUpdate {
    let mask = recv.read_u32();
    let mut out = StatsUpdate::default();

    macro_rules! has {
        ($b:expr) => {
            mask & ($b) != 0
        };
    }
    if has!(1) {
        recv.skip(1);
    } // SKIN byte
    if has!(2) {
        recv.skip_int();
    } // FACE
    if has!(4) {
        recv.skip_int();
    } // HAIR
    if has!(64) {
        out.level = Some(recv.read_u8());
    } // LEVEL byte
    if has!(128) {
        recv.skip_short();
    } // JOB
    if has!(256) {
        out.str = Some(recv.read_i16());
    } // STR
    if has!(512) {
        out.dex = Some(recv.read_i16());
    } // DEX
    if has!(1024) {
        out.int = Some(recv.read_i16());
    } // INT
    if has!(2048) {
        out.luk = Some(recv.read_i16());
    } // LUK
    if has!(4096) {
        out.hp = Some(recv.read_i16());
    }
    if has!(8192) {
        out.maxhp = Some(recv.read_i16());
    }
    if has!(16384) {
        out.mp = Some(recv.read_i16());
    }
    if has!(32768) {
        out.maxmp = Some(recv.read_i16());
    }
    if has!(65536) {
        out.ap = Some(recv.read_i16());
    } // AVAILABLEAP
    if has!(131072) {
        out.sp = Some(recv.read_i16());
    } // AVAILABLESP
    if has!(262144) {
        out.exp = Some(recv.read_i32());
    } // EXP
    if has!(524288) {
        recv.skip_short();
    } // FAME
    if has!(0x100000) {
        out.meso = Some(recv.read_i32());
    } // MESO
    if mask & 56 != 0 {
        recv.skip(24);
    } // PET: 3 longs
    if has!(0x200000) {
        recv.skip_int();
    } // GACHAEXP

    out
}

/// Character info parsed from the SET_FIELD charinfo packet (mode2 == 1),
/// including the full inventory written by `addInventoryInfo`.
#[derive(Debug, Clone)]
pub struct CharInfo {
    pub id: i32,
    pub name: String,
    pub level: u8,
    pub job: i16,
    pub str: i16,
    pub dex: i16,
    pub int: i16,
    pub luk: i16,
    pub hp: i16,
    pub maxhp: i16,
    pub mp: i16,
    pub maxmp: i16,
    pub ap: i16,
    /// remaining skill points (from addCharStats)
    pub sp: i16,
    pub exp: i32,
    pub meso: i32,
    pub inventory: BTreeMap<(u8, i16), Item>,
    /// worn equipment keyed by body slot (negative), from the two equipped
    /// blocks of addInventoryInfo
    pub equipped: BTreeMap<i16, Item>,
    /// learned skills keyed by skillid (from addSkillInfo)
    pub skills: BTreeMap<i32, SkillEntry>,
}

/// Parse the common `PacketHelper.addCharacterInfo` body (everything after the
/// leading `long -1; byte 0`): `addCharStats; byte buddylist; [bless];
/// addInventoryInfo`. The caller handles the (non-)CS variants that follow the
/// inventory block itself (addSkillInfo vs short 0).
pub fn parse_character_info_body(recv: &mut Cursor<'_>) -> Option<CharInfo> {
    // addCharStats (all fields are fixed-length, verified against source).
    let id = recv.read_i32();
    let name = recv.read_padded_string_gb(13);
    recv.skip(1); // gender
    recv.skip(1); // skin
    recv.skip(4); // face
    recv.skip(4); // hair
    recv.skip(24); // pet ids
    let level = recv.read_u8();
    let job = recv.read_i16();
    let st = recv.read_i16();
    let d = recv.read_i16();
    let it = recv.read_i16();
    let lk = recv.read_i16();
    let hp = recv.read_i16();
    let maxhp = recv.read_i16();
    let mp = recv.read_i16();
    let maxmp = recv.read_i16();
    let ap = recv.read_i16();
    let sp = recv.read_i16();
    let exp = recv.read_i32();
    recv.skip(2); // fame
    recv.skip(4); // int 0
    recv.skip(8); // long time
    recv.skip(4); // mapid
    recv.skip(1); // spawnpoint
    if recv.failed() {
        return None;
    }
    recv.skip(1); // buddylist capacity
    if recv.read_u8() == 1 {
        recv.skip_string(); // bless of fairy origin
    }
    // addInventoryInfo
    let meso = recv.read_i32();
    recv.skip(4); // charid
    recv.skip(4); // beans
    recv.skip(4); // int 0
    recv.skip(5); // slot limits
    recv.skip(8); // long
    if recv.failed() {
        return None;
    }

    let mut inventory = BTreeMap::new();
    let mut equipped = BTreeMap::new();
    parse_equipped_block(recv, 0, &mut equipped); // equipped -1..-99
    parse_equipped_block(recv, 100, &mut equipped); // cash-equipped -100..-999
    parse_inventory_block(recv, 1, Some(&mut inventory)); // EQUIP
    parse_inventory_block(recv, 2, Some(&mut inventory)); // USE
    parse_inventory_block(recv, 3, Some(&mut inventory)); // SETUP
    parse_inventory_block(recv, 4, Some(&mut inventory)); // ETC
    parse_inventory_block(recv, 5, None); // CASH (items may be pets; discard)

    Some(CharInfo {
        id,
        name,
        level,
        job,
        str: st,
        dex: d,
        int: it,
        luk: lk,
        hp,
        maxhp,
        mp,
        maxmp,
        ap,
        sp,
        exp,
        meso,
        inventory,
        equipped,
        skills: BTreeMap::new(),
    })
}

/// Parse the charinfo body that follows `int channel; byte mode1; byte mode2`
/// in SET_FIELD. Layout (verified against `getCharInfo` /
/// `PacketHelper.addCharacterInfo`): `byte 1; short 0; CRand(3 ints);
///  long -1; byte 0; addCharStats; byte buddylist; [bless]; addInventoryInfo`.
/// Returns None if the fixed header doesn't line up.
pub fn parse_charinfo(recv: &mut Cursor<'_>) -> Option<CharInfo> {
    recv.skip(1); // byte 1
    recv.skip(2); // short 0
    recv.skip(12); // CRand seeds (3 ints)
    recv.skip(8); // long -1
    recv.skip(1); // byte 0
    let mut info = parse_character_info_body(recv)?;
    // addSkillInfo
    info.skills = parse_skill_info(recv)?;
    Some(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::Packet;

    #[test]
    fn attack_broadcast_parses_targets() {
        // Server `closeRangeAttack` layout with skill=0.
        let mut p = Packet::new(0);
        p.write_i32(3951); // cid
        p.write_u8(0x21); // tbyte: 2 targets, 1 hit
        p.write_u8(1); // lvl
        p.write_u8(0); // skill=0 (single byte)
        p.write_u8(0); // unk
        p.write_u8(0); // display
        p.write_u8(0); // animation
        p.write_u8(4); // speed
        p.write_u8(0); // mastery
        p.write_i32(0); // int 0
        p.write_i32(50001); // target oid
        p.write_u8(7);
        p.write_i32(120);
        p.write_i32(50002); // target oid
        p.write_u8(7);
        p.write_i32(85);

        let mut c = Cursor::new(&p.as_bytes()[2..]);
        let atk = parse_attack_broadcast(&mut c);
        assert_eq!(atk.cid, 3951);
        assert_eq!(atk.skill, 0);
        assert_eq!(atk.targets.len(), 2);
        assert_eq!(atk.targets[0].oid, 50001);
        assert_eq!(atk.targets[0].hits, vec![120]);
        assert_eq!(atk.targets[1].oid, 50002);
        assert_eq!(atk.targets[1].hits, vec![85]);
    }

    #[test]
    fn stats_update_parses_hp_mp() {
        // HP(4096) + MAXMP(32768) set.
        let mask = 4096 | 32768;
        let mut p = Packet::new(0);
        p.write_u8(0); // itemReaction
        p.write_i32(mask);
        p.write_i16(1234); // hp
        p.write_i16(500); // maxmp
        p.write_u8(0);
        p.write_u8(0);

        let mut c = Cursor::new(&p.as_bytes()[2..]);
        let u = parse_stats_update(&mut c);
        assert_eq!(u.hp, Some(1234));
        assert_eq!(u.maxmp, Some(500));
        assert_eq!(u.mp, None);
    }

    #[test]
    fn stats_update_parses_exp_and_level() {
        // EXP(262144) + LEVEL(64) set.
        let mask = 262144 | 64;
        let mut p = Packet::new(0);
        p.write_u8(0); // itemReaction
        p.write_i32(mask);
        p.write_u8(12); // level
        p.write_i32(12345); // exp
        p.write_u8(0);
        p.write_u8(0);

        let mut c = Cursor::new(&p.as_bytes()[2..]);
        let u = parse_stats_update(&mut c);
        assert_eq!(u.level, Some(12));
        assert_eq!(u.exp, Some(12345));
    }

    #[test]
    fn stats_update_parses_available_ap() {
        // AVAILABLEAP(65536) + STR(256) set (a DISTRIBUTE_AP echo).
        let mask = 65536 | 256;
        let mut p = Packet::new(0);
        p.write_u8(1); // itemReaction
        p.write_i32(mask);
        p.write_i16(18); // STR
        p.write_i16(4); // remaining AP
        p.write_u8(0);
        p.write_u8(0);

        let mut c = Cursor::new(&p.as_bytes()[2..]);
        let u = parse_stats_update(&mut c);
        assert_eq!(u.ap, Some(4));
        assert_eq!(u.hp, None);
        assert!(!c.failed());
    }

    #[test]
    fn drop_parses_oid_itemid_and_position() {
        // Server dropItemFromMapObject layout (mod=0, not meso).
        let mut p = Packet::new(0);
        p.write_u8(0); // mod
        p.write_i32(90001); // oid
        p.write_u8(0); // isMeso
        p.write_i32(4000000); // itemId (blue snail shell)
        p.write_i32(123); // owner
        p.write_u8(1); // dropType
        p.write_i16(100); // x
        p.write_i16(214); // y
        p.write_i32(0); // extra

        let mut c = Cursor::new(&p.as_bytes()[2..]);
        let (oid, itemid, is_meso, x, y) = parse_drop(&mut c).unwrap();
        assert_eq!(oid, 90001);
        assert_eq!(itemid, 4000000);
        assert!(!is_meso);
        assert_eq!((x, y), (100, 214));
    }

    #[test]
    fn drop_parses_meso_flag_and_amount() {
        // Meso drop: isMeso=1, the int slot holds the meso AMOUNT.
        let mut p = Packet::new(0);
        p.write_u8(0); // mod
        p.write_i32(90002); // oid
        p.write_u8(1); // isMeso
        p.write_i32(500); // meso amount
        p.write_i32(0); // owner
        p.write_u8(0); // dropType
        p.write_i16(50); // x
        p.write_i16(60); // y
        p.write_i32(0); // extra

        let mut c = Cursor::new(&p.as_bytes()[2..]);
        let (oid, itemid, is_meso, x, y) = parse_drop(&mut c).unwrap();
        assert_eq!(oid, 90002);
        assert_eq!(itemid, 500, "meso amount lives in the itemid slot");
        assert!(is_meso);
        assert_eq!((x, y), (50, 60));
    }

    #[test]
    fn spawn_monster_parses_standard_layout() {
        // no-status spawn: int oid; byte 1; int mobid; 16B zero mask;
        // short x; short y; byte stance; short 0; short fh.
        let mut p = Packet::new(0);
        p.write_i32(50001); // oid
        p.write_u8(1);
        p.write_i32(1210102); // mobid
        for _ in 0..4 {
            p.write_i32(0); // status mask, no EMPTY bit
        }
        p.write_i16(100); // x
        p.write_i16(214); // y
        p.write_u8(4); // stance
        p.write_i16(0);
        p.write_i16(65); // fh

        let mut c = Cursor::new(&p.as_bytes()[2..]);
        let e = parse_spawn_monster(&mut c).unwrap();
        assert_eq!(e.oid, 50001);
        assert_eq!(e.mobid, 1210102);
        assert_eq!((e.x, e.y), (100, 214));
        assert_eq!(e.fh, 65);
        assert!(!c.failed());
    }

    #[test]
    fn spawn_monster_parses_third_party_layout() {
        // Variant server live capture (after opcode):
        // oid 0x0007A121; 01; mobid 1210102; 15B zeros; 88 00 00 00;
        // 00 00 00; x=0xFE68(-408); y=0x045A(1114); stance 05; tail.
        let bytes: &[u8] = &[
            0x21, 0xA1, 0x07, 0x00, 0x01, 0xF6, 0x76, 0x12, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x88, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x68, 0xFE,
            0x5A, 0x04, 0x05, 0x00, 0x00, 0x04, 0x00, 0xFF, 0xFF, 0x50, 0x00,
            0x00, 0x00,
        ];
        let mut c = Cursor::new(bytes);
        let e = parse_spawn_monster(&mut c).unwrap();
        assert_eq!(e.oid, 0x0007_A121);
        assert_eq!(e.mobid, 1210102);
        assert_eq!((e.x, e.y), (-408, 1114));
        assert_eq!(e.stance, 5);
        assert!(!c.failed());
    }

    #[test]
    fn spawn_monster_control_parses_third_party_layout() {
        // F0 packet: one extra leading aggro byte (01).
        let bytes: &[u8] = &[
            0x01, 0x21, 0xA1, 0x07, 0x00, 0x01, 0xF6, 0x76, 0x12, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x88, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x68,
            0xFE, 0x5A, 0x04, 0x05, 0x00, 0x00, 0x04, 0x00, 0xFF, 0xFF, 0x50,
            0x00, 0x00, 0x00,
        ];
        let mut c = Cursor::new(bytes);
        let e = parse_spawn_monster_control(&mut c).unwrap();
        assert_eq!(e.oid, 0x0007_A121);
        assert_eq!(e.mobid, 1210102);
        assert_eq!((e.x, e.y), (-408, 1114));
        assert!(!c.failed());
    }

    #[test]
    fn simple_options_parse_ids_and_labels() {
        // standard sendSimple: #L<id>#<text>#l, options concatenated, text may contain newlines
        let msg = "#L0#购买#l\r\n#L1#出售\r\n装备#l\r\n#L5#离开#l";
        let opts = parse_simple_options(msg);
        assert_eq!(opts.len(), 3);
        assert_eq!(opts[0], (0, "购买".to_string()));
        assert_eq!(opts[1], (1, "出售\r\n装备".to_string()));
        assert_eq!(opts[2], (5, "离开".to_string()));
    }

    #[test]
    fn simple_options_without_closing_marker() {
        // Some servers omit the #l terminator: text runs to the end of the string
        let msg = "#L0#选项A#L1#选项B";
        let opts = parse_simple_options(msg);
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0], (0, "选项A".to_string()));
        assert_eq!(opts[1], (1, "选项B".to_string()));
    }

    #[test]
    fn simple_options_no_menu_returns_empty() {
        assert!(parse_simple_options("你好, 世界").is_empty());
        assert!(parse_simple_options("").is_empty());
        // A #L in plain text is not an option (no digits follow)
        assert!(parse_simple_options("#L 不是选项").is_empty());
    }

    #[test]
    fn clean_npc_text_strips_format_tags() {
        let raw = "\t\t#fEffect/ItemEff/1071085/effect/walk1/2#  #e#r 服务端079 #k#n  #r  #fEffect/ItemEff/1071085/effect/walk1/2##b#k#n";
        let clean = clean_npc_text(raw, 400);
        assert!(!clean.contains("#fEffect"));
        assert!(!clean.contains("#e#r"));
        assert!(!clean.contains("#k#n"));
        assert!(clean.contains("服务端079"));
        assert!(!clean.contains('\t'));
        // menu options are kept
        let menu = "#e欢迎#n\r\n#L0#购买#l\r\n#L1#出售\r\n装备#l";
        let clean_menu = clean_npc_text(menu, 400);
        assert!(clean_menu.contains("#L0#"));
        assert!(clean_menu.contains("购买"));
        assert!(clean_menu.contains("#L1#"));
        assert!(clean_menu.contains("出售"));
        // options still parse (trailing newline separators kept)
        let opts = parse_simple_options(&clean_menu);
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0].0, 0);
        assert!(opts[0].1.starts_with("购买"));
        assert_eq!(opts[1].0, 1);
        assert!(opts[1].1.starts_with("出售"));
    }

    #[test]
    fn clean_npc_text_truncates_long_text() {
        let long = format!("0123456789{}尾", "很长很长很长很长很长很长很长".repeat(20));
        let clean = clean_npc_text(&long, 30);
        assert!(clean.chars().count() <= 31); // 30 + ellipsis
        assert!(clean.ends_with('…'));
    }

    #[test]
    fn long_menu_must_parse_from_raw_not_cleaned() {
        // Auction-style menu with many options:
        // `#r#L<id>##v<item>##t<item># label#l` — clean_npc_text(400) truncates
        // the message BEFORE parsing, silently dropping later options.
        let mut raw = String::from("请选择你要兑换的商品:\r\n\r\n");
        for i in 0..30 {
            raw.push_str(&format!(
                "#r#L{i}##v5210000##t5210000# 时限：3小时  需要 点券 0#l\r\n"
            ));
        }
        let cleaned = clean_npc_text(&raw, 400);
        let from_cleaned = parse_simple_options(&cleaned);
        assert!(
            from_cleaned.len() < 30,
            "400-char truncation must cut options (bug repro): got {}",
            from_cleaned.len()
        );
        // The fix: parse from the RAW message — every option survives.
        let from_raw = parse_simple_options(&raw);
        assert_eq!(from_raw.len(), 30);
        assert_eq!(from_raw[29].0, 29);
        assert!(from_raw[29].1.contains("5210000"), "raw label keeps #v/#t tags: {:?}", from_raw[29].1);
    }
}




