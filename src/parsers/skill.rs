//! Skill list parsing (`PacketHelper.addSkillInfo`).
//!
//! charinfo layout after the inventory blocks:
//! `short count + [int skillid + int level + (4th job: int masterlevel)] * count`.
//! The masterlevel field is only present for 4th-job skills, so the count and
//! the 4th-job check drive the walk.

use crate::packet::Cursor;
use crate::state::SkillEntry;

use std::collections::BTreeMap;

/// Whether a skill belongs to a 4th-job class (skill id prefix ends in
/// 12/22/32/42/52/62/72/82/92, e.g. 112xxxx hero, 212xxxx archmage).
pub fn is_fourth_job_skill(skillid: i32) -> bool {
    match skillid / 10000 % 100 {
        12 | 22 | 32 | 42 | 52 | 62 | 72 | 82 | 92 => true,
        _ => false,
    }
}

/// Parse the `addSkillInfo` block. Returns None if the cursor failed; on
/// success the skills map may be empty (count 0).
pub fn parse_skill_info(recv: &mut Cursor<'_>) -> Option<BTreeMap<i32, SkillEntry>> {
    let count = recv.read_i16();
    if recv.failed() {
        return None;
    }
    let mut skills = BTreeMap::new();
    for _ in 0..count {
        let skillid = recv.read_i32();
        let level = recv.read_i32();
        let masterlevel = if is_fourth_job_skill(skillid) {
            recv.read_i32()
        } else {
            0
        };
        if recv.failed() {
            return None;
        }
        if level > 0 {
            skills.insert(
                skillid,
                SkillEntry {
                    level: level as u8,
                    masterlevel: masterlevel as u8,
                },
            );
        }
    }
    Some(skills)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::Packet;

    #[test]
    fn fourth_job_detection() {
        assert!(!is_fourth_job_skill(1001005)); // warrior 1st
        assert!(!is_fourth_job_skill(1300000)); // paladin 2nd
        assert!(!is_fourth_job_skill(1111000)); // warrior 3rd
        assert!(is_fourth_job_skill(1121000)); // hero 4th
        assert!(is_fourth_job_skill(2121000)); // archmage 4th
        assert!(is_fourth_job_skill(3221000)); // sniper 4th
        assert!(is_fourth_job_skill(5221000)); // outlaw 4th
        assert!(!is_fourth_job_skill(1000)); // beginner
    }

    #[test]
    fn skill_info_parses_levels() {
        // count 3: Slash Blast lv5, paladin skill lv3, hero skill lv2 master 30.
        let mut p = Packet::new(0);
        p.write_i16(3);
        p.write_i32(1001005);
        p.write_i32(5);
        p.write_i32(1300000);
        p.write_i32(3);
        p.write_i32(1121000);
        p.write_i32(2);
        p.write_i32(30);

        let mut c = Cursor::new(&p.as_bytes()[2..]);
        let skills = parse_skill_info(&mut c).expect("parse");
        assert_eq!(skills.len(), 3);
        assert_eq!(skills.get(&1001005).unwrap().level, 5);
        assert_eq!(skills.get(&1001005).unwrap().masterlevel, 0);
        assert_eq!(skills.get(&1300000).unwrap().level, 3);
        assert_eq!(skills.get(&1121000).unwrap().level, 2);
        assert_eq!(skills.get(&1121000).unwrap().masterlevel, 30);
        assert!(!c.failed());
    }

    #[test]
    fn skill_info_empty() {
        let mut p = Packet::new(0);
        p.write_i16(0);
        let mut c = Cursor::new(&p.as_bytes()[2..]);
        let skills = parse_skill_info(&mut c).expect("parse");
        assert!(skills.is_empty());
        assert!(!c.failed());
    }
}
