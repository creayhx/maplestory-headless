//! TUI 应用状态: 事件轮询、按键处理、补全/历史/滚动。

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc as smpsc;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};
use openstory_bot::emit::{Category, Event, Level};
use openstory_bot::runtime::RunOutcome;
use openstory_bot::state::{BotState, Phase};
use tokio::sync::mpsc as tmpsc;
use tokio::sync::Mutex;

use crate::wizard::{Wizard, WizardResult};
use openstory_console_lib::completion;
use openstory_console_lib::eventq::EventQueue;
use openstory_console_lib::logbuf::LogBuf;
use openstory_console_lib::snapshot::Snapshot;

pub const STAGE_WIZARD: u8 = 0;
pub const STAGE_RUNNING: u8 = 1;
pub const STAGE_FINISHED: u8 = 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    All,
    Cat(Category),
    /// 「状态」页：运行时 bot 状态 + 内存 config 一览（诊断用）。
    Status,
}

impl Filter {
    pub const ORDER: &'static [Filter] = &[
        Filter::All,
        Filter::Cat(Category::Chat),
        Filter::Cat(Category::Notice),
        Filter::Cat(Category::Npc),
        Filter::Cat(Category::Hunt),
        Filter::Cat(Category::Cmd),
        Filter::Cat(Category::View),
        Filter::Cat(Category::Rule),
        Filter::Cat(Category::Group),
        Filter::Cat(Category::Task),
        Filter::Cat(Category::Packet),
        Filter::Cat(Category::System),
        Filter::Cat(Category::Error),
        Filter::Status,
    ];

    pub fn cat(self) -> Option<Category> {
        match self {
            Filter::All => None,
            Filter::Cat(c) => Some(c),
            Filter::Status => None,
        }
    }

    pub fn label(self) -> String {
        match self {
            Filter::All => "全部".to_string(),
            Filter::Cat(c) => openstory_console_lib::theme::cat_label(c).to_string(),
            Filter::Status => "状态".to_string(),
        }
    }

    pub fn next(self) -> Filter {
        let idx = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0);
        Self::ORDER[(idx + 1) % Self::ORDER.len()]
    }

    pub fn prev(self) -> Filter {
        let idx = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0);
        Self::ORDER[(idx + Self::ORDER.len() - 1) % Self::ORDER.len()]
    }

    /// 编码成 `u8` 下标, 存进会话 (每会话一份过滤状态要用)。
    ///
    /// 存下标而不是枚举本身: `BotSession` 因此不必依赖 `app` 模块
    /// (会话层不该知道 UI 有哪些页签)。
    pub fn code(self) -> u8 {
        Self::ORDER.iter().position(|f| *f == self).unwrap_or(0) as u8
    }

    /// 从下标还原 (越界回落到「全部」)。
    pub fn from_code(code: u8) -> Filter {
        Self::ORDER
            .get(code as usize)
            .copied()
            .unwrap_or(Filter::All)
    }
}

pub enum Picker {
    None,
    Gender { sel: usize },
    World { sel: usize },
    Channel { world: usize, sel: usize },
    Char { sel: usize },
}

#[derive(Default)]
pub struct CompState {
    pub candidates: Vec<completion::Candidate>,
    pub idx: usize,
}

/// F2 看板页的一行: 一个账号的全部关键字段。
///
/// 数据直接读各会话的 `BotState` (零延迟、零协议) —— 这是 in-process 多会话
/// 相对 IPC 方案最直接的收益。
pub struct BoardRow {
    pub selected: bool,
    pub profile: String,
    pub running: bool,
    pub finished: bool,
    /// 结束原因是 `Failed` (登录失败 / 重连耗尽) —— 需要人工处理
    pub failed: bool,
    pub dropped: u64,
    pub snap: Snapshot,
    /// 正在等退避拉起 (左栏显示倒计时)
    pub waiting_restart: bool,
    /// 退避到点时刻 (`waiting_restart` 时有值)
    pub restart_at: Option<std::time::Instant>,
    /// 已放弃自动拉起 / 需人工 (左栏亮红)
    pub gave_up: bool,
    /// 需要人工介入: 已放弃 / 登录失败 / **本该拉起却没拉成**。
    ///
    /// 与 `gave_up` 分开: 前者是"试满次数放弃了", 后者还包括"根本没能拉起"
    /// (没有配置模板)。两种情况对用户的含义一样 ("没人管这个号了"), 但诊断
    /// 时要知道是哪一种。
    pub needs_attention: bool,
    /// 第几次拉起 (0 = 首跑)
    pub generation: u32,
    /// 从未启动过 —— 手动模式下的初始状态 (左栏显示 `◌ 未启动`)。
    ///
    /// 与"已结束"必须分开: 一个从没被启动的号和一个跑完退出的号, 用户的下一步
    /// 动作完全相反 (前者"去启动它", 后者"去看它怎么了")。
    pub never_started: bool,
    /// 用户按过 stop, 现在停着 (左栏显示"已停止 (手动)")。
    pub manually_stopped: bool,
    /// 用户按 start 但没起来 (没有密码 / 档案加载失败)。
    pub start_failed: bool,
}

pub struct App {
    pub queue: Arc<EventQueue>,
    pub state: Arc<Mutex<BotState>>,
    /// 共享命令通道: 主线程每次重连会换新 Sender, App 通过锁取当前值
    pub cmd_tx: Arc<StdMutex<tmpsc::Sender<String>>>,
    pub wiz_tx: Option<smpsc::Sender<WizardResult>>,
    pub stage_flag: Arc<AtomicU8>,
    pub outcome: Arc<StdMutex<Option<RunOutcome>>>,
    /// 登录向导预填值 (连接失败重开向导时复用)
    pub login_initial: (String, u16, String, String, String),

    pub log: LogBuf,
    pub scroll: usize,
    pub filter: Filter,
    pub input: String,
    pub cursor: usize,
    pub history: Vec<String>,
    pub hist: Option<usize>,
    draft: String,
    pub comp: CompState,
    pub snap: Snapshot,
    pub wizard: Option<Wizard>,
    /// 向导这次是给**哪个会话**开的 (档案名); `None` = 单会话启动路径。
    ///
    /// 手动模式下按 F4 而那个号没有密码时, 弹的是**同一个**登录向导 (账号/IP
    /// 已从它的档案预填, 焦点直接落在密码上), 提交后要把凭据用回这个会话,
    /// 而不是像单会话那样去重建一个全局会话。
    pub wizard_target: Option<String>,
    /// 向导提交的凭据**已经由渲染线程应用过一次** (给主线程看的一次性标记)。
    ///
    /// 多会话下手动/自动拉起都由渲染线程负责, 主线程不该再去 spawn 一个
    /// 全局会话 —— 但没有这个标记它分辨不出"这次向导提交是不是已经被处理了"。
    ///
    /// **必须是原子量**: 渲染线程写、主线程读。用一个普通 `bool` 字段是数据竞争
    /// (未定义行为), 而且编译器完全不会提醒 —— 它看起来只是一个字段。
    pub login_apply: Arc<AtomicBool>,
    pub picker: Picker,
    pub help_open: bool,
    /// F1 帮助弹窗滚动偏移
    pub help_scroll: usize,
    /// bag 背包界面开关 (本地页面, 不进 bot 命令)
    pub bag_open: bool,
    /// bag 背包界面滚动偏移
    pub bag_scroll: usize,
    /// bag 当前面板: 0=全部 1..=5 = EQUIP/USE/SETUP/ETC/CASH (Tab/Shift+Tab 切换)
    pub bag_panel: usize,
    /// bag 光标 (当前面板内物品索引, Enter 卖出)
    pub bag_cursor: usize,
    /// bag 贩卖模式: 0=卖出选中物品, 1=卖出当前面板整类 (m 切换)
    pub bag_mode: u8,
    /// NPC 选项弹框开关 (npmenu on|off): 默认开启 — 服务器回菜单自动弹框,
    /// ↑↓ 选择 Enter 回复; 关闭时选项只进日志, 手动 `npc reply <id>` 回复。
    /// 任务持锁期间无论开关一律抑制 (见 auto_npc_pick), 防止与串行任务的
    /// `npc reply` 步骤竞争。
    pub npmenu: bool,
    /// NPC 选项弹框当前是否打开 (服务器回菜单 && 开关开)
    pub npc_pick_open: bool,
    /// NPC 选项弹框光标 (选项列表索引)
    pub npc_pick_cursor: usize,
    /// NPC 选项弹框滚动偏移 (选项多于一屏时)
    pub npc_pick_scroll: usize,
    /// 已见的 npc_options_seq: seq 变化 = 服务器发了新对话页。
    /// Enter 确认后置为当前 seq, 弹框关闭直到下一个新菜单。
    pub npc_seen_seq: u64,
    /// 商店购买弹框开关 (shop menu on|off): 默认开启 — 服务器打开商店自动
    /// 弹框, 数字输入数量 Enter 购买; 关闭后商店只进日志, 手动 `buy` 购买。
    pub shop_menu: bool,
    /// 商店购买弹框当前是否打开 (服务器开商店 && 开关开)
    pub shop_pick_open: bool,
    /// 商店弹框光标 (商品列表索引)
    pub shop_pick_cursor: usize,
    /// 商店弹框滚动偏移
    pub shop_pick_scroll: usize,
    /// 已见的 shop_seq: seq 变化 = 服务器发了新商店清单
    pub shop_seen_seq: u64,
    /// 商店弹框当前输入的数量缓冲 (数字键追加, Enter 购买)
    pub shop_qty: String,
    pub dropped: u64,
    pub dirty: bool,
    pub last_draw: Instant,
    pub ctrl_c: Option<Instant>,
    pub force_quit: bool,
    pub interactive: bool,
    pub stage: u8,
    /// 上次渲染尺寸 (日志区宽/高) — 滚动钳制用
    pub last_log_w: u16,
    pub last_log_h: usize,

    /// 多会话集合 (阶段 5)。
    ///
    /// `None` = 单会话模式: 上面那些字段 (queue/state/cmd_tx/log/stage/outcome)
    /// 就是唯一真相, 行为与阶段 4 之前**逐字节一致**。
    ///
    /// `Some(set)` = 多会话模式: 上面那些字段退化为**当前选中会话的缓存视图**,
    /// 由 [`App::sync_from_selected`] 每帧同步。这样 58 处 `self.snap` / 21 处
    /// `self.log` 的渲染代码一行都不用改 —— 渲染逻辑不需要知道有几个账号。
    pub sessions: Option<crate::sessions::SessionSet>,
    /// 左栏是否显示 (多会话时才有意义; 单会话隐藏以保持版面不变)
    pub show_sidebar: bool,
    /// F2 看板页
    pub board_open: bool,
    /// 看板页滚动偏移
    pub board_scroll: usize,
    /// 上次同步到缓存视图的会话下标 (日志 O(1) 交换的自反依据)
    synced: Option<usize>,
    /// 自动拉起所需的句柄 (阶段 6)。`None` = 不做自动拉起。
    ///
    /// 放在 `App` 而不是 `SessionSet` 里: `SessionSet` 是"有哪些会话"的
    /// 数据模型, 而 runtime handle / 退出条件这些是**进程级**的东西,
    /// 混进去会让会话层依赖 tokio runtime (破坏"会话可被进程内模拟")。
    restart: Option<crate::watcher::RestartCtx>,
    /// 上一帧"活着" (在跑 或 在等退避) 的会话数, 用于维护退出计数。
    last_live: usize,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        queue: Arc<EventQueue>,
        state: Arc<Mutex<BotState>>,
        cmd_tx: Arc<StdMutex<tmpsc::Sender<String>>>,
        wiz_tx: Option<smpsc::Sender<WizardResult>>,
        stage_flag: Arc<AtomicU8>,
        outcome: Arc<StdMutex<Option<RunOutcome>>>,
        // 渲染线程写、主线程读 → 必须原子 (见字段文档)
        login_apply: Arc<AtomicBool>,
        interactive: bool,
        ip: String,
        port: u16,
        account: String,
        password: String,
        aes_key: String,
    ) -> Self {
        let wizard = if interactive {
            Some(Wizard::new(
                ip.clone(),
                port,
                account.clone(),
                password.clone(),
                aes_key.clone(),
            ))
        } else {
            None
        };
        Self {
            queue,
            state,
            cmd_tx,
            wiz_tx,
            stage_flag,
            outcome,
            login_initial: (ip, port, account, password, aes_key),
            log: LogBuf::default(),
            scroll: 0,
            filter: Filter::All,
            input: String::new(),
            cursor: 0,
            history: Vec::new(),
            hist: None,
            draft: String::new(),
            comp: CompState::default(),
            snap: Snapshot::default(),
            wizard,
            wizard_target: None,
            login_apply,
            picker: Picker::None,
            help_open: false,
            help_scroll: 0,
            bag_open: false,
            bag_scroll: 0,
            bag_panel: 0,
            bag_cursor: 0,
            bag_mode: 0,
            npmenu: true, // 临时默认开 (测试自动化抑制的实际触发节点), 稳定后再定
            npc_pick_open: false,
            npc_pick_cursor: 0,
            npc_pick_scroll: 0,
            npc_seen_seq: 0,
            shop_menu: true,
            shop_pick_open: false,
            shop_pick_cursor: 0,
            shop_pick_scroll: 0,
            shop_seen_seq: 0,
            shop_qty: String::new(),
            dropped: 0,
            dirty: true,
            last_draw: Instant::now(),
            ctrl_c: None,
            force_quit: false,
            interactive,
            stage: if interactive {
                STAGE_WIZARD
            } else {
                STAGE_RUNNING
            },
            last_log_w: 80,
            last_log_h: 10,
            sessions: None,
            show_sidebar: false,
            board_open: false,
            board_scroll: 0,
            synced: None,
            restart: None,
            last_live: 0,
        }
    }

    /// 挂上多会话集合 (阶段 5)。
    ///
    /// 挂上之后 `poll_events` 每帧把当前选中会话的事件与状态同步进缓存视图,
    /// 渲染代码完全不用改。单会话模式不调用它, 因此那条路径行为不变。
    ///
    /// `restart` 给 `None` 时本 App 完全不碰自动拉起 (单会话 / 测试路径),
    /// 行为与阶段 5 之前一致。
    /// 挂上多会话集合 (渲染线程起跑**之前**必须完成)。
    ///
    /// `started` = **本次启动真正会去登录的会话数**, 不一定等于集合大小:
    /// 手动模式 (`--no-autostart`) 或 `--start <name...>` 下, 有些号只挂上
    /// 会话、停在「未启动」。退出条件 (keepalive) 必须按这个数起算 —— 按集合
    /// 大小记的话, 那些永远不启动的号会让 `keepalive` 永远不为零, 程序于是
    /// 永远不肯退出。
    pub fn attach_sessions(
        &mut self,
        mut set: crate::sessions::SessionSet,
        restart: Option<crate::watcher::RestartCtx>,
        started: usize,
    ) {
        self.show_sidebar = set.len() > 1;
        // 用选中会话的初始视图填充缓存
        if let Some(s) = set.selected() {
            self.stage_flag = s.stage.clone();
            self.outcome = s.outcome.clone();
            self.queue = s.queue.clone();
            self.state = s.state.clone();
        }
        // 已经在跑的会话数: 主线程紧接着就会 spawn 它们, 这里先记上, 免得
        // 退出条件在启动之前的那个窗口里被误判成"全部结束"。
        if let Some(ctx) = &restart {
            ctx.keepalive.store(started, Ordering::SeqCst);
        }
        self.last_live = started;
        // 声明"选中会话的日志缓冲归我了" —— 下面的 sync 会把它 swap 到渲染
        // 位置, 集合里那份就此变成旧缓冲。不声明的话, 选中会话的新日志会被写进
        // 那个永远不上屏的旧缓冲 (现场: "切换时日志偶尔空白, 再切一下又出来")。
        set.set_render_taken_over(true);
        self.sessions = Some(set);
        self.restart = restart;
        self.sync_from_selected();
    }

    /// 每帧驱动自动拉起: 登记刚结束的会话 → 到点重新 spawn → 维护退出条件。
    ///
    /// 为什么不放在主线程: 会话集合归本 App 所有 (渲染线程独占), 让主线程
    /// 拉起就必须把集合共享出来。而"每帧看一眼有没有到点"本来就是渲染循环
    /// 在做的事, 顺手做掉最省事, 也最不容易出竞态。
    ///
    /// 幂等: `note_exit` / `poll_ready` 都只在状态允许时动一次 (见 watcher)。
    pub fn drive_restarts(&mut self, now: std::time::Instant) {
        let Some(ctx) = self.restart.clone() else {
            return;
        };
        let Some(set) = &mut self.sessions else {
            return;
        };
        // 1. 登记本帧刚结束的会话 (排定退避), 并在它自己的日志里留一行
        //
        // 用 `note_at` 而不是 `s.note`: 选中会话的活动缓冲在渲染位置, 直接写
        // 会话里的那份会永远不上屏 (见 `SessionSet::drain_all`)。
        for (i, line) in set.note_exits(now) {
            set.note_at(i, openstory_bot::emit::Level::Err, line);
        }
        // 2. 到点的真正拉起
        let creds = ctx.creds.lock().ok().map(|g| g.clone());
        let (spawned, errs) = set.advance_watchers(&ctx.handle, now, creds.as_ref());
        drop(creds);
        for &i in &spawned {
            ctx.counters.spawned.fetch_add(1, Ordering::SeqCst);
            let generation = set.iter().nth(i).map(|s| s.generation()).unwrap_or(0);
            set.note_at(
                i,
                openstory_bot::emit::Level::Info,
                format!("已自动重新拉起 (第 {generation} 次)"),
            );
        }
        if !spawned.is_empty() {
            self.dirty = true;
        }
        for e in errs {
            ctx.counters.push_error(e.clone());
            self.dirty = true;
        }
        // 3. 维护退出条件: 降为 0 时通知主线程"可以收尾了"
        let live = set.live_count();
        match live.cmp(&self.last_live) {
            std::cmp::Ordering::Less => {
                ctx.keepalive
                    .fetch_sub(self.last_live - live, Ordering::SeqCst);
            }
            std::cmp::Ordering::Greater => {
                ctx.keepalive
                    .fetch_add(live - self.last_live, Ordering::SeqCst);
            }
            std::cmp::Ordering::Equal => {}
        }
        self.last_live = live;
        // 手动模式 (`--no-autostart`): 还有号从没启动过 → 不许收尾。
        //
        // 否则用户启动第一个号、它正常退出之后 `all_done` 立刻置位, 整个程序
        // 会在用户还没来得及启动第二个号的时候就退掉。
        if set.all_stopped() && !set.has_never_started() {
            ctx.mark_all_done();
        }
    }

    /// 全账号共用的一条"哨兵"命令发送端。
    ///
    /// 多会话下 `cmd_tx` 不再是唯一入口 (指令要按目标分发), 但单会话命令路径
    /// (`self.cmd_tx.lock().try_send`) 仍在用。把它指向当前选中会话, 这样
    /// "输入框直接敲指令" = 发给当前选中会话, 与 `@` 目标语法一致。
    fn point_cmd_tx_at_selected(&mut self) {
        let Some(set) = &self.sessions else { return };
        let Some(s) = set.selected() else { return };
        // 会话内部的 cmd_slot 是私有的, 通过一个转发 sender 桥接:
        // 这里不改会话, 而是让 App 的 cmd_tx 直接持有同一把锁的克隆。
        let tx = s.cmd_tx_handle();
        self.cmd_tx = tx;
    }

    /// 把当前选中会话的状态同步进缓存视图, 并把本帧的滚动写回会话。
    ///
    /// # 日志的 O(1) 交换
    ///
    /// 日志缓冲只有一份在"渲染位置" (`self.log`), 其余都在各自会话里。用
    /// `swap` (三个 `VecDeque` 指针交换) 而不是复制 —— 1000 行 × N 个账号
    /// 每帧复制是不可接受的。
    ///
    /// 交换是**自反**的: 缓存里始终是"上次同步的那个会话"的日志, 所以
    /// `swap` 天然把旧会话的日志还回去、把新会话的换出来。用 `synced` 记录
    /// 上次同步的是谁, 只有真的换了会话才交换 (同一会话每帧交换一次等于没换,
    /// 但会白白丢掉折行缓存)。
    pub fn sync_from_selected(&mut self) {
        if self.sessions.is_none() {
            return;
        }
        let cur = self
            .sessions
            .as_ref()
            .map(|s| s.selected_index())
            .unwrap_or(0);
        // 1. 本帧的滚动位置与日志页签属于"上次同步的那个会话", 先写回
        if let Some(prev) = self.synced {
            if let Some(set) = &mut self.sessions {
                if let Some(s) = set.iter_mut().nth(prev) {
                    s.scroll = self.scroll;
                    s.filter = self.filter.code();
                }
            }
        }
        // 2. 换会话: **两步轮换**, 不是一步交换。
        //
        // 选中会话的日志缓冲就是 `self.log` (它的新事件也写在这里, 见
        // `drain_all_sessions`)。所以切换必须:
        //   2a. 把 `self.log` 里的内容**还回上一个会话**;
        //   2b. 再把新会话的换出来。
        //
        // 只做一步 `swap(cur)` 会把上一个会话的日志塞进**新会话**的槽位,
        // 而它自己的槽位仍是空的 —— 于是切回去时 `self.log` 拿到空缓冲:
        // 用户看到的正是"Alt+↑↓ 切着切着日志变空白, 再切一下又出来"。
        //
        // 两步都结束后, 每个会话的 `s.log` 都恰好装着自己的日志, 而 `self.log`
        // 装着当前选中会话的 —— 中间那份"没人要的旧内容"总会在下一次交换里被
        // 整个覆盖, 不会污染任何人。
        if self.synced != Some(cur) {
            if let Some(set) = &mut self.sessions {
                if let Some(prev) = self.synced {
                    if let Some(s) = set.iter_mut().nth(prev) {
                        s.log.swap(&mut self.log);
                    }
                }
                if let Some(s) = set.iter_mut().nth(cur) {
                    s.log.swap(&mut self.log);
                }
            }
        }
        // 3. 读取新会话的视图
        let (queue, state, stage, outcome, scroll, dropped, filter) = {
            let Some(set) = &self.sessions else { return };
            let Some(s) = set.selected() else { return };
            (
                s.queue.clone(),
                s.state.clone(),
                s.stage.clone(),
                s.outcome.clone(),
                s.scroll,
                s.dropped(),
                s.filter,
            )
        };
        self.queue = queue;
        self.state = state;
        self.stage_flag = stage;
        self.outcome = outcome;
        self.scroll = scroll;
        self.dropped = dropped;
        // 每会话一份过滤: 在 A 上切到"聊天"不该把 B 也切成"聊天"
        self.filter = Filter::from_code(filter);
        let changed_session = self.synced != Some(cur);
        self.synced = Some(cur);
        self.point_cmd_tx_at_selected();
        // 只在**真的换了会话**时立即刷一次快照, 免得切换后那一帧显示上一个账号
        // 的状态。
        //
        // 从前这里是每帧无条件刷, 而且是 `try_lock().map(..).unwrap_or_default()`
        // —— 那是"状态栏闪烁"的根源 (实测确认): bot 在 `command::tick` 里持锁
        // 干活 (含网络 I/O, 可能几十上百毫秒), 渲染线程这一帧 `try_lock` 失败,
        // 于是右侧面板被塞进一个**全 0 的默认快照**显示一帧, 下一帧又恢复。
        // 用户看到的就是"状态参数莫名其妙地闪一下"。
        //
        // 现在: 换会话时才强制刷 (那时锁失败也不能拿旧账号的数据), 其余每帧由
        // `poll_events` 用 `try_lock` 更新 —— 失败就保留上一份好数据。
        if changed_session {
            self.snap = self
                .state
                .try_lock()
                .map(|st| Snapshot::from(&*st))
                .unwrap_or_default();
        }
        self.dirty = true;
    }

    /// 把所有会话的事件取进各自的日志缓冲, 再刷新缓存视图。
    ///
    /// 多会话模式下 `poll_events` 调它; 单会话模式不调 (那条路径仍是原来的
    /// 单队列 drain)。
    pub fn drain_all_sessions(&mut self) {
        if let Some(set) = &mut self.sessions {
            set.drain_all();
            // 选中会话的日志缓冲**就在这里** (`self.log`) —— 它的新事件由
            // `drain_all` 攒在 `render_pending` 里, 必须在这里落到渲染缓冲上,
            // 否则那些日志永远不上屏。
            for ev in set.take_render_pending() {
                self.log.push(ev);
            }
        }
    }

    /// 切换账号 (`d` 为相对位移, 循环)。返回是否真的切换了。
    pub fn select_session_move(&mut self, d: i32) -> bool {
        let Some(set) = &mut self.sessions else {
            return false;
        };
        if set.len() <= 1 {
            return false;
        }
        set.select_move(d);
        self.sync_from_selected();
        true
    }

    /// 当前选中账号的下标 / 总数 (左栏与看板用)。
    pub fn session_position(&self) -> Option<(usize, usize)> {
        let set = self.sessions.as_ref()?;
        if set.is_empty() {
            return None;
        }
        Some((set.selected_index(), set.len()))
    }

    /// 全部会话的状态快照 (看板页用)。`try_lock` 失败时给默认值,
    /// 保证一个卡住的会话不会让整页渲染不出来。
    pub fn board_rows(&self) -> Vec<BoardRow> {
        let Some(set) = &self.sessions else {
            return Vec::new();
        };
        let selected = set.selected_index();
        set.iter()
            .enumerate()
            .map(|(i, s)| {
                let snap = s
                    .state
                    .try_lock()
                    .map(|st| Snapshot::from(&*st))
                    .unwrap_or_default();
                BoardRow {
                    selected: i == selected,
                    profile: s.profile.clone(),
                    running: s.is_running(),
                    finished: s.is_finished(),
                    failed: matches!(s.outcome(), Some(RunOutcome::Failed)),
                    dropped: s.dropped(),
                    snap,
                    waiting_restart: s.is_waiting_restart(),
                    restart_at: s.restart_deadline(),
                    gave_up: s.gave_up_restart(),
                    needs_attention: s.needs_attention(),
                    generation: s.generation(),
                    never_started: s.never_started(),
                    manually_stopped: s.is_manually_stopped(),
                    start_failed: s.start_failed(),
                }
            })
            .collect()
    }

    /// 手动重启当前选中会话 (F3)。
    ///
    /// 三种情况:
    /// - 会话还在跑 → 只清退避计数 (等价于"别再重试了, 从现在重新算"),
    ///   日志里说明一下, 不真的断线重连 (那会把一个正常工作的号踢下来)。
    /// - 会话已结束 → 立刻重新拉起, 不等退避。
    /// - 单会话 / 没挂集合 → 什么都不做 (那条路径的重连走向导)。
    pub fn manual_restart_selected(&mut self) {
        let Some(ctx) = self.restart.clone() else {
            return;
        };
        let Some(set) = &mut self.sessions else {
            return;
        };
        let i = set.selected_index();
        if set.is_empty() {
            return;
        }
        let was_running = set.selected().map(|s| s.is_running()).unwrap_or(false);
        let profile = set
            .selected()
            .map(|s| s.profile.clone())
            .unwrap_or_default();
        if was_running {
            if let Some(s) = set.iter_mut().nth(i) {
                s.reset_watcher();
            }
            set.note_at(
                i,
                openstory_bot::emit::Level::Info,
                "已清除重连计数 (会话仍在运行)".to_string(),
            );
            self.dirty = true;
            return;
        }
        let creds = ctx.creds.lock().ok().map(|g| g.clone());
        let res = set.manual_restart(&ctx.handle, i, std::time::Instant::now(), creds.as_ref());
        drop(creds);
        match res {
            Ok(()) => {
                set.note_at(i, openstory_bot::emit::Level::Info, "手动重新拉起");
                self.dirty = true;
            }
            Err(e) => {
                ctx.counters.push_error(e.clone());
                set.note_at(
                    i,
                    openstory_bot::emit::Level::Err,
                    format!("手动拉起失败: {e}"),
                );
                self.dirty = true;
            }
        }
        let _ = profile;
    }

    /// 重新打开登录向导 (连接失败/服务器维护后): 用上次的登录信息预填。
    /// 优先读 config.json `login` 节 (向导提交后已保存, 含密码),
    /// 兜底用启动时的初始值。
    pub fn reopen_wizard(&mut self) {
        let (ip, port, account, password, aes_key) = match openstory_bot::login::load_default() {
            Some(info) => (
                info.ip,
                info.port.unwrap_or(8484),
                info.account,
                info.password,
                info.aes_key.unwrap_or_default(),
            ),
            None => self.login_initial.clone(),
        };
        self.wizard = Some(Wizard::new(ip, port, account, password, aes_key));
        self.picker = Picker::None;
        self.help_open = false;
        self.dirty = true;
    }

    pub fn should_exit(&self) -> bool {
        if self.force_quit {
            return true;
        }
        match self.stage {
            STAGE_FINISHED => {
                let out = self.outcome.lock().map(|o| *o).unwrap_or(None);
                // quit/duration → 直接退出; 断线 → 留屏供回顾, Ctrl+C 退出
                matches!(
                    out,
                    Some(RunOutcome::Quit) | Some(RunOutcome::DurationElapsed)
                ) || self.ctrl_c.is_some()
            }
            _ => false,
        }
    }

    /// 每帧轮询: 事件队列 + 状态快照 + 阶段切换 + 自动拉起。
    pub fn poll_events(&mut self) {
        // 多会话模式: 事件分散在各会话自己的队列里, 先各自 drain 再同步视图。
        // 单会话模式: 走下面的原路径 (行为与阶段 4 之前一致)。
        if self.sessions.is_some() {
            self.drain_all_sessions();
            // 自动拉起: 必须在 sync_from_selected 之前 —— 它可能刚刚重新
            // spawn 了会话并改写阶段, 先拉起再同步才能让本帧就显示"重连中"。
            self.drive_restarts(std::time::Instant::now());
            // 每帧同步: 拾取各会话最新的阶段/结束原因/丢弃计数, 并刷新快照
            self.sync_from_selected();
        }
        let in_multi = self.sessions.is_some();
        let (evs, dropped) = if in_multi {
            (Vec::new(), self.dropped)
        } else {
            self.queue.take_all()
        };
        if !evs.is_empty() {
            for ev in evs {
                self.log.push(ev);
            }
            self.dirty = true;
        }
        if dropped != self.dropped {
            self.dropped = dropped;
            self.dirty = true;
        }

        // 多会话模式下 **不** 用 `stage_flag` 覆盖 `App.stage`。
        //
        // `attach_sessions` 把 `stage_flag` 指向**选中会话自己的** stage (单会话
        // 缓冲视图那一套), 于是这里读到的其实是"那个会话在什么阶段"。而在手动
        // 模式下会话全都停在 `STAGE_WIZARD` (未启动) —— 每帧都会把主线程刚设好的
        // `STAGE_RUNNING` 覆盖回 `WIZARD`, 结果向导永远关不掉: 用户按了 Enter,
        // 主线程也确实收到了 (实测日志确认), 但画面卡在向导上不动。
        //
        // 多会话下 `App.stage` 由主线程直接管理 (向导只在单会话启动路径出现),
        // 因此这里整段跳过。
        let in_multi = self.sessions.is_some();
        if !in_multi {
            let stage = self.stage_flag.load(Ordering::Relaxed);
            if stage != self.stage {
                self.stage = stage;
                self.dirty = true;
                // 向导结束 → 收键盘焦点
                if stage == STAGE_RUNNING {
                    self.wizard = None;
                    self.picker = Picker::None;
                } else {
                    // 离开运行阶段 (断开回向导 / 结束): 清掉 NPC/商店弹框状态,
                    // 避免重连向导期间残留 (重连后新菜单/清单到达时 auto_*_pick 会重开)。
                    self.npc_pick_open = false;
                    self.shop_pick_open = false;
                    self.shop_qty.clear();
                }
            }
            // 连接失败/维护后主线程把 stage 置回 STAGE_WIZARD → 重新打开向导
            if self.stage == STAGE_WIZARD && self.wizard.is_none() {
                self.reopen_wizard();
            }
        }

        let snap_opt = self.state.try_lock().ok().map(|st| Snapshot::from(&st));
        if let Some(snap) = snap_opt {
            let phase_changed = snap.phase != self.snap.phase;
            self.snap = snap;
            if phase_changed {
                self.picker = Picker::None;
                self.dirty = true;
            }
        }
        self.auto_picker();
        self.auto_npc_pick();
        self.auto_shop_pick();

        // 滚动钳制
        let total = self
            .log
            .total_display_lines(self.filter.cat(), self.last_log_w);
        let max_off = total.saturating_sub(self.last_log_h).max(0);
        if self.scroll > max_off {
            self.scroll = max_off;
            self.dirty = true;
        }
    }

    fn auto_picker(&mut self) {
        if !self.interactive || self.stage != STAGE_RUNNING || self.wizard.is_some() {
            return;
        }
        if !matches!(self.picker, Picker::None) {
            return;
        }
        match self.snap.phase {
            Phase::GenderPick => self.picker = Picker::Gender { sel: 0 },
            Phase::WorldSelect if !self.snap.worlds.is_empty() => {
                self.picker = Picker::World { sel: 0 }
            }
            Phase::CharSelect if !self.snap.characters.is_empty() => {
                self.picker = Picker::Char { sel: 0 }
            }
            _ => {}
        }
    }

    /// NPC 选项弹框自动开合: 开关开 && 服务器发了新对话页 (seq 变化)。
    /// 新页有选项 → 弹框 (光标归零); 无选项 (文本/输入对话) → 不弹, 对话
    /// 内容照常进日志。Enter 确认后由 seen_seq 标记"已看", 弹框关闭,
    /// 直到下一个新菜单。服务器每个菜单页只发一次 — 每次到达都是新对话。
    /// 注意: 不依赖 `interactive` — 带账号密码启动 (interactive=false) 时
    /// 一样要弹; 只要在游戏运行阶段且向导未开即可。
    ///
    /// 自动化抑制: 任务持锁期间 (task_stack 非空) 禁止弹出, 已开的强制本地
    /// 收起 (不发任何包) — 脚本自己驱动对话, 弹框会与之竞争 (双通道注入 +
    /// 按键劫持)。seen_seq 故意不更新: 任务栈清空后下一轮按**当前状态**
    /// 重新评估, 保证自动化结束后显示正确 (残留菜单已随商店打开被清空,
    /// 不会再误弹)。
    fn auto_npc_pick(&mut self) {
        if self.stage != STAGE_RUNNING || self.wizard.is_some() {
            return;
        }
        if !self.snap.task_stack.is_empty() {
            if self.npc_pick_open {
                self.npc_pick_open = false;
                self.dirty = true;
            }
            return;
        }
        if !self.npmenu {
            return;
        }
        let seq = self.snap.npc_options_seq;
        if seq == self.npc_seen_seq {
            return;
        }
        self.npc_seen_seq = seq;
        if self.snap.npc_options.is_empty() {
            self.npc_pick_open = false;
        } else {
            self.npc_pick_open = true;
            self.npc_pick_cursor = 0;
            self.npc_pick_scroll = 0;
        }
        self.dirty = true;
    }

    /// 商店购买弹框自动开合: 开关开 && 服务器发了新商店清单 (seq 变化)。
    /// 新清单非空 → 弹框 (光标/数量重置); 空 (商店关闭) → 收起并清数量。
    /// 服务器只在打开商店时发一次 0x146, 购买成功不回发清单 — 每次到达都
    /// 是全新商店, 统一归零。与 NPC 弹框同样不依赖 `interactive`。
    ///
    /// 自动化抑制与 NPC 弹框一致: 任务持锁期间不弹 + 强制收起; seen_seq
    /// 不更新, 任务栈清空后按当前商店状态重新评估 — 纯开店任务跑完商店
    /// 还开着 → 弹框正常出现 (手动购买); 卖装链跑完 shop leave 已清空 →
    /// 不弹。自动化期间脚本自己买/卖, 弹框会误触消费。
    fn auto_shop_pick(&mut self) {
        if self.stage != STAGE_RUNNING || self.wizard.is_some() {
            return;
        }
        if !self.snap.task_stack.is_empty() {
            if self.shop_pick_open {
                self.shop_pick_open = false;
                self.shop_qty.clear();
                self.dirty = true;
            }
            return;
        }
        if !self.shop_menu {
            return;
        }
        let seq = self.snap.shop_seq;
        if seq == self.shop_seen_seq {
            return;
        }
        self.shop_seen_seq = seq;
        if self.snap.shop_items.is_empty() {
            self.shop_pick_open = false;
            self.shop_qty.clear();
        } else {
            self.shop_pick_open = true;
            self.shop_pick_cursor = 0;
            self.shop_pick_scroll = 0;
            self.shop_qty.clear();
        }
        self.dirty = true;
    }

    /// 处理单个按键事件。
    pub fn on_key(&mut self, key: KeyEvent) {
        // Windows 上 crossterm 会同时上报按下/松开两种事件,
        // 松开(Release)必须忽略, 否则每个字符输入两次。
        if key.kind == KeyEventKind::Release {
            return;
        }
        match self.stage {
            STAGE_WIZARD => {
                if let Some(w) = self.wizard.as_mut() {
                    if key.code == KeyCode::Esc {
                        // 这是"给某个会话补登录信息"打开的向导 → Esc 只是取消,
                        // 不该顺手退出整个控制台 (用户很可能只是改主意了)。
                        if self.wizard_target.is_some() {
                            self.wizard = None;
                            self.wizard_target = None;
                            self.stage = STAGE_RUNNING;
                            self.dirty = true;
                            return;
                        }
                        self.force_quit = true;
                        return;
                    }
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        self.force_quit = true;
                        return;
                    }
                    if w.on_key(key) {
                        if let Some(r) = w.result.clone() {
                            // 手动模式下这个向导是给**某个会话**补凭据用的:
                            // 用回那个会话并启动它, 不惊动主线程的全局会话路径。
                            if self.apply_wizard_to_session(&r) {
                                return;
                            }
                            if let Some(tx) = &self.wiz_tx {
                                let _ = tx.send(r);
                            }
                        }
                    }
                }
            }
            STAGE_RUNNING => {
                if !matches!(self.picker, Picker::None) {
                    self.on_picker_key(key);
                    return;
                }
                if self.npc_pick_open {
                    // NPC 选项弹框: 完全拦截按键, Esc 才能退出 (不发送命令)。
                    match key.code {
                        KeyCode::Up => {
                            self.npc_pick_move(-1);
                            self.dirty = true;
                        }
                        KeyCode::Down => {
                            self.npc_pick_move(1);
                            self.dirty = true;
                        }
                        KeyCode::Enter => {
                            self.npc_pick_confirm();
                            self.dirty = true;
                        }
                        KeyCode::Esc => {
                            self.npc_pick_open = false;
                            self.dirty = true;
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            self.on_ctrl_c();
                        }
                        _ => {}
                    }
                    return;
                }
                if self.shop_pick_open {
                    // 商店购买弹框: 完全拦截按键。↑↓ 选商品, 数字键输入数量,
                    // Enter 购买 (无数量 = 1), Esc 先清数量再收起。
                    match key.code {
                        KeyCode::Up => {
                            self.shop_pick_move(-1);
                            self.dirty = true;
                        }
                        KeyCode::Down => {
                            self.shop_pick_move(1);
                            self.dirty = true;
                        }
                        KeyCode::Char(c) if c.is_ascii_digit() => {
                            // 数量上限 5 位 (9999+ 足够, 防 i16 溢出)
                            if self.shop_qty.len() < 5 {
                                self.shop_qty.push(c);
                            }
                            self.dirty = true;
                        }
                        KeyCode::Backspace => {
                            self.shop_qty.pop();
                            self.dirty = true;
                        }
                        KeyCode::Enter => {
                            self.shop_pick_confirm();
                            self.dirty = true;
                        }
                        KeyCode::Esc => {
                            if !self.shop_qty.is_empty() {
                                self.shop_qty.clear();
                            } else {
                                self.shop_pick_open = false;
                            }
                            self.dirty = true;
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            self.on_ctrl_c();
                        }
                        _ => {}
                    }
                    return;
                }
                if self.board_open {
                    match key.code {
                        KeyCode::Esc | KeyCode::F(2) => {
                            self.board_open = false;
                            self.dirty = true;
                        }
                        KeyCode::PageUp => {
                            self.board_scroll = self.board_scroll.saturating_sub(8);
                            self.dirty = true;
                        }
                        KeyCode::PageDown => {
                            self.board_scroll = self.board_scroll.saturating_add(8);
                            self.dirty = true;
                        }
                        KeyCode::Up => {
                            self.board_scroll = self.board_scroll.saturating_sub(1);
                            self.dirty = true;
                        }
                        KeyCode::Down => {
                            self.board_scroll = self.board_scroll.saturating_add(1);
                            self.dirty = true;
                        }
                        _ => {}
                    }
                    return;
                }
                if self.help_open {
                    match key.code {
                        KeyCode::Esc | KeyCode::F(1) => {
                            self.help_open = false;
                            self.dirty = true;
                        }
                        KeyCode::PageUp => {
                            self.help_scroll = self.help_scroll.saturating_sub(8);
                            self.dirty = true;
                        }
                        KeyCode::PageDown => {
                            self.help_scroll = self.help_scroll.saturating_add(8);
                            self.dirty = true;
                        }
                        KeyCode::Up => {
                            self.help_scroll = self.help_scroll.saturating_sub(1);
                            self.dirty = true;
                        }
                        KeyCode::Down => {
                            self.help_scroll = self.help_scroll.saturating_add(1);
                            self.dirty = true;
                        }
                        _ => {}
                    }
                    return;
                }
                if self.bag_open {
                    match key.code {
                        KeyCode::Esc => {
                            self.bag_open = false;
                            self.dirty = true;
                        }
                        KeyCode::Tab => {
                            self.bag_panel = (self.bag_panel + 1) % 6;
                            self.bag_scroll = 0;
                            self.bag_cursor = 0;
                            self.dirty = true;
                        }
                        KeyCode::BackTab => {
                            self.bag_panel = (self.bag_panel + 5) % 6;
                            self.bag_scroll = 0;
                            self.bag_cursor = 0;
                            self.dirty = true;
                        }
                        KeyCode::Up => {
                            self.bag_cursor = self.bag_cursor.saturating_sub(1);
                            self.follow_bag_cursor();
                            self.dirty = true;
                        }
                        KeyCode::Down => {
                            self.bag_cursor = self.bag_cursor.saturating_add(1);
                            self.clamp_bag_cursor();
                            self.follow_bag_cursor();
                            self.dirty = true;
                        }
                        KeyCode::Char('m') => {
                            self.bag_mode ^= 1;
                            self.dirty = true;
                        }
                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            self.bag_drop();
                            self.dirty = true;
                        }
                        KeyCode::Char('e') | KeyCode::Char('E') => {
                            self.bag_equip();
                            self.dirty = true;
                        }
                        KeyCode::Enter => {
                            self.bag_enter();
                            self.dirty = true;
                        }
                        _ => {}
                    }
                    return;
                }
                self.on_input_key(key);
            }
            _ => {
                // 结束后仍可滚动/查看
                match key.code {
                    KeyCode::PageUp => {
                        self.scroll += 10;
                        self.dirty = true;
                    }
                    KeyCode::PageDown => {
                        self.scroll = self.scroll.saturating_sub(10);
                        self.dirty = true;
                    }
                    // 切账号: Alt+↑↓ (必须排在裸 ↑↓ 之前 —— 裸 ↑↓ 会吃掉全部
                    // Up/Down, 否则这两个带守卫的分支永远不可达)
                    KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => {
                        if self.select_session_move(-1) {
                            self.dirty = true;
                        }
                    }
                    KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
                        if self.select_session_move(1) {
                            self.dirty = true;
                        }
                    }
                    KeyCode::Up => {
                        self.scroll += 1;
                        self.dirty = true;
                    }
                    KeyCode::Down => {
                        self.scroll = self.scroll.saturating_sub(1);
                        self.dirty = true;
                    }
                    KeyCode::F(1) => {
                        self.help_open = !self.help_open;
                        self.help_scroll = 0;
                        self.dirty = true;
                    }
                    KeyCode::F(2) => {
                        // 多会话才有看板可看; 单会话按 F2 不做事 (版面不变)
                        if self.sessions.is_some() {
                            self.board_open = !self.board_open;
                            self.board_scroll = 0;
                            self.dirty = true;
                        }
                    }
                    // 手动重启当前会话 (F3)。
                    //
                    // 两个 stage 分支里都要挂: 运行阶段 (会话还在跑 / 刚掉线)
                    // 与"已结束"阶段 (整个应用结束了, 但多会话下常常只是某一个
                    // 号结束)。而恰恰是"已经结束"的时候最需要手动拉起 ——
                    // 只挂在运行阶段的话, F3 在最需要它的场合恰好不工作。
                    //
                    // 用 F3 而不是 `R`: 运行阶段的普通字符都要进输入框,
                    // 抢掉一个字母会让 `chat ...` 里的大写 R 打不出来。
                    // 单会话不生效 (那条路径的重连走向导)。
                    KeyCode::F(3) => {
                        self.manual_restart_selected();
                    }
                    // 启停当前会话 (F4)。同样两个 stage 分支都要挂 ——
                    // "已经结束"的会话正是最需要按 F4 重新启动的那一个。
                    KeyCode::F(4) => {
                        self.toggle_selected_session();
                    }
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.ctrl_c = Some(Instant::now());
                    }
                    _ => {}
                }
            }
        }
    }

    fn on_picker_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => {
                self.picker_move(-1);
                self.dirty = true;
            }
            KeyCode::Down => {
                self.picker_move(1);
                self.dirty = true;
            }
            KeyCode::Enter => {
                self.picker_confirm();
                self.dirty = true;
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.on_ctrl_c();
            }
            _ => {}
        }
    }

    fn picker_move(&mut self, d: i32) {
        let (sel, len) = match &mut self.picker {
            Picker::Gender { sel } => (*sel, 2usize),
            Picker::World { sel } => (*sel, self.snap.worlds.len().max(1)),
            Picker::Channel { world, sel } => {
                let n = self
                    .snap
                    .worlds
                    .get(*world)
                    .map(|(_, _, ch)| *ch as usize)
                    .unwrap_or(1)
                    .max(1);
                (*sel, n)
            }
            Picker::Char { sel } => (*sel, self.snap.characters.len().max(1)),
            Picker::None => return,
        };
        let n = len as i32;
        *match &mut self.picker {
            Picker::Gender { sel } => sel,
            Picker::World { sel } => sel,
            Picker::Channel { sel, .. } => sel,
            Picker::Char { sel } => sel,
            Picker::None => unreachable!(),
        } = ((sel as i32 + d + n) % n) as usize;
    }

    fn picker_confirm(&mut self) {
        match self.picker {
            Picker::Gender { sel } => {
                let _ = self
                    .cmd_tx
                    .lock()
                    .unwrap()
                    .try_send(format!("login gender {sel}"));
            }
            Picker::World { sel } => {
                self.picker = Picker::Channel { world: sel, sel: 0 };
            }
            Picker::Channel { world, sel } => {
                let _ = self
                    .cmd_tx
                    .lock()
                    .unwrap()
                    .try_send(format!("login world {world} {}", sel + 1));
            }
            Picker::Char { sel } => {
                let _ = self
                    .cmd_tx
                    .lock()
                    .unwrap()
                    .try_send(format!("login char {sel}"));
            }
            Picker::None => {}
        }
    }

    fn on_input_key(&mut self, key: KeyEvent) {
        match key.code {
            // 手动重启当前会话 (F3)。
            //
            // 必须放在这里 (运行阶段的输入处理里), 而不是只挂在"已结束"那个
            // 分支 —— 否则会话还在跑 (或刚掉线、正等着退避) 的时候按键完全
            // 没反应, 而那正是最需要它的场合。
            //
            // 用 F3 而不是 `R`: 运行阶段的普通字符都要进输入框, 抢掉一个字母
            // 会让 `chat ...` 里的大写 R 打不出来。
            KeyCode::F(3) => {
                self.manual_restart_selected();
            }
            // 启停当前会话 (F4)。
            //
            // 与 F3 分工: F3 = "重启"(清退避计数后立刻拉起), F4 = "启动/停止"。
            // 手动模式下最常用的是 F4 —— 挂上去的号由用户决定谁连。
            //
            // 用 F4 而不是 `S`: 这里的普通字符都要进输入框, 而 `s` 是拼音输入
            // 里出现频率极高的字母 (shang/sha/...), 抢掉它等于让用户打不出字。
            KeyCode::F(4) => {
                self.toggle_selected_session();
            }
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    if c == 'c' {
                        self.on_ctrl_c();
                    }
                    return;
                }
                insert_at(&mut self.input, self.cursor, c);
                self.cursor += 1;
                self.hist = None;
                self.refresh_completion();
                self.dirty = true;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    remove_at(&mut self.input, self.cursor - 1);
                    self.cursor -= 1;
                    self.hist = None;
                    self.refresh_completion();
                    self.dirty = true;
                }
            }
            KeyCode::Delete => {
                if self.cursor < char_len(&self.input) {
                    remove_at(&mut self.input, self.cursor);
                    self.hist = None;
                    self.refresh_completion();
                    self.dirty = true;
                }
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                self.dirty = true;
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(char_len(&self.input));
                self.dirty = true;
            }
            KeyCode::Home => {
                self.cursor = 0;
                self.dirty = true;
            }
            KeyCode::End => {
                self.cursor = char_len(&self.input);
                self.dirty = true;
            }
            // 切账号: Alt+↑↓。
            //
            // 必须排在裸 ↑↓ 之前 —— 裸 ↑↓ 已占用 (补全轮换 / 输入历史),
            // 带守卫的分支写在后面永远不可达。
            KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => {
                if self.select_session_move(-1) {
                    self.dirty = true;
                }
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
                if self.select_session_move(1) {
                    self.dirty = true;
                }
            }
            KeyCode::Up => {
                if !self.comp.candidates.is_empty() {
                    self.comp.idx = (self.comp.idx + self.comp.candidates.len() - 1)
                        % self.comp.candidates.len();
                    self.apply_completion();
                    self.dirty = true;
                } else {
                    self.history_up();
                }
            }
            KeyCode::Down => {
                if !self.comp.candidates.is_empty() {
                    self.comp.idx = (self.comp.idx + 1) % self.comp.candidates.len();
                    self.apply_completion();
                    self.dirty = true;
                } else {
                    self.history_down();
                }
            }
            KeyCode::Tab => {
                if self.input.trim().is_empty() {
                    self.filter = self.filter.next();
                    self.scroll = 0;
                    self.dirty = true;
                } else if self.comp.candidates.is_empty() {
                    self.refresh_completion();
                    if !self.comp.candidates.is_empty() {
                        self.apply_completion();
                        self.dirty = true;
                    }
                } else {
                    self.comp.idx = (self.comp.idx + 1) % self.comp.candidates.len();
                    self.apply_completion();
                    self.dirty = true;
                }
            }
            KeyCode::BackTab => {
                if self.input.trim().is_empty() {
                    self.filter = self.filter.prev();
                    self.scroll = 0;
                    self.dirty = true;
                } else if self.comp.candidates.is_empty() {
                    self.refresh_completion();
                    if !self.comp.candidates.is_empty() {
                        self.comp.idx = self.comp.candidates.len() - 1;
                        self.apply_completion();
                        self.dirty = true;
                    }
                } else {
                    self.comp.idx = (self.comp.idx + self.comp.candidates.len() - 1)
                        % self.comp.candidates.len();
                    self.apply_completion();
                    self.dirty = true;
                }
            }
            KeyCode::Enter => {
                self.submit();
            }
            KeyCode::Esc => {
                if !self.comp.candidates.is_empty() {
                    // 第一次 Esc 只关闭补全提示, 保留输入
                    self.comp = CompState::default();
                } else {
                    self.input.clear();
                    self.cursor = 0;
                }
                self.dirty = true;
            }
            KeyCode::PageUp => {
                self.scroll += 10;
                self.dirty = true;
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_sub(10);
                self.dirty = true;
            }
            KeyCode::F(1) => {
                self.help_open = !self.help_open;
                self.help_scroll = 0;
                self.dirty = true;
            }
            KeyCode::F(2) => {
                // 多会话才有看板可看。
                //
                // 判据必须是"有多个会话"而不是"sessions 是 Some" —— 单档也走
                // 会话层 (`attach_sessions` 挂一个会话), 用 is_some() 会让单档
                // 的 F2 也打开看板, 版面就不再与原来的 TUI 一致。
                if self.session_position().map_or(false, |(_, n)| n > 1) {
                    self.board_open = !self.board_open;
                    self.board_scroll = 0;
                    self.dirty = true;
                }
            }
            // 切账号: Alt+↑↓ (处理在下面的 Up/Down 守卫分支里 —— 这里不能重复,
            // 否则先出现的裸 ↑↓ 会把它变成不可达模式)
            _ => {}
        }
    }

    pub fn on_mouse(&mut self, kind: MouseEventKind) {
        match kind {
            MouseEventKind::ScrollUp => {
                if self.help_open {
                    self.help_scroll = self.help_scroll.saturating_sub(3);
                } else if self.bag_open {
                    self.bag_scroll = self.bag_scroll.saturating_sub(3);
                } else if self.npc_pick_open {
                    self.npc_pick_scroll = self.npc_pick_scroll.saturating_sub(3);
                } else if self.shop_pick_open {
                    self.shop_pick_scroll = self.shop_pick_scroll.saturating_sub(3);
                } else {
                    self.scroll += 3;
                }
                self.dirty = true;
            }
            MouseEventKind::ScrollDown => {
                if self.help_open {
                    self.help_scroll = self.help_scroll.saturating_add(3);
                } else if self.bag_open {
                    self.bag_scroll = self.bag_scroll.saturating_add(3);
                } else if self.npc_pick_open {
                    self.npc_pick_scroll = self.npc_pick_scroll.saturating_add(3);
                } else if self.shop_pick_open {
                    self.shop_pick_scroll = self.shop_pick_scroll.saturating_add(3);
                } else {
                    self.scroll = self.scroll.saturating_sub(3);
                }
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn on_ctrl_c(&mut self) {
        match self.ctrl_c {
            Some(t) if t.elapsed() < std::time::Duration::from_secs(1) => {
                self.force_quit = true;
            }
            _ => {
                self.ctrl_c = Some(Instant::now());
                self.log.push(Event::new(
                    Level::Info,
                    Category::System,
                    "> [Ctrl+C] 发送 quit …".to_string(),
                ));
                let _ = self.cmd_tx.lock().unwrap().try_send("quit".to_string());
                self.dirty = true;
            }
        }
    }

    /// 实时补全: 每次输入变化后重算候选, 弹出下拉提示。
    fn refresh_completion(&mut self) {
        if self.input.trim().is_empty() {
            self.comp = CompState::default();
            return;
        }
        let snap = &self.snap;
        let rules: Vec<&str> = snap.rule_ids.iter().map(|s| s.as_str()).collect();
        let tasks: Vec<&str> = snap.task_ids.iter().map(|s| s.as_str()).collect();
        let groups: Vec<&str> = snap.group_ids.iter().map(|s| s.as_str()).collect();
        // 多会话: 提供账号名列表, `@` 目标补全才出现 (`console::completion`
        // 在 profiles.len() < 2 时返回空 —— 单会话不会冒出"发给谁"的概念)
        let profiles: Vec<String> = self
            .sessions
            .as_ref()
            .map(|s| {
                s.profile_names()
                    .into_iter()
                    .map(|x| x.to_string())
                    .collect()
            })
            .unwrap_or_default();
        let ctx = completion::Ctx {
            npcs: &snap.npcs,
            mob_oids: &snap.mob_oids,
            reactor_oids: &snap.reactor_oids,
            rule_ids: &rules,
            task_ids: &tasks,
            group_ids: &groups,
            portals: &snap.portal_data,
            item_ids: &snap.item_ids,
            skill_ids: &snap.skill_ids,
            profiles: &profiles,
        };
        self.comp.candidates = completion::complete_with(&self.input, &ctx);
        self.comp.idx = 0;
    }

    /// 应用当前高亮候选到输入框 (只填 cmd, 描述不进入指令)。
    fn apply_completion(&mut self) {
        if let Some(c) = self.comp.candidates.get(self.comp.idx) {
            self.input = c.cmd.clone();
            self.cursor = char_len(&self.input);
        }
    }

    fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.hist {
            None => {
                self.draft = self.input.clone();
                1
            }
            Some(i) => (i + 1).min(self.history.len()),
        };
        self.hist = Some(next);
        self.input = self.history[self.history.len() - next].clone();
        self.cursor = char_len(&self.input);
        self.comp = CompState::default();
        self.dirty = true;
    }

    fn history_down(&mut self) {
        match self.hist {
            None => {}
            Some(1) => {
                self.input = std::mem::take(&mut self.draft);
                self.hist = None;
            }
            Some(i) => {
                self.hist = Some(i - 1);
                self.input = self.history[self.history.len() - (i - 1)].clone();
            }
        }
        self.cursor = char_len(&self.input);
        self.comp = CompState::default();
        self.dirty = true;
    }

    /// 记入输入历史 (去重相邻项, 上限 200)。`submit` 的各条早退路径都要用。
    fn push_history(&mut self, line: &str) {
        if !line.is_empty() && self.history.last() != Some(&line.to_string()) {
            self.history.push(line.to_string());
            if self.history.len() > 200 {
                self.history.remove(0);
            }
        }
    }

    /// 清空输入框与相关状态 (提交后的统一收尾)。
    fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
        self.hist = None;
        self.comp = CompState::default();
        self.dirty = true;
    }

    /// 把 App 置于「运行中」阶段。
    ///
    /// 测试用: 键位分发按 stage 分支, 停在向导阶段时 F2 / Alt+↑↓ 都不会被处理。
    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn enter_running_for_test(&mut self) {
        self.stage = STAGE_RUNNING;
        self.wizard = None;
        self.stage_flag.store(STAGE_RUNNING, Ordering::SeqCst);
    }

    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn submit_for_test(&mut self) {
        self.submit();
    }

    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn refresh_completion_for_test(&mut self) {
        self.refresh_completion();
    }

    /// 对**当前选中**会话执行一条内建指令 (start / stop)。
    ///
    /// 走的是与 `@<目标> start` 完全相同的代码路径, 只是目标固定为 Selected ——
    /// 两条入口行为必须一致, 否则"按键能起来、指令起不来"这类差异会很难查。
    pub fn run_builtin_on_selected(&mut self, b: crate::sessions::Builtin) -> bool {
        // 增删账号是**集合级**动作, 不需要 runtime handle / 选中会话 ——
        // 先处理掉, 免得被下面那两个 `let Some(..) else` 当成"环境不支持"。
        match &b {
            crate::sessions::Builtin::AddProfile(path) => return self.add_profile(path.clone()),
            crate::sessions::Builtin::RemoveProfile(name) => {
                return self.remove_profile(name.clone())
            }
            crate::sessions::Builtin::ListProfiles => {
                return self.list_profiles();
            }
            crate::sessions::Builtin::Start | crate::sessions::Builtin::Stop => {}
        }
        let Some(ctx) = self.restart.clone() else {
            self.log.push(Event::new(
                Level::Info,
                Category::System,
                format!(
                    "[system] {} 需要控制台接管运行时 (单会话模式用向导重连)",
                    b.describe()
                ),
            ));
            return false;
        };
        let Some(i) = self.session_position().map(|(i, _)| i) else {
            self.log.push(Event::new(
                Level::Info,
                Category::System,
                "[system] 没有会话".to_string(),
            ));
            return false;
        };
        let creds = ctx.creds.lock().ok().map(|g| g.clone());
        let r = match self.sessions.as_mut() {
            Some(set) => match b {
                crate::sessions::Builtin::Start => set.start_session(&ctx.handle, i, creds.as_ref()),
                crate::sessions::Builtin::Stop => set.stop_session(i),
                // 上面已经提前 return 掉了; 这里只为穷尽匹配
                _ => Err("该指令不支持按会话执行".to_string()),
            },
            None => Err("没有会话".to_string()),
        };
        drop(creds);
        // 拉起成功之后 keepalive 由 `drive_restarts` 每帧按 `live_count` 校准,
        // 这里不必手动加减 —— 手动维护两份计数迟早会不一致。
        self.dirty = true;
        self.sync_from_selected();
        match r {
            Ok(msg) => {
                self.log.push(Event::new(
                    Level::Info,
                    Category::System,
                    format!("[system] {msg}"),
                ));
                true
            }
            Err(e) => {
                self.log.push(Event::new(
                    Level::Err,
                    Category::System,
                    format!("[system] {e}"),
                ));
                false
            }
        }
    }

    /// F4 / `S`: 当前会话没在跑就启动它, 在跑就停掉它。
    ///
    /// 一个键同时管启动和停止 —— 手动模式下的常用操作就是这两件, 再分两个键
    /// 只会让用户记更多东西。按键旁的状态点已经说明了当前是哪一种。
    ///
    /// **没有可用密码时弹登录向导** (账号/IP 已从该会话的档案预填, 焦点直接
    /// 落在密码上, 敲完回车就连)。从前只报一句错, 用户得退出程序去改档案或
    /// 重敲命令行 —— 而"启动这个号"本来就是一个明确的意图, 那一刻问密码最自然。
    pub fn toggle_selected_session(&mut self) -> bool {
        let Some(set) = self.sessions.as_ref() else {
            return false;
        };
        let Some(s) = set.selected() else {
            return false;
        };
        if s.is_running() || s.is_waiting_restart() {
            return self.run_builtin_on_selected(crate::sessions::Builtin::Stop);
        }
        // 启动前先看有没有密码 —— 没有就直接问, 不要先失败一次再问。
        if !self.selected_session_has_password() {
            return self.open_wizard_for_selected();
        }
        self.run_builtin_on_selected(crate::sessions::Builtin::Start)
    }

    /// 选中会话是否已有可用密码 (模板里的 or 凭据存储里的)。
    fn selected_session_has_password(&self) -> bool {
        let Some(set) = self.sessions.as_ref() else {
            return false;
        };
        let Some(s) = set.selected() else {
            return false;
        };
        if s.template().map(|c| !c.password.is_empty()).unwrap_or(false) {
            return true;
        }
        let name = s.profile.clone();
        self.restart
            .as_ref()
            .and_then(|ctx| ctx.creds.lock().ok().map(|g| g.password_for(&name).is_some()))
            .unwrap_or(false)
    }

    /// 给**当前选中会话**打开登录向导 (账号/IP 从它自己的档案预填)。
    ///
    /// 复用同一个向导而不是另做一个"只要密码"的小框: 用户已经认识这个界面,
    /// 而且 IP/端口/账号偶尔也会写错 —— 能顺手改掉比只能改密码更有用。
    ///
    /// `wizard_target` 记下这次是给哪个档案开的; 提交后凭据用回那个会话。
    pub fn open_wizard_for_selected(&mut self) -> bool {
        let Some(set) = self.sessions.as_ref() else {
            return false;
        };
        let Some(s) = set.selected() else {
            return false;
        };
        let profile = s.profile.clone();
        let cfg = s.template();
        let (ip, port, account, password, aes_key) = match cfg {
            Some(c) => (
                c.ip.clone(),
                c.port,
                c.account.clone(),
                c.password.clone(),
                String::new(),
            ),
            None => (
                self.login_initial.0.clone(),
                self.login_initial.1,
                self.login_initial.2.clone(),
                String::new(),
                String::new(),
            ),
        };
        self.wizard = Some(Wizard::new(ip, port, account, password, aes_key));
        self.wizard_target = Some(profile);
        self.stage = STAGE_WIZARD;
        self.dirty = true;
        true
    }

    /// 向导提交时: 把凭据用回**它对应的那个会话**并启动它。
    ///
    /// 返回 `true` 表示已经处理完 (主线程不该再去 spawn 全局会话)。
    fn apply_wizard_to_session(&mut self, r: &WizardResult) -> bool {
        let Some(profile) = self.wizard_target.clone() else {
            return false;
        };
        // 凭据记进存储 (自动拉起时从这里取密码)。落盘与否由 `--remember-password`
        // 决定 —— 与启动时问到的密码走同一条策略, 不因为"这次是向导输的"就变。
        if let Some(ctx) = self.restart.clone() {
            if let Ok(mut creds) = ctx.creds.lock() {
                creds.record(&profile, &r.account, &r.password);
            }
        }
        let Some(set) = self.sessions.as_mut() else {
            return false;
        };
        let Some(i) = set.index_of(&profile) else {
            self.log.push(Event::new(
                Level::Err,
                Category::System,
                format!("[system] {profile}: 会话已经不在列表里了"),
            ));
            return true;
        };
        // 向导里的 ip/端口/账号也一并写回模板 —— 用户可能就是来改这些的。
        if let Some(s) = set.iter_mut().nth(i) {
            if let Some(mut cfg) = s.template() {
                cfg.ip = r.ip.clone();
                cfg.port = r.port;
                cfg.account = r.account.clone();
                cfg.password = r.password.clone();
                s.set_template(cfg);
            }
        }
        set.note_at(
            i,
            Level::Info,
            format!(
                "已从向导收下登录信息 (密码 {} 位) —— 正在启动",
                r.password.chars().count()
            ),
        );
        // 回到控制台并启动这个号
        self.stage = STAGE_RUNNING;
        self.wizard = None;
        self.wizard_target = None;
        // 让主线程知道这次向导提交已经处理过了, 别再按"单会话重连"去 spawn
        self.login_apply.store(true, Ordering::SeqCst);
        self.dirty = true;
        self.run_builtin_on_selected(crate::sessions::Builtin::Start);
        true
    }

    /// 运行时挂上一个新账号 (`profiles add <路径>`)。
    ///
    /// 新会话停在「未启动」: 加一个账号不该顺手把它登上去。挂上之后
    /// `show_sidebar` 要重算 —— 从 1 个变成 2 个时左栏才第一次出现,
    /// 不重算的话用户会觉得"加了但界面没变"。
    pub fn add_profile(&mut self, path: std::path::PathBuf) -> bool {
        let Some(set) = self.sessions.as_mut() else {
            self.log.push(Event::new(
                Level::Info,
                Category::System,
                "[system] 单会话模式不支持运行时加账号 (用 --profiles 或选择屏勾选多个)"
                    .to_string(),
            ));
            return false;
        };
        if path.as_os_str().is_empty() {
            self.log.push(Event::new(
                Level::Err,
                Category::System,
                "[system] 用法: profiles add <档案路径>".to_string(),
            ));
            return false;
        }
        match set.add_profile(path) {
            Ok(_) => {
                self.show_sidebar = set.len() > 1;
                self.dirty = true;
                let n = set.len();
                self.log.push(Event::new(
                    Level::Info,
                    Category::System,
                    format!("[system] 已挂上账号 (共 {n} 个) — 它是「未启动」的, 按 F4 启动"),
                ));
                true
            }
            Err(e) => {
                self.log
                    .push(Event::new(Level::Err, Category::System, format!("[system] {e}")));
                false
            }
        }
    }

    /// 运行时摘掉一个账号 (`profiles remove <档案名>`)。
    pub fn remove_profile(&mut self, name: String) -> bool {
        let Some(set) = self.sessions.as_mut() else {
            self.log.push(Event::new(
                Level::Info,
                Category::System,
                "[system] 单会话模式没有可摘的账号".to_string(),
            ));
            return false;
        };
        if name.is_empty() {
            self.log.push(Event::new(
                Level::Err,
                Category::System,
                "[system] 用法: profiles remove <档案名> (名单见 profiles list)".to_string(),
            ));
            return false;
        }
        match set.remove_profile(&name) {
            Ok((idx, profile)) => {
                // 下标全体前移了 → 日志缓存视图的"上次同步的是谁"必须作废。
                //
                // 不作废的话 `synced` 还指着旧下标, 而那个下标现在代表**另一个
                // 账号** —— 界面会把 B 的日志当成 A 的继续显示 (最难查的一类错)。
                //
                // 但作废之前必须先把渲染位置的那份日志**还回它所属的会话**:
                // `self.log` 里装的是 `synced` 那个会话的日志 (选中会话的缓冲在
                // 渲染位置), 直接丢掉它等于把那个号的日志抹了; 直接留给下一个
                // 选中项则会让两个账号串台。
                if let Some(prev) = self.synced {
                    if let Some(s) = set.iter_mut().nth(prev) {
                        s.log.swap(&mut self.log);
                    }
                }
                self.synced = None;
                // 被摘掉的若是当前选中项 (或它后面已经空了) → 选到最后一个
                if set.selected_index() >= set.len() && set.len() > 0 {
                    let last = set.len() - 1;
                    set.select(last);
                }
                let (sel, total) = (set.selected_index(), set.len());
                self.show_sidebar = total > 1;
                self.dirty = true;
                self.log.push(Event::new(
                    Level::Info,
                    Category::System,
                    format!(
                        "[system] 已摘掉 {profile} (原第 {} 个, 剩 {total} 个, 当前选第 {} 个)",
                        idx + 1,
                        sel + 1
                    ),
                ));
                self.sync_from_selected();
                true
            }
            Err(e) => {
                self.log
                    .push(Event::new(Level::Err, Category::System, format!("[system] {e}")));
                false
            }
        }
    }

    /// 列出已挂上的账号与状态 (`profiles list`)。
    pub fn list_profiles(&mut self) -> bool {
        let text = match self.sessions.as_ref() {
            Some(set) if !set.is_empty() => {
                let sel = set.selected_index();
                let mut s = format!("[system] 已挂 {} 个账号 (▶ = 当前选中):", set.len());
                for (i, line) in set.status_lines().iter().enumerate() {
                    s.push('\n');
                    s.push_str(if i == sel { "▶" } else { " " });
                    s.push_str(line);
                }
                s
            }
            _ => "[system] 还没有挂上任何账号 (profiles add <路径>)".to_string(),
        };
        self.log.push(Event::new(Level::Info, Category::System, text));
        self.dirty = true;
        true
    }

    fn submit(&mut self) {
        let line = self.input.trim().to_string();
        // 多会话: `@all <cmd>` / `@<档案前缀> <cmd>` 批量下发。
        //
        // 只认首 token 的 `@` (聊天内容里的 @某人 不受影响)。目标无命中时
        // **报错而不是静默**: 否则用户会以为自己敲的指令生效了。
        if self.sessions.is_some() && line.starts_with('@') {
            self.push_history(&line);
            let text = match crate::sessions::split_target(&line) {
                Ok((_t, cmd)) if cmd.is_empty() => "目标之后没有指令内容".to_string(),
                Ok((target, cmd)) => {
                    // `start` / `stop` / `profiles ...` 是控制台自己执行的内建
                    // 指令, **不发给 bot** (bot 不认识这些词, 发过去只会回一句
                    // "未知指令")。
                    if let Some(b) = crate::sessions::SessionSet::builtin_for_target(&cmd) {
                        // 集合级动作 (`profiles add/remove/list`) 不需要 runtime
                        // handle —— 它们动的是"有哪些账号", 不启动任何东西。
                        if !b.is_per_session() {
                            self.run_builtin_on_selected(b);
                            self.clear_input();
                            return;
                        }
                        let Some(ctx) = self.restart.clone() else {
                            self.log.push(Event::new(
                                Level::Info,
                                Category::System,
                                format!(
                                    "[system] {} 需要控制台接管运行时 (单会话模式不支持)",
                                    b.describe()
                                ),
                            ));
                            self.clear_input();
                            return;
                        };
                        let creds = ctx.creds.lock().ok().map(|g| g.clone());
                        let what = b.describe();
                        let (hit, ok, errs) = match self.sessions.as_mut() {
                            Some(set) => {
                                set.dispatch_builtin(&ctx.handle, &target, b, creds.as_ref())
                            }
                            None => (0, 0, Vec::new()),
                        };
                        drop(creds);
                        let mut s = format!(
                            "[system] {} {} → 命中 {hit}, 成功 {ok}",
                            target.describe(),
                            what
                        );
                        for (name, e) in &errs {
                            s.push_str(&format!("\n  {name}: {e}"));
                        }
                        self.log.push(Event::new(Level::Info, Category::System, s));
                        self.clear_input();
                        return;
                    }
                    let (hit, ok, errs) = if target == crate::sessions::Target::Selected {
                        match self.cmd_tx.lock() {
                            Ok(tx) => match tx.try_send(cmd.clone()) {
                                Ok(()) => (1usize, 1usize, Vec::new()),
                                Err(e) => (1, 0, vec![("当前会话".to_string(), e.to_string())]),
                            },
                            Err(_) => (
                                1,
                                0,
                                vec![("当前会话".to_string(), "cmd slot poisoned".into())],
                            ),
                        }
                    } else {
                        match self.sessions.as_ref() {
                            Some(set) => set.dispatch(&target, &cmd),
                            None => (0, 0, Vec::new()),
                        }
                    };
                    let mut s = format!("[system] {} → 命中 {hit}, 送达 {ok}", target.describe());
                    for (name, e) in &errs {
                        s.push_str(&format!("\n  {name}: {e}"));
                    }
                    s
                }
                Err(e) => format!("[system] {e}"),
            };
            self.log
                .push(Event::new(Level::Info, Category::System, text));
            self.clear_input();
            return;
        }
        if !line.is_empty() {
            if self.history.last() != Some(&line) {
                self.history.push(line.clone());
                if self.history.len() > 200 {
                    self.history.remove(0);
                }
            }
            // start / stop: 控制台自己执行 (启动/停止当前选中会话), 不发给 bot。
            // 带 `@` 的形式在上一段处理; 这里管裸敲的那一种。
            if let Some(b) = crate::sessions::SessionSet::builtin_for_target(&line) {
                match b {
                    // 裸敲 `start` 必须与 F4 **完全等价**, 包括"号没密码就弹向导
                    // 补输"那一步。直接走 `run_builtin_on_selected` 会绕开
                    // `toggle_selected_session` 里的那个分支 —— 同一个动作按键能
                    // 补密码、敲指令却只会报一句错并把号打成「启动失败 — 需人工」,
                    // 用户于是卡在一个"想输密码但找不到入口"的状态里。
                    //
                    // `stop` 不适用: 停止一个号不该弹向导。
                    crate::sessions::Builtin::Start if self.sessions.is_some() => {
                        self.toggle_selected_session();
                    }
                    // 单会话模式 (没有会话集合) 仍走原路, 由它给出
                    // "用向导重连"的说明 —— 静默什么都不做是最差的选择。
                    other => {
                        self.run_builtin_on_selected(other);
                    }
                }
                self.clear_input();
                return;
            }
            // clear/cls: 本地清空日志, 不发给 bot (bot 端无此命令)
            if line == "clear" || line == "cls" {
                self.log.clear();
                self.log.push(Event::new(
                    Level::Info,
                    Category::System,
                    "[system] log cleared".to_string(),
                ));
                self.input.clear();
                self.cursor = 0;
                self.hist = None;
                self.comp = CompState::default();
                self.scroll = 0;
                self.dirty = true;
                return;
            }
            // bag: 本地背包界面, 不进 bot 命令
            if line == "bag" {
                self.bag_open = !self.bag_open;
                self.bag_scroll = 0;
                self.bag_panel = 0;
                self.help_open = false;
                self.input.clear();
                self.cursor = 0;
                self.hist = None;
                self.comp = CompState::default();
                self.dirty = true;
                return;
            }
            // npmenu: NPC 选项弹框开关 (本地界面, 不进 bot 命令)。
            // 默认关闭 (选项进日志, 手动 npc reply); 开启后服务器回菜单自动弹框。
            if line == "npmenu" || line.starts_with("npmenu ") {
                let arg = line.trim_start_matches("npmenu").trim();
                let on = match arg {
                    "" | "on" | "1" | "open" => true,
                    "off" | "0" | "close" => false,
                    other => {
                        self.log.push(Event::new(
                            Level::Info,
                            Category::System,
                            format!("[npmenu] bad arg: {other} (on|off)"),
                        ));
                        self.input.clear();
                        self.cursor = 0;
                        self.dirty = true;
                        return;
                    }
                };
                self.npmenu = on;
                if !on {
                    self.npc_pick_open = false;
                }
                self.log.push(Event::new(
                    Level::Info,
                    Category::System,
                    format!("[npmenu] {} (NPC 选项弹框)", if on { "开" } else { "关" }),
                ));
                self.input.clear();
                self.cursor = 0;
                self.hist = None;
                self.comp = CompState::default();
                self.dirty = true;
                return;
            }
            // shop menu: 商店购买弹框开关 (本地界面, 不进 bot 命令)。
            // `shop` (无参) 仍是商店离开命令, 发给 bot。
            if line.starts_with("shop menu") {
                let arg = line.trim_start_matches("shop menu").trim();
                let on = match arg {
                    "" | "on" | "1" | "open" => true,
                    "off" | "0" | "close" => false,
                    other => {
                        self.log.push(Event::new(
                            Level::Info,
                            Category::System,
                            format!("[shop] bad menu arg: {other} (on|off)"),
                        ));
                        self.input.clear();
                        self.cursor = 0;
                        self.dirty = true;
                        return;
                    }
                };
                self.shop_menu = on;
                if !on {
                    self.shop_pick_open = false;
                }
                self.log.push(Event::new(
                    Level::Info,
                    Category::System,
                    format!(
                        "[shop] menu {} (商店购买弹框)",
                        if on { "开" } else { "关" }
                    ),
                ));
                self.input.clear();
                self.cursor = 0;
                self.hist = None;
                self.comp = CompState::default();
                self.dirty = true;
                return;
            }
            self.log
                .push(Event::new(Level::Info, Category::Cmd, format!("> {line}")));
            let _ = self.cmd_tx.lock().unwrap().try_send(line);
        }
        self.input.clear();
        self.cursor = 0;
        self.hist = None;
        self.comp = CompState::default();
        self.scroll = 0;
        self.dirty = true;
    }

    /// D 键: 丢弃光标选中槽位的物品 (装备/消耗/其他均可丢)。
    fn bag_drop(&mut self) {
        let items = self.bag_items();
        let Some(&(_, slot, itemid, qty)) = items.get(self.bag_cursor) else {
            return;
        };
        let cmd = format!("drop {itemid} {} slot {slot}", qty.max(1));
        let _ = self.cmd_tx.lock().unwrap().try_send(cmd.clone());
        self.log
            .push(Event::new(Level::Info, Category::Cmd, format!("> {cmd}")));
    }

    /// E 键: 穿光标选中装备 (equip <itemid>, 仅装备栏物品)。
    fn bag_equip(&mut self) {
        let items = self.bag_items();
        let Some(&(tab, _, itemid, _)) = items.get(self.bag_cursor) else {
            return;
        };
        if tab != 1 {
            self.log.push(Event::new(
                Level::Info,
                Category::System,
                "[bag] 仅装备栏物品可穿 (切换到 装备 面板)".to_string(),
            ));
            return;
        }
        let cmd = format!("equip {itemid}");
        let _ = self.cmd_tx.lock().unwrap().try_send(cmd.clone());
        self.log
            .push(Event::new(Level::Info, Category::Cmd, format!("> {cmd}")));
    }

    /// 当前面板过滤后的物品列表 (Copy 元组, 避免借用冲突)。
    fn bag_items(&self) -> Vec<(u8, i16, i32, i16)> {
        let panel = self.bag_panel.min(5);
        self.snap
            .inventory
            .iter()
            .filter(|(tab, _, _, _)| panel == 0 || *tab == panel as u8)
            .copied()
            .collect()
    }

    fn clamp_bag_cursor(&mut self) {
        let n = self.bag_items().len();
        if self.bag_cursor >= n {
            self.bag_cursor = n.saturating_sub(1);
        }
    }

    /// 光标下移时保持可见: 两列布局一行 2 个物品, 光标行 = 2 头部 + cursor/2。
    fn follow_bag_cursor(&mut self) {
        let vis = 20usize; // 保守值, 渲染时会按实际高度重新钳制
        let row = 2 + self.bag_cursor / 2;
        if row >= self.bag_scroll + vis {
            self.bag_scroll = row.saturating_sub(vis) + 1;
        }
        if row < self.bag_scroll {
            self.bag_scroll = row;
        }
    }

    /// Enter: 按当前贩卖模式动作。
    /// 模式 0 "卖出选中": 只卖光标选中槽位的物品 (sell <id> <qty> slot <n>)。
    /// 模式 1 "卖出所有": 卖光背包所有槽位的该物品 (sell <id> 0)。
    fn bag_enter(&mut self) {
        let items = self.bag_items();
        if !self.snap.shop_open {
            self.log.push(Event::new(
                Level::Info,
                Category::System,
                "[bag] 商店未打开, 无法卖出 (先 npc 打开商店)".to_string(),
            ));
            return;
        }
        let Some(&(_, slot, itemid, qty)) = items.get(self.bag_cursor) else {
            return;
        };
        let cmd: String = if self.bag_mode == 1 {
            // 卖出这类: 所有槽位的该物品
            format!("sell {itemid} 0")
        } else {
            // 卖出当前: 只卖选中槽位 (装备 qty=0 时按 1 卖)
            format!("sell {itemid} {} slot {slot}", qty.max(1))
        };
        let _ = self.cmd_tx.lock().unwrap().try_send(cmd.clone());
        self.log
            .push(Event::new(Level::Info, Category::Cmd, format!("> {cmd}")));
    }

    /// NPC 选项弹框: 上下移动光标 (循环)。
    /// 滚动跟随由渲染层负责 (渲染知道弹框实际高度, 保证光标可见);
    /// 这里只移动光标, 避免移动/渲染两套滚动逻辑脱节 (硬编码阈值导致
    /// 选到末尾时窗口提前滚动、弹框下半空白)。
    fn npc_pick_move(&mut self, d: i32) {
        let n = self.snap.npc_options.len();
        if n == 0 {
            return;
        }
        self.npc_pick_cursor = ((self.npc_pick_cursor as i32 + d).rem_euclid(n as i32)) as usize;
    }

    /// NPC 选项弹框: Enter 确认 — 发 `npc reply <id>` 并关闭弹框。
    /// seen_seq 标记当前菜单已看过, 下次服务器再回菜单才重新弹出。
    fn npc_pick_confirm(&mut self) {
        let Some(&(id, _)) = self.snap.npc_options.get(self.npc_pick_cursor) else {
            return;
        };
        let cmd = format!("npc reply {id}");
        self.npc_seen_seq = self.snap.npc_options_seq;
        self.npc_pick_open = false;
        self.log
            .push(Event::new(Level::Info, Category::Cmd, format!("> {cmd}")));
        let _ = self.cmd_tx.lock().unwrap().try_send(cmd);
    }

    /// 商店购买弹框: 上下移动光标 (循环)。滚动由渲染层跟随。
    fn shop_pick_move(&mut self, d: i32) {
        let n = self.snap.shop_items.len();
        if n == 0 {
            return;
        }
        self.shop_pick_cursor = ((self.shop_pick_cursor as i32 + d).rem_euclid(n as i32)) as usize;
    }

    /// 商店购买弹框: Enter 购买 — 发 `buy <itemid> <qty>` (无数量 = 1)。
    /// 购买后**保持数量** (方便持续购买同类商品; Esc 可清除)。
    fn shop_pick_confirm(&mut self) {
        let Some(&(itemid, _, _)) = self.snap.shop_items.get(self.shop_pick_cursor) else {
            return;
        };
        let qty: i32 = self.shop_qty.parse().unwrap_or(1).clamp(1, 9999);
        let cmd = format!("buy {itemid} {qty}");
        self.log
            .push(Event::new(Level::Info, Category::Cmd, format!("> {cmd}")));
        let _ = self.cmd_tx.lock().unwrap().try_send(cmd);
    }
}

/// 文本度量与编辑统一走 `openstory_console_lib::text` —— 与日志折行、补全弹窗的
/// 宽度计算共用同一份实现, 避免"输入框按字符、渲染按列"两套算法漂移。
pub use openstory_console_lib::text::{char_len, display_width, insert_at, remove_at};

#[cfg(test)]
mod tests {
    use super::*;
    use openstory_console_lib::snapshot::Snapshot;

    #[test]
    fn insert_remove_cjk() {
        let mut s = String::from("你好");
        insert_at(&mut s, 1, '中');
        assert_eq!(s, "你中好");
        remove_at(&mut s, 1);
        assert_eq!(s, "你好");
        insert_at(&mut s, 99, '!');
        assert_eq!(s, "你好!");
    }

    fn test_app() -> (App, tokio::sync::mpsc::Receiver<String>) {
        let queue = Arc::new(EventQueue::new(16));
        let state = Arc::new(tokio::sync::Mutex::new(
            openstory_bot::state::BotState::default(),
        ));
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let tx = Arc::new(StdMutex::new(tx));
        let stage = Arc::new(AtomicU8::new(STAGE_RUNNING));
        let outcome = Arc::new(StdMutex::new(None));
        let app = App::new(
            queue,
            state,
            tx,
            None,
            stage,
            outcome,
            Arc::new(AtomicBool::new(false)),
            false,
            "127.0.0.1".into(),
            8484,
            String::new(),
            String::new(),
            String::new(),
        );
        (app, rx)
    }

    #[test]
    fn bag_drop_discards_selected_slot() {
        let (mut app, mut rx) = test_app();
        app.snap = Snapshot {
            inventory: vec![(1, 1, 1302000, 0), (2, 3, 2000001, 25), (2, 5, 2000001, 10)],
            ..Default::default()
        };
        // 光标 0 = 装备槽位1 → 也可丢弃
        app.bag_drop();
        assert_eq!(rx.try_recv().unwrap(), "drop 1302000 1 slot 1");
        // 光标 1 = 槽位3 qty=25 → 丢该槽位全部
        app.bag_cursor = 1;
        app.bag_drop();
        assert_eq!(rx.try_recv().unwrap(), "drop 2000001 25 slot 3");
        // 光标 2 = 同物品另一槽位 → 丢的是选中槽位
        app.bag_cursor = 2;
        app.bag_drop();
        assert_eq!(rx.try_recv().unwrap(), "drop 2000001 10 slot 5");
    }

    #[test]
    fn bag_equip_wears_selected_equip() {
        let (mut app, mut rx) = test_app();
        app.snap = Snapshot {
            inventory: vec![(1, 1, 1302000, 0), (2, 3, 2000001, 25)],
            ..Default::default()
        };
        // 光标 0 = 装备 → equip <itemid>
        app.bag_equip();
        assert_eq!(rx.try_recv().unwrap(), "equip 1302000");
        // 光标 1 = 消耗品 → 提示, 不发命令
        app.bag_cursor = 1;
        app.bag_equip();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn bag_enter_sells_selected_slot_or_all_slots() {
        let (mut app, mut rx) = test_app();
        app.snap = Snapshot {
            inventory: vec![(1, 1, 1302000, 0), (2, 3, 2000001, 25), (2, 5, 2000001, 10)],
            shop_open: true,
            ..Default::default()
        };
        // 卖出当前: 光标 0 = 槽位1 装备 → 只卖该槽位
        app.bag_enter();
        assert_eq!(rx.try_recv().unwrap(), "sell 1302000 1 slot 1");
        // 卖出当前: 光标 1 = 槽位3 qty=25
        app.bag_cursor = 1;
        app.bag_enter();
        assert_eq!(rx.try_recv().unwrap(), "sell 2000001 25 slot 3");
        // 卖出这类: 光标仍在 2000001 → 卖光所有槽位
        app.bag_mode = 1;
        app.bag_enter();
        assert_eq!(rx.try_recv().unwrap(), "sell 2000001 0");
        // 商店未打开: 不发命令
        app.snap.shop_open = false;
        app.bag_enter();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn filter_cycles() {
        assert_eq!(Filter::All.next(), Filter::Cat(Category::Chat));
        // 最后一个页签是「状态」(诊断页), 循环回「全部」
        assert_eq!(Filter::Status.next(), Filter::All);
        assert_eq!(Filter::prev(Filter::Status), Filter::Cat(Category::Error));
    }

    #[test]
    fn display_width_cjk() {
        assert_eq!(display_width("ab"), 2);
        assert_eq!(display_width("你好"), 4);
    }

    // ---- NPC 选项弹框 (npmenu) ----

    fn key(kc: KeyCode) -> KeyEvent {
        KeyEvent::new(kc, KeyModifiers::NONE)
    }

    #[test]
    fn npmenu_default_on_and_toggle_off_blocks_popup() {
        let (mut app, rx) = test_app();
        // 默认开启
        assert!(app.npmenu);
        // 开关开时: 新菜单 seq 变化 → 弹框
        app.snap = Snapshot {
            npc_options: vec![(1, "A".into()), (2, "B".into())],
            npc_options_seq: 7,
            ..Default::default()
        };
        app.auto_npc_pick();
        assert!(app.npc_pick_open);
        // 关 (本地命令, 不进 bot 通道)
        app.input.push_str("npmenu off");
        app.submit();
        assert!(!app.npmenu);
        // 开关关时: 新菜单 seq 变化也不弹
        app.snap = Snapshot {
            npc_options: vec![(3, "C".into())],
            npc_options_seq: 8,
            ..Default::default()
        };
        app.auto_npc_pick();
        assert!(!app.npc_pick_open);
        drop(rx);
    }

    #[test]
    fn automation_suppresses_npc_and_shop_popups() {
        let (mut app, _rx) = test_app();
        app.npmenu = true;
        app.shop_menu = true;
        // 任务持锁: NPC 菜单/商店清单到达都不弹
        app.snap = Snapshot {
            task_stack: vec![("sell_chain".into(), 1, 4)],
            npc_options: vec![(1, "A".into())],
            npc_options_seq: 5,
            shop_items: vec![(2000000, 50, 100)],
            shop_seq: 3,
            ..Default::default()
        };
        app.auto_npc_pick();
        app.auto_shop_pick();
        assert!(!app.npc_pick_open);
        assert!(!app.shop_pick_open);
        // seen_seq 未更新 — 记下待评估的版本号
        assert_eq!(app.npc_seen_seq, 0);
        assert_eq!(app.shop_seen_seq, 0);
        // 已开着的弹框在任务启动瞬间被强制本地收起 (数量一并清空)
        app.npc_pick_open = true;
        app.shop_pick_open = true;
        app.shop_qty.push('5');
        app.auto_npc_pick();
        app.auto_shop_pick();
        assert!(!app.npc_pick_open);
        assert!(!app.shop_pick_open);
        assert!(app.shop_qty.is_empty());
    }

    #[test]
    fn after_automation_shop_popup_shows_only_if_shop_still_open() {
        let (mut app, _rx) = test_app();
        app.npmenu = true;
        app.shop_menu = true;
        // 场景一 纯开店任务: 跑完栈空, 商店还开着 → shop 弹框出现,
        // 残留 NPC 菜单已被核心清空 (handle_open_npc_shop) → npc 不弹
        app.snap = Snapshot {
            task_stack: Vec::new(),
            shop_items: vec![(2000000, 50, 100)],
            shop_seq: 3,
            shop_open: true,
            npc_options: Vec::new(),
            npc_options_seq: 9,
            ..Default::default()
        };
        app.auto_npc_pick();
        app.auto_shop_pick();
        assert!(app.shop_pick_open, "开店任务结束后商店弹框应正常出现");
        assert!(!app.npc_pick_open, "NPC 菜单已清空不应误弹");
        // 场景二 卖装链: shop leave 清空清单后栈才空 → 什么都不弹
        let (mut app2, _rx2) = test_app();
        app2.npmenu = true;
        app2.shop_menu = true;
        app2.snap = Snapshot {
            task_stack: Vec::new(),
            shop_items: Vec::new(),
            shop_seq: 4,
            shop_open: false,
            npc_options: Vec::new(),
            npc_options_seq: 10,
            ..Default::default()
        };
        app2.auto_npc_pick();
        app2.auto_shop_pick();
        assert!(!app2.shop_pick_open);
        assert!(!app2.npc_pick_open);
    }

    #[test]
    fn npc_pick_opens_on_menu_and_closes_on_plain_dialog() {
        let (mut app, _rx) = test_app();
        app.npmenu = true;
        // 服务器回菜单 (seq 变化 + 有选项) → 弹框
        app.snap = Snapshot {
            npc_options: vec![(203, "远征副本".into()), (204, "金币地图".into())],
            npc_options_seq: 3,
            ..Default::default()
        };
        app.auto_npc_pick();
        assert!(app.npc_pick_open);
        assert_eq!(app.npc_pick_cursor, 0);
        assert_eq!(app.npc_seen_seq, 3);
        // 同 seq 不再重复弹 (Enter 后)
        app.npc_pick_open = false;
        app.auto_npc_pick();
        assert!(!app.npc_pick_open);
        // 服务器回无选项对话 (seq 再变, options 空) → 不弹
        app.snap = Snapshot {
            npc_options: vec![],
            npc_options_seq: 4,
            ..Default::default()
        };
        app.auto_npc_pick();
        assert!(!app.npc_pick_open);
        assert_eq!(app.npc_seen_seq, 4);
        // 再回新菜单 → 再弹 (光标归零)
        app.snap = Snapshot {
            npc_options: vec![(1, "X".into())],
            npc_options_seq: 5,
            ..Default::default()
        };
        app.auto_npc_pick();
        assert!(app.npc_pick_open);
        assert_eq!(app.npc_pick_cursor, 0);
    }

    #[test]
    fn npc_pick_enter_sends_reply_and_closes() {
        let (mut app, mut rx) = test_app();
        app.npmenu = true;
        app.snap = Snapshot {
            npc_options: vec![
                (203, "远征副本".into()),
                (204, "金币地图".into()),
                (205, "匠人街".into()),
            ],
            npc_options_seq: 9,
            ..Default::default()
        };
        app.auto_npc_pick();
        assert!(app.npc_pick_open);
        // 光标 0 → Enter 发 npc reply 203
        app.npc_pick_confirm();
        assert_eq!(rx.try_recv().unwrap(), "npc reply 203");
        assert!(!app.npc_pick_open);
        assert_eq!(app.npc_seen_seq, 9, "确认后标记已看, 同菜单不再弹");
        // 光标移动: 上越界循环到尾
        app.auto_npc_pick(); // 同 seq, 不应重开
        assert!(!app.npc_pick_open);
    }

    #[test]
    fn npc_pick_esc_closes_without_sending() {
        let (mut app, mut rx) = test_app();
        app.npmenu = true;
        app.snap = Snapshot {
            npc_options: vec![(203, "远征副本".into())],
            npc_options_seq: 2,
            ..Default::default()
        };
        app.auto_npc_pick();
        app.on_key(key(KeyCode::Esc));
        assert!(!app.npc_pick_open);
        assert!(rx.try_recv().is_err(), "Esc 不发命令");
    }

    #[test]
    fn npc_pick_arrows_cycle() {
        let (mut app, _rx) = test_app();
        app.npmenu = true;
        app.snap = Snapshot {
            npc_options: vec![
                (203, "远征副本".into()),
                (204, "金币地图".into()),
                (205, "匠人街".into()),
            ],
            npc_options_seq: 6,
            ..Default::default()
        };
        app.auto_npc_pick();
        // 下 → 1
        app.npc_pick_move(1);
        assert_eq!(app.npc_pick_cursor, 1);
        // 下 → 2, 下 → 0 (循环)
        app.npc_pick_move(1);
        assert_eq!(app.npc_pick_cursor, 2);
        app.npc_pick_move(1);
        assert_eq!(app.npc_pick_cursor, 0);
        // 上 → 2 (循环)
        app.npc_pick_move(-1);
        assert_eq!(app.npc_pick_cursor, 2);
        // 按键入口: 弹框打开时其他键被拦截 (输入 'x' 不进入输入框)
        app.input.push('z');
        app.on_key(key(KeyCode::Char('x')));
        assert_eq!(app.input, "z", "弹框打开时按键完全拦截");
    }

    // ---- 商店购买弹框 (shop menu) ----

    #[test]
    fn shop_menu_default_on_and_toggle_off_blocks_popup() {
        let (mut app, rx) = test_app();
        // 默认开启
        assert!(app.shop_menu);
        // 开时: 新清单 seq 变化 → 弹框
        app.snap = Snapshot {
            shop_items: vec![(2000001, 50, 1000)],
            shop_seq: 3,
            ..Default::default()
        };
        app.auto_shop_pick();
        assert!(app.shop_pick_open);
        // 关 (本地命令)
        app.input.push_str("shop menu off");
        app.submit();
        assert!(!app.shop_menu);
        // 关时: 新清单 seq 变化也不弹
        app.snap = Snapshot {
            shop_items: vec![(2000002, 60, 1000)],
            shop_seq: 4,
            ..Default::default()
        };
        app.auto_shop_pick();
        assert!(!app.shop_pick_open);
        drop(rx);
    }

    #[test]
    fn shop_pick_opens_on_list_and_closes_when_cleared() {
        let (mut app, _rx) = test_app();
        app.shop_menu = true;
        // 服务器开商店 (seq 变化 + 有商品) → 弹框
        app.snap = Snapshot {
            shop_items: vec![(2000001, 50, 1000), (2000002, 60, 1000)],
            shop_seq: 1,
            ..Default::default()
        };
        app.auto_shop_pick();
        assert!(app.shop_pick_open);
        assert_eq!(app.shop_pick_cursor, 0);
        assert_eq!(app.shop_seen_seq, 1);
        // 同 seq 不重复弹
        app.shop_pick_open = false;
        app.auto_shop_pick();
        assert!(!app.shop_pick_open);
        // 服务器发新清单 (seq 再变) → 弹框 (光标归零)
        app.snap = Snapshot {
            shop_items: vec![(2000003, 70, 500)],
            shop_seq: 2,
            ..Default::default()
        };
        app.auto_shop_pick();
        assert!(app.shop_pick_open);
        assert_eq!(app.shop_pick_cursor, 0);
        // 商店关闭 (seq 变化 + items 空) → 收起并清数量
        app.snap = Snapshot {
            shop_items: vec![],
            shop_seq: 3,
            ..Default::default()
        };
        app.auto_shop_pick();
        assert!(!app.shop_pick_open);
        assert!(app.shop_qty.is_empty());
    }

    #[test]
    fn shop_pick_enter_buys_with_qty_and_keeps_qty() {
        let (mut app, mut rx) = test_app();
        app.shop_menu = true;
        app.snap = Snapshot {
            shop_items: vec![(2000001, 50, 1000), (2000002, 60, 1000)],
            shop_seq: 5,
            ..Default::default()
        };
        app.auto_shop_pick();
        // 无数量 → 默认 1
        app.shop_pick_confirm();
        assert_eq!(rx.try_recv().unwrap(), "buy 2000001 1");
        // 输入数量 50 → Enter 买 50
        app.shop_qty.push('5');
        app.shop_qty.push('0');
        app.shop_pick_confirm();
        assert_eq!(rx.try_recv().unwrap(), "buy 2000001 50");
        // 数量保持 (方便持续购买), 弹框保持打开
        assert_eq!(app.shop_qty, "50", "购买后数量保持");
        assert!(app.shop_pick_open);
        // 再买一次用同样的数量
        app.shop_pick_cursor = 1;
        app.shop_pick_confirm();
        assert_eq!(rx.try_recv().unwrap(), "buy 2000002 50");
    }

    #[test]
    fn shop_pick_keys_digits_backspace_esc() {
        let (mut app, rx) = test_app();
        app.shop_menu = true;
        app.snap = Snapshot {
            shop_items: vec![(2000001, 50, 1000)],
            shop_seq: 6,
            ..Default::default()
        };
        app.auto_shop_pick();
        // 数字键追加
        app.on_key(key(KeyCode::Char('1')));
        app.on_key(key(KeyCode::Char('2')));
        assert_eq!(app.shop_qty, "12");
        // Backspace 删除
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.shop_qty, "1");
        // 非数字键被拦截 (不进入输入框) — 弹框还开着
        app.input.push('z');
        app.on_key(key(KeyCode::Char('a')));
        assert_eq!(app.input, "z");
        // 数量上限 5 位
        app.on_key(key(KeyCode::Char('1')));
        app.on_key(key(KeyCode::Char('2')));
        app.on_key(key(KeyCode::Char('3')));
        app.on_key(key(KeyCode::Char('4')));
        app.on_key(key(KeyCode::Char('5')));
        app.on_key(key(KeyCode::Char('6')));
        assert_eq!(app.shop_qty.len(), 5);
        // Esc: 有数量先清数量, 弹框不关
        app.on_key(key(KeyCode::Esc));
        assert!(app.shop_pick_open);
        assert!(app.shop_qty.is_empty());
        // Esc: 无数量收起
        app.on_key(key(KeyCode::Esc));
        assert!(!app.shop_pick_open);
        drop(rx);
    }

    #[test]
    fn shop_pick_arrows_cycle() {
        let (mut app, _rx) = test_app();
        app.shop_menu = true;
        app.snap = Snapshot {
            shop_items: vec![(2000001, 50, 1000), (2000002, 60, 1000), (2000003, 70, 500)],
            shop_seq: 7,
            ..Default::default()
        };
        app.auto_shop_pick();
        app.shop_pick_move(1);
        assert_eq!(app.shop_pick_cursor, 1);
        app.shop_pick_move(1);
        assert_eq!(app.shop_pick_cursor, 2);
        app.shop_pick_move(1);
        assert_eq!(app.shop_pick_cursor, 0, "循环");
        app.shop_pick_move(-1);
        assert_eq!(app.shop_pick_cursor, 2, "上循环");
    }

    #[test]
    fn reconnect_back_to_wizard_clears_popups() {
        // 断线 → 主线程把 stage 置回 WIZARD (重开向导): NPC/商店弹框与数量
        // 状态必须清空, 不残留到重连后的新会话。
        let (mut app, _rx) = test_app();
        app.npc_pick_open = true;
        app.shop_pick_open = true;
        app.shop_qty.push('5');
        app.stage_flag.store(STAGE_WIZARD, Ordering::SeqCst);
        app.poll_events();
        assert_eq!(app.stage, STAGE_WIZARD);
        assert!(!app.npc_pick_open, "重连向导清 NPC 弹框");
        assert!(!app.shop_pick_open, "重连向导清商店弹框");
        assert!(app.shop_qty.is_empty(), "重连向导清数量");
    }
}
