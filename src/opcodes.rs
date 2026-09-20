//! CongMS 079 opcode tables.
//!
//! Values come from `handling/SendPacketOpcode` + `properties/send.ini`
//! (server -> client) and `handling/RecvPacketOpcode` + `properties/recv.ini`
//! (client -> server) in the CongMS `079.jar` (decompiled to
//! `dist/079-src/`). The header XOR version is 79 (handshake byte [2..3]).
//!
//! The opcode tables are copied wholesale from the server. Constants marked
//! `(reserved)` have no call sites yet but are kept on purpose: future features
//! can reference them by name without re-reversing 079.jar. Used constants
//! carry detailed layout comments.

// ---- Inbound (server -> client), from send.ini ----
pub mod recv {
    /// LOGIN_STATUS (login result / failure)
    pub const LOGIN_STATUS: u16 = 0x00;
    /// LICENSE_RESULT (reserved)
    pub const LICENSE_RESULT: u16 = 0x02;
    /// CHOOSE_GENDER (account needs gender selection)
    pub const CHOOSE_GENDER: u16 = 0x04;
    /// GENDER_SET (gender was set)
    pub const GENDER_SET: u16 = 0x05;
    /// SERVERSTATUS
    pub const SERVERSTATUS: u16 = 0x06;
    /// SERVERLIST
    pub const SERVERLIST: u16 = 0x09;
    /// CHARLIST
    pub const CHARLIST: u16 = 0x0A;
    /// SERVER_IP
    pub const SERVER_IP: u16 = 0x0B;
    /// CHANGE_CHANNEL (server -> client, reply to channel switch: ip + port, no cid)
    pub const CHANGE_CHANNEL: u16 = 0x13;
    /// CHAR_NAME_RESPONSE (reserved)
    pub const CHAR_NAME_RESPONSE: u16 = 0x0C;
    /// ADD_NEW_CHAR_ENTRY (reserved)
    pub const ADD_NEW_CHAR_ENTRY: u16 = 0x11;
    /// DELETE_CHAR_RESPONSE (reserved)
    pub const DELETE_CHAR_RESPONSE: u16 = 0x12;
    /// PING
    pub const PING: u16 = 0x14;
    /// CS_USE (cash shop enabled)
    pub const CS_USE: u16 = 0x15;

    /// MODIFY_INVENTORY_ITEM
    pub const MODIFY_INVENTORY_ITEM: u16 = 0x20;
    /// UPDATE_STATS
    pub const UPDATE_STATS: u16 = 0x22;
    /// GIVE_BUFF (reserved)
    pub const GIVE_BUFF: u16 = 0x23;
    /// CANCEL_BUFF (reserved)
    pub const CANCEL_BUFF: u16 = 0x24;
    /// UPDATE_SKILLS
    pub const UPDATE_SKILLS: u16 = 0x27;
    /// SHOW_STATUS_INFO (reserved)
    pub const SHOW_STATUS_INFO: u16 = 0x2A;

    /// CHAR_INFO (reserved)
    pub const CHAR_INFO: u16 = 0x3A;
    /// PARTY_OPERATION — server→client = 0x3B (live capture: the server replies 3B 00 08 to confirm creation)
    pub const PARTY_OPERATION: u16 = 0x3B;
    /// BUDDYLIST (reserved)
    pub const BUDDYLIST: u16 = 0x3C;
    /// GUILD_OPERATION (reserved)
    pub const GUILD_OPERATION: u16 = 0x3E;
    /// SERVERMESSAGE
    pub const SERVERMESSAGE: u16 = 0x41;

    /// SET_FIELD
    pub const SET_FIELD: u16 = 0x81;
    /// SET_ITC
    pub const SET_ITC: u16 = 0x82;
    /// SET_CASH_SHOP
    pub const SET_CASH_SHOP: u16 = 0x83;

    /// CS_UPDATE (cash shop balance: int nx + int points)
    pub const CS_UPDATE: u16 = 0x161;
    /// CS_OPERATION (cash shop sub-operation: 66 inv, 68 gifts, 76 bought, ...)
    pub const CS_OPERATION: u16 = 0x162;

    /// MULTICHAT
    pub const MULTICHAT: u16 = 0x8A;
    /// WHISPER
    pub const WHISPER: u16 = 0x8B;
    /// SET_WEEK_EVENT_MESSAGE (yellow GM chat)
    pub const SET_WEEK_EVENT_MESSAGE: u16 = 0x4E;

    /// SPAWN_PLAYER (another player entered the map)
    pub const SPAWN_PLAYER: u16 = 0xA2;
    /// REMOVE_PLAYER_FROM_MAP
    pub const REMOVE_PLAYER_FROM_MAP: u16 = 0xA3;
    /// CHATTEXT
    pub const CHATTEXT: u16 = 0xA4;

    /// MOVE_PLAYER broadcast (other players' movements)
    pub const MOVE_PLAYER: u16 = 0xBB;
    /// CLOSE_RANGE_ATTACK broadcast (other players attacking)
    pub const CLOSE_RANGE_ATTACK: u16 = 0xBC;

    /// SPAWN_MONSTER
    pub const SPAWN_MONSTER: u16 = 0xEE;
    /// KILL_MONSTER
    pub const KILL_MONSTER: u16 = 0xEF;
    /// SPAWN_MONSTER_CONTROL
    pub const SPAWN_MONSTER_CONTROL: u16 = 0xF0;
    /// MOVE_MONSTER
    pub const MOVE_MONSTER: u16 = 0xF1;
    /// DAMAGE_MONSTER
    pub const DAMAGE_MONSTER: u16 = 0xF8;
    /// SHOW_MONSTER_HP
    pub const SHOW_MONSTER_HP: u16 = 0xFC;

    /// SPAWN_NPC
    pub const SPAWN_NPC: u16 = 0x104;
    /// REMOVE_NPC
    pub const REMOVE_NPC: u16 = 0x105;
    /// SPAWN_NPC_REQUEST_CONTROLLER (reserved)
    pub const SPAWN_NPC_REQUEST_CONTROLLER: u16 = 0x106;
    /// NPC_ACTION (reserved)
    pub const NPC_ACTION: u16 = 0x107;

    /// REACTOR_HIT (reactor state animation advance)
    pub const REACTOR_HIT: u16 = 0x11C;
    /// REACTOR_SPAWN (reactor appeared on the map)
    pub const REACTOR_SPAWN: u16 = 0x11E;
    /// REACTOR_DESTROY (reactor destroyed / despawned)
    pub const REACTOR_DESTROY: u16 = 0x11F;

    /// DROP_ITEM_FROM_MAPOBJECT
    pub const DROP_ITEM_FROM_MAPOBJECT: u16 = 0x110;
    /// REMOVE_ITEM_FROM_MAP
    pub const REMOVE_ITEM_FROM_MAP: u16 = 0x111;

    /// NPC_TALK
    pub const NPC_TALK: u16 = 0x145;
    /// OPEN_NPC_SHOP
    pub const OPEN_NPC_SHOP: u16 = 0x146;
    /// CONFIRM_SHOP_TRANSACTION
    pub const CONFIRM_SHOP_TRANSACTION: u16 = 0x147;
    /// OPEN_STORAGE (reserved)
    pub const OPEN_STORAGE: u16 = 0x14A;
    /// PLAYER_INTERACTION
    pub const PLAYER_INTERACTION: u16 = 0x14F;
    /// KEYMAP
    pub const KEYMAP: u16 = 0x16F;
}

// ---- Outbound (client -> server), from recv.ini ----
pub mod send {
    /// LOGIN_PASSWORD
    pub const LOGIN_PASSWORD: u16 = 0x01;
    /// SERVERLIST_REQUEST
    pub const SERVERLIST_REQUEST: u16 = 0x02;
    /// SET_GENDER
    pub const SET_GENDER: u16 = 0x04;
    /// SERVERSTATUS_REQUEST
    pub const SERVERSTATUS_REQUEST: u16 = 0x05;
    /// CHARLIST_REQUEST
    pub const CHARLIST_REQUEST: u16 = 0x09;
    /// CHAR_SELECT
    pub const CHAR_SELECT: u16 = 0x0A;
    /// PLAYER_LOGGEDIN
    pub const PLAYER_LOGGEDIN: u16 = 0x0B;
    /// CHECK_CHAR_NAME (reserved)
    pub const CHECK_CHAR_NAME: u16 = 0x0C;
    /// CREATE_CHAR (reserved)
    pub const CREATE_CHAR: u16 = 0x11;
    /// DELETE_CHAR (reserved)
    pub const DELETE_CHAR: u16 = 0x12;
    /// PONG
    pub const PONG: u16 = 0x13;

    /// CHANGE_MAP
    pub const CHANGE_MAP: u16 = 0x21;
    /// CHANGE_CHANNEL
    pub const CHANGE_CHANNEL: u16 = 0x22;
    /// ENTER_CASH_SHOP
    pub const ENTER_CASH_SHOP: u16 = 0x23;
    /// ENTER_MTS — open the auction (empty body; the client's "auction"
    /// button), used when REWARD_ITEM is blocked. recv.ini: ENTER_MTS = 0x8D.
    pub const ENTER_MTS: u16 = 0x8D;
    /// CS_UPDATE (reserved) (client requests a cash shop refresh — empty body)
    pub const CS_UPDATE: u16 = 0xE8;
    /// CASHSHOP_OPERATION (buy / take out / store: byte action + body)
    pub const CASHSHOP_OPERATION: u16 = 0xE9;
    /// MOVE_PLAYER
    pub const MOVE_PLAYER: u16 = 0x24;
    /// MOVE_LIFE (0xB7) — client→server: move a mob (controller-side "gather",
    /// the server parses it as MoveMonster and broadcasts it; same opcode as the
    /// server→client mob-move broadcast)
    pub const MOVE_LIFE: u16 = 0xB7;
    /// CANCEL_CHAIR (reserved)
    pub const CANCEL_CHAIR: u16 = 0x25;
    /// USE_CHAIR (reserved)
    pub const USE_CHAIR: u16 = 0x26;
    /// CLOSE_RANGE_ATTACK
    pub const CLOSE_RANGE_ATTACK: u16 = 0x28;
    /// RANGED_ATTACK (reserved)
    pub const RANGED_ATTACK: u16 = 0x29;
    /// MAGIC_ATTACK (reserved)
    pub const MAGIC_ATTACK: u16 = 0x2A;
    /// TAKE_DAMAGE (reserved)
    pub const TAKE_DAMAGE: u16 = 0x2C;
    /// GENERAL_CHAT
    pub const GENERAL_CHAT: u16 = 0x2D;

    /// CHANGE_MAP_SPECIAL — directly enters a portal by name, bypassing GM checks
    pub const CHANGE_MAP_SPECIAL: u16 = 0x61;
    /// USE_INNER_PORTAL (reserved)
    pub const USE_INNER_PORTAL: u16 = 0x62;

    /// NPC_TALK
    pub const NPC_TALK: u16 = 0x36;
    /// NPC_TALK_MORE
    pub const NPC_TALK_MORE: u16 = 0x38;
    /// NPC_SHOP
    pub const NPC_SHOP: u16 = 0x3A;
    /// STORAGE (reserved)
    pub const STORAGE: u16 = 0x3B;
    /// ITEM_MOVE
    pub const ITEM_MOVE: u16 = 0x44;
    /// USE_ITEM
    pub const USE_ITEM: u16 = 0x45;
    /// USE_UPGRADE_SCROLL — apply an upgrade scroll to an equip
    pub const USE_UPGRADE_SCROLL: u16 = 0x53;
    /// REWARD_ITEM — open reward box / special item (e.g. auction box 2022552)
    pub const REWARD_ITEM: u16 = 0x70;
    /// DISTRIBUTE_AP
    pub const DISTRIBUTE_AP: u16 = 0x54;
    /// AUTO_ASSIGN_AP (reserved) (client "auto assign" button — bulk-spends AP in one packet)
    pub const AUTO_ASSIGN_AP: u16 = 0x55;
    /// HEAL_OVER_TIME (reserved)
    pub const HEAL_OVER_TIME: u16 = 0x56;
    /// DISTRIBUTE_SP
    pub const DISTRIBUTE_SP: u16 = 0x57;
    /// GIVE_FAME (reserved)
    pub const GIVE_FAME: u16 = 0x5C;
    /// CHAR_INFO_REQUEST (reserved)
    pub const CHAR_INFO_REQUEST: u16 = 0x5E;
    /// QUEST_ACTION
    pub const QUEST_ACTION: u16 = 0x68;
    /// SKILL_MACRO (reserved)
    pub const SKILL_MACRO: u16 = 0x6D;
    /// PARTYCHAT (reserved)
    pub const PARTYCHAT: u16 = 0x74;
    /// WHISPER
    pub const WHISPER: u16 = 0x75;
    /// MESSENGER (reserved) (chat room)
    pub const MESSENGER: u16 = 0x76;
    /// PLAYER_INTERACTION
    pub const PLAYER_INTERACTION: u16 = 0x77;
    /// PARTY_OPERATION — client→server = 0x78 (live capture: the client sends 78 00 01 to create)
    pub const PARTY_OPERATION: u16 = 0x78;
    /// DENY_PARTY_REQUEST (respond to party invite; 27 = accept/join)
    pub const DENY_PARTY_REQUEST: u16 = 0x79;
    /// GUILD_OPERATION (reserved)
    pub const GUILD_OPERATION: u16 = 0x7A;
    /// BUDDYLIST_MODIFY (reserved)
    pub const BUDDYLIST_MODIFY: u16 = 0x7E;
    /// CHANGE_KEYMAP
    pub const CHANGE_KEYMAP: u16 = 0x83;
    /// MOVE_PET (reserved)
    pub const MOVE_PET: u16 = 0xA5;
    /// PET_CHAT (reserved)
    pub const PET_CHAT: u16 = 0xA6;
    /// MOVE_SUMMON (reserved)
    pub const MOVE_SUMMON: u16 = 0xAD;
    /// ITEM_PICKUP
    pub const ITEM_PICKUP: u16 = 0xC6;
    /// DAMAGE_REACTOR — hit a reactor (oid + charPos + stance; the server does
    /// NOT validate distance/cooldown, only oid liveness)
    pub const DAMAGE_REACTOR: u16 = 0xC9;
    /// MESO_DROP
    pub const MESO_DROP: u16 = 0x5B;
    /// SPECIAL_MOVE — cast a buff/assist skill (no target)
    pub const SPECIAL_MOVE: u16 = 0x58;
    /// CANCEL_BUFF — cancel a buff by skill id (int sourceid)
    pub const CANCEL_BUFF: u16 = 0x59;
}

