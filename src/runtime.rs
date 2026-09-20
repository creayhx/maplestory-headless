//! Shared bot main loop, embedded by both frontends:
//! the headless CLI (`main.rs`) and the terminal console (`crates/console`).
//!
//! The loop owns the `Session`; game state lives in the caller-provided
//! `Arc<Mutex<BotState>>` so a UI can read it concurrently. All output goes
//! through the `emit!` macros (default = stdout/stderr, or a registered
//! emitter).

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::sync::Mutex;

use crate::command;
use crate::config::Config;
use crate::handlers;
use crate::packets::cashshop as pcash;
use crate::packets::login as plogin;
use crate::runtime_config::RuntimeConfig;
use crate::session::Session;
use crate::state::{BotState, Phase};

/// Why the loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// `quit` command
    Quit,
    /// server closed the connection / receive error
    ConnectionClosed,
    /// `--duration` elapsed
    DurationElapsed,
    /// fatal startup error (connect / login send)
    Failed,
    /// TCP connect failed (server down / restarting / network blip). Kept
    /// distinct from `Failed` so the reconnect loop retries it instead of
    /// bouncing back to the wizard.
    ConnectFailed,
}

/// Reset session-level game data before reconnecting: only clears the current
/// "run" data, keeping config-like in-memory state (cfg: rule open / group run
/// toggles, hunt config, damage overrides, potion, etc.) and idle toggles
/// (hunt/hunt_reactor) — the same functions resume after reconnect.
/// Session state (task stack / party / trade / cash shop) is cleared too.
fn reset_session(st: &mut BotState) {
    st.phase = Phase::Disconnected;
    st.entities.clear();
    st.drops.clear();
    st.npcs.clear();
    st.reactors.clear();
    st.position = (0, 0);
    st.ground_y = 0;
    st.mapid = 0;
    st.hunt_target = None;
    st.discovery_sent = false;
    st.party = None;
    st.trade = Default::default();
    st.shop_open = false;
    st.dialog_open = false;
    crate::handlers::field::clear_npc_menu(st);
    crate::handlers::field::clear_shop(st);
    st.cs_inventory.clear();
    st.cs_items.clear();
    st.cs_nx = 0;
    st.cs_points = 0;
    st.keymap.clear();
    st.rule_last_fire.clear();
    st.task_stack.clear();
    st.reenter = None;
    st.facing = 1;
    st.last_stance = 0;
    st.last_kill = std::time::Instant::now();
    st.map_enter_time = std::time::Instant::now();
    // kept: cfg (rule/group toggles), hunt toggle, damage,
    // selected_world / selected_channel / my_cid / relogin / ever_in_game
    // (silent re-login after reconnect; "connected once" gate)
}

/// Run the bot until quit / disconnect / duration, with automatic reconnect.
/// `input` receives command lines (stdin reader in CLI mode, input box in TUI
/// mode). On server disconnect / connect failure, retries every
/// `cfg.reconnect_delay` seconds up to `cfg.reconnect_max` times (0 = forever)
/// while keeping config state (open rules/groups, hunt toggles) alive.
pub async fn run(
    config: Config,
    rx: &mut mpsc::Receiver<String>,
    state: Arc<Mutex<BotState>>,
) -> RunOutcome {
    // Profile config chain for this session (`--config` files, last = write
    // target). Empty = the legacy `./config.json`.
    let session_paths: Vec<std::path::PathBuf> = config
        .config_paths
        .iter()
        .map(std::path::PathBuf::from)
        .collect();
    let cfg_paths = if session_paths.is_empty() {
        crate::runtime_config::legacy_config_paths()
    } else {
        session_paths.clone()
    };
    {
        // Load config from file up front — available even when the server is
        // unreachable.
        let mut st = state.lock().await;
        st.cfg = RuntimeConfig::load_from(&cfg_paths);
        crate::command::run::apply_default_attack_mode(&mut st);
        // CLI `--chat-admin` names are merged into the config whitelist
        // (config.json `chat_admins`).
        for name in &config.chat_admins {
            if !st.cfg.chat_admins.iter().any(|n| n == name) {
                st.cfg.chat_admins.push(name.clone());
            }
        }
        // CLI `--damage` overrides the config's `hunt.damage` (the single
        // source; 0 = auto formula).
        if let Some(d) = config.damage {
            st.cfg.hunt.damage = d;
        }
    }
    // This session's name dictionaries, keyed by ITS OWN `data_dir` — read
    // after the config load so the profile's directory wins.
    //
    // Two sessions with different `data_dir` values therefore hold different
    // `Arc<NameTable>`s and can never see each other's names: this is exactly
    // why names are per-session instead of one process-global table.
    let session_names = {
        let st = state.lock().await;
        crate::names::NameTable::for_dir(&st.cfg.data_dir)
    };
    // Keep the legacy process default table in step with this session so the
    // headless CLI (and, until phase 3, the frontends' render path) resolve
    // the same names. Idempotent — free for the 2nd..Nth session on the same
    // `data_dir`, and it does NOT feed back into any session's own table.
    crate::names::reload_if_needed(session_names.dir());
    let mut attempts = 0u32;
    loop {
        let outcome = run_once(
            &config,
            rx,
            &state,
            attempts == 0,
            &session_names,
            &session_paths,
        )
        .await;
        match outcome {
            // Login-level failures (bad credentials, kicked during login) are
            // terminal: retrying with bad data loops forever — the frontend
            // takes it back to the wizard. Connect failures and disconnects
            // are retried below.
            RunOutcome::Quit | RunOutcome::DurationElapsed | RunOutcome::Failed => {
                return outcome;
            }
            RunOutcome::ConnectFailed | RunOutcome::ConnectionClosed => {
                // Reconnect requires the bot to have reached in-game at some
                // point (ever_in_game — survives reset_session, so it also
                // covers drops DURING a relogin and server-down windows):
                // session data is then valid and relogin silently re-enters
                // with the remembered world/character + this session's login
                // info. Without it (never connected / first login issues) the
                // failure is terminal — the frontend re-runs the wizard.
                let (enabled, delay, max, ever_in_game) = {
                    let st = state.lock().await;
                    (
                        st.cfg.reconnect,
                        st.cfg.reconnect_delay,
                        st.cfg.reconnect_max,
                        st.ever_in_game,
                    )
                };
                if !enabled {
                    return RunOutcome::Failed;
                }
                if !ever_in_game {
                    crate::emit_err_f!([] =>
                        "[reconnect] never reached in-game — not retrying, back to login");
                    return RunOutcome::Failed;
                }
                attempts += 1;
                if max > 0 && attempts >= max {
                    crate::emit_err_f!([crate::emit::Field::Text => attempts] =>
                        "[reconnect] giving up after {attempts} attempts");
                    return RunOutcome::Failed;
                }
                crate::emit_f!([crate::emit::Field::Duration => delay, crate::emit::Field::Number => attempts] =>
                    "[reconnect] attempt {attempts}: retrying in {delay}s (keeping rules/groups/hunt)");
                {
                    let mut st = state.lock().await;
                    reset_session(&mut st);
                    // Back to the game: login silently re-enters with the last
                    // remembered world/character, no interactive prompts
                    // (ever_in_game survives the reset → the gate stays open).
                    st.relogin = true;
                }
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            }
        }
    }
}

/// One session: connect → login → world/character select → enter map → main
/// loop, until quit/disconnect.
async fn run_once(
    config: &Config,
    rx: &mut mpsc::Receiver<String>,
    state: &Arc<Mutex<BotState>>,
    first_session: bool,
    names: &Arc<crate::names::NameTable>,
    config_paths: &[std::path::PathBuf],
) -> RunOutcome {
    let addr = format!("{}:{}", config.ip, config.port);
    crate::emit_f!([crate::emit::Field::Value => config.version] => 
        "openstory-bot connecting to {addr} (version {})",
        config.version);

    let mut session = match Session::connect_ex_keyed(
        &addr,
        config.version,
        config.show_packets,
        config.dump_packets,
        config.aes_key,
    )
    .await
    {
        Ok(s) => s.with_profile(names.clone(), config_paths.to_vec()),
        Err(e) => {
            crate::emit_err_f!([crate::emit::Field::Text => e] => "[fatal] {e}");
            return RunOutcome::ConnectFailed;
        }
    };

    // Config is loaded up front in `run` (available even before the
    // connection). `cfg` survives reconnects — in-memory rule open/
    // group run toggles are kept (a reload would reset rule enabled
    // back to file values).

    // Kick off the login flow. The server replies LOGIN_STATUS on success
    // (handled in handlers), which then requests the server list.
    if let Err(e) = session
        .send_packet(plogin::login(&config.account, &config.password))
        .await
    {
        crate::emit_err_f!([crate::emit::Field::Text => e] => "[fatal] login send: {e}");
        return RunOutcome::Failed;
    }

    if first_session {
        crate::emit_f!([] => "> ready. type 'help' for commands.");
    }

    let deadline = config
        .duration
        .map(|secs| std::time::Instant::now() + std::time::Duration::from_secs(secs));

    let mut auto_index = 0usize;
    let mut sleep_until = std::time::Instant::now();
    // tick_ms=0 → tick fires on every loop iteration (busy loop; packet rate
    // limited only by CPU/network). tokio::time::interval rejects a 0 period,
    // so use yield_now (always ready but still yields to the scheduler, so the
    // packet-handling branch is never starved).
    let tick_period = {
        let st = state.lock().await;
        std::time::Duration::from_millis(st.cfg.tick_ms)
    };
    let mut tick_interval = if tick_period.is_zero() {
        None
    } else {
        let mut iv = tokio::time::interval(tick_period);
        // Skip the first tick immediately; align to the period boundary.
        iv.tick().await;
        Some(iv)
    };

    let outcome = loop {
        tokio::select! {
            packet = session.recv_packet() => {
                match packet {
                    Ok(Some(body)) => {
                        let mut st = state.lock().await;
                        if let Err(e) = handlers::handle(&body, &mut st, &mut session, config).await {
                            crate::emit_err_f!([crate::emit::Field::Text => e] => "[handler error] {e}");
                        }
                    }
                    Ok(None) => {
                        crate::emit_f!([] => "[connection closed by server]");
                        break RunOutcome::ConnectionClosed;
                    }
                    Err(e) => {
                        crate::emit_err_f!([crate::emit::Field::Text => e] => "[recv error] {e}");
                        break RunOutcome::ConnectionClosed;
                    }
                }
                // Run the --auto commands one at a time once in game. A `sleep`
                // command just bumps the deadline; packets keep being processed
                // while we wait, so mobs/players are tracked during it.
                // Wait for discovery move to complete so the character is
                // visible to other players before the next auto command.
            }
            line = rx.recv() => {
                let Some(line) = line else {
                    // Input closed (e.g. not attached). Keep running: replace
                    // rx with one that never completes by keeping a dummy
                    // sender alive (std::mem::forget prevents drop, so recv()
                    // waits forever instead of returning None).
                    let (tx_keep, rx_keep) = mpsc::channel::<String>(1);
                    std::mem::forget(tx_keep);
                    *rx = rx_keep;
                    continue;
                };
                let mut st = state.lock().await;
                match command::parse(&line) {
                    Ok(Some(cmd)) => {
                        match command::run(cmd, &mut st, &mut session).await {
                            Ok(true) => break RunOutcome::Quit,
                            Ok(false) => {}
                            Err(e) => crate::emit_err_f!([crate::emit::Field::Text => e] => "[command error] {e}"),
                        }
                    }
                    Ok(None) => {}
                    Err(e) => crate::emit_err_f!([crate::emit::Field::Text => e] => "{e}"),
                }
            }
            _ = async {
                match &mut tick_interval {
                    Some(iv) => {
                        iv.tick().await;
                    }
                    None => tokio::task::yield_now().await,
                }
            } => {
                let mut st = state.lock().await;
                if st.phase == Phase::InGame {
                    // reached in-game: the prerequisite for allowing reconnect on disconnect
                    st.ever_in_game = true;
                    if let Err(e) = command::tick(&mut st, &mut session).await {
                        if e == "quit" {
                            // rule action quit (e.g. "log off when someone shows up") → end the session
                            break RunOutcome::Quit;
                        }
                        crate::emit_err_f!([crate::emit::Field::Text => e] => "[tick error] {e}");
                    }
                }
                // Timeout: if stuck in EnteringMap for >10s, reset to InGame
                if st.phase == Phase::EnteringMap
                    && st.map_enter_time.elapsed() > std::time::Duration::from_secs(10)
                {
                    st.phase = Phase::InGame;
                    crate::emit_f!([crate::emit::Field::MapId => st.mapid] => "[timeout] map change timed out — staying on map={}", st.mapid);
                }
                // reenter return: inside the cash shop server (SET_CASH_SHOP
                // 0x83) → send leave once (0x21, LeaveCashShop on the CS server);
                // the server drops the character back via CharacterTransfer to
                // the same channel/map. leave_sent prevents 50ms packet spamming.
                if st.phase == Phase::CashShop {
                    if let Some(ctx) = st.reenter.as_mut() {
                        if !ctx.leave_sent {
                            match session.send_packet(pcash::leave()).await {
                                Ok(()) => {
                                    ctx.leave_sent = true;
                                    crate::emit_f!([] => "[reenter] in cash shop, leaving...");
                                }
                                Err(e) => crate::emit_err_f!([crate::emit::Field::Text => e] => "[reenter] leave send failed: {e}"),
                            }
                        }
                    }
                }
                // reenter-denied fallback: didn't reach the cash shop server
                // within 10s (CS_ENABLE=false / map disallows entry / event maps
                // only reply enableActions without reconnecting, phase stays
                // InGame) → abandon the round trip and restore hunt from the snapshot.
                if st.phase == Phase::InGame {
                    let denied = matches!(
                        st.reenter.as_ref(),
                        Some(ctx)
                            if !ctx.leave_sent
                                && ctx.started.elapsed() > std::time::Duration::from_secs(10)
                    );
                    if denied {
                        if let Some(ctx) = st.reenter.take() {
                            st.hunt = ctx.restore_hunt;
                            if ctx.restore_hunt {
                                // restoring hunting: mark the hunt clock fresh
                                st.last_kill = std::time::Instant::now();
                            }
                            crate::emit_f!([crate::emit::Field::Toggle => if st.hunt { "on" } else { "off" }] => 
                                "[reenter] denied (no cash shop reply in 10s) — restoring hunt={}",
                                st.hunt);
                        }
                    }
                }
                // reenter stuck in the shop fallback: on the CS server with leave
                // already sent, but no 0x13 return within 20s (no reply / broken
                // connection) → abandon the round trip and restore hunt;
                // otherwise phase stays CashShop, tick never runs, and the
                // character never attacks.
                if st.phase == Phase::CashShop {
                    let stuck = matches!(
                        st.reenter.as_ref(),
                        Some(ctx)
                            if ctx.leave_sent
                                && ctx.started.elapsed() > std::time::Duration::from_secs(20)
                    );
                    if stuck {
                        if let Some(ctx) = st.reenter.take() {
                            st.hunt = ctx.restore_hunt;
                            if ctx.restore_hunt {
                                st.last_kill = std::time::Instant::now();
                            }
                            st.phase = Phase::InGame;
                            crate::emit_f!([crate::emit::Field::Toggle => if st.hunt { "on" } else { "off" }] => 
                                "[reenter] stuck in cash shop 20s — giving up, restoring hunt={}",
                                st.hunt);
                        }
                    }
                }
            }
            _ = tokio::time::sleep_until(deadline.map(|d| d.into()).unwrap_or(std::time::Instant::now() + std::time::Duration::from_secs(24 * 3600)).into()), if deadline.is_some() => {
                crate::emit_f!([] => "[duration elapsed]");
                break RunOutcome::DurationElapsed;
            }
        }

        // Run auto commands on every loop iteration (not just after packets).
        // discovery gate: some servers (e.g. the variant server) don't send the
        // bot's own SPAWN_PLAYER after entering the map, so the unknown position
        // would block discovery forever — 5s after entering, relax the gate and
        // run auto commands anyway (npc dialogs etc. don't need our position).
        let mut st = state.lock().await;
        let discovery_ok = st.discovery_sent
            || st.map_enter_time.elapsed() > std::time::Duration::from_secs(5);
        if st.phase == Phase::InGame
            && discovery_ok
            && auto_index < config.auto.len()
            && std::time::Instant::now() >= sleep_until
        {
            let cmdline = config.auto[auto_index].clone();
            auto_index += 1;
            if let Ok(Some(command::Command::Sleep(secs))) = command::parse(&cmdline) {
                sleep_until = std::time::Instant::now()
                    + std::time::Duration::from_secs(secs);
            } else {
                match command::parse(&cmdline) {
                    Ok(Some(c)) => {
                        if command::run(c, &mut st, &mut session).await.unwrap_or(false) {
                            break RunOutcome::Quit;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => crate::emit_err_f!([crate::emit::Field::Text => e] => "[auto] {e}"),
                }
            }
        }
    };

    outcome
}

