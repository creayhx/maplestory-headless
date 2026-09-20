//! Interactive stdin commands to drive the bot.

pub mod parse;
pub mod rules;
pub mod run;
pub mod tasks;
pub mod tick;
pub mod view;

pub use parse::parse;
pub use run::run;
pub use tick::tick;

use crate::packets::stats as pstats;
use crate::state::BotState;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Move(i16, i16),
    /// move to a portal's coordinates: `move <portal_name>`
    MovePortal(String),
    /// move to a portal and warp through it: `movewarp <portal>` / `mw <portal>`
    MoveWarp(String),
    Chat(String),
    /// unified view commands: `view <object>`
    View(View),
    Sleep(u64),
    /// wait <pred> [timeout_ms]: gate used inside rule action sequences (a
    /// rule containing a wait action is inlined into the task runtime; the
    /// step waits for the predicate, abandon-on-timeout). Not directly
    /// executable as a free command.
    Wait(String, Option<u64>),
    /// while <pred> do <cmd>: loop step used inside task step sequences (a rule
    /// containing a `while` action is inlined into the task runtime as a loop
    /// step). While `pred` holds, run `cmd` each tick and stay on the step;
    /// once `pred` is false, the task advances immediately (no hidden grace
    /// window — use a `wait <pred>` step for any inter-phase gap).
    /// Not directly executable as a free stdin command.
    While(String, String),
    Attack(i32),
    Hunt(bool),
    /// hunt attack mode: `hunt attack [n]` / `hunt skill <id> [n] [range] [mp]`
    HuntMode(HuntMode),
    /// hunt attack range: `hunt range <px>` (writes back to cfg.hunt.all_attack_range)
    HuntRange(i32),
    /// hunt pickup toggle/radius: `hunt pickup on|off|<px>` (writes back to cfg.hunt.pickup_enabled / pickup_range)
    HuntPickup(bool),
    /// hunt pickup range: `hunt pickuprange <px>` (0 = whole map; writes back to cfg.hunt.pickup_range)
    HuntPickupRange(i32),
    /// attack damage: `hunt damage <n>` (0 = internal formula level², >0 = fixed value)
    HuntDamage(i32),
    /// skill hits per target: `hunt hits <n>` (1..15, e.g. 2 for group attack skills)
    HuntHits(u8),
    /// attack cooldown: `hunt atkcd <ms>` (writes back to cfg.hunt.attack_cooldown, default 700)
    HuntAtkCd(u64),
    /// attack only controlled mobs: `hunt controller on|off`
    /// (writes back to cfg.hunt.attack_controller_only, default on (safe); off = attack all mobs)
    HuntController(bool),
    /// gather toggle: `gather on|off` (pulls out-of-range mobs toward the player, independent of hunt)
    Gather(bool),
    /// gather step: `gather step <px>` (0 = pull straight to the player's position)
    GatherStep(i32),
    /// gather round interval: `gather interval <ms>` (0 = every tick; raise it for rate-sensitive servers)
    GatherInterval(u64),
    /// max mobs pulled per round: `gather max <n>` (0 = unlimited; 3~5 recommended for batch-move-sensitive servers)
    GatherMax(usize),
    /// gather only server-controlled mobs: `gather controller on|off`.
    /// Default on (safe); off = ignore control and force-pull everything (special cases).
    GatherController(bool),
    /// stand mode: `hunt stand on|off` (don't chase; fight in place with gather)
    HuntStand(bool),
    /// print the current hunt config: `hunt status` (English in CLI / Chinese in TUI)
    HuntStatus,
    /// persist the in-memory hunt config into config.json: `hunt save`
    /// (only the `hunt` section is written; other keys untouched)
    HuntSave,
    /// pickup filter: `hunt filter [off|allow <ids>|deny <ids>|+ <ids>|- <ids>]`
    /// (status / clear / replace whitelist / replace blacklist / append to the
    /// active list / remove from the active list; immediate prune + emit)
    HuntFilter(HuntFilterCmd),
    /// one manual attack round: `hunt once` — locate mobs in range and fire
    /// once per call, nothing else (no pickup/gather/far-drop). Gated by
    /// attack_cooldown; works while a task holds the lock.
    HuntOnce,
    /// set a flow variable: `setvar <name> <value>`
    SetVar(String, String),
    /// remove a flow variable: `clearvar <name>` (manual lifecycle only)
    ClearVar(String),
    /// cast one buff/assist skill (0x58 SPECIAL_MOVE, no target)
    BuffSkill(i32),
    /// cancel a buff by skill id (0x59 CANCEL_BUFF)
    BuffCancel(i32),
    /// rebind a key: `keymap set <key> <type> <action>` (0x83, no validation)
    KeymapSet(i32, u8, i32),
    /// switch channel: `channel <n>` (1-based; 0x22 + byte n-1)
    ChangeChannel(u8),
    /// enter the cash shop (`cashshop` / `cs`)
    CashShop,
    /// open the auction (`auction`, ENTER_MTS 0x8D), used when REWARD_ITEM
    /// is blocked
    AuctionOpen,
    /// re-enter the current map: forced cash shop round trip (`reenter`) —
    /// enter the CS, immediately leave, the server lands the character back
    /// on the same map (CharacterTransfer handoff, no login notice) and all
    /// map objects respawn fresh
    Reenter,
    /// buy a cash item: `csbuy <sn> [nx|points]` (default nx)
    CsBuy(i32, bool),
    /// take an item from the cash inventory into the backpack:
    /// `csget [uniqueid]` (no arg = the last one seen)
    CsTakeOut(Option<i64>),
    /// leave the cash shop back to the channel server (`csout`)
    CsOut,
    /// list the cash shop sale items (from SET_CASH_SHOP 0x83):
    /// `cslist` — every entry with a real itemid
    CsList,
    /// store a cash item from the backpack back into the cash shop:
    /// `csstore <uniqueid>` (only items with a unique id can be stored)
    CsStore(i64),
    /// toggle a config rule at runtime: `rule open|close|status [id]`
    Rule(RuleCmd),
    Group(GroupCmd),
    /// auto-reconnect settings: `reconnect on|off|delay <sec>|max <count>|status`
    Reconnect(ReconnectCmd),
    Use(i32),
    UseReward(i32),
    /// Drop items: (itemid, qty, optional slot). slot=None finds the first matching slot,
    /// Some(slot) drops only that slot. Equips/consumables/etc. are all droppable.
    Drop(i32, i16, Option<i16>),
    /// Use an upgrade scroll: (scroll itemid, target equip itemid, whether to protect with a blessing scroll).
    /// The scroll sits in the USE tab; the target is looked up in worn equips first, then the backpack equip tab.
    /// bless=true sends ws=2 (the server deducts the white scroll; it is not consumed on failure).
    UseScroll(i32, i32, bool),
    DropMeso(i32),
    Ap(ApCmd),
    Party(PartyCmd),
    ChangeMap(i32, String),
    ChangeMapSpecial(String),
    /// List portals for current/specified map: `view portals [map_id]`
    PortalList(Option<i32>),
    NpcTalk(i32),
    NpcReply(i32),
    NpcNext,
    NpcPrev,
    NpcCancel,
    NpcYesNo(bool),
    NpcNumber(i32),
    NpcText(String),
    NpcBuy(i32, i16),
    /// Sell items: (itemid, qty, optional slot). qty<=0 = sell out that slot/all slots;
    /// slot=None gathers the quantity across slots, Some(slot) sells only that slot.
    NpcSell(i32, i16, Option<i16>),
    /// sell all ammo items (arrows 206xxxx / throwing stars 207xxxx / bullets 233xxxx)
    SellAmmo,
    /// sell every item in one inventory tab (equip/consume/etc)
    SellType(SellTab),
    /// start/stop/inspect config tasks: `task start <id>|stop|status`
    Task(TaskCmd),
    /// hot-reload config.json (tasks/rules/potion take effect)
    Reload,
    ShopLeave,
    Trade(TradeCmd),
    Equip(EquipCmd),
    Skill(SkillCmd),
    /// list / hit map reactors (0x11E track + 0xC9 attack)
    Reactor(ReactorCmd),
    /// no-range pick up every drop on the map (reports the drop's own position;
    /// server only warns past 800px real distance, never blocks)
    PickupAll,
    /// interactive-login steps (used by the TUI wizard; scriptable too)
    Login(LoginCmd),
    Help,
    Quit,
}

/// Interactive login flow commands (`login world|char ...`). These only
/// matter when `Config.interactive` is set — the handlers stop at each login
/// phase and the frontend advances the flow with these commands.
#[derive(Debug, Clone, PartialEq)]
pub enum LoginCmd {
    /// `login world <index> [channel]` — pick a SERVERLIST world + channel (1-based)
    World(usize, u8),
    /// `login char <index>` — pick a CHARLIST character (dedup by id)
    Char(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReactorCmd {
    /// start/stop autonomous reactor farming (`reactor on|off`)
    On(bool),
    /// hit every alive reactor on the map (server does not validate range)
    HitAll,
    /// hit one reactor by oid
    Hit(i32),
    /// set the reactor hit round interval (ms, 0 = every tick)
    Cooldown(u64),
}

/// config task control: start/stop/status
#[derive(Debug, Clone, PartialEq)]
pub enum TaskCmd {
    Start(String),
    Stop,
    Status,
}

/// runtime toggles for config rules (default `enabled: false`; `reload` resets
/// to the values in config.json)
#[derive(Debug, Clone, PartialEq)]
pub enum RuleCmd {
    Open(String),
    Close(String),
    Status(Option<String>),
}

/// runtime toggles for config groups (master switch for a feature bundle of
/// rules + tasks; `reload` resets to the values in config.json)
#[derive(Debug, Clone, PartialEq)]
pub enum GroupCmd {
    Open(String),
    Close(String),
    Status(Option<String>),
}

/// Auto-reconnect settings: `reconnect on|off|delay <sec>|max <count>|status`
/// (0 count = unlimited; writes the in-memory cfg, `reload` restores file values)
#[derive(Debug, Clone, PartialEq)]
pub enum ReconnectCmd {
    On,
    Off,
    Delay(u64),
    Max(u64),
    Status,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TradeCmd {
    Invite(String),
    Accept,
    Confirm,
    Decline,
    Quit,
    Put(i32, i16),
    Meso(i32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum EquipCmd {
    /// equip <itemid> from the backpack
    Equip(i32),
    /// unequip <itemid> back to the backpack
    Unequip(i32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SkillCmd {
    /// spend one skill point on <skillid> (`skill learn <id>`)
    Learn(i32),
    /// cast one attack skill on a target (`skill cast <id> <oid>`)
    Cast(i32, i32),
    /// show a skill's levels (`skill info <id>`)
    Info(i32),
}

/// What `view <object>` reports. All read-only inspection goes through `view`.
#[derive(Debug, Clone, PartialEq)]
pub enum View {
    /// full status dump (position/hp/party/follow...)
    Status,
    /// mobs on the map
    Mobs,
    /// other players on the map
    Players,
    /// NPCs on the map
    Npcs,
    /// map drops
    Drops,
    /// inventory + meso (optionally one tab only)
    Inventory,    /// one inventory tab: 1=EQUIP 2=USE 3=SETUP 4=ETC 5=CASH
    InventoryTab(u8),
    /// learned skills + remaining SP
    Skills,
    /// worn equipment (`view equiped`)
    Equips,
    /// one equip's info by itemid (`view eqpinfo <itemid>`): all matches in
    /// backpack and worn, each tagged with its source
    EqpInfo(i32),
    /// party state (`view party`)
    Party,
    /// trade state (`view trade`)
    Trade,
    /// remaining AP (`view attr`)
    Attr,
    /// map reactors (`view reactor`)
    Reactor,
    /// keybindings (`view keymap`)
    Keymap,
    /// cash shop balance + stored items (`view cs`)
    CashShop,
    /// map portals (`view portals`)
    Portals,
}

/// Attack mode (hunting):
/// - Attack(n): basic attack — one basic attack packet per mob in range, up to n
///   (n defaults to config hunt.attack_max_targets)
/// - Skill(id, n, range, mp): skill — id=0 is a skill-less group attack (one skill=0 packet,
///   multiple targets, processed per-target as basic attacks by the server), >0 is a skill group
///   packet. n = max targets per packet, range = skill filter radius, mp = MP cost check;
///   missing values all default to config.
#[derive(Debug, Clone, PartialEq)]
pub enum HuntMode {
    Attack(Option<usize>),
    Skill(Option<i32>, Option<usize>, Option<i32>, Option<i16>),
}

/// Pickup filter operations (`hunt filter ...`). The two lists are kept
/// separate: `SetAllow` switches to whitelist mode (wins over deny),
/// `SetDeny` to blacklist mode; `Add`/`Del` mutate whichever list is
/// currently active (allow if non-empty, else deny).
#[derive(Debug, Clone, PartialEq)]
pub enum HuntFilterCmd {
    /// `hunt filter` — print both lists + the effective mode
    Status,
    /// `hunt filter off` — clear both lists (no filtering)
    Off,
    /// `hunt filter allow <id>...` — replace the whitelist (switch to allow mode)
    SetAllow(Vec<i32>),
    /// `hunt filter deny <id>...` — replace the blacklist (switch to deny mode)
    SetDeny(Vec<i32>),
    /// `hunt filter + <id>...` — append to the ACTIVE list
    Add(Vec<i32>),
    /// `hunt filter - <id>...` — remove from the ACTIVE list
    Del(Vec<i32>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ApCmd {
    /// spend `n` ability points on `stat` (`ap str 1`), each point one packet
    Add(ApStat, i32),
    Status,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ApStat {
    Str,
    Dex,
    Int,
    Luk,
    MaxHp,
    MaxMp,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PartyCmd {
    Create,
    Invite(String),
    /// invite by cid: `party invitecid <cid>` (use cid when the name has rare characters)
    InviteCid(i32),
    Leave,
    /// kick a member: `party kick <name>` (packet uses the member's cid, 78 00 05 + cid)
    Kick(String),
    /// kick by cid: `party kickcid <cid>`
    KickCid(i32),
}

/// Inventory tabs `sell type` can clear out. SETUP/CASH are excluded because
/// NPC shops cannot buy them back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SellTab {
    Equip,
    Consume,
    Etc,
}

impl SellTab {
    /// inventory tab byte: 1=EQUIP 2=USE 3=SETUP 4=ETC 5=CASH
    pub fn tab(&self) -> u8 {
        match self {
            SellTab::Equip => 1,
            SellTab::Consume => 2,
            SellTab::Etc => 4,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            SellTab::Equip => "equip",
            SellTab::Consume => "consume",
            SellTab::Etc => "etc",
        }
    }

    pub fn parse(s: &str) -> Option<SellTab> {
        match s {
            "equip" => Some(SellTab::Equip),
            "consume" | "use" => Some(SellTab::Consume),
            "etc" => Some(SellTab::Etc),
            _ => None,
        }
    }
}

pub fn ap_stat_value(s: &ApStat) -> i32 {
    use pstats::stat as st;
    match s {
        ApStat::Str => st::STR,
        ApStat::Dex => st::DEX,
        ApStat::Int => st::INT,
        ApStat::Luk => st::LUK,
        ApStat::MaxHp => st::MAXHP,
        ApStat::MaxMp => st::MAXMP,
    }
}

pub fn ap_stat_name(v: i32) -> &'static str {
    use pstats::stat as st;
    match v {
        st::STR => "STR",
        st::DEX => "DEX",
        st::INT => "INT",
        st::LUK => "LUK",
        st::MAXHP => "MAXHP",
        st::MAXMP => "MAXMP",
        _ => "?",
    }
}

/// Map an equip's itemid category to its body slot (negative position in the
/// EQUIPPED tab). Standard 079 client body positions; `None` for slots the
/// bot does not manage (pets, androids, totems...).
pub fn equip_slot(itemid: i32) -> Option<i16> {
    match itemid / 10000 {
        100 => Some(-1),      // cap
        101 => Some(-2),      // face accessory
        102 => Some(-3),      // eye accessory
        103 => Some(-4),      // earrings
        104 => Some(-5),      // top
        105 => Some(-5),      // overall
        106 => Some(-6),      // bottom
        107 => Some(-7),      // gloves
        108 => Some(-8),      // shoes
        109 => Some(-9),      // cape
        110 => Some(-10),     // shield
        111 => Some(-12),     // ring (uses -12..-15, first one)
        112 => Some(-16),     // pendant
        113 => Some(-17),     // badge
        114 => Some(-21),     // medal
        115 => Some(-19),     // belt
        116 => Some(-20),     // pocket
        119 => Some(-18),     // mount
        130..=149 => Some(-11), // weapon
        _ => None,
    }
}

/// Human-readable name for a body slot (negative key of the equipped tab).
pub fn equip_slot_name(slot: i16) -> &'static str {
    match slot {
        -1 => "cap",
        -2 => "face",
        -3 => "eye",
        -4 => "earrings",
        -5 => "top",
        -6 => "bottom",
        -7 => "gloves",
        -8 => "shoes",
        -9 => "cape",
        -10 => "shield",
        -11 => "weapon",
        -12 => "ring1",
        -13 => "ring2",
        -14 => "ring3",
        -15 => "ring4",
        -16 => "pendant",
        -17 => "badge",
        -18 => "mount",
        -19 => "belt",
        -20 => "pocket",
        -21 => "medal",
        _ => "unknown",
    }
}

/// One-line summary of equip stats, used by `equips` / `equipinfo`.
pub fn equip_stats_line(itemid: i32, st: &crate::state::EquipStats) -> String {
    // req (base_level) = max(baseLevel, equipLevel) sent by the server's addEquipStats:
    // always 1 for normal weapons, 0/equip exp level for special upgradeable weapons — unrelated
    // to Item.wz's real level requirement (10/20/50); the server only uses wz data for validation, never sends it.
    let mut parts = Vec::new();
    if st.str != 0 {
        parts.push(format!("str:{}", st.str));
    }
    if st.dex != 0 {
        parts.push(format!("dex:{}", st.dex));
    }
    if st.int != 0 {
        parts.push(format!("int:{}", st.int));
    }
    if st.luk != 0 {
        parts.push(format!("luk:{}", st.luk));
    }
    if st.hp != 0 {
        parts.push(format!("hp:{}", st.hp));
    }
    if st.mp != 0 {
        parts.push(format!("mp:{}", st.mp));
    }
    if st.watk != 0 {
        parts.push(format!("watk:{}", st.watk));
    }
    if st.matk != 0 {
        parts.push(format!("matk:{}", st.matk));
    }
    if st.wdef != 0 {
        parts.push(format!("wdef:{}", st.wdef));
    }
    if st.mdef != 0 {
        parts.push(format!("mdef:{}", st.mdef));
    }
    if st.acc != 0 {
        parts.push(format!("acc:{}", st.acc));
    }
    if st.avoid != 0 {
        parts.push(format!("avoid:{}", st.avoid));
    }
    if st.speed != 0 {
        parts.push(format!("speed:{}", st.speed));
    }
    if st.jump != 0 {
        parts.push(format!("jump:{}", st.jump));
    }
    if parts.is_empty() {
        parts.push("no stats".to_string());
    }
    format!(
        "itemid={} lvl:{} req:{} slots:{}/{} [{}]",
        itemid,
        st.level,
        st.base_level,
        st.upgrade_slots,
        st.vicious + st.upgrade_slots as i32,
        parts.join(" ")
    )
}

/// xorshift64* simple PRNG (avoids a rand dependency); used for damage randomization.
fn dmg_rand_u32() -> u32 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEED: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
    let mut x = SEED.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    x as u32
}

pub(crate) fn reported_damage(state: &BotState) -> i32 {
    let base = if state.cfg.hunt.damage > 0 {
        // Sole source cfg.damage: fixed reported damage (aligned with the character's real damage;
        // the server validates plausibility; inflated estimates trigger anomaly detection).
        state.cfg.hunt.damage
    } else {
        let lvl = state.level as i32;
        // level² / 2: the server validates the client's reported damage for plausibility (high level
        // without stat points makes lvl² exceed the server cap, kicking with "abnormal behavior detected").
        // Dividing by 2 stays closer to the real basic-attack range of a normal stat panel at the same level.
        (lvl * lvl / 2).max(20)
    };
    // Randomize each attack within 90%~100%: real players' damage varies per hit, and fixed damage
    // can look like a script to the server, triggering anomaly detection; the range stays narrow to
    // avoid losing too much damage (an 80% range survived 3 minutes without disconnect, then tightened to 90%).
    let pct = 90 + (dmg_rand_u32() % 11) as i32; // 90..=100
    (base * pct / 100).max(20)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ap_commands_parse() {
        assert!(matches!(parse("view attr"), Ok(Some(Command::View(View::Attr)))));
        assert!(parse("ap").is_err()); // bare ap needs a stat
        assert!(matches!(
            parse("ap str"),
            Ok(Some(Command::Ap(ApCmd::Add(ApStat::Str, 1))))
        ));
        assert!(matches!(
            parse("AP luk 3"),
            Ok(Some(Command::Ap(ApCmd::Add(ApStat::Luk, 3))))
        ));
        assert!(matches!(
            parse("ap maxhp 2"),
            Ok(Some(Command::Ap(ApCmd::Add(ApStat::MaxHp, 2))))
        ));
        assert!(matches!(
            parse("ap status"),
            Ok(Some(Command::Ap(ApCmd::Status)))
        ));
        assert!(parse("ap bogus").is_err());
        assert!(parse("ap str 0").is_err());
        assert!(parse("ap str -1").is_err());
        assert!(parse("ap str banana").is_err());
        assert!(parse("ap auto on").is_err()); // removed
    }

    #[test]
    fn ap_stat_values_match_maplestat() {
        use crate::packets::stats::stat as st;
        assert_eq!(ap_stat_value(&ApStat::Str), st::STR);
        assert_eq!(ap_stat_value(&ApStat::Dex), st::DEX);
        assert_eq!(ap_stat_value(&ApStat::Int), st::INT);
        assert_eq!(ap_stat_value(&ApStat::Luk), st::LUK);
        assert_eq!(ap_stat_value(&ApStat::MaxHp), st::MAXHP);
        assert_eq!(ap_stat_value(&ApStat::MaxMp), st::MAXMP);
    }

    #[test]
    fn aoe_command_removed() {
        // aoe/pose removed: all attack modes go through hunt attack/skill
        assert!(parse("aoe").is_err());
        assert!(parse("aoe on").is_err());
        assert!(parse("pose 20").is_err());
        assert!(parse("anim 6").is_err());
    }

    #[test]
    fn inventory_commands_parse() {
        // view shortcuts were removed: all viewing goes through `view <obj>`
        assert!(parse("inv").is_err());
        assert!(parse("status").is_err());
        assert!(parse("mobs").is_err());
        // `bag` stays: composite backpack UI command (TUI-local popup with
        // sell/equip), not a pure view duplicate; headless = inventory view
        assert!(matches!(
            parse("bag"),
            Ok(Some(Command::View(View::Inventory)))
        ));
        assert!(matches!(
            parse("view inventory"),
            Ok(Some(Command::View(View::Inventory)))
        ));
        assert!(matches!(
            parse("view"),
            Ok(Some(Command::View(View::Status)))
        ));
        assert!(matches!(
            parse("view player"),
            Ok(Some(Command::View(View::Attr)))
        ));
        assert!(matches!(
            parse("view attr"),
            Ok(Some(Command::View(View::Attr)))
        ));
        assert!(matches!(
            parse("view mobs"),
            Ok(Some(Command::View(View::Mobs)))
        ));
        assert!(matches!(parse("view bogus"), Err(_)));
        assert!(matches!(parse("use 2000003"), Ok(Some(Command::Use(2000003)))));
        assert!(matches!(parse("drop 4000000"), Ok(Some(Command::Drop(4000000, 1, None)))));
        assert!(matches!(
            parse("drop 4000000 5"),
            Ok(Some(Command::Drop(4000000, 5, None)))
        ));
        assert!(matches!(
            parse("drop 4000000 5 slot 3"),
            Ok(Some(Command::Drop(4000000, 5, Some(3))))
        ));
        assert!(matches!(
            parse("scroll 2040000 1302000"),
            Ok(Some(Command::UseScroll(2040000, 1302000, false)))
        ));
        assert!(matches!(
            parse("scroll 2040000 1302000 bless"),
            Ok(Some(Command::UseScroll(2040000, 1302000, true)))
        ));
        assert!(matches!(
            parse("upgrade 2040000 1302000"),
            Ok(Some(Command::UseScroll(2040000, 1302000, false)))
        ));
        assert!(matches!(
            parse("dropmeso 1000"),
            Ok(Some(Command::DropMeso(1000)))
        ));
        assert!(parse("use").is_err());
    }

    #[test]
    fn equip_commands_parse() {
        assert!(matches!(
            parse("view equiped"),
            Ok(Some(Command::View(View::Equips)))
        ));
        assert!(matches!(
            parse("view eqpinfo 1302000"),
            Ok(Some(Command::View(View::EqpInfo(1302000))))
        ));
        assert!(parse("view eqpinfo").is_err());
        // removed aliases: view equips / view equip <id> / equipinfo / top-level
        // view shortcuts (equiped / eqpinfo / inv / mobs ...)
        assert!(parse("view equips").is_err());
        assert!(parse("view equip 1302000").is_err());
        assert!(parse("equipinfo 1302000").is_err());
        assert!(matches!(
            parse("equip 1302000"),
            Ok(Some(Command::Equip(EquipCmd::Equip(1302000))))
        ));
        assert!(matches!(
            parse("unequip 1002140"),
            Ok(Some(Command::Equip(EquipCmd::Unequip(1002140))))
        ));
        assert!(matches!(
            parse("takeoff 1002140"),
            Ok(Some(Command::Equip(EquipCmd::Unequip(1002140))))
        ));
        assert!(parse("equip").is_err());
    }

    #[test]
    fn skill_commands_parse() {
        assert!(matches!(
            parse("view skills"),
            Ok(Some(Command::View(View::Skills)))
        ));
        assert!(matches!(
            parse("sp"),
            Ok(Some(Command::View(View::Skills)))
        ));
        assert!(matches!(
            parse("skills"),
            Ok(Some(Command::View(View::Skills)))
        ));
        assert!(matches!(
            parse("skill learn 1001003"),
            Ok(Some(Command::Skill(SkillCmd::Learn(1001003))))
        ));
        assert!(matches!(
            parse("sp 1001003"),
            Ok(Some(Command::Skill(SkillCmd::Learn(1001003))))
        ));
        assert!(matches!(
            parse("skill cast 1001005 50001"),
            Ok(Some(Command::Skill(SkillCmd::Cast(1001005, 50001))))
        ));
        assert!(matches!(
            parse("cast 1001005 50001"),
            Ok(Some(Command::Skill(SkillCmd::Cast(1001005, 50001))))
        ));
        assert!(matches!(
            parse("skill info 1001005"),
            Ok(Some(Command::Skill(SkillCmd::Info(1001005))))
        ));
        assert!(matches!(
            parse("buff 1001004"),
            Ok(Some(Command::BuffSkill(1001004)))
        ));
        assert!(parse("skill bogus").is_err());
    }

    #[test]
    fn hunt_once_and_vars_parse() {
        assert!(matches!(parse("hunt once"), Ok(Some(Command::HuntOnce))));
        // setvar: name + value (value may contain spaces)
        assert!(matches!(
            parse("setvar boss_summoned 1"),
            Ok(Some(Command::SetVar(n, v))) if n == "boss_summoned" && v == "1"
        ));
        assert!(matches!(
            parse("setvar phase enter map now"),
            Ok(Some(Command::SetVar(n, v))) if n == "phase" && v == "enter map now"
        ));
        assert!(parse("setvar").is_err());
        assert!(parse("setvar only_name").is_err());
        // clearvar
        assert!(matches!(
            parse("clearvar boss_summoned"),
            Ok(Some(Command::ClearVar(n))) if n == "boss_summoned"
        ));
        assert!(parse("clearvar").is_err());
    }

    #[test]
    fn hunt_mode_parses() {
        assert!(matches!(
            parse("hunt attack"),
            Ok(Some(Command::HuntMode(HuntMode::Attack(None))))
        ));
        assert!(matches!(
            parse("hunt attack 4"),
            Ok(Some(Command::HuntMode(HuntMode::Attack(Some(4)))))
        ));
        assert!(matches!(
            parse("hunt skill"),
            Ok(Some(Command::HuntMode(HuntMode::Skill(None, None, None, None))))
        ));
        assert!(matches!(
            parse("hunt skill 1001005"),
            Ok(Some(Command::HuntMode(HuntMode::Skill(Some(1001005), None, None, None))))
        ));
        assert!(matches!(
            parse("hunt skill 0 6 200 7"),
            Ok(Some(Command::HuntMode(HuntMode::Skill(Some(0), Some(6), Some(200), Some(7)))))
        ));
        assert!(matches!(
            parse("hunt range 400"),
            Ok(Some(Command::HuntRange(400)))
        ));
        assert!(parse("hunt 传送").is_err());
        assert!(parse("hunt 普攻").is_err());
        assert!(parse("hunt bogus").is_err());
        assert!(matches!(parse("hunt status"), Ok(Some(Command::HuntStatus))));
        // removed aliases: hunt tp / hunt info / hunt all|map / walk|teleport
        assert!(parse("hunt tp").is_err());
        assert!(parse("hunt info").is_err());
        assert!(parse("hunt all").is_err());
        assert!(parse("hunt map").is_err());
        assert!(parse("hunt walk").is_err());
        assert!(parse("hunt teleport").is_err());
    }

    #[test]
    fn hunt_pickup_parses() {
        assert!(matches!(
            parse("hunt pickup on"),
            Ok(Some(Command::HuntPickup(true)))
        ));
        assert!(matches!(
            parse("hunt pickup off"),
            Ok(Some(Command::HuntPickup(false)))
        ));
        assert!(parse("hunt pickup").is_err());
        assert!(parse("hunt pickup true").is_err());
        assert!(parse("hunt pickup bogus").is_err());
        assert!(parse("hunt pickup -1").is_err());
        assert!(matches!(
            parse("hunt pickup 1"),
            Ok(Some(Command::HuntPickupRange(1)))
        ));
        assert!(matches!(
            parse("hunt pickup 400"),
            Ok(Some(Command::HuntPickupRange(400)))
        ));
        assert!(matches!(
            parse("hunt pickup 0"),
            Ok(Some(Command::HuntPickupRange(0)))
        ));
        assert!(matches!(
            parse("hunt damage 500"),
            Ok(Some(Command::HuntDamage(500)))
        ));
        assert!(matches!(
            parse("hunt damage 0"),
            Ok(Some(Command::HuntDamage(0)))
        ));
        assert!(parse("hunt damage -1").is_err());
        assert!(parse("hunt damage").is_err());
        assert!(matches!(
            parse("hunt atkcd 500"),
            Ok(Some(Command::HuntAtkCd(500)))
        ));
        assert!(matches!(
            parse("hunt atkcd 0"),
            Ok(Some(Command::HuntAtkCd(0)))
        ));
        assert!(parse("hunt atkcd").is_err());
        assert!(parse("hunt atkcd abc").is_err());
        assert!(matches!(parse("hunt save"), Ok(Some(Command::HuntSave))));
        assert!(matches!(
            parse("hunt filter"),
            Ok(Some(Command::HuntFilter(HuntFilterCmd::Status)))
        ));
        assert!(matches!(
            parse("hunt filter off"),
            Ok(Some(Command::HuntFilter(HuntFilterCmd::Off)))
        ));
        assert!(matches!(
            parse("hunt filter allow 2000000 2000001"),
            Ok(Some(Command::HuntFilter(HuntFilterCmd::SetAllow(v)))) if v == vec![2000000, 2000001]
        ));
        assert!(matches!(
            parse("hunt filter deny 4000000"),
            Ok(Some(Command::HuntFilter(HuntFilterCmd::SetDeny(v)))) if v == vec![4000000]
        ));
        assert!(matches!(
            parse("hunt filter add 2000002"),
            Ok(Some(Command::HuntFilter(HuntFilterCmd::Add(v)))) if v == vec![2000002]
        ));
        assert!(matches!(
            parse("hunt filter del 2000000"),
            Ok(Some(Command::HuntFilter(HuntFilterCmd::Del(v)))) if v == vec![2000000]
        ));
        assert!(parse("hunt filter + 2000002").is_err(), "legacy '+' alias removed");
        assert!(parse("hunt filter - 2000000").is_err(), "legacy '-' alias removed");
        assert!(parse("hunt filter allow").is_ok(), "allow without ids = switch mode only");
        assert!(parse("hunt filter deny").is_ok(), "deny without ids = switch mode only");
        assert!(parse("hunt filter add").is_err(), "add still requires ids");
        assert!(parse("hunt filter bogus 1").is_err());
    }

    #[test]
    fn equip_slot_mapping() {
        assert_eq!(equip_slot(1002140), Some(-1)); // cap
        assert_eq!(equip_slot(1040013), Some(-5)); // top
        assert_eq!(equip_slot(1050000), Some(-5)); // overall
        assert_eq!(equip_slot(1072001), Some(-7)); // gloves
        assert_eq!(equip_slot(1302000), Some(-11)); // sword
        assert_eq!(equip_slot(1492001), Some(-11)); // claw
        assert_eq!(equip_slot(1112000), Some(-12)); // ring
        assert_eq!(equip_slot(1122000), Some(-16)); // pendant
        assert_eq!(equip_slot(2020003), None); // potion
        assert_eq!(equip_slot(1802000), None); // pet equip
    }

    #[test]
    fn reported_damage_scales_with_level_under_cap() {
        let mut s = BotState::default();
        s.cfg.hunt.damage = 0;
        s.level = 3;
        assert_eq!(reported_damage(&s), 20); // 4 -> floored to 20
        s.level = 5;
        assert_eq!(reported_damage(&s), 20); // 12 -> floored to 20
        s.level = 10;
        // lvl^2/2 = 50, 90%~100% variance → [45, 50]
        for _ in 0..50 {
            let d = reported_damage(&s);
            assert!((45..=50).contains(&d), "dmg={d}");
        }
        s.level = 30;
        // lvl^2/2 = 450, variance → [405, 450]
        for _ in 0..50 {
            let d = reported_damage(&s);
            assert!((405..=450).contains(&d), "dmg={d}");
        }
        // The variance must actually vary (50 samples should show at least 2 distinct values)
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..50 {
            seen.insert(reported_damage(&s));
        }
        assert!(seen.len() >= 2, "damage must vary, got {seen:?}");
        s.cfg.hunt.damage = 12345;
        for _ in 0..20 {
            let d = reported_damage(&s);
            assert!((11110..=12345).contains(&d), "dmg={d}"); // 12345*90/100 floored
        }
        s.cfg.hunt.damage = 5;
        assert_eq!(reported_damage(&s), 20); // floor 20; variance doesn't change the floor
    }
}






