//! Bot state and shared data structs.

use std::collections::BTreeMap;

use crate::runtime_config::RuntimeConfig;

/// A world from SERVERLIST.
#[derive(Debug, Default, Clone)]
pub struct World {
    pub wid: i8,
    pub name: String,
    pub flag: u8,
    pub message: String,
    pub channelcount: u8,
    pub chloads: Vec<i32>,
}

/// Character entry from CHARLIST.
#[derive(Debug, Default, Clone)]
pub struct CharEntry {
    pub id: i32,
    pub stats: StatsEntry,
    pub look: LookEntry,
}

#[derive(Debug, Default, Clone)]
pub struct StatsEntry {
    pub name: String,
    pub female: bool,
    pub job: i16,
    pub level: u8,
    pub str: i16,
    pub dex: i16,
    pub int: i16,
    pub luk: i16,
    pub hp: i16,
    pub maxhp: i16,
    pub mp: i16,
    pub maxmp: i16,
    pub ap: i16,
    pub sp: i16,
    pub exp: i32,
    pub fame: i16,
    pub mapid: i32,
    pub portal: u8,
}

#[derive(Debug, Default, Clone)]
pub struct LookEntry {
    pub female: bool,
    pub skin: u8,
    pub faceid: i32,
    pub hairid: i32,
    pub equips: BTreeMap<i8, i32>,
    pub maskedequips: BTreeMap<i8, i32>,
}

/// A living entity on the current map (player or mob).
#[derive(Debug, Clone)]
pub struct Entity {
    pub oid: i32,
    pub kind: EntityKind,
    pub mobid: i32,
    pub x: i16,
    pub y: i16,
    pub fh: i16,
    pub stance: u8,
    pub charname: String,
    /// remaining HP % from the latest 0xFC mobhp (sent only to the attacker);
    /// 0% = death confirmation fallback (removed immediately when 0xEF is
    /// delayed/lost). Unknown at spawn.
    pub hp_pct: Option<u8>,
    /// whether the mob's controller is the bot (0xF0 SPAWN_MONSTER_CONTROL aggro=1/2).
    pub controlled: bool,
}

impl Default for Entity {
    fn default() -> Self {
        Self {
            oid: 0,
            kind: EntityKind::Unknown,
            mobid: 0,
            x: 0,
            y: 0,
            fh: 0,
            stance: 0,
            charname: String::new(),
            hp_pct: None,
            controlled: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EntityKind {
    #[default]
    Unknown,
    Mob,
    Player,
}

/// A dropped item on the map (DROP_ITEM_FROM_MAPOBJECT 0x110).
#[derive(Debug, Clone, Default)]
pub struct Drop {
    pub oid: i32,
    pub itemid: i32,
    /// True = this is a meso (gold) drop; `itemid` then holds the meso AMOUNT,
    /// not an item id. Meso always bypasses the pickup filter.
    pub is_meso: bool,
    pub x: i16,
    pub y: i16,
    /// When we last sent a pickup for this drop. The server silently ignores
    /// denied pickups (other players' items / duplicate unique items), so we
    /// wait a bit before retrying instead of spamming.
    pub last_try: Option<std::time::Instant>,
    /// Consecutive pickup attempts without a 0x111 removal; at MAX_DROP_TRIES
    /// the drop is abandoned.
    pub tries: u32,
}

impl Drop {
    pub const MAX_DROP_TRIES: u32 = 3;
    /// Cooldown between pickup attempts of a denied drop.
    pub const RETRY_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(2);
}

/// One member of the bot's party.
#[derive(Debug, Clone, Default)]
pub struct PartyMember {
    pub id: i32,
    pub name: String,
    pub job: i32,
    pub level: i32,
    pub channel: i32,
    pub mapid: i32,
}

/// The bot's current party.
#[derive(Debug, Clone, Default)]
pub struct Party {
    pub partyid: i32,
    pub leader_id: i32,
    pub members: BTreeMap<i32, PartyMember>,
}

/// An NPC on the map, tracked by OID.
#[derive(Debug, Clone, Copy)]
pub struct Npc {
    pub oid: i32,
    pub npcid: i32,
    pub x: i16,
    pub y: i16,
}

/// A map reactor (breakable/harvestable prop), tracked by OID. `state` is the
/// reactor's current state byte (0-based); the server advances it on each hit
/// and destroys the reactor when the final state is reached.
#[derive(Debug, Clone, Copy)]
pub struct Reactor {
    pub oid: i32,
    pub rid: i32,
    pub state: u8,
    pub x: i16,
    pub y: i16,
}

/// An item in an NPC shop.
#[derive(Debug, Clone, Copy)]
pub struct ShopItem {
    pub itemid: i32,
    pub price: i32,
    pub qty: i16,
}

/// One inventory item. `item_type` is the item's own type byte from
/// `addItemInfo` (1=equip, 2=use, 3=setup, 4=etc, 5=cash); `qty` is 0 for equips.
/// Equips carry their real stats in `stats` (from `PacketHelper.addEquipStats`).
/// `unique_id` is the 8-byte unique id when the item has one (cash/point items,
/// pets), 0 otherwise.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Item {
    pub itemid: i32,
    pub qty: i16,
    pub item_type: u8,
    pub stats: Option<EquipStats>,
    pub unique_id: i64,
}

/// Worn-equip stat block (`addEquipStats`): all the numeric stats an equip
/// carries. Slot info is the map key of the equipped tab, not stored here.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EquipStats {    /// scroll slots remaining
    pub upgrade_slots: u8,
    /// star/scroll level
    pub level: u8,
    pub str: i16,
    pub dex: i16,
    pub int: i16,
    pub luk: i16,
    pub hp: i16,
    pub mp: i16,
    pub watk: i16,
    pub matk: i16,
    pub wdef: i16,
    pub mdef: i16,
    pub acc: i16,
    pub avoid: i16,
    pub hands: i16,
    pub speed: i16,
    pub jump: i16,
    /// item flag bits (untradeable etc.)
    pub flag: u16,
    /// 1 when the equip grants a skill
    pub inc_skill: u8,
    /// required level of the equip
    pub base_level: u8,
    /// experience percentage (as stored on the wire, /100000)
    pub exp_percent: i32,
    /// vicious hammer uses
    pub vicious: i32,
}

/// One learned skill (from `addSkillInfo` / `UPDATE_SKILLS 0x27`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SkillEntry {
    /// current skill level
    pub level: u8,
    /// master level (4th job only, 0 otherwise)
    pub masterlevel: u8,
}

/// Current bot phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Disconnected,
    Connecting,
    LoggingIn,
    GenderPick,
    WorldSelect,
    CharSelect,
    EnteringMap,
    InGame,
    /// connected to the cash shop server (0x83 received)
    CashShop,
}

/// One item in the cash shop inventory (from CS_OPERATION 66 / 76).
#[derive(Debug, Clone, Copy, Default)]
pub struct CashItem {
    pub unique_id: i64,
    pub accid: i64,
    pub itemid: i32,
    pub sn: i32,
    pub qty: i16,
}

/// One sale entry of the cash shop (from SET_CASH_SHOP 0x83 `addModCashItemInfo`).
/// `price` is the discount price when present; the base price comes from the
/// shop database and is not sent.
#[derive(Debug, Clone, Copy, Default)]
pub struct CashShopItem {
    pub sn: i32,
    pub itemid: i32,
    pub count: i16,
    pub price: i32,
    pub period: i16,
    pub meso: i32,
    pub gender: u8,
    pub mark: u8,
    pub priority: u8,
    pub show_up: bool,
}

/// In-flight `reenter` round trip (cash shop forced re-entry).
///
/// The bot enters the cash shop (0x23), immediately leaves it (0x21 on the CS
/// server = `LeaveCashShop`), and the server drops the character back onto
/// the SAME map via the CharacterTransfer handoff — refreshing all map
/// objects without any login notice. `leave_sent` distinguishes the phases:
/// `false` = still waiting to enter the CS (denied if the round trip times
/// out in-game), `true` = leave packet sent, now waiting for SET_FIELD back
/// on `mapid`.
#[derive(Debug, Clone)]
pub struct ReenterCtx {
    /// hunt was on when `reenter` ran; restored when back on the origin map
    pub restore_hunt: bool,
    /// origin map id; the round trip only "completes" if SET_FIELD lands here
    pub mapid: i32,
    /// whether the CS-leave packet (0x21) has been sent
    pub leave_sent: bool,
    /// when the `reenter` command ran (watchdog: ~10s to reach the CS)
    pub started: std::time::Instant,
}

/// A pending trade invite (PLAYER_INTERACTION action 2).
#[derive(Debug, Clone)]
pub struct TradeInvite {
    pub name: String,
    pub time: std::time::Instant,
}

/// Player trade window state (PLAYER_INTERACTION 0x14F / 0x77).
#[derive(Debug, Clone, Default)]
pub struct TradeState {
    /// whether a trade window is currently open
    pub active: bool,
    /// partner character name
    pub partner: String,
    /// our slot in the trade (0 = inviter, 1 = invitee)
    pub my_slot: u8,
    /// meso the partner has put up
    pub meso_partner: i32,
    /// (itemid, qty) the partner has put up
    pub items_partner: Vec<(i32, i16)>,
    /// partner has confirmed (locked)
    pub partner_locked: bool,
    /// we have confirmed (locked)
    pub locked: bool,
    /// pending invite, cleared once the window opens
    pub invite: Option<TradeInvite>,
}

/// Mutable bot state.
#[derive(Debug)]
pub struct BotState {
    pub phase: Phase,
    pub worlds: Vec<World>,
    pub characters: Vec<CharEntry>,
    pub selected_world: i8,
    /// channel chosen for this session (1-based; picker / login world /
    /// config.channel). Runtime-only: reused by reconnect (relogin) instead of
    /// config.channel; never persisted to config.json.
    pub selected_channel: i32,
    /// current channel (1-based), last selected/connected (wire value + 1)
    pub channel: i32,
    pub my_cid: i32,
    /// current character name (from charinfo)
    pub name: String,
    /// reconnect flag (set true when auto-reconnecting after a disconnect):
    /// the login flow skips interaction and silently re-logs in with the last
    /// remembered world/charlist (returning to the game, not a first login).
    pub relogin: bool,
    /// has this process EVER reached in-game (any session)? Never reset by
    /// reset_session — the reconnect gate ("reconnect requires having
    /// connected once"); survives mid-relogin drops and server-down windows.
    pub ever_in_game: bool,
    pub mapid: i32,
    /// town map ID for map change (e.g. 100000000 = Henesys)
    pub town_mapid: i32,
    /// town portal ID for spawning in town
    pub town_portal: String,
    pub position: (i16, i16),
    /// Ground level (y) learned from the nearest mob; used for walking so the
    /// character stays on a foothold.
    pub ground_y: i16,
    pub hp: i16,
    pub maxhp: i16,
    pub mp: i16,
    pub maxmp: i16,
    /// the four stats (STR/DEX/INT/LUK), from charinfo / 0x22 updates
    pub str: i16,
    pub dex: i16,
    pub int: i16,
    pub luk: i16,
    /// entities keyed by oid
    pub entities: BTreeMap<i32, Entity>,
    /// map drops keyed by oid
    pub drops: BTreeMap<i32, Drop>,
    /// experience / level (from UPDATE_STATS 0x22)
    pub exp: i32,
    pub level: u8,
    /// autonomous hunt mode (nearest mob + loot loop)
    pub hunt: bool,
    /// autonomous reactor farming: hit every reactor + insta-pickup drops
    pub hunt_reactor: bool,
    /// reactor farming per-tick summary-log throttle (packets every tick;
    /// at most one log line per second)
    pub last_rhunt_log: std::time::Instant,
    /// reactor hit round gate (reactor_cooldown ms, 0 = every tick)
    pub last_rhunt_attack: std::time::Instant,
    /// gather on: pull out-of-range mobs toward the character (MOVE_LIFE 0xB7)
    /// for stand-and-fight hunting; small steps (≤150px) avoid MOB_VAC detection.
    pub gather: bool,
    /// only gather mobs the server assigned to us (`gather controller`). Default true.
    pub gather_controller_only: bool,
    /// stand move mode (hunt stand on): stay in place instead of chasing mobs,
    /// hunting from a stand with gather.
    pub hunt_stand: bool,
    /// last animation state sent (for stand/stance control)
    pub last_stance: u8,
    /// facing direction (0 = right, 1 = left)
    pub facing: u8,
    /// last attack time (attack cooldown)
    pub last_attack: std::time::Instant,
    /// last time a mob died (0xEF) — used to detect "stuck" hunting loops where
    /// the bot keeps attacking mobs that never die (stale oids / skill-immune).
    pub last_kill: std::time::Instant,
    /// the mob oid the single-target hunt is committed to (prevents oscillating
    /// between nearest mobs every tick while chasing).
    pub hunt_target: Option<i32>,
    /// remaining ability points (from UPDATE_STATS AVAILABLEAP bit, short)
    pub ap: i16,
    /// last time AP was distributed (distribution cooldown)
    pub last_ap: std::time::Instant,
    /// inventory keyed by (tab, slot); tab: 1=EQUIP 2=USE 3=SETUP 4=ETC 5=CASH
    pub inventory: BTreeMap<(u8, i16), Item>,
    /// currently worn equipment keyed by body slot (negative: -1 cap .. -11 weapon)
    pub equipped: BTreeMap<i16, Item>,
    /// learned skills keyed by skillid (from charinfo / 0x27)
    pub skills: BTreeMap<i32, SkillEntry>,
    /// remaining skill points (from charinfo / 0x22 AVAILABLESP)
    pub sp: i16,
    /// current meso (from charinfo / MESO updates)
    pub meso: i32,
    /// last potion use time (server consume cooldown guard)
    pub last_potion: std::time::Instant,
    /// current party, if any
    pub party: Option<Party>,
    /// oid of the mob the bot is targeting
    pub attack_target: Option<i32>,
    /// whether the post-entry discovery move has been sent
    pub discovery_sent: bool,
    /// when the bot entered the current map
    pub map_enter_time: std::time::Instant,
    /// NPCs on the current map, keyed by OID
    pub npcs: BTreeMap<i32, Npc>,
    /// reactors on the current map, keyed by OID
    pub reactors: BTreeMap<i32, Reactor>,
    /// keybindings from KEYMAP (0x16F): 90 fixed keys 0-89, each (type, action).
    /// type 0 = empty, 1 = skill, 2 = item, others = UI/macro etc.
    pub keymap: Vec<(u8, i32)>,
    /// whether an NPC shop is currently open
    pub shop_open: bool,
    /// whether an NPC dialog is currently open (server NPC_TALK arrived and no
    /// dialog-advancing command was sent since). Set true by handle_npc_talk,
    /// cleared optimistically when the bot sends a reply/next/select etc.
    /// Drives the `dialog` predicate (`wait: "dialog==1"` in tasks).
    pub dialog_open: bool,
    /// items in the currently open NPC shop
    pub shop_items: Vec<ShopItem>,
    /// bumped every time the server opens/refreshes the NPC shop (0x146) or
    /// the shop closes: the TUI shop picker detects "a fresh item list arrived"
    /// by comparing this sequence.
    pub shop_seq: u64,
    /// last NPC dialog message type (for NPC_TALK_MORE responses)
    pub last_npc_msg_type: u8,
    /// options of the current NPC sendSimple menu (msg_type 4): (option id, cleaned label).
    /// Filled by handle_npc_talk when the server sends a menu; cleared when a
    /// non-menu dialog arrives or on map change. The TUI option picker reads this.
    pub npc_options: Vec<(i32, String)>,
    /// bumped on every NPC dialog content from the server (menu or not):
    /// the TUI detects "a new dialog arrived" by comparing this sequence.
    pub npc_options_seq: u64,
    /// last teleport time (teleport cooldown window; delay comes from
    /// `cfg.hunt.teleport_delay`)
    pub last_teleport: std::time::Instant,
    /// map the sell task should return to after selling (snapshot on task start)
    pub hunt_mapid: i32,
    /// gather throttle (records the time after each pull round)
    pub last_gather: std::time::Instant,
    /// gather moveid table: incremented independently per mob (per-mob moveids
    /// increase steadily; a fixed 0 or a shared counter looks scripted; per-mob
    /// counters keep the server from rejecting regressions)
    pub gather_moveids: std::collections::HashMap<i32, i16>,
    /// 0x15 STRANGE_DATA periodic report timer (official client: ~3-4 times/sec)
    pub last_strange: std::time::Instant,
    /// last position synced to the server via a move packet: when position
    /// changes (e.g. spawn overwrote local coords) unsynced, resend a move
    /// packet, otherwise the server-side character stays at the old spot (stuck in air).
    pub last_sent_pos: (i16, i16),
    /// player trade state
    pub trade: TradeState,
    /// cash shop NX credit balance (from CS_UPDATE 0x161)
    pub cs_nx: i32,
    /// cash shop reward-point balance (0x161 second int)
    pub cs_points: i32,
    /// items stored in the cash shop inventory (CS_OPERATION 66 / 76)
    pub cs_inventory: Vec<CashItem>,
    /// cash shop sale list (SET_CASH_SHOP 0x83 `addModCashItemInfo` entries)
    pub cs_items: Vec<CashShopItem>,
    /// in-flight `reenter` round trip: enter the cash shop, then immediately
    /// leave it — the server drops the character back onto the SAME map
    /// (CharacterTransfer handoff, no login notice), refreshing all map
    /// objects. `Some` only between the `reenter` command and the SET_FIELD
    /// that lands back on the origin map.
    pub reenter: Option<ReenterCtx>,
    /// runtime configuration (config.json, mutable via commands)
    pub cfg: RuntimeConfig,
    /// last fire time of each rule (keyed by rule id)
    pub rule_last_fire: BTreeMap<String, std::time::Instant>,
    /// user-defined flow variables (`setvar <name> <value>` / `clearvar <name>`,
    /// checked via the `var==<name>:<value>` predicate). Manual lifecycle only —
    /// nothing clears them implicitly.
    pub vars: BTreeMap<String, String>,
    /// task lock stack: the top holder runs while lock-free behaviors (hunt,
    /// etc.) yield. Nested tasks A→B→C pop and release as C→B→A. Empty stack
    /// = no lock, default behavior resumes.
    pub task_stack: Vec<crate::runtime_config::TaskRuntime>,
}

impl Default for BotState {
    fn default() -> Self {
        Self {
            phase: Phase::Disconnected,
            worlds: Vec::new(),
            characters: Vec::new(),
            selected_world: -1,
            selected_channel: -1,
            channel: 1,
            my_cid: 0,
            name: String::new(),
            relogin: false,
            ever_in_game: false,
            mapid: 0,
            town_mapid: 100000000, // Henesys default
            town_portal: "sp".to_string(),
            position: (0, 0),
            ground_y: 0,
            hp: 0,
            maxhp: 0,
            mp: 0,
            maxmp: 0,
            str: 0,
            dex: 0,
            int: 0,
            luk: 0,
            entities: BTreeMap::new(),
            drops: BTreeMap::new(),
            exp: 0,
            level: 0,
            hunt: false,
            hunt_reactor: false,
            last_rhunt_log: std::time::Instant::now(),
            last_rhunt_attack: std::time::Instant::now(),
            gather: false,
            gather_controller_only: true,
            hunt_stand: false,

            last_stance: 0,
            facing: 1,
            last_attack: std::time::Instant::now() - std::time::Duration::from_secs(60),
            last_kill: std::time::Instant::now(),
            hunt_target: None,
            ap: 0,
            last_ap: std::time::Instant::now() - std::time::Duration::from_secs(60),
            inventory: BTreeMap::new(),
            equipped: BTreeMap::new(),
            skills: BTreeMap::new(),
            sp: 0,
            meso: 0,
            last_potion: std::time::Instant::now() - std::time::Duration::from_secs(60),
            party: None,
            attack_target: None,
            discovery_sent: false,
            map_enter_time: std::time::Instant::now(),
            npcs: BTreeMap::new(),
            reactors: BTreeMap::new(),
            keymap: Vec::new(),
            shop_open: false,
            dialog_open: false,
            shop_items: Vec::new(),
            shop_seq: 0,
            last_npc_msg_type: 0,
            npc_options: Vec::new(),
            npc_options_seq: 0,
            last_teleport: std::time::Instant::now() - std::time::Duration::from_secs(60),
            hunt_mapid: 0,
            last_gather: std::time::Instant::now(),
            gather_moveids: std::collections::HashMap::new(),
            last_strange: std::time::Instant::now(),
            last_sent_pos: (0, 0),
            trade: TradeState::default(),
            cs_nx: 0,
            cs_points: 0,
            cs_inventory: Vec::new(),
            cs_items: Vec::new(),
            reenter: None,
            cfg: RuntimeConfig::default(),
            rule_last_fire: BTreeMap::new(),
            vars: BTreeMap::new(),
            task_stack: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_defaults_are_sane() {
        let s = BotState::default();
        assert_eq!(s.entities.len(), 0);
        assert_eq!(s.drops.len(), 0);
        assert!(!s.hunt);
        assert_eq!(s.position, (0, 0));
    }
}

impl BotState {
    /// Clear cash shop caches when leaving the shop (the sale list 0x83 / cash
    /// inventory 66 can hold thousands of entries; keeping them in memory is
    /// pointless — the shop re-pushes them on the next visit).
    pub fn clear_cash_shop_cache(&mut self) {
        self.cs_items.clear();
        self.cs_inventory.clear();
        self.cs_nx = 0;
        self.cs_points = 0;
    }

    /// Drop on-the-ground items that the current pickup filter rejects
    /// (meso always kept). Called after a config reload so items already on
    /// the map follow the new filter immediately.
    pub fn prune_filtered_drops(&mut self) {
        let cfg = &self.cfg.hunt;
        self.drops.retain(|_, d| {
            d.is_meso || crate::runtime_config::pickup_allowed(cfg, d.itemid)
        });
    }
}

#[cfg(test)]
mod drop_prune_tests {
    use super::*;

    fn drop(oid: i32, itemid: i32, is_meso: bool) -> Drop {
        Drop { oid, itemid, is_meso, ..Default::default() }
    }

    #[test]
    fn prune_removes_denied_items_but_keeps_meso() {
        let mut s = BotState::default();
        s.drops.insert(1, drop(1, 4000000, false));
        s.drops.insert(2, drop(2, 2000000, false));
        s.drops.insert(3, drop(3, 500, true));
        // deny 4000000 → drop 1 must go; 2000000 and meso stay
        s.cfg.hunt.pickup_filter_mode = "deny".into();
        s.cfg.hunt.pickup_deny = vec![4000000];
        s.prune_filtered_drops();
        assert_eq!(s.drops.len(), 2);
        assert!(!s.drops.contains_key(&1));
        assert!(s.drops.contains_key(&2));
        assert!(s.drops.contains_key(&3));
    }

    #[test]
    fn prune_whitelist_mode_removes_everything_not_listed() {
        let mut s = BotState::default();
        s.drops.insert(1, drop(1, 4000000, false));
        s.drops.insert(2, drop(2, 4000006, false));
        s.drops.insert(3, drop(3, 500, true));
        s.cfg.hunt.pickup_filter_mode = "allow".into();
        s.cfg.hunt.pickup_allow = vec![4000006];
        s.prune_filtered_drops();
        assert_eq!(s.drops.len(), 2);
        assert!(s.drops.contains_key(&2), "whitelisted item kept");
        assert!(s.drops.contains_key(&3), "meso always kept");
    }
}





