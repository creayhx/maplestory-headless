//! Level-up experience table for MapleStory 079 (ExpTable).
//!
//! Source: the `ExpTable` array in the 079 server's
//! `constants/GameConstants.java` (standard 079 curve, matching HeavenMS).
//! On level-up the server settles with `exp -= ExpTable[level]`
//! (MapleCharacter.gainExp), so `ExpTable[level]` = exp needed to go from
//! level to level+1. Given only the cumulative current-level exp the server
//! reports (state.exp), this table yields the remaining exp to the next level.

/// The standard official table covers up to level 200. Some servers have
/// higher caps (e.g. 255); the server special-cases them by filling
/// ExpTable 201+ with a fixed sentinel value, so levels ≥200 reuse the
/// level-200 exp.
pub const MAX_LEVEL: i32 = 200;

/// `EXP_TABLE[level]` = exp needed for level → level+1; `EXP_TABLE[0]` unused.
/// Indices 1..=200 match the server value for value (201+ uses server
/// sentinels, not included here).
static EXP_TABLE: [i64; 201] = [
    0,
    15, 34, 57, 92, 135, 372, 560, 840, 1242, // 1..=9
    1716, 2360, 3216, 4200, 5460, 7050, 8840, 11040, 13716, 16680, // 10..=19
    20216, 24402, 28980, 34320, 40512, 47216, 54900, 63666, 73080, 83720, // 20..=29
    95700, 108480, 122760, 138666, 155540, 174216, 194832, 216600, 240500, 266682, // 30..=39
    294216, 324240, 356916, 391160, 428280, 468450, 510420, 555680, 604416, 655200, // 40..=49
    709716, 748608, 789631, 832902, 878545, 926689, 977471, 1031036, 1087536, 1147132, // 50..=59
    1209994, 1276301, 1346242, 1420016, 1497832, 1579913, 1666492, 1757815, 1854143, 1955750, // 60..=69
    2062925, 2175973, 2295216, 2410993, 2553663, 2693603, 2841212, 2996910, 3161140, 3334370, // 70..=79
    3517093, 3709829, 3913127, 4127566, 4353756, 4592341, 4844001, 5109452, 5389449, 5684790, // 80..=89
    5996316, 6324914, 6671519, 7037118, 7422752, 7829518, 8258575, 8711144, 9188514, 9692044, // 90..=99
    10223168, 10783397, 11374327, 11997640, 12655110, 13348610, 14080113, 14851703, 15665576, 16524049, // 100..=109
    17429566, 18384706, 19392187, 20454878, 21575805, 22758159, 24005306, 25320796, 26708375, 28171993, // 110..=119
    29715818, 31344244, 33061908, 34873700, 36784778, 38800583, 40926854, 43169645, 45535341, 48030677, // 120..=129
    50662758, 53439077, 56367538, 59456479, 62714694, 66151459, 69776558, 73600313, 77633610, 81887931, // 130..=139
    86375389, 91108760, 96101520, 101367883, 106922842, 112782213, 118962678, 125481832, 132358236, 139611467, // 140..=149
    147262175, 155332142, 163844343, 172823012, 182293713, 192283408, 202820538, 213935103, 225658746, 238024845, // 150..=159
    251068606, 264827165, 279339693, 294647508, 310794191, 327825712, 345790561, 364739883, 384727628, 405810702, // 160..=169
    428049128, 451506220, 476248760, 502347192, 529875818, 558913012, 589541445, 621848316, 655925603, 691870326, // 170..=179
    729784819, 769777027, 811960808, 856456260, 903390063, 952895838, 1005114529, 1060194805, 1118293480, 1179575962, // 180..=189
    1244216724, 1312399800, 1384319309, 1460180007, 1540197871, 1624600714, 1713628833, 1807535693, 1906588648, 2011069705, // 190..=199
    2121276324, // 200 (max-level sentinel: the server stops leveling past 200)
];

/// Exp needed for level → level+1. Returns 0 for ≤0; levels ≥200 (including
/// 255-cap servers) reuse the level-200 value (hack: the server special-cases
/// levels above 200).
pub fn exp_needed(level: i32) -> i64 {
    if level <= 0 {
        0
    } else {
        EXP_TABLE[level.min(MAX_LEVEL) as usize]
    }
}

/// Exp remaining until the next level (levels ≥200 use the level-200 value).
pub fn exp_remain(level: i32, exp: i64) -> i64 {
    (exp_needed(level) - exp).max(0)
}

/// Current level exp percentage (0..=100; levels ≥200 use the level-200 value).
pub fn exp_pct(level: i32, exp: i64) -> i32 {
    let needed = exp_needed(level);
    if needed <= 0 {
        100
    } else {
        ((exp as f64 * 100.0 / needed as f64).floor() as i32).clamp(0, 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_levels_match_server_table() {
        // Spot-check the first entries against the server ExpTable
        assert_eq!(exp_needed(1), 15);
        assert_eq!(exp_needed(2), 34);
        assert_eq!(exp_needed(9), 1242);
        assert_eq!(exp_needed(10), 1716);
        assert_eq!(exp_needed(11), 2360);
    }

    #[test]
    fn level_83_matches_server_table() {
        // Level 83→84 needs 4127566 (server ExpTable[83])
        assert_eq!(exp_needed(83), 4_127_566);
    }

    #[test]
    fn above_200_reuses_level_200_value() {
        // hack: levels above 200 reuse the level-200 exp (255-cap servers)
        let v200 = EXP_TABLE[200];
        assert_eq!(exp_needed(200), v200);
        assert_eq!(exp_needed(201), v200);
        assert_eq!(exp_needed(255), v200);
        assert_eq!(exp_remain(255, v200 - 10), 10);
        assert_eq!(exp_needed(0), 0);
        assert_eq!(exp_needed(-5), 0);
    }

    #[test]
    fn remain_clamps_at_zero() {
        assert_eq!(exp_remain(83, 4_000_000), 127_566);
        assert_eq!(exp_remain(83, 4_127_566), 0); // full exp (before settlement)
        assert_eq!(exp_remain(83, 5_000_000), 0); // over (waiting for the level-up packet)
    }

    #[test]
    fn pct_rounds_down() {
        assert_eq!(exp_pct(83, 2_063_783), 50); // exactly half
        assert_eq!(exp_pct(83, 0), 0);
        // levels ≥200 use the level-200 exp for the percentage
        assert_eq!(exp_pct(255, EXP_TABLE[200] / 2), 50);
        assert_eq!(exp_pct(255, 0), 0);
    }
}
