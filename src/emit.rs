//! Structured output events: every print site in the bot emits an `Event`
//! instead of writing to stdout/stderr directly.
//!
//! A process-wide emitter is registered once (TUI registers its channel
//! emitter; future HTTP frontends register their own). When no emitter is
//! registered the default prints to stdout (`Level::Info`) / stderr
//! (`Level::Err`) with the exact original text — the headless CLI keeps its
//! current output byte-for-byte.
//!
//! Categories are inferred from the leading `[tag]` of each line, so the TUI
//! can color and filter the stream without any extra plumbing.

use std::sync::Arc;
use std::sync::OnceLock;

/// Severity of an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Err,
}

/// Event category, inferred from the printed line's leading `[tag]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// lifecycle / login / config / system messages
    System,
    /// explicit player-command outputs
    Cmd,
    /// map chat / whispers
    Chat,
    /// server notices / yellow broadcasts
    Notice,
    /// NPC dialogs and shops
    Npc,
    /// hunting / combat / reactors / movement
    Hunt,
    /// config rules
    Rule,
    /// config feature groups
    Group,
    /// config tasks
    Task,
    /// `view ...` outputs
    View,
    /// packet tracing (SEND/RECV/...)
    Packet,
    /// errors and warnings
    Error,
}

/// Semantic field keys (structured facts carried by a message).
/// Frontends render any language from the fields, decoupled from the English text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    // identity
    Oid,
    Cid,
    MobId,
    ItemId,
    NpcId,
    MapId,
    PartyId,
    SkillId,
    ReactorId,
    /// generic id (world id / job, etc.)
    Id,
    Job,
    // numeric
    Count,
    Qty,
    Slot,
    /// total items (inventory)
    Items,
    /// equip count
    Equips,
    /// skill count
    Skills,
    Hp,
    MaxHp,
    Mp,
    MaxMp,
    Exp,
    Level,
    Ap,
    Sp,
    Meso,
    Damage,
    Percent,
    Range,
    Targets,
    Channel,
    Selection,
    Value,
    Number,
    Duration,
    Price,
    // position
    Pos,
    Ground,
    // text/status
    Name,
    Text,
    World,
    Portal,
    MoveMode,
    AttackMode,
    Toggle,
    Until,
    Step,
    Phase,
    Status,
}

/// One structured output event.
#[derive(Debug, Clone)]
pub struct Event {
    pub ts: std::time::SystemTime,
    pub level: Level,
    pub category: Category,
    /// the exact original line (may contain multiple lines for block events)
    pub text: String,
    /// structured semantic fields (optional; frontends render from these)
    pub fields: Vec<(Field, String)>,
}

impl Event {
    pub fn new(level: Level, category: Category, text: String) -> Self {
        Self {
            ts: std::time::SystemTime::now(),
            level,
            category,
            text,
            fields: Vec::new(),
        }
    }

    /// Event with structured fields (category inferred automatically from the text).
    pub fn new_f(level: Level, text: String, fields: Vec<(Field, String)>) -> Self {
        let category = categorize(&text);
        Self {
            ts: std::time::SystemTime::now(),
            level,
            category,
            text,
            fields,
        }
    }
}

/// Receives output events. Implemented by the TUI (channel queue) and, in the
/// future, the HTTP frontend. The default (no registration) writes to the
/// real stdout/stderr — the headless CLI never registers one.
pub trait Emitter: Send + Sync {
    fn emit(&self, ev: Event);
}

static EMITTER: OnceLock<Arc<dyn Emitter>> = OnceLock::new();

tokio::task_local! {
    /// Per-session emitter override, scoped to the task that runs one bot.
    ///
    /// # Why task-local rather than another global
    ///
    /// With N accounts in one process, a single process-wide emitter would
    /// funnel every session's events into one log stream — F5 (per-bot logs)
    /// would be impossible. A task-local lets each session carry its own
    /// emitter while **every one of the ~250 `emit!` call sites stays
    /// untouched**: they all funnel through [`emit_event`], which consults
    /// this scope first.
    ///
    /// This is safe because a bot's whole life is one task:
    /// `runtime::run` → `run_once` → handlers/commands, with no `tokio::spawn`
    /// anywhere in the bot path (verified: the only spawns in `src/` are in the
    /// deleted `worker.rs` and a `#[cfg(test)]` helper). Task-locals propagate
    /// through `.await` within that task.
    ///
    /// A task-local is NOT a global: two sessions in the same process hold two
    /// independent values, and nothing outside a session's task can observe
    /// them.
    static SESSION_EMITTER: Arc<dyn Emitter>;
}

/// Run `fut` with `emitter` receiving every event it emits.
///
/// The scope covers the whole bot lifetime, so reconnect loops, handlers,
/// command execution and periodic ticks all route to the same emitter.
/// Nested calls compose (the innermost wins).
pub fn with_session_emitter<F>(emitter: Arc<dyn Emitter>, fut: F) -> impl std::future::Future<Output = F::Output>
where
    F: std::future::Future,
{
    SESSION_EMITTER.scope(emitter, fut)
}

/// True when the current task has a session emitter installed (diagnostics/tests).
pub fn has_session_emitter() -> bool {
    SESSION_EMITTER.try_with(|_| ()).is_ok()
}

/// Register the process-wide emitter. Fails if one is already registered.
///
/// Used by single-session frontends (the TUI in single-account mode, the
/// headless CLI). Multi-session frontends register **nothing** here and use
/// [`with_session_emitter`] per session instead.
pub fn set_emitter(e: Arc<dyn Emitter>) -> Result<(), String> {
    EMITTER
        .set(e)
        .map_err(|_| "emitter already registered".to_string())
}

/// Deliver an event to the session emitter when inside a session scope,
/// else to the registered process emitter, else print it directly (headless
/// CLI behavior — byte-for-byte the original output).
///
/// The event value is moved exactly once: the session destination takes
/// priority, so there is no clone on the hot path.
pub fn emit_event(ev: Event) {
    if has_session_emitter() {
        // `try_with` cannot fail here (just checked) and consumes the value.
        let _ = SESSION_EMITTER.try_with(move |e| e.emit(ev));
        return;
    }
    match EMITTER.get() {
        Some(e) => e.emit(ev),
        None => match ev.level {
            Level::Err => eprintln!("{}", ev.text),
            _ => println!("{}", ev.text),
        },
    }
}

/// Infer the event category from a printed line's leading `[tag]`.
pub fn categorize(line: &str) -> Category {
    let t = line.trim_start();
    if t.starts_with("SEND")
        || t.starts_with("RECV")
        || t.starts_with("UNHANDLED")
    {
        return Category::Packet;
    }
    let tag = match t.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => "",
    };
    match tag {
        "chat" | "whisper" => Category::Chat,
        // Server broadcasts: yellow text / server messages ([message] =
        // SERVERMESSAGE, [notice] = SET_WEEK_EVENT_MESSAGE GM broadcast).
        "notice" | "message" => Category::Notice,
        tag if tag.starts_with("message ") => Category::Notice, // [message type=11]
        "npc" | "npc_talk" | "shop" => Category::Npc,
        "hunt" | "attack" | "dmg" | "kill" | "pickup" | "reactor" | "rhunt"
        | "discover" | "mob" | "mobhp" | "self" | "exp" | "level" | "ap" | "sp"
        | "potion" => Category::Hunt,
        "rule" | "rules" => Category::Rule,
        "group" => Category::Group,
        "task" => Category::Task,
        "view" => Category::View,
        tag if tag.starts_with("view ") => Category::View, // [view mobs] and other subcommands
        "fatal" | "recv error" | "handler error" | "command error" | "tick error" => {
            Category::Error
        }
        "login" | "world" | "serverlist" | "serverstatus" | "char" | "charlist"
        | "server_ip" | "choose_gender" | "gender_set" | "set_field" | "config"
        | "timeout" | "connection closed by server" | "duration elapsed" | "auto"
        | "report" | "warn" => Category::System,
        "" => {
            // Untagged lines: command echoes ("move -> ...", "chat: ...",
            // "bye", "> ready...") go to Cmd, startup banner to System.
            if t.starts_with("> ") || t.starts_with("openstory-bot ") {
                Category::System
            } else {
                Category::Cmd
            }
        }
        _ => Category::Cmd,
    }
}

/// Format + emit an info line (the workhorse behind `emit!`).
pub fn emit_line(level: Level, line: String) {
    emit_event(Event::new(level, categorize(&line), line));
}

/// Replace `println!` — identical formatting, structured delivery.
#[macro_export]
macro_rules! emit {
    ($($arg:tt)*) => {
        $crate::emit::emit_line($crate::emit::Level::Info, format!($($arg)*))
    };
}

/// Replace `eprintln!` — identical formatting, structured delivery.
#[macro_export]
macro_rules! emit_err {
    ($($arg:tt)*) => {
        $crate::emit::emit_line($crate::emit::Level::Err, format!($($arg)*))
    };
}

/// `emit!` + structured fields: `emit_f!([Field::X => v, ...] => "text {v}")`.
/// The English text is kept as-is (CLI output unchanged); the fields let any
/// frontend render its own language.
#[macro_export]
macro_rules! emit_f {
    ($fields:tt => $($fmt:tt)*) => {
        $crate::emit_f_impl!($crate::emit::Level::Info, $fields, format!($($fmt)*))
    };
}

/// `emit_err!` + structured fields.
#[macro_export]
macro_rules! emit_err_f {
    ($fields:tt => $($fmt:tt)*) => {
        $crate::emit_f_impl!($crate::emit::Level::Err, $fields, format!($($fmt)*))
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! emit_f_impl {
    ($level:expr, [$($k:expr => $v:expr),* $(,)?], $text:expr) => {
        $crate::emit::emit_event($crate::emit::Event::new_f(
            $level,
            $text,
            vec![$(($k, ($v).to_string())),*],
        ))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categorize_tags() {
        assert_eq!(categorize("[chat] hi"), Category::Chat);
        assert_eq!(categorize("[whisper] hi"), Category::Chat);
        assert_eq!(categorize("[notice] server msg"), Category::Notice);
        assert_eq!(categorize("[message type=11] hello"), Category::Notice);
        assert_eq!(categorize("[npc] talk oid=1"), Category::Npc);
        assert_eq!(categorize("[npc_talk] options"), Category::Npc);
        assert_eq!(categorize("[shop] open npcid=1"), Category::Npc);
        assert_eq!(categorize("[hunt] on"), Category::Hunt);
        assert_eq!(categorize("[attack] oid=1 dmg=10"), Category::Hunt);
        assert_eq!(categorize("[kill] oid=1"), Category::Hunt);
        assert_eq!(categorize("[reactor] hit"), Category::Hunt);
        assert_eq!(categorize("[exp] +135"), Category::Hunt);
        assert_eq!(categorize("[level] up!"), Category::Hunt);
        assert_eq!(categorize("[ap] +1"), Category::Hunt);
        assert_eq!(categorize("[sp] 6 -> 5"), Category::Hunt);
        assert_eq!(categorize("[potion] hp 20%"), Category::Hunt);
        assert_eq!(categorize("[rule] open sellauto"), Category::Rule);
        assert_eq!(categorize("[group] 1. autosell (off)"), Category::Group);
        assert_eq!(categorize("[task] started"), Category::Task);
        assert_eq!(categorize("[view] status"), Category::View);
        assert_eq!(categorize("[fatal] boom"), Category::Error);
        assert_eq!(categorize("[command error] bad"), Category::Error);
        assert_eq!(categorize("[handler error] bad"), Category::Error);
        assert_eq!(categorize("[tick error] bad"), Category::Error);
        assert_eq!(categorize("[recv error] bad"), Category::Error);
        assert_eq!(categorize("[config] loaded"), Category::System);
        assert_eq!(categorize("[login] failed"), Category::System);
        assert_eq!(categorize("[serverlist] x"), Category::System);
        assert_eq!(categorize("[set_field] in game!"), Category::System);
        assert_eq!(categorize("[report] [WARN] stuck"), Category::System);
        assert_eq!(categorize("[connection closed by server]"), Category::System);
        assert_eq!(categorize("[duration elapsed]"), Category::System);
        assert_eq!(categorize("SEND [0x2D] 5 bytes"), Category::Packet);
        assert_eq!(categorize("RECVHEX: 00 01"), Category::Packet);
        assert_eq!(categorize("UNHANDLED [0x1234] 3 bytes"), Category::Packet);
    }

    #[test]
    fn categorize_untagged_and_unknown() {
        assert_eq!(categorize("> ready. type 'help' for commands."), Category::System);
        assert_eq!(categorize("openstory-bot connecting to 1.2.3.4:8484 (version 79)"), Category::System);
        assert_eq!(categorize("move -> (100, 20)"), Category::Cmd);
        assert_eq!(categorize("chat: hello"), Category::Cmd);
        assert_eq!(categorize("bye"), Category::Cmd);
        assert_eq!(categorize("[skill] learned"), Category::Cmd);
        assert_eq!(categorize("[party] created"), Category::Cmd);
        assert_eq!(categorize("[trade] confirmed"), Category::Cmd);
        assert_eq!(categorize("[some_future_tag] x"), Category::Cmd);
    }

    #[test]
    fn emit_macro_compiles_and_roundtrips() {
        let line = format!("[chat] {} {}", "a", 1);
        assert_eq!(line, "[chat] a 1");
    }

    // ── Phase 1 (G1): per-session emitter scoping ─────────────────────

    use std::sync::Mutex;

    /// Collects the text of everything it receives.
    #[derive(Default)]
    struct Collector {
        seen: Mutex<Vec<String>>,
    }

    impl Collector {
        fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }
        fn texts(&self) -> Vec<String> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl Emitter for Collector {
        fn emit(&self, ev: Event) {
            self.seen.lock().unwrap().push(ev.text);
        }
    }

    /// The whole point of G1: two concurrent sessions never cross-talk.
    #[tokio::test]
    async fn session_emitters_are_isolated() {
        let a = Collector::new();
        let b = Collector::new();
        let (ta, tb) = (a.clone(), b.clone());

        let ha = tokio::spawn(with_session_emitter(ta, async {
            emit_line(Level::Info, "[chat] from A".into());
            tokio::task::yield_now().await;
            emit_line(Level::Info, "[chat] A again".into());
        }));
        let hb = tokio::spawn(with_session_emitter(tb, async {
            emit_line(Level::Info, "[chat] from B".into());
            tokio::task::yield_now().await;
            emit_line(Level::Err, "[fatal] B error".into());
        }));
        ha.await.unwrap();
        hb.await.unwrap();

        assert_eq!(a.texts(), vec!["[chat] from A", "[chat] A again"]);
        assert_eq!(b.texts(), vec!["[chat] from B", "[fatal] B error"]);
    }

    /// A session scope must also cover nested helpers and long await chains.
    #[tokio::test]
    async fn session_scope_covers_nested_awaits() {
        async fn inner() {
            tokio::task::yield_now().await;
            emit_line(Level::Info, "[npc] nested".into());
        }
        let c = Collector::new();
        let cc = c.clone();
        with_session_emitter(
            cc,
            async {
                inner().await;
                emit_line(Level::Info, "[hunt] outer".into());
            },
        )
        .await;
        assert_eq!(c.texts(), vec!["[npc] nested", "[hunt] outer"]);
    }

    /// Outside a session scope nothing is installed, so the process emitter
    /// (or stdout) is used — this is what keeps the TUI/CLI behavior intact.
    #[tokio::test]
    async fn no_scope_means_no_session_emitter() {
        assert!(!has_session_emitter());
        let c = Collector::new();
        let cc = c.clone();
        with_session_emitter(cc, async {
            assert!(has_session_emitter());
        })
        .await;
        assert!(!has_session_emitter(), "scope must end with the future");
        assert!(c.texts().is_empty());
    }

    /// A nested scope replaces the outer one and the outer one is restored
    /// afterwards (restart/session-control paths rely on this).
    #[tokio::test]
    async fn nested_scope_shadows_then_restores() {
        let outer = Collector::new();
        let inner = Collector::new();
        let (o1, o2) = (outer.clone(), outer.clone());
        let i1 = inner.clone();
        with_session_emitter(
            o1,
            async move {
                emit_line(Level::Info, "outer-1".into());
                with_session_emitter(i1, async {
                    emit_line(Level::Info, "inner".into());
                })
                .await;
                emit_line(Level::Info, "outer-2".into());
            },
        )
        .await;
        assert_eq!(o2.texts(), vec!["outer-1", "outer-2"]);
        assert_eq!(inner.texts(), vec!["inner"]);
    }

    /// A spawned child task does NOT inherit the scope. This is the documented
    /// boundary of the design: a bot must not spawn detached tasks expecting
    /// its events to be routed.
    #[tokio::test]
    async fn spawned_child_task_does_not_inherit_scope() {
        let c = Collector::new();
        let cc = c.clone();
        with_session_emitter(
            cc,
            async {
                let child = tokio::spawn(async { has_session_emitter() });
                assert!(
                    !child.await.unwrap(),
                    "child tasks must not inherit the session scope"
                );
            },
        )
        .await;
        assert!(c.texts().is_empty());
    }

    /// Structured events keep their fields and category through the scope.
    #[tokio::test]
    async fn scoped_events_keep_fields_and_category() {
        #[derive(Default)]
        struct Rec(Mutex<Vec<Event>>);
        impl Emitter for Rec {
            fn emit(&self, ev: Event) {
                self.0.lock().unwrap().push(ev);
            }
        }
        let rec = Arc::new(Rec::default());
        let r2 = rec.clone();
        with_session_emitter(
            r2,
            async {
                emit_f_impl!(Level::Info, [Field::Oid => 123, Field::Damage => 4821],
                    "[attack] oid=123 dmg=4821".to_string());
            },
        )
        .await;
        let got = rec.0.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].category, Category::Hunt);
        assert_eq!(got[0].level, Level::Info);
        assert_eq!(
            got[0].fields,
            vec![
                (Field::Oid, "123".to_string()),
                (Field::Damage, "4821".to_string())
            ]
        );
    }
}
