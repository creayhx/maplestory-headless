//! Party-operation packet handlers (server -> client 0x3B).
//! Captured from real traffic (party.txt/party2.txt/party3.txt, server-side view [sent]):
//! 07=full status (pushed on login/map entry) 08=create confirm (partyid assigned by server) 0F=full sync (after a member joins)
//! 0C=change notice (member left/kicked) 04=invite/join request 02=leave/disband confirm 10(16)=invite failed.

use std::collections::BTreeMap;

use crate::packet::Cursor;
use crate::session::Session;
use crate::state::{BotState, Party, PartyMember};

/// Handle party messages (0x3B).
pub(super) async fn handle_party_operation(
    recv: &mut Cursor<'_>,
    state: &mut BotState,
    _session: &mut Session,
) -> Result<(), String> {
    let op = recv.read_u8();
    match op {
        8 => {
            // Create confirm: partyid + 0x3B9AC9FF + 0x3B9AC9FF + 0 (partyid assigned by the server)
            let partyid = recv.read_i32();
            crate::emit_f!([crate::emit::Field::PartyId => partyid] => "[party] created partyid={partyid}");
            let party = state.party.get_or_insert_with(Party::default);
            party.partyid = partyid;
        }
        4 => {
            // Invite/join request: partyid + name + 00. When in a party = someone requests to join.
            let partyid = recv.read_i32();
            let name = recv.read_string_gb();
            if state.party.is_some() {
                crate::emit_f!([crate::emit::Field::Name => name, crate::emit::Field::PartyId => partyid] => "[party] '{name}' requests to join (partyid={partyid})");
            } else {
                crate::emit_f!([crate::emit::Field::Name => name, crate::emit::Field::PartyId => partyid] => "[party] invited by '{name}' (partyid={partyid})");
            }
        }
        7 => {
            // Full party status (07): partyid + addPartyStatus block (no name prefix)
            let partyid = recv.read_i32();
            let (members, leader) = parse_party_status(recv);
            crate::emit_f!([crate::emit::Field::PartyId => partyid, crate::emit::Field::Count => members.len(), crate::emit::Field::Cid => leader] => 
                "[party] sync partyid={partyid} members={} leader={leader}", members.len());
            for m in members.values() {
                crate::emit_f!([crate::emit::Field::Cid => m.id, crate::emit::Field::Name => m.name, crate::emit::Field::Job => m.job, crate::emit::Field::Level => m.level] => 
                    "[member] id={} name={} job={} lvl={}", m.id, m.name, m.job, m.level);
            }
            state.party = Some(Party {
                partyid,
                leader_id: leader,
                members,
            });
        }
        15 => {
            // Full sync (0F): partyid + changed member name + addPartyStatus block
            let partyid = recv.read_i32();
            let changed = recv.read_string_gb();
            let (members, leader) = parse_party_status(recv);
            crate::emit_f!([crate::emit::Field::PartyId => partyid, crate::emit::Field::Count => members.len(), crate::emit::Field::Cid => leader, crate::emit::Field::Name => changed] => 
                "[party] sync partyid={partyid} members={} leader={leader} (changed: {changed})", members.len());
            for m in members.values() {
                crate::emit_f!([crate::emit::Field::Cid => m.id, crate::emit::Field::Name => m.name, crate::emit::Field::Job => m.job, crate::emit::Field::Level => m.level] => 
                    "[member] id={} name={} job={} lvl={}", m.id, m.name, m.job, m.level);
            }
            state.party = Some(Party {
                partyid,
                leader_id: leader,
                members,
            });
        }
        12 => {
            // Change notice: partyid + targetid. Ourselves = kicked/left; others = member left.
            let partyid = recv.read_i32();
            let targetid = recv.read_i32();
            if targetid == state.my_cid {
                crate::emit_f!([crate::emit::Field::PartyId => partyid] => "[party] we left/disbanded partyid={partyid}");
                state.party = None;
                state.attack_target = None;
            } else {
                let name = state
                    .party
                    .as_ref()
                    .and_then(|p| p.members.get(&targetid))
                    .map(|m| m.name.clone())
                    .unwrap_or_default();
                crate::emit_f!([crate::emit::Field::Cid => targetid, crate::emit::Field::Name => name, crate::emit::Field::PartyId => partyid] => 
                    "[party] member {name} ({targetid}) left/expelled (partyid={partyid})");
                if let Some(p) = &mut state.party {
                    p.members.remove(&targetid);
                }
            }
        }
        2 => {
            // Leave/disband confirm
            crate::emit!("[party] we left/disbanded");
            state.party = None;
            state.attack_target = None;
        }
        16 => {
            // Invite failed (target already in a party, verified in real traffic)
            crate::emit!("[party] invite failed (target already in a party)");
        }
        other => {
            crate::emit_f!([crate::emit::Field::Value => other] => "[party] unhandled op={other}");
        }
    }
    Ok(())
}

/// Parse the `addPartyStatus` block: 6 padded member slots followed by leader
/// id and per-member map/portal data. Returns (members, leader_id).
fn parse_party_status(recv: &mut Cursor<'_>) -> (BTreeMap<i32, PartyMember>, i32) {
    let mut ids = Vec::with_capacity(6);
    for _ in 0..6 {
        ids.push(recv.read_i32());
    }
    let mut names = Vec::with_capacity(6);
    for _ in 0..6 {
        names.push(recv.read_padded_string_gb(13));
    }
    let mut jobs = Vec::with_capacity(6);
    for _ in 0..6 {
        jobs.push(recv.read_i32());
    }
    let mut levels = Vec::with_capacity(6);
    for _ in 0..6 {
        levels.push(recv.read_i32());
    }
    let mut channels = Vec::with_capacity(6);
    for _ in 0..6 {
        channels.push(recv.read_i32());
    }
    let leader = recv.read_i32();
    let mut mapids = Vec::with_capacity(6);
    for _ in 0..6 {
        mapids.push(recv.read_i32());
    }
    recv.skip(120);

    let mut members = BTreeMap::new();
    for i in 0..6 {
        if ids[i] != 0 {
            members.insert(
                ids[i],
                PartyMember {
                    id: ids[i],
                    name: names[i].clone(),
                    job: jobs[i],
                    level: levels[i],
                    channel: channels[i],
                    mapid: mapids[i],
                },
            );
        }
    }
    (members, leader)
}
