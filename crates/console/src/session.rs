//! 单个 bot 会话: 一个档案 + 一份实时状态 + 一条指令通道。
//!
//! 这是 UI 与 bot 之间的**唯一接口**。把"一个账号跑起来"所需的全部东西
//! 收在一个值里, 上层就不再需要知道 `runtime::run` 的参数、emitter 的注册
//! 方式、命令通道何时重建等细节。
//!
//! # 为远期服务器版留的两个约束 (零成本, 现在就要守住)
//!
//! 1. **输入只走 `mpsc::Receiver<String>`** —— 不去直接改别的会话的内存。
//!    远端实现只需把"从输入框收指令"换成"从 WebSocket 收指令"。
//! 2. **构造可以从一条通道开始** ([`BotSession::from_channel`]) ——
//!    不依赖同进程的其它会话、不依赖全局单例。这样"会话"这个概念在远端
//!    进程里同样成立。
//!
//! 破坏这两条 (跨会话直接读内存 / 全局单例 / 会话间共享可变静态) 会让服务器版
//! 从"加一个前端"变成"重写引擎"。

#![allow(dead_code)] // 阶段 4 落地, 阶段 5 接入 UI 后自然消除

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;

use openstory_bot::config::Config;
use openstory_bot::emit::{self, Emitter};
use openstory_bot::runtime::{self, RunOutcome};
use openstory_bot::state::BotState;
use openstory_console_lib::logbuf::LogBuf;
use tokio::sync::{mpsc, Mutex};

use crate::watcher::{RestartPolicy, RestartState, Watcher};
use openstory_console_lib::eventq::EventQueue;

/// 会话阶段 (与 `app.rs` 的 `STAGE_*` 常量同值, 便于直接塞进原子量)。
pub const STAGE_WIZARD: u8 = 0;
pub const STAGE_RUNNING: u8 = 1;
pub const STAGE_FINISHED: u8 = 2;

/// 命令通道容量。64 足够吸收 UI 连按, 又不至于让人手误刷爆内存。
pub const CMD_CHANNEL_CAP: usize = 64;
/// 每会话的事件队列上限 (超出丢最旧, 保证 UI 卡顿时内存有界)。
pub const EVENT_QUEUE_CAP: usize = 1000;

/// 在跑的 bot task 的取消句柄存放位 (由监督任务写入)。
///
/// 为什么不存 `JoinHandle` 本身: 它必须被 `await`(才能拿到 panic 信息),
/// 而 `SessionArcs` 需要把句柄交给会话层。存 `AbortHandle` 两边都能用 ——
/// 会话层能取消卡死的 bot, 监督任务负责 `await` 与写回。
pub type JoinSlot = Arc<StdMutex<Option<tokio::task::AbortHandle>>>;

/// bot task 是否 panic 结束。`RunOutcome` 表达不了 panic (panic 的 task 没有
/// 返回值), 所以单独一个原子标志, 由监督 task 在 `JoinError::is_panic()` 时置位。
pub type PanicFlag = Arc<AtomicBool>;

/// 一个 bot 会话。
pub struct BotSession {
    /// 档案名 (`profiles/<这个名字>.json` 的 stem), 也是 UI 上的显示名与
    /// `@<目标>` 的匹配对象。
    pub profile: String,
    /// 该会话的档案路径链 (最后一个为回写目标)。
    pub config_paths: Vec<PathBuf>,
    /// 实时游戏状态 (UI 只读; `try_lock` 避免卡帧)。
    pub state: Arc<Mutex<BotState>>,
    /// 该会话的事件队列 —— 每个会话一份, 日志天然分离 (见 G1)。
    pub queue: Arc<EventQueue>,
    /// 该会话的日志缓冲 (中文渲染 + 折行缓存 + 滚动窗口)。
    pub log: LogBuf,
    /// 命令发送端。重连会换新的 (旧 receiver 随旧 runtime 一起结束),
    /// 因此用锁换值而不是固定持有。
    cmd_slot: Arc<StdMutex<mpsc::Sender<String>>>,
    /// 阶段标志: 向导 / 运行中 / 已结束。
    pub stage: Arc<AtomicU8>,
    /// `runtime::run` 的结束原因 (结束后有值)。
    pub outcome: Arc<StdMutex<Option<RunOutcome>>>,
    /// 日志滚动偏移 (0 = 贴底)。
    pub scroll: usize,
    /// 该会话的日志页签过滤 (`Filter::All` 等)。
    ///
    /// **每会话一份**: 过滤是"我在看哪个号"的一部分。全局一份的话, 在 A 账号
    /// 上切到"聊天", 切到 B 账号也变成"聊天" —— 而 B 的日志根本没被看过,
    /// 用户会以为 B 没有日志。与 `scroll` 同样在切换时同步。
    pub filter: u8,
    /// 累计丢弃事件数 (队列满丢最旧)。
    dropped_total: u64,
    /// 掉线自动拉起的状态机 (退避 / 放弃)。
    ///
    /// 每个会话一份: 一个账号放弃不影响别的账号继续退避。
    watcher: Watcher,
    /// bot task 是否 panic 结束。
    panicked: PanicFlag,
    /// bot task 的 handle (用于 `is_finished` 探测)。
    join: JoinSlot,
    /// 拉起用的配置模板 (档案加载后的完整 `Config`)。
    ///
    /// 存"已加载的模板"而不是每次重新读盘: 重连时用户可能已经改过运行期
    /// 参数 (例如 `tick_ms`), 重新读盘会把这些改动悄悄丢掉。
    template: Option<Config>,
    /// 第几次拉起 (0 = 首跑)。每次重新 spawn 加一, UI 据此显示"第 N 次重连"。
    generation: u32,
    /// 用户按过 stop (UI 的"已停止"状态)。
    ///
    /// 与 `watcher` 的 `Stopped { Manual }` 是**两件事**, 都要留:
    /// - 这个标志: "现在停着" —— 按 start 时清掉。
    /// - watcher 那个: "这次结束的原因" —— 恢复自动拉起时要用它判断,
    ///   以免下一次掉线被当成 `Disconnected` 而自动唤醒。
    ///
    /// 分开的另一个理由: 从没启动过的会话 (手动模式初始状态) 也没在跑, 但
    /// 它不算"停", UI 要显示"未启动"而不是"已停止"。
    manual_stopped: bool,
    /// 用户按了 start 但**没启动成功** (没有密码 / 档案没加载)。
    ///
    /// 必须与 `manual_stopped` 分开: 那个在 UI 上显示"已停止", 而这里是
    /// "你想启动但失败了" —— 两种都**不再算"从未启动"**。
    ///
    /// 为什么这件事要紧 (踩过的): 手动模式下程序"没有号在跑、也没有号能自己
    /// 起来"时才允许收尾。若启动失败后会话仍被算作"从未启动", 那一刻
    /// `all_stopped()` 为假、`all_done` 永不置位 —— 用户按了一次失败的 start,
    /// 程序就再也不会自己退出了。
    start_failed: bool,
    /// "用户按了 stop, bot 还在退出" 的闩。
    ///
    /// 存在的理由: 阶段标志 `STAGE_RUNNING` 同时承担了"bot 活着"和"用户刚
    /// 停掉但还没退完"两种含义, 而这两者对"能不能再启动"的答案是相反的。
    /// 拿阶段当判据就会有一个窗口让同一个账号被登录两次。
    ///
    /// 只由新的 spawn 清掉 —— 它是"这只 task 正在收尾"的事实, 不会过期。
    stopping: Arc<AtomicBool>,
}

impl BotSession {
    /// 从一条外部创建的指令通道构造会话。
    ///
    /// `rx` 交给 [`Self::spawn`] 或由调用方自己驱动 —— 会话本身不关心
    /// 指令从哪来 (输入框 / 测试 / 远端的 WebSocket)。
    ///
    /// 这是"会话可被进程内模拟"的落点: 只要有通道和一份档案路径, 就能立起
    /// 一个会话, 不需要同进程的其它会话、也不需要注册任何全局。
    pub fn from_channel(
        profile: impl Into<String>,
        config_paths: Vec<PathBuf>,
        cmd_tx: mpsc::Sender<String>,
    ) -> Self {
        Self::from_channel_with_policy(profile, config_paths, cmd_tx, RestartPolicy::disabled())
    }

    /// 同 [`Self::from_channel`], 但指定自动拉起策略。
    pub fn from_channel_with_policy(
        profile: impl Into<String>,
        config_paths: Vec<PathBuf>,
        cmd_tx: mpsc::Sender<String>,
        policy: RestartPolicy,
    ) -> Self {
        Self {
            profile: profile.into(),
            config_paths,
            state: Arc::new(Mutex::new(BotState::default())),
            queue: Arc::new(EventQueue::new(EVENT_QUEUE_CAP)),
            log: LogBuf::default(),
            cmd_slot: Arc::new(StdMutex::new(cmd_tx)),
            stage: Arc::new(AtomicU8::new(STAGE_WIZARD)),
            outcome: Arc::new(StdMutex::new(None)),
            scroll: 0,
            filter: 0,
            dropped_total: 0,
            watcher: Watcher::new(policy),
            panicked: Arc::new(AtomicBool::new(false)),
            join: Arc::new(StdMutex::new(None)),
            template: None,
            generation: 0,
            manual_stopped: false,
            start_failed: false,
            stopping: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 新建一条自有指令通道的会话, 返回 `(会话, 接收端)`。
    ///
    /// 接收端要交给 [`Self::spawn`] —— 正常情况下会话自己发出去的指令由自己
    /// 的 bot 消费。
    pub fn new(
        profile: impl Into<String>,
        config_paths: Vec<PathBuf>,
    ) -> (Self, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel::<String>(CMD_CHANNEL_CAP);
        (Self::from_channel(profile, config_paths, tx), rx)
    }

    /// 建会话, 同时指定自动拉起策略, 返回 `(会话, 接收端)`。
    pub fn new_with_policy(
        profile: impl Into<String>,
        config_paths: Vec<PathBuf>,
        policy: RestartPolicy,
    ) -> (Self, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel::<String>(CMD_CHANNEL_CAP);
        (
            Self::from_channel_with_policy(profile, config_paths, tx, policy),
            rx,
        )
    }

    // ── 拉起相关 ────────────────────────────────────────────────────

    /// 记下拉起用的配置模板 (档案加载后调用)。
    pub fn set_template(&mut self, cfg: Config) {
        self.template = Some(cfg);
    }

    /// 取配置模板的副本 (重新拉起时用)。
    pub fn template(&self) -> Option<Config> {
        self.template.clone()
    }

    /// 取 handle 存放位 (交给 [`spawn_one`](crate::sessions::spawn_one))。
    pub fn join_slot(&self) -> JoinSlot {
        self.join.clone()
    }

    /// 取 panic 标志 (交给 `spawn_one`)。
    pub fn panic_flag(&self) -> PanicFlag {
        self.panicked.clone()
    }

    /// 第几次拉起 (0 = 首跑)。
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// 自动拉起的状态机 (只读, UI 用)。
    pub fn watcher(&self) -> &Watcher {
        &self.watcher
    }

    /// 距下次自动拉起还有多久 (非等待时为 `None`)。
    pub fn restart_remaining(&self, now: Instant) -> Option<std::time::Duration> {
        self.watcher.remaining(now)
    }

    /// 退避的到点时刻 (UI 每帧自己算倒计时, 不必让会话层知道"现在")。
    pub fn restart_deadline(&self) -> Option<Instant> {
        match self.watcher.state() {
            RestartState::Waiting { until, .. } => Some(*until),
            _ => None,
        }
    }

    /// 这次等待窗口是否已经触发过 (UI 把倒计时显示成"重连中")。
    pub fn restart_fired(&self) -> bool {
        self.watcher.is_waiting_restart_fired()
    }

    /// 当前是否正在等退避拉起。
    pub fn is_waiting_restart(&self) -> bool {
        self.watcher.is_waiting()
    }

    /// 是否已放弃自动拉起 (UI 亮红)。
    pub fn gave_up_restart(&self) -> bool {
        self.watcher.given_up()
    }

    /// 用户是否手动停掉了本会话 (UI 显示"已停止"而不是"未启动")。
    pub fn is_manually_stopped(&self) -> bool {
        self.manual_stopped || self.watcher.is_manually_stopped()
    }

    /// 从未启动过 —— 手动模式 (`--no-autostart`) 下会话的初始状态。
    ///
    /// 四条同时成立才算: 阶段还在"向导"、拉起状态机从未动过 (`Idle` 且代数
    /// 为 0)、不是被用户停掉的、也没有一次启动尝试失败过。
    ///
    /// 不能只看阶段: 一个刚掉线、正在退避的会话阶段也不在 RUNNING, 但它显示
    /// 的必须是"N 秒后重连"而不是"未启动"。
    pub fn never_started(&self) -> bool {
        self.stage.load(Ordering::SeqCst) == STAGE_WIZARD
            && self.generation == 0
            && !self.is_manually_stopped()
            && !self.start_failed
            && matches!(self.watcher.state(), RestartState::Idle)
    }

    /// 是否需要人工介入。
    ///
    /// 两个来源, 取并集 —— 少任何一个都会让"要不要过去看一眼"这个提示漏掉
    /// 一半情况:
    /// - `outcome == Failed`: 登录失败 / 重连耗尽。**不依赖**拉起状态机是否
    ///   已经被喂过 (UI 可能在登记之前就渲染了那一帧)。
    /// - 拉起状态机自身: 已放弃重连 / 认证失败被判定为停止。
    pub fn needs_attention(&self) -> bool {
        self.watcher.needs_attention() || matches!(self.outcome(), Some(RunOutcome::Failed))
    }

    /// 拉起的退避状态 (UI 左栏显示用)。
    pub fn restart_state(&self) -> &RestartState {
        self.watcher.state()
    }

    /// 手动重启: 清空退避计数与放弃状态, 并把阶段从"已结束"拉回"向导"。
    ///
    /// 不在这里真的 spawn —— 拉起需要 runtime handle, 由 [`SessionSet`] 负责。
    pub fn reset_watcher(&mut self) {
        self.watcher.reset();
    }

    /// 关掉本会话的自动拉起 (用户按 R 之外的场合, 例如显式 stop)。
    pub fn set_restart_enabled(&mut self, enabled: bool) {
        self.watcher.set_enabled(enabled);
    }

    /// 自动拉起是否开着。
    pub fn restart_enabled(&self) -> bool {
        self.watcher.policy().enabled
    }

    // ── 手动启停 (控制台的 start / stop) ────────────────────────────

    /// 用户按 start 时的状态迁移 (不 spawn, 由 `SessionSet` 负责)。
    ///
    /// 三件事:
    /// 1. 清掉"手动停止"标志 —— 会话重新进入可被拉起的状态。
    /// 2. `watcher.resume()` —— 把上次 `stop_manual` 写下的
    ///    `Stopped { Manual }` 换回 `Idle`, 否则下一次掉线会被它挡掉。
    /// 3. 已经结束过的会话把阶段拉回"向导", 让它看起来是"要开始了"而不是
    ///    "已结束"; 从没跑过的会话本来就在向导阶段, 这一步是 no-op。
    ///
    /// 返回 `false` 表示这个会话**还有一个 bot task 活着 (或正在退出)** ——
    /// 调用方不该再 spawn 一次 (重复的 bot 会拿同一个账号登录两次, 服务器会
    /// 踢掉先来的那个)。
    ///
    /// 三条判据:
    /// - `stopping` 闩: 用户刚按完 stop、bot 还在退出的那几百毫秒里阶段**仍是**
    ///   `RUNNING`, 这时再 spawn 就会同时有两个登录尝试 —— 正是要防的那件事。
    ///   它只由新的 spawn 清掉 ([`Self::note_respawned`]), 不会因为"等一会儿"
    ///   而失效。
    /// - `task_finished()`: 阶段标志由监督任务在收尾时置位, 是"任务没了"的
    ///   权威回答。从没跑过的会话阶段是 `WIZARD` —— 那也不算"没结束", 所以下面
    ///   还要单独认出"从未启动"。
    /// - `is_waiting_restart()`: 正在等退避的会话任务确实没了, 但它已经在队列
    ///   里, 同样不能再 spawn。
    pub fn prepare_start(&mut self) -> bool {
        // 用 `generation` 而不是 `never_started()` 判断"跑过没有": 后者把
        // "手动停过 / 启动失败过"也算进去, 而那两种情况恰恰是**应该**允许重新
        // 启动的 —— 用它当守卫会把用户永久锁在外面。
        let ever_ran = self.generation > 0;
        if ever_ran && !self.task_finished() {
            // 跑过、而且这个 task 还没收尾 → 活着 (或正在退出)
            return false;
        }
        if self.is_running() && !self.manual_stopped {
            // 首跑正在进行中 (代数还是 0, 阶段已经是 RUNNING)
            return false;
        }
        if self.stopping.load(Ordering::SeqCst) && !self.task_finished() {
            // 用户按了 stop, 但那只 task 还没退干净 —— 这时 spawn 会同时挂着
            // 两个登录尝试。task 一旦收尾 (`task_finished`), 这个闩就无关紧要了,
            // 用户可以立刻重新启动 (否则"stop 完再 start"会被永久挡住)。
            return false;
        }
        if self.is_waiting_restart() {
            return false;
        }
        // 清掉上一次的结束原因: 留着的话 `needs_attention` 会因为一个陈旧的
        // `Failed` 一直亮红, 哪怕这次已经重新启动了。
        if let Ok(mut o) = self.outcome.lock() {
            *o = None;
        }
        self.resume_restart();
        self.manual_stopped = false;
        self.start_failed = false;
        // 阶段离开"向导" —— 这一步同时让 `never_started()` 变假, 于是手动模式
        // 下"还有号没动过"的判断不会把一个启动失败的会话当成"还能再起来"。
        self.stage.store(STAGE_WIZARD, Ordering::SeqCst);
        true
    }

    /// 记一次"用户按了 start 但没起来"。
    ///
    /// 与 [`Self::stop_manual_keep_attempts`] 的区别只体现在 UI 与"还算不算
    /// 从未启动"上: 这个标记让会话显示成"启动失败 — 需人工", 并且不再被当成
    /// "从未启动"(那会让手动模式下的程序永远不肯退出)。
    pub fn mark_start_failed(&mut self) {
        self.start_failed = true;
    }

    /// 这个会话**再也不会自己动了** —— 退出条件只认这个判据。
    ///
    /// 三种情况算"静了":
    /// 1. **从未启动** —— 手动模式下等着用户按 F4。
    /// 2. **启动失败** —— 用户按了 start 但没起来 (没密码 / 档案加载不了)。
    ///    它同样是静的: 没有任何东西会再推动它, 用户只能自己再试一次。
    ///    把它算成"活着"会让程序永远不肯退出 (真实踩到过)。
    /// 3. **任务结束且不在等退避** —— 跑完/停了。
    ///
    /// 反过来, "正在跑"和"正在等退避重连"都是**没静** —— 它们还会自己动。
    pub fn is_settled(&self) -> bool {
        if self.never_started() {
            return true;
        }
        if self.start_failed {
            return true;
        }
        self.task_finished() && !self.is_waiting_restart()
    }

    /// 是否处于"启动尝试失败"的状态。
    pub fn start_failed(&self) -> bool {
        self.start_failed
    }

    /// 用户按 stop: 关掉自动拉起并把状态落到"已停止"。
    ///
    /// 只改状态, **不**杀 task —— 真正的停止由调用方发 `quit` 指令完成 (让
    /// bot 走正常退出路径, 记录结束原因)。这里的顺序很重要: 状态先落到
    /// `Manual`, 于是 bot 随后报上来的 `Disconnected` 不会排定一次拉起
    /// (见 `Watcher::on_exit` 的守卫 3)。
    ///
    /// 返回 `false` 表示本来就没在跑 (调用方可以据此不给用户假反馈)。
    pub fn mark_stopped_by_user(&mut self) -> bool {
        let was_active = self.is_running() || self.is_waiting_restart();
        self.manual_stopped = true;
        // 置"正在收尾"闩: 在 task 真的退出之前, 这个会话不能再被启动
        // (阶段标志这时还是 RUNNING, 不足以区分"活着"与"正在退")。
        self.stopping.store(true, Ordering::SeqCst);
        // 清掉结束原因。`needs_attention()` 把 `outcome == Failed` 也算"需人工",
        // 而用户按 stop 就是在说"这个号我不要了, 别再提示我" —— 留着那条陈旧
        // 的 `Failed`, 界面会继续亮红, 看起来像按键没生效。
        if let Ok(mut o) = self.outcome.lock() {
            *o = None;
        }
        self.watcher.stop_manual();
        was_active
    }

    /// 新的 bot task 已经起来了: 清掉"正在收尾"闩。
    ///
    /// 由会话层在 `spawn_one` 之后调用 —— 那是唯一能让这个闩失效的事件。
    pub fn note_respawned(&self) {
        self.stopping.store(false, Ordering::SeqCst);
    }

    /// 是否正在收尾 (用户按了 stop, 或刚启动还没来得及跑起来)。
    pub fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// 登记一次"用户手动停"的结束原因 (bot 已经真的退了)。
    pub fn note_manual_stop(&mut self) {
        self.watcher
            .on_exit(Some(RunOutcome::Quit), false, Instant::now());
        // on_exit 会把 Normal 落到 Stopped{Normal} —— 但那丢掉"是手动停的"
        // 这个信息。这里再压回 Manual, 保证恢复自动拉起时的判定正确。
        self.watcher.stop_manual();
    }

    /// 落到"已停止"(手动), 但 **不**清退避计数 —— 拉起失败时用。
    ///
    /// 与 [`Self::mark_stopped_by_user`] 的区别:`mark_stopped_by_user` 是用户
    /// 主动停, 语义上"从头开始", 计数清零; 这里是"想拉起但拉不了", 计数要
    /// 留着 —— 那正是"有没有再试的可能"的依据。
    ///
    /// 为什么不能只 `set_restart_enabled(false)`: 那只是关掉开关, 状态仍是
    /// `Idle`。用户之后按 start → `resume()` 因为状态不是 `Manual` 而 no-op,
    /// 阶段被拉回向导, 界面显示"未启动", 但下一次掉线**也不会**被拉起
    /// (开关还关着) —— 用户会以为 start 成功了。
    pub fn stop_manual_keep_attempts(&mut self) {
        self.manual_stopped = true;
        self.watcher.stop_manual_keep_attempts();
    }

    /// 从"手动停掉"恢复自动拉起 (按 start 时由 [`Self::prepare_start`] 调)。
    ///
    /// 返回是否真的恢复了 —— 不是手动停的状态返回 `false`。
    pub fn resume_restart(&mut self) -> bool {
        let r = self.watcher.resume();
        if r {
            self.manual_stopped = false;
        }
        r
    }

    /// 是否有一个 bot task 在 (abort 句柄已入槽)。
    ///
    /// 比 `is_running()` (阶段标志) 更贴近"真的有没有一只 task": 阶段由 task
    /// **内部**置位, 因此 `spawn` 之后的那一小段时间里阶段还是旧值 ——
    /// 而"该不该发 quit"问的正是"有没有东西可以通知"。
    pub fn has_task(&self) -> bool {
        match self.join.lock() {
            Ok(slot) => slot.is_some(),
            Err(e) => e.into_inner().is_some(),
        }
    }

    /// 强行取消本会话的 bot task (卡死时的兜底)。
    ///
    /// 正常路径是发 `quit` 让 bot 自己退; 只有 bot 卡住 (不响应指令) 时才
    /// 需要 abort。abort 的任务没有返回值, 于是结束原因会是
    /// "没有原因的结束" —— 所以调用方必须先 `mark_stopped_by_user`。
    pub fn abort_task(&self) -> bool {
        match self.join.lock() {
            Ok(slot) => match slot.as_ref() {
                Some(h) => {
                    h.abort();
                    true
                }
                None => false,
            },
            Err(e) => {
                if let Some(h) = e.into_inner().as_ref() {
                    h.abort();
                    true
                } else {
                    false
                }
            }
        }
    }

    /// 往本会话的日志里塞一条控制台自己的通知。
    ///
    /// 自动拉起 / 手动重启 / 指令没送出去 都走这里 —— 它们必须落在**对应
    /// 会话的日志里**, 多开时才能回答"这个号为什么重连了"。
    pub fn note(&mut self, level: openstory_bot::emit::Level, text: impl Into<String>) {
        self.log.push_note(level, text);
    }

    /// 后台 bot 是否已经结束。
    ///
    /// 判据是 `stage == FINISHED` —— 它由监督任务在**写完结束原因之后**置位,
    /// 因此"已结束"必然伴随可读的 `outcome`。比轮询 `JoinHandle::is_finished()`
    /// 可靠: 后者可能先变真、写回还没完成, 读者会看到 `outcome == None` 而
    /// 误判成"没有原因的结束"。
    pub fn task_finished(&self) -> bool {
        self.stage.load(Ordering::SeqCst) == STAGE_FINISHED
    }

    /// 会话真的结束后登记一次退出, 返回"是否已排定一次自动拉起"。
    ///
    /// **幂等**: 靠 [`Watcher`] 自己的守卫 —— 已经在一个**未触发**的等待窗口
    /// 里时, `on_exit` 直接返回 `false`; 窗口触发过之后新的结束才会推进到
    /// 下一次退避。因此每帧调用既不会把退避无限推后, 也不会拉起多次。
    pub fn note_exit(&mut self, now: Instant) -> bool {
        if !self.task_finished() {
            return false;
        }
        let panicked = self.panicked.load(Ordering::SeqCst);
        // `outcome` 为空说明监督任务还没把原因写下去 (极短的一个窗口) ——
        // 传 `None` 让它推进状态而不是拿一个假原因去判定。
        let outcome = self.outcome();
        self.watcher.on_exit(outcome, panicked, now)
    }

    /// 自动拉起到点了 (由 [`SessionSet::advance_watchers`] 调用)。
    ///
    /// 返回 `true` 时调用方应当立刻重新 spawn 本会话 —— 返回 `true` 只会
    /// 出现一次 (见 [`Watcher::poll_ready`])。
    pub fn poll_restart(&mut self, now: Instant) -> bool {
        self.watcher.poll_ready(now)
    }

    /// 记一次拉起: 加代数, 并返回本次拉起要用的配置模板副本。
    ///
    /// `password` 给出时覆盖模板里的密码 —— 密码可能来自控制台的凭据存储
    /// (档案里没写 `login.password` 的场合)。**只改密码**: ip / tick / 向导里
    /// 改过的运行期参数一律延续模板, 不重新读盘 (读盘会悄悄丢掉用户的改动)。
    ///
    /// 返回 `None` 表示没有模板 (档案从未加载成功) —— 此时不该拉起, 否则只会
    /// 立刻再失败一次。
    pub fn begin_respawn(&mut self, password: Option<&str>) -> Option<Config> {
        let mut cfg = self.template.clone()?;
        if let Some(p) = password {
            if !p.is_empty() {
                cfg.password = p.to_string();
                // 把补上的密码写回模板: 一是"下一次拉起"不该再依赖存储还在
                // (用户可能刚清掉它), 二是诊断时能从模板看出实际用了什么。
                self.template = Some(cfg.clone());
            }
        }
        self.generation = self.generation.saturating_add(1);
        self.panicked.store(false, Ordering::SeqCst);
        if let Ok(mut slot) = self.join.lock() {
            *slot = None;
        }
        if let Ok(mut o) = self.outcome.lock() {
            *o = None;
        }
        self.stage.store(STAGE_WIZARD, Ordering::SeqCst);
        // 重新武装退避状态机 —— 否则新的 bot 再掉线时会被"已触发的窗口"挡住,
        // 拉起只发生一次 (见 `Watcher::rearm`)。
        self.watcher.rearm();
        Some(cfg)
    }

    /// 档案名 -> 显示名 / 目标匹配名。取档案路径的 stem。
    pub fn profile_from_path(path: &Path) -> String {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string()
    }

    /// 该会话的 data_dir (名字表来源)。档案缺失/损坏时为空 = 默认 `data/`。
    pub fn data_dir(&self) -> String {
        self.config_paths
            .last()
            .map(|p| openstory_bot::runtime_config::peek_data_dir(p))
            .unwrap_or_default()
    }

    /// 发送一条指令。会话未启动或已结束 (receiver 被丢弃) 时返回 `Err`。
    ///
    /// 刻意不静默吞掉: "指令没送出去"必须能被 UI 显示出来, 否则用户会以为
    /// 自己敲的指令生效了。
    pub fn send(&self, cmd: impl Into<String>) -> Result<(), String> {
        self.cmd_slot
            .lock()
            .map_err(|_| "cmd slot poisoned".to_string())?
            .try_send(cmd.into())
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => "指令队列已满 (会话卡住?)".to_string(),
                mpsc::error::TrySendError::Closed(_) => "会话未运行或已结束".to_string(),
            })
    }

    /// 换掉命令发送端 (重连时调用: 新的 bot 有新的 receiver)。
    pub fn replace_cmd_tx(&self, tx: mpsc::Sender<String>) {
        if let Ok(mut slot) = self.cmd_slot.lock() {
            *slot = tx;
        }
    }

    /// 拿到"发送到本会话"的句柄 (与 [`Self::send`] 走同一条通道)。
    ///
    /// 给 UI 的通用指令路径用: 输入框直接敲的指令 = 发给当前选中会话。
    /// 共享同一把锁, 因此重连换 sender 后句柄自动跟着新通道走 ——
    /// UI 不需要感知重连。
    pub fn cmd_tx_handle(&self) -> Arc<StdMutex<mpsc::Sender<String>>> {
        self.cmd_slot.clone()
    }

    /// 会话是否还在跑。
    pub fn is_running(&self) -> bool {
        self.stage.load(Ordering::SeqCst) == STAGE_RUNNING
    }

    /// 会话是否已结束。
    pub fn is_finished(&self) -> bool {
        self.stage.load(Ordering::SeqCst) == STAGE_FINISHED
    }

    /// 结束原因 (未结束时为 `None`)。
    ///
    /// **毒化安全**: 毒化的锁要拿回内部值而不是当成"没有原因"。
    ///
    /// 这不是洁癖: bot panic 会毒化这把锁, 而 `outcome` 正是"这个会话怎么
    /// 结束的"的唯一来源 —— 用 `.ok()` 会把它读成 `None`, 拉起逻辑于是一路
    /// 走默认分支。崩溃之后连"它是怎么崩的"都读不出来是最糟的情况。
    /// (写入侧 [`crate::sessions::write_back`] 用同样的方式容错, 两边必须一致,
    /// 否则会出现"写了但读不到"。)
    pub fn outcome(&self) -> Option<RunOutcome> {
        self.outcome
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .to_owned()
    }

    /// 读一份状态快照。`try_lock` 失败时返回 `None` —— 渲染不能因为 bot
    /// 正持锁而卡住整帧。
    pub fn try_snapshot(&self) -> Option<openstory_console_lib::snapshot::Snapshot> {
        self.state
            .try_lock()
            .ok()
            .map(|st| openstory_console_lib::snapshot::Snapshot::from(&st))
    }

    /// 读一份快照, 必要时等待锁 (非渲染路径用, 例如测试与导出)。
    pub async fn snapshot(&self) -> openstory_console_lib::snapshot::Snapshot {
        let st = self.state.lock().await;
        openstory_console_lib::snapshot::Snapshot::from(&st)
    }

    /// 把队列里的新事件取进本会话的日志缓冲。
    ///
    /// 返回本次新增的事件数。由 UI 在每帧前调用 (与改造前 `App::poll_events`
    /// 的行为一致), 因此每个会话的日志严格来自它自己的队列。
    ///
    /// `take_all` 的 dropped 计数是**累计值**, 这里换算成增量累加到
    /// `dropped_total` —— UI 显示"日志丢弃 N 条"用的是累计数。
    pub fn drain_events(&mut self) -> usize {
        let (evs, _) = self.take_queued();
        let n = evs.len();
        for ev in evs {
            self.log.push(ev);
        }
        n
    }

    /// 取出本会话队列里的全部事件, **不动日志缓冲**。
    ///
    /// 返回 `(事件, 累计丢弃数)`。给多会话的分发用: 到底写进哪个缓冲由调用方
    /// 决定 —— 选中会话的缓冲在渲染位置, 不是 `self.log` (见
    /// [`SessionSet::drain_all`](crate::sessions::SessionSet::drain_all))。
    pub fn take_queued(&mut self) -> (Vec<openstory_bot::emit::Event>, u64) {
        let (evs, dropped_total) = self.queue.take_all();
        self.dropped_total = dropped_total;
        (evs, dropped_total)
    }

    /// 把一条事件写进**本会话自己的**日志缓冲。
    pub fn push_log(&mut self, ev: openstory_bot::emit::Event) {
        self.log.push(ev);
    }

    /// 累计丢弃的事件数 (队列满时丢最旧), UI 用来提示"日志有缺失"。
    pub fn dropped(&self) -> u64 {
        self.dropped_total
    }

    /// 在本会话的事件作用域内跑 bot, 直到结束。
    ///
    /// 两件事一起做:
    /// - `with_session_emitter`: 该会话的事件只进**它自己的**队列 (G1),
    ///   因此 N 个会话同时跑也不会串台。
    /// - 结束后写 `stage` / `outcome`, UI 据此显示"已结束/失败"。
    ///
    /// `rx` 必须来自本会话的通道 (见 [`Self::new`])。
    pub async fn spawn(&self, config: Config, rx: &mut mpsc::Receiver<String>) -> RunOutcome {
        self.stage.store(STAGE_RUNNING, Ordering::SeqCst);
        let emitter: Arc<dyn Emitter> = self.queue.clone();
        let state = self.state.clone();
        let outcome =
            emit::with_session_emitter(
                emitter,
                async move { runtime::run(config, rx, state).await },
            )
            .await;
        self.stage.store(STAGE_FINISHED, Ordering::SeqCst);
        if let Ok(mut slot) = self.outcome.lock() {
            *slot = Some(outcome);
        }
        outcome
    }

    /// 取本会话的全部分布式句柄 (交给 `spawn_one`)。
    ///
    /// 每次 spawn 都重新取: 重新拉起后 `stage` / `outcome` / `join` 必须是
    /// **同一个 `Arc`** (UI 与监督任务持有的都是它), 只是里面的值刷新了。
    /// 复用同一份 `Arc` 是 UI 能看穿"第 N 次重连"的前提。
    pub fn arcs(&self) -> crate::sessions::SessionArcs {
        (
            self.queue.clone(),
            self.state.clone(),
            self.stage.clone(),
            self.outcome.clone(),
            self.panic_flag(),
            self.join_slot(),
        )
    }

    /// 从档案路径派生会话所需的一切 (档案名 + 路径链 + 该档案的 data_dir)。
    ///
    /// 名字表按 data_dir 在这里**预解析**: 渲染层在会话还没连上时就要显示
    /// 中文名, 不能等 `runtime::run` 起来才设置。
    pub fn prepare(
        paths: Vec<PathBuf>,
    ) -> (String, Vec<PathBuf>, Arc<openstory_bot::names::NameTable>) {
        let profile = paths
            .last()
            .map(|p| Self::profile_from_path(p))
            .unwrap_or_else(|| "unknown".to_string());
        let dir = paths
            .last()
            .map(|p| openstory_bot::runtime_config::peek_data_dir(p))
            .unwrap_or_default();
        let names = openstory_bot::names::NameTable::for_dir(&dir);
        (profile, paths, names)
    }
}

/// 一个会话的静态描述 (UI 选择屏的产物): 档案路径 + 可选显示名。
///
/// 与 [`BotSession`] 分开是刻意的: "要开哪些账号"在跑起来之前就确定了,
/// 而会话对象在运行期才有。UI 的档案多选屏只需要前者。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSpec {
    pub config_path: PathBuf,
    /// 显示名 (默认 = 档案 stem)。
    pub name: String,
    /// 是否记住密码 (阶段 6 的自动拉起需要)。
    pub remember_password: bool,
}

impl SessionSpec {
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        let config_path = config_path.into();
        let name = BotSession::profile_from_path(&config_path);
        Self {
            config_path,
            name,
            remember_password: false,
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn remembering_password(mut self) -> Self {
        self.remember_password = true;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn spec(name: &str) -> SessionSpec {
        SessionSpec::new(format!("profiles/{name}.json"))
    }

    #[test]
    fn session_from_channel_needs_nothing_global() {
        // 阶段 4 的硬性架构验收: 一条通道 + 一份档案路径就能立起会话。
        // 这条测试故意不接触任何全局 (不 set_emitter / 不 set_default_dir)。
        let (tx, _rx) = mpsc::channel::<String>(4);
        let s = BotSession::from_channel("acc1", vec![PathBuf::from("profiles/acc1.json")], tx);
        assert_eq!(s.profile, "acc1");
        assert!(!s.is_running());
        assert!(s.outcome().is_none());
        assert_eq!(s.scroll, 0);
    }

    #[test]
    fn session_new_returns_its_own_receiver() {
        let (s, mut rx) = BotSession::new("acc1", vec![PathBuf::from("profiles/acc1.json")]);
        // 会话自己发出去的指令由自己的接收端消费 (不串到别的会话)
        s.send("hunt on").unwrap();
        assert_eq!(rx.try_recv().unwrap(), "hunt on");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn two_sessions_have_independent_queues_and_state() {
        // 多会话隔离的最小断言: 队列与状态不共享
        let (a, _ra) = BotSession::new("a", vec![PathBuf::from("profiles/a.json")]);
        let (b, _rb) = BotSession::new("b", vec![PathBuf::from("profiles/b.json")]);
        assert!(!Arc::ptr_eq(&a.queue, &b.queue), "事件队列必须每会话一份");
        assert!(!Arc::ptr_eq(&a.state, &b.state), "状态必须每会话一份");
        assert!(!Arc::ptr_eq(&a.stage, &b.stage), "阶段标志必须每会话一份");
        assert!(
            !Arc::ptr_eq(&a.cmd_slot, &b.cmd_slot),
            "命令通道必须每会话一份"
        );
    }

    #[test]
    fn send_reports_error_when_session_ended() {
        let (s, rx) = BotSession::new("acc1", vec![]);
        drop(rx); // bot 结束 -> receiver 没了
        let err = s.send("hunt on").unwrap_err();
        assert!(
            err.contains("已结束") || err.contains("未运行"),
            "got {err}"
        );
    }

    #[test]
    fn send_reports_full_queue() {
        // 通道容量 64; 塞满之后必须报"队列已满"而不是静默丢弃
        let (tx, _rx) = mpsc::channel::<String>(2);
        let s = BotSession::from_channel("acc1", vec![], tx);
        s.send("a").unwrap();
        s.send("b").unwrap();
        let err = s.send("c").unwrap_err();
        assert!(err.contains("已满"), "got {err}");
    }

    #[test]
    fn replace_cmd_tx_redirects_commands() {
        // 重连场景: 换 sender 后指令走新通道
        let (s, mut old_rx) = BotSession::new("acc1", vec![]);
        assert!(old_rx.try_recv().is_err());
        let (tx2, mut rx2) = mpsc::channel::<String>(4);
        s.replace_cmd_tx(tx2);
        s.send("view status").unwrap();
        assert_eq!(rx2.try_recv().unwrap(), "view status");
        assert!(old_rx.try_recv().is_err(), "旧通道不应再收到指令");
    }

    #[test]
    fn stage_helpers_track_the_flag() {
        let (s, _r) = BotSession::new("acc1", vec![]);
        s.stage.store(STAGE_RUNNING, Ordering::SeqCst);
        assert!(s.is_running() && !s.is_finished());
        s.stage.store(STAGE_FINISHED, Ordering::SeqCst);
        assert!(!s.is_running() && s.is_finished());
        s.stage.store(STAGE_WIZARD, Ordering::SeqCst);
        assert!(!s.is_running() && !s.is_finished());
    }

    #[test]
    fn profile_name_comes_from_file_stem() {
        assert_eq!(
            BotSession::profile_from_path(Path::new("profiles/本地服_100000001.json")),
            "本地服_100000001"
        );
        assert_eq!(BotSession::profile_from_path(Path::new("a/b/c.json")), "c");
        // 没有文件名 / 非 UTF-8 路径都不能 panic
        assert_eq!(BotSession::profile_from_path(Path::new("/")), "unknown");
    }

    #[test]
    fn spec_defaults_to_file_stem_name() {
        let sp = spec("serverA_100000003");
        assert_eq!(sp.name, "serverA_100000003");
        assert!(!sp.remember_password);
        assert_eq!(
            sp.config_path,
            PathBuf::from("profiles/serverA_100000003.json")
        );
    }

    #[test]
    fn spec_builders_override_defaults() {
        let sp = spec("a").with_name("小号A").remembering_password();
        assert_eq!(sp.name, "小号A");
        assert!(sp.remember_password);
    }

    #[test]
    fn data_dir_falls_back_to_default_when_profile_missing() {
        let (s, _r) = BotSession::new("ghost", vec![PathBuf::from("profiles/不存在的档案.json")]);
        // 档案不存在 -> peek_data_dir 返回空 -> NameTable 用默认 data/
        assert_eq!(s.data_dir(), "");
        let (_p, _paths, names) =
            BotSession::prepare(vec![PathBuf::from("profiles/不存在的档案.json")]);
        assert_eq!(names.dir(), "data", "空 data_dir 必须落到默认目录");
    }

    #[test]
    fn prepare_reads_data_dir_from_the_profile_file() {
        // 用临时档案验证 data_dir 真的从文件里读出来 (而不是猜的)
        let tmp = std::env::temp_dir().join(format!("sess_prep_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let p = tmp.join("acc.json");
        std::fs::write(&p, r#"{"data_dir":"data/server_b","tick_ms":50}"#).unwrap();
        let (name, paths, names) = BotSession::prepare(vec![p.clone()]);
        assert_eq!(name, "acc");
        assert_eq!(paths, vec![p.clone()]);
        assert_eq!(names.dir(), "data/server_b");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn snapshot_is_readable_without_awaiting_the_lock() {
        let (s, _r) = BotSession::new("acc1", vec![]);
        // 未被持锁时 try_snapshot 应当成功
        assert!(s.try_snapshot().is_some());
        // 持锁期间必须返回 None (渲染不能卡帧)
        let guard = s.state.try_lock().unwrap();
        assert!(s.try_snapshot().is_none(), "持锁时应放弃而不是阻塞");
        drop(guard);
        assert!(s.try_snapshot().is_some());
    }

    #[tokio::test]
    async fn drain_events_moves_queue_into_log() {
        use openstory_bot::emit::{Event, Level};
        let (mut s, _r) = BotSession::new("acc1", vec![]);
        assert!(s.log.is_empty());
        s.queue.emit(Event::new(
            Level::Info,
            openstory_bot::emit::Category::Chat,
            "hi".into(),
        ));
        s.queue.emit(Event::new(
            Level::Info,
            openstory_bot::emit::Category::Chat,
            "there".into(),
        ));
        assert_eq!(s.drain_events(), 2);
        assert!(!s.log.is_empty(), "事件必须进入本会话的日志缓冲");
        // 再取一次应为空 (take_all 语义)
        assert_eq!(s.drain_events(), 0);
    }

    // ── 自动拉起 (阶段 6) ───────────────────────────────────────────
    //
    // 这里用**合成会话** (直接写 `stage` / `outcome`) 而不是真跑 bot, 理由是
    // 精确: 控制台层的输入就是"阶段标志 + 结束原因", 而真跑一个 bot 只能产生
    // 少数几种组合 (本地假服务器永远进不了游戏, 核心层会把断线归约成
    // `Failed`)。把输入直接合成出来, 每一条决策分支都能被测到 —— 包括真实
    // 环境里很难复现的那些 (task panic / 半死状态 / 缺模板)。
    //
    // 真的跑 bot 的那部分由 `spawn_one` 的测试 (下面 `spawn_one_*`) 与联调
    // 测试 `tests/session_layer_live.rs` 覆盖。

    /// 拉起策略: 退避几乎为 0 + 试到第 2 次就放弃 —— 测试不该等 45 秒。
    fn fast_policy() -> RestartPolicy {
        RestartPolicy {
            enabled: true,
            max_attempts: 2,
            backoff_secs: vec![0],
        }
    }

    /// 造一个"刚刚以 `outcome` 结束"的会话 —— 这正是控制台每帧看到的东西。
    fn ended(profile: &str, outcome: RunOutcome, policy: RestartPolicy) -> BotSession {
        let (s, _rx) = BotSession::new_with_policy(profile, vec![], policy);
        s.stage
            .store(STAGE_FINISHED, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut o) = s.outcome.lock() {
            *o = Some(outcome);
        }
        s
    }

    /// 走完"登记 → 退避到点"两步, 返回是否真的触发了拉起。
    ///
    /// `t` 只是**起始**时刻; 内部会跳到到点之后再触发, 因此调用方不用自己
    /// 算退避时长 (退避策略改了也不用改测试)。
    fn advance(s: &mut BotSession, t: Instant) -> bool {
        if !s.note_exit(t) {
            return false;
        }
        match s.restart_deadline() {
            // `max` 是必要的: `t` 可能已经晚于到点时刻 (调用方在循环里递增
            // 一个假的 "now"), 那时不该把时间往回拨。
            Some(at) => {
                let fire_at = at.max(t) + std::time::Duration::from_millis(1);
                s.poll_restart(fire_at)
            }
            None => false,
        }
    }

    #[test]
    fn disconnect_is_scheduled_for_restart() {
        let mut s = ended(
            "acc1",
            RunOutcome::ConnectionClosed,
            RestartPolicy::default(),
        );
        assert!(s.task_finished());
        let now = Instant::now();
        assert!(s.note_exit(now), "断线应排定一次拉起");
        assert!(s.is_waiting_restart());
        assert!(s.restart_deadline().is_some());
        assert!(!s.restart_fired(), "刚排定还没触发");
        assert!(
            s.restart_remaining(now).unwrap() > std::time::Duration::ZERO,
            "默认策略第一次要等 5 秒"
        );
        assert!(!s.needs_attention(), "正常退避期间不该报需人工");
    }

    #[test]
    fn connect_failed_is_scheduled_for_restart() {
        // 服务器维护/重启 → 值得一直试
        let mut s = ended("acc1", RunOutcome::ConnectFailed, RestartPolicy::default());
        assert!(s.note_exit(Instant::now()));
        assert!(s.is_waiting_restart());
    }

    #[test]
    fn panic_is_scheduled_for_restart() {
        // panic 的 task 没有返回值, 结束原因会是默认的 Quit —— 只有 panic
        // 标志能把它区分出来。不看这个标志就会把崩溃当"正常退出"而不拉起。
        let mut s = ended("acc1", RunOutcome::Quit, RestartPolicy::default());
        s.panicked.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            s.note_exit(Instant::now()),
            "panic 必须被当成崩溃而不是正常退出"
        );
        assert!(s.is_waiting_restart());
    }

    #[test]
    fn normal_exit_is_never_restarted() {
        for out in [RunOutcome::Quit, RunOutcome::DurationElapsed] {
            let mut s = ended("acc1", out, RestartPolicy::default());
            assert!(!s.note_exit(Instant::now()), "{out:?} 不该拉起");
            assert!(!s.is_waiting_restart());
            assert!(!s.needs_attention(), "{out:?} 是用户自己要停, 不该报警");
        }
    }

    #[test]
    fn note_exit_is_idempotent() {
        // UI 每帧都调 note_exit —— 排定两次会把退避无限推后, 也会拉起两次。
        let mut s = ended(
            "acc1",
            RunOutcome::ConnectionClosed,
            RestartPolicy::default(),
        );
        let t = Instant::now();
        assert!(s.note_exit(t));
        let deadline = s.restart_deadline();
        assert!(!s.note_exit(t), "第二次必须是 no-op");
        assert!(!s.note_exit(t + std::time::Duration::from_secs(1)));
        assert_eq!(s.restart_deadline(), deadline, "到点时刻不应被推后");
    }

    #[test]
    fn note_exit_after_a_fired_window_schedules_the_next_backoff() {
        // 拉起之后又掉线 → 必须排定**下一次** (更长的) 退避, 而不是无限
        // 重复同一个窗口。
        let mut s = ended(
            "acc1",
            RunOutcome::ConnectionClosed,
            RestartPolicy::default(),
        );
        s.set_template(cfg_at(1));
        let t = Instant::now();
        assert!(s.note_exit(t));
        let first = s.restart_remaining(t).unwrap();
        let at = s.restart_deadline().unwrap() + std::time::Duration::from_millis(1);
        assert!(s.poll_restart(at));
        // 拉起: 会话回到未运行 (阶段被 begin_respawn 拉回 WIZARD)
        assert!(s.begin_respawn(None).is_some());
        assert!(!s.is_waiting_restart(), "拉起后不再处于等待");
        // 新 bot 跑起来又掉线
        simulate_bot_end(&s, RunOutcome::ConnectionClosed);
        assert!(s.note_exit(at + std::time::Duration::from_secs(1)));
        let second = s
            .restart_remaining(at + std::time::Duration::from_secs(1))
            .unwrap();
        assert!(
            second > first,
            "第二次退避必须更长: {first:?} vs {second:?}"
        );
    }

    #[test]
    fn poll_restart_fires_exactly_once() {
        let mut s = ended("acc1", RunOutcome::ConnectionClosed, fast_policy());
        let t = Instant::now();
        assert!(s.note_exit(t));
        let at = s.restart_deadline().unwrap() + std::time::Duration::from_millis(1);
        assert!(s.poll_restart(at), "到点应触发一次");
        assert!(!s.poll_restart(at), "第二次不能重复触发");
        assert!(!s.poll_restart(at + std::time::Duration::from_secs(60)));
        assert!(s.restart_fired());
    }

    #[test]
    fn poll_restart_before_the_deadline_does_nothing() {
        let mut s = ended(
            "acc1",
            RunOutcome::ConnectionClosed,
            RestartPolicy::default(),
        );
        let t = Instant::now();
        assert!(s.note_exit(t));
        assert!(!s.poll_restart(t), "刚排定不该立刻触发");
        assert!(!s.poll_restart(t + std::time::Duration::from_secs(4)));
        assert!(s.is_waiting_restart(), "没触发就还在等");
        assert!(!s.restart_fired());
    }

    /// 把会话拨回"刚跑完一次、以 `outcome` 结束"的状态。
    ///
    /// 真实流程里这一步由 `spawn_one` 里那个 bot task 完成 (写 `outcome` 再置
    /// `FINISHED`)。合成测试里手动做, 才能把"拉起 → 新 bot 又掉线"这条循环
    /// 走出去 —— 只用 `begin_respawn` 的话阶段会被拉回"未运行", 下一次
    /// `note_exit` 会因为"任务还没结束"而拒绝登记。
    fn simulate_bot_end(s: &BotSession, outcome: RunOutcome) {
        if let Ok(mut o) = s.outcome.lock() {
            *o = Some(outcome);
        }
        s.stage
            .store(STAGE_FINISHED, std::sync::atomic::Ordering::SeqCst);
    }

    /// 一次完整的"掉线 → 退避 → 拉起"循环, 返回触发时等的那一档退避。
    ///
    /// 刻意不复刻 `SessionSet::advance_watchers` 的实现, 只做它做的三件事:
    /// 登记掉线、到点 `poll_restart`、`begin_respawn` (后者重新武装窗口)。
    fn drop_and_restart(s: &mut BotSession, drop_at: Instant) -> std::time::Duration {
        assert!(
            s.note_exit(drop_at),
            "登记掉线失败 (状态 {:?})",
            s.restart_state()
        );
        let wait = s.restart_remaining(drop_at).expect("应在等待");
        let fire = s.restart_deadline().unwrap() + std::time::Duration::from_millis(1);
        assert!(
            s.poll_restart(fire),
            "到点未触发 (状态 {:?})",
            s.restart_state()
        );
        assert!(s.begin_respawn(None).is_some(), "拉起失败");
        // 新 bot 跑起来, 然后又掉线 —— 为下一圈准备好状态
        simulate_bot_end(s, RunOutcome::ConnectionClosed);
        wait
    }

    #[test]
    fn gives_up_after_max_attempts_and_needs_attention() {
        // 服务器一直不可用 → 试到上限就放弃, 不能无限重连
        let mut s = ended("acc1", RunOutcome::ConnectionClosed, fast_policy());
        s.set_template(cfg_at(1));
        let t = Instant::now();
        for i in 0..2u32 {
            drop_and_restart(&mut s, t + std::time::Duration::from_secs(i as u64 * 20));
        }
        // 第三次结束: 已到上限 → 放弃
        assert!(
            !s.note_exit(t + std::time::Duration::from_secs(60)),
            "试满上限之后不该再排定 (状态 {:?})",
            s.restart_state()
        );
        assert!(s.gave_up_restart(), "试满上限必须放弃");
        assert!(s.needs_attention(), "放弃后必须标记需人工");
        assert!(!s.is_waiting_restart());
        assert!(
            !s.poll_restart(t + std::time::Duration::from_secs(9999)),
            "放弃之后不能还能被触发"
        );
    }

    #[test]
    fn backoff_grows_across_real_restarts() {
        // 反复掉线的号: 每次退避都要更长, 否则就成了 5 秒一轮的重连风暴
        let mut s = ended(
            "acc1",
            RunOutcome::ConnectionClosed,
            RestartPolicy::default(),
        );
        s.set_template(cfg_at(1));
        let t = Instant::now();
        let mut waits = Vec::new();
        for i in 0..3u64 {
            waits.push(drop_and_restart(
                &mut s,
                t + std::time::Duration::from_secs(i * 600),
            ));
        }
        assert!(waits[0] < waits[1], "{waits:?}");
        assert!(waits[1] < waits[2], "{waits:?}");
        assert_eq!(waits[0], std::time::Duration::from_secs(5));
        assert_eq!(waits[1], std::time::Duration::from_secs(15));
        assert_eq!(waits[2], std::time::Duration::from_secs(45));
        // 第 4 次结束 → 放弃
        assert!(!s.note_exit(t + std::time::Duration::from_secs(2000)));
        assert!(s.gave_up_restart());
    }

    #[test]
    fn a_stale_pending_window_does_not_block_forever() {
        // 窗口被排定却从未触发 (调用方拿了 token 却没 spawn) → 不能永久挡住
        // 后续的掉线, 否则会话卡在"准备重连"里什么都不会发生。
        let mut s = ended("acc1", RunOutcome::ConnectionClosed, fast_policy());
        let t = Instant::now();
        assert!(s.note_exit(t));
        let first = s.restart_deadline().unwrap();
        assert!(
            !s.note_exit(t + std::time::Duration::from_secs(1)),
            "窗口还新鲜时必须拒绝重复登记"
        );
        assert_eq!(s.restart_deadline(), Some(first), "拒绝时不能动到点时刻");
        // 超过 `PENDING_WINDOW_STALE` (60s) 之后必须放行
        assert!(
            s.note_exit(first + std::time::Duration::from_secs(61)),
            "过期窗口必须让新的登记通过"
        );
    }

    #[test]
    fn auth_failure_marks_attention_and_never_waits() {
        let mut s = ended("acc1", RunOutcome::Failed, RestartPolicy::default());
        assert!(!s.note_exit(Instant::now()), "登录失败绝不排定拉起");
        assert!(!s.is_waiting_restart());
        assert!(s.needs_attention(), "必须标记需人工");
        assert!(!s.poll_restart(Instant::now() + std::time::Duration::from_secs(999)));
        assert!(s.restart_deadline().is_none());
    }

    #[test]
    fn reset_watcher_clears_give_up_so_a_new_exit_can_schedule() {
        let mut s = ended("acc1", RunOutcome::ConnectionClosed, fast_policy());
        s.set_template(cfg_at(1));
        let t = Instant::now();
        for i in 0..2u64 {
            drop_and_restart(&mut s, t + std::time::Duration::from_secs(i * 20));
        }
        assert!(
            !s.note_exit(t + std::time::Duration::from_secs(60)),
            "试满上限"
        );
        assert!(s.gave_up_restart(), "试满上限必须放弃");
        assert!(s.needs_attention());
        s.reset_watcher();
        assert!(!s.gave_up_restart());
        assert!(!s.needs_attention());
        assert!(
            s.note_exit(t + std::time::Duration::from_secs(120)),
            "清零后又能排定"
        );
    }

    #[test]
    fn restart_switch_can_be_turned_off_per_session() {
        let mut s = ended(
            "acc1",
            RunOutcome::ConnectionClosed,
            RestartPolicy::default(),
        );
        assert!(s.restart_enabled());
        s.set_restart_enabled(false);
        assert!(!s.restart_enabled());
        assert!(!s.note_exit(Instant::now()), "关掉后不再拉起");
        assert!(!s.is_waiting_restart());
    }

    #[test]
    fn one_session_giving_up_does_not_affect_another() {
        // 每个会话一份状态机 —— 一个号放弃不该让另一个号也停
        let mut a = ended("a", RunOutcome::ConnectionClosed, fast_policy());
        let mut b = ended("b", RunOutcome::ConnectionClosed, fast_policy());
        a.set_template(cfg_at(1));
        let t = Instant::now();
        // 把 a 推到放弃
        for i in 0..2u64 {
            drop_and_restart(&mut a, t + std::time::Duration::from_secs(i * 20));
        }
        assert!(!a.note_exit(t + std::time::Duration::from_secs(60)));
        assert!(a.gave_up_restart());
        // b 完全不受影响
        assert!(b.note_exit(t), "b 应能正常排定");
        assert!(b.is_waiting_restart());
        assert!(!b.gave_up_restart());
        assert!(!b.needs_attention());
    }

    #[test]
    fn note_exit_without_template_still_schedules_but_respawn_fails() {
        // 没有模板 (档案从未加载) 时, 会话层照样会排定 —— 由
        // `SessionSet::advance_watchers` 负责发现拉不起来并关掉自动拉起。
        let mut s = ended("acc1", RunOutcome::ConnectionClosed, fast_policy());
        // 刻意不 set_template
        assert!(s.note_exit(Instant::now()));
        assert!(s.begin_respawn(None).is_none(), "没模板就不能拉起");
    }

    // ── spawn_one 的监督语义 (真的跑一次 bot) ───────────────────────
    //
    // 这两条需要"能连上但立刻断"的假服务器: 它让 `spawn_one` 走完
    // spawn → 结束 → 写回 的完整路径, 从而验证 stage / outcome / panic 标志
    // 这三样在真实结束时都被正确写下 (拉起逻辑全靠它们)。

    /// 只在握手后立刻断开的假服务器, 返回 `(端口, 已接受连接数)`。
    ///
    /// 必须**先读写一次再关**: 握手完就立刻关的话, 客户端连登录包都发不出去,
    /// `runtime::run` 会在 `send_packet` 上失败 —— 那是另一条分支, 覆盖不到
    /// "连接建立后掉线"。
    async fn fake_server(accepts: usize) -> (u16, Arc<AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let list = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = list.local_addr().unwrap().port();
        let count = Arc::new(AtomicUsize::new(0));
        let c2 = count.clone();
        tokio::spawn(async move {
            for _ in 0..accepts {
                let Ok((mut sock, _)) = list.accept().await else {
                    break;
                };
                c2.fetch_add(1, Ordering::SeqCst);
                // 16 字节握手 (内容无关紧要, 只要长度对)
                if sock.write_all(&[0u8; 16]).await.is_err() {
                    continue;
                }
                // 等客户端的首个包到达再断开
                let mut buf = [0u8; 64];
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(2), sock.read(&mut buf))
                        .await;
                let _ = sock.shutdown().await;
                drop(sock);
            }
        });
        (port, count)
    }

    fn cfg_at(port: u16) -> Config {
        Config {
            ip: "127.0.0.1".to_string(),
            port,
            account: "test".to_string(),
            password: "test".to_string(),
            ..Config::default()
        }
    }

    /// 等会话结束 (最多 `secs` 秒)。
    async fn wait_finished(s: &BotSession, secs: u64) -> bool {
        for _ in 0..(secs * 20) {
            if s.task_finished() {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn spawn_one_writes_outcome_then_finishes() {
        let (port, accepts) = fake_server(1).await;
        let (s, rx) = BotSession::new("acc1", vec![]);
        let _jh = crate::sessions::spawn_one(
            &tokio::runtime::Handle::current(),
            cfg_at(port),
            rx,
            s.arcs(),
        );
        assert!(s.is_running(), "spawn 之后必须立刻是运行中");
        assert!(!s.is_finished());
        assert!(wait_finished(&s, 20).await, "假服务器断开后必须结束");
        assert_eq!(accepts.load(Ordering::SeqCst), 1, "必须真的连过一次");
        // 结束时: 阶段已置位 **且** 结束原因已写好 (顺序在 spawn_one 里保证)
        assert!(s.is_finished());
        assert!(s.outcome().is_some(), "看到 FINISHED 时必须能读到原因");
    }

    /// 一个"一收到事件就 panic"的 emitter。用来在真实 `spawn_with_emitter`
    /// 路径上制造 bot 崩溃。
    ///
    /// 只在 `#[ignore]` 的测试里用 —— 让 bot 崩会污染测试进程的 panic hook
    /// (输出里多出一条无法归属的 "boom"), 而它想验的那条分支已经由
    /// `sessions::tests::classify_join_flags_a_panic` +
    /// `classify_join_treats_abort_as_a_clean_stop` 精确覆盖 (纯函数, 无副作用)。
    #[allow(dead_code)]
    struct Boom;

    impl openstory_bot::emit::Emitter for Boom {
        fn emit(&self, _ev: openstory_bot::emit::Event) {
            panic!("boom (测试用)");
        }
    }

    /// 假服务器: 接受连接、发 16 字节握手, 之后**只读不回**。
    ///
    /// 于是客户端能建链、能发登录包, 然后永远卡在等服务端回复上 —— 正好是
    /// "abort 一个正在运行的会话" 需要的那种稳定可中断状态。
    /// 返回 `(端口, 已接受连接数)`。
    async fn fake_server_that_never_answers(accepts: usize) -> (u16, Arc<AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let list = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = list.local_addr().unwrap().port();
        let count = Arc::new(AtomicUsize::new(0));
        let c2 = count.clone();
        tokio::spawn(async move {
            for _ in 0..accepts {
                let Ok((mut sock, _)) = list.accept().await else {
                    break;
                };
                c2.fetch_add(1, Ordering::SeqCst);
                if sock.write_all(&[0u8; 16]).await.is_err() {
                    continue;
                }
                let mut buf = [0u8; 256];
                // 一直读, 但永不回复 —— 客户端会永久等下去
                while let Ok(n) = sock.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                }
            }
        });
        (port, count)
    }

    #[tokio::test]
    async fn spawn_one_abort_still_finishes_the_session() {
        // 退出流程会 abort 会话的 bot。此时**必须**仍然走完收尾 (置
        // `FINISHED`), 否则 UI 会一直显示"运行中", 而主线程的退出条件
        // (`live_count`) 会认为还有会话活着而永远不退出。
        let (port, accepts) = fake_server_that_never_answers(1).await;
        let (s, rx) = BotSession::new("acc1", vec![]);
        let _handle = crate::sessions::spawn_one(
            &tokio::runtime::Handle::current(),
            cfg_at(port),
            rx,
            s.arcs(),
        );
        assert!(s.is_running());
        // 等它真的连上 (这样才是"跑起来之后被 abort", 而不是"还没开始就 abort")
        for _ in 0..100 {
            if accepts.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(accepts.load(Ordering::SeqCst), 1, "必须真的连上过");
        {
            let slot = s.join_slot();
            let g = slot.lock().unwrap();
            let h = g.as_ref().expect("handle 应该在 spawn 后立刻入槽");
            h.abort();
        }
        assert!(wait_finished(&s, 10).await, "abort 之后也必须收尾");
        assert!(s.task_finished());
        assert!(
            !s.panicked.load(Ordering::SeqCst),
            "abort 是我们自己的动作, 不能算崩溃 (否则退出时会排队等拉起)"
        );
        assert_eq!(s.outcome(), Some(RunOutcome::Quit), "abort 归约为正常退出");
    }

    // ── 手动启停 (阶段 8) ─────────────────────────────────────────────

    #[test]
    fn a_fresh_session_is_never_started() {
        let (s, _rx) = BotSession::new("acc1", vec![]);
        assert!(s.never_started(), "刚建出来的会话是'从未启动'");
        assert!(!s.is_manually_stopped(), "但**不是**'已停止' —— 两者语义不同");
        assert!(!s.start_failed());
        // 单会话路径 (启动器自己 spawn) 用了 `from_channel_with_policy`, 它会
        // 走 disabled 策略; 这里只关心"从没跑过"这个判据。
    }

    #[test]
    fn prepare_start_clears_the_manual_stop_and_lets_it_be_never_started() {
        // 现场路径: 会话跑过 (代数 > 0) → 结束 → 用户 stop → 用户 start。
        // start 必须成功, 且"已停止"被清掉。
        let (mut s, _rx) = BotSession::new_with_policy(
            "acc1",
            vec![],
            crate::watcher::RestartPolicy::default(),
        );
        s.set_template(Config {
            ip: "127.0.0.1".into(),
            port: 1,
            account: "t".into(),
            password: "t".into(),
            ..Config::default()
        });
        assert!(s.begin_respawn(None).is_some(), "第一次拉起 → 代数 1");
        assert_eq!(s.generation(), 1);
        // bot 退了 (监督任务置 FINISHED)
        s.stage.store(STAGE_FINISHED, Ordering::SeqCst);
        // `mark_stopped_by_user` 返回 false = "刚才没有 task 在跑"; 但状态照样落到
        // "已停止" —— 已结束但亮着"需人工"的会话正是最需要被停掉的那一种。
        // (会话层 `stop_session` 会把这个 false 与"从未启动"区分开。)
        assert!(!s.mark_stopped_by_user(), "已结束的会话没有 task 在跑");
        assert!(s.is_manually_stopped());
        assert!(!s.never_started(), "'已停止'不该同时算'从未启动'");

        assert!(s.prepare_start(), "停掉的会话必须能重新启动");
        assert!(!s.is_manually_stopped(), "start 之后不该还是'已停止'");
    }

    #[test]
    fn a_stopped_but_still_running_session_cannot_be_started_again() {
        // 反例 (真 bug 的形状): 用户按 stop → bot 正在退出的那几百毫秒里再按
        // start。阶段仍是 RUNNING, 若只按阶段判断就会放行 —— 于是同一个账号
        // 同时有两个登录尝试, 服务器踢掉先来的那个, 用户看到的是"它自己在
        // 反复掉线"。
        let (mut s, _rx) = BotSession::new_with_policy(
            "acc1",
            vec![],
            crate::watcher::RestartPolicy::default(),
        );
        s.set_template(Config {
            ip: "127.0.0.1".into(),
            port: 1,
            account: "t".into(),
            password: "t".into(),
            ..Config::default()
        });
        assert!(s.begin_respawn(None).is_some());
        s.stage.store(STAGE_RUNNING, Ordering::SeqCst);
        assert!(s.mark_stopped_by_user());
        assert!(
            !s.prepare_start(),
            "bot 还在收尾时不能再启动 (会双重登录)"
        );

        // task 收尾之后就可以了 (闩只在"这只 task 还活着"期间有意义)
        s.stage.store(STAGE_FINISHED, Ordering::SeqCst);
        assert!(s.prepare_start(), "收尾完成之后必须能重新启动");
    }

    #[test]
    fn prepare_start_refuses_a_session_that_is_already_active() {
        // 重复 spawn 会让同一个账号登录两次, 服务器会踢掉先来的那个 ——
        // 必须挡住。
        let (mut s, _rx) = BotSession::new("acc1", vec![]);
        s.stage.store(STAGE_RUNNING, Ordering::SeqCst);
        assert!(!s.prepare_start(), "运行中的会话不该被再启动一次");

        // 等重连中的会话同样不该被再启动一次 (那会同时挂着两个登录尝试)
        let (mut s2, _rx2) = BotSession::new_with_policy(
            "acc2",
            vec![],
            crate::watcher::RestartPolicy::default(),
        );
        s2.watcher
            .on_exit(Some(RunOutcome::ConnectionClosed), false, Instant::now());
        assert!(s2.is_waiting_restart(), "前置条件: 已排定拉起");
        assert!(!s2.prepare_start(), "正在等重连的会话不该被再启动一次");
    }

    #[test]
    fn a_failed_start_stops_being_never_started() {
        // 关键回归: 启动失败必须让会话**不再**算"从未启动"。
        //
        // 手动模式下程序"没有号在跑、也没有号能自己起来"时才允许收尾; 若失败
        // 之后仍算"从未启动", `has_never_started()` 就一直为真 —— 用户按了一次
        // 失败的 start, 程序就再也不会自己退出了。
        let (mut s, _rx) = BotSession::new("acc1", vec![]);
        assert!(s.never_started());
        s.prepare_start();
        s.stop_manual_keep_attempts();
        s.mark_start_failed();
        assert!(s.start_failed());
        assert!(!s.never_started(), "启动失败之后不能再算'从未启动'");
        assert!(s.is_manually_stopped(), "失败之后是停着的");
        // 再次 prepare_start 要能把失败标记清掉 (用户补了密码再试一次)
        s.set_template(Config {
            ip: "127.0.0.1".into(),
            port: 1,
            account: "t".into(),
            password: "t".into(),
            ..Config::default()
        });
        assert!(s.prepare_start(), "补上配置之后应该可以再试");
        assert!(!s.start_failed(), "重试要清掉上一次的失败标记");
    }

    #[test]
    fn mark_stopped_by_user_reports_whether_it_was_active() {
        let (mut s, _rx) = BotSession::new("acc1", vec![]);
        s.stage.store(STAGE_RUNNING, Ordering::SeqCst);
        assert!(s.mark_stopped_by_user(), "在跑 → true");
        assert!(!s.restart_enabled(), "stop 必须关掉自动拉起");

        // 没在跑 → false, 调用方据此不给"已停止"的假反馈
        let (mut s2, _rx2) = BotSession::new("acc2", vec![]);
        assert!(!s2.mark_stopped_by_user(), "没在跑 → false");
    }
}
