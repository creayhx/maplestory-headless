//! 崩溃自动拉起: 检测会话异常结束 → 退避重启。
//!
//! # 什么算"崩溃", 什么不算
//!
//! 这个区别是整套策略的关键 —— 分错了会造成**重启风暴**。
//!
//! | 结束原因 | 判定 | 处理 |
//! |---|---|---|
//! | task panic (`JoinError::is_panic`) | 崩溃 | 退避重启 |
//! | `RunOutcome::Failed` (登录失败/重连耗尽) | 认证或环境问题 | **不重启**, 标记「需人工」 |
//! | `RunOutcome::ConnectFailed` | 服务器不可达 | 退避重启 (服务器重启的场景) |
//! | `RunOutcome::ConnectionClosed` | 断线 | 退避重启 |
//! | `RunOutcome::Quit` | 用户主动退出 | 不重启 |
//! | `RunOutcome::DurationElapsed` | `--duration` 到点 | 不重启 |
//!
//! `Failed` 刻意不重启: 密码错 / 账号被顶 这类问题重试一万次也是同样结果,
//! 而 5 秒一轮的重试会把自己的账号刷成风控对象。宁可标记出来让人看一眼。
//!
//! # 退避
//!
//! `5s → 15s → 45s`, 最多 3 次; 之后标记「已放弃」(左栏亮红) 不再尝试。
//! 手动重启 ([`Watcher::reset`]) 清零计数。
//!
//! 这个模块是**纯状态机**, 不碰 tokio / 不碰 UI —— 时间由调用方传入,
//! 因此退避与放弃逻辑可以用测试完全覆盖 (最重要的一条: 不能无限重启)。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use openstory_bot::runtime::RunOutcome;

/// 默认退避序列 (秒)。
pub const DEFAULT_BACKOFF_SECS: &[u64] = &[5, 15, 45];

/// 一个"已排定但从未触发"的等待窗口, 超过这个时长就认为它已经失效。
///
/// 正常情况下窗口一定会在几秒内到点并被 `poll_ready` 消费掉。超过这个时长
/// 还挂在那里, 说明那次拉起**根本没发生** (调用方拿了 token 却没 spawn, 或者
/// 进程当时正忙)。没有这条保护的话那个窗口会永远挡住后续的掉线 —— 会话卡在
/// "准备重连"却什么也不会发生, 而 UI 还显示着一个倒计时。
///
/// 60 秒: 比最长的一档退避 (45s) 长, 因此绝不会误伤正常的等待窗口。
pub const PENDING_WINDOW_STALE: Duration = Duration::from_secs(60);

/// 会话结束的"种类" —— 决定要不要拉起。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    /// 用户主动退出 / duration 到点 —— 正常结束
    Normal,
    /// 崩溃 (task panic)
    Panic,
    /// 登录失败 / 重连耗尽 —— 认证或环境问题, 重试无意义
    AuthFailed,
    /// 连接失败 / 断线 —— 服务器可能只是重启, 值得重试
    Disconnected,
    /// 用户**手动停掉**了这个会话 (控制台的 stop 操作)。
    ///
    /// 与 [`ExitKind::Normal`] 分开是必须的, 两者在"恢复自动拉起"这一件事上
    /// 要求相反的行为:
    /// - `Normal`: 用户按了 quit, 之后掉线照样该拉起 —— 所以 `reset` 要清掉它。
    /// - `Manual`: 用户明确要这个号停着, 之后它**结束多少次都不该被拉起** ——
    ///   `reset` 必须原样留着它, 否则下一次自动拉起会立刻把它唤醒。
    Manual,
}

impl ExitKind {
    /// 从 `runtime::run` 的返回值判定。
    pub fn from_outcome(o: RunOutcome) -> Self {
        match o {
            RunOutcome::Quit | RunOutcome::DurationElapsed => ExitKind::Normal,
            RunOutcome::Failed => ExitKind::AuthFailed,
            RunOutcome::ConnectFailed | RunOutcome::ConnectionClosed => ExitKind::Disconnected,
        }
    }

    /// 是否值得自动拉起。
    pub fn should_restart(self) -> bool {
        matches!(self, ExitKind::Panic | ExitKind::Disconnected)
    }
}

/// 会话当前的自动拉起状态。
///
/// 只有 `Waiting` 一个"进行中"的状态, `attempts` / `fired` 都记在这里。
/// **尝试次数不放在这个枚举里**: 它是跨"拉起成功"活下来的计数器 (见
/// [`Watcher::attempts`]), 放进一个会被清掉的状态里就必然在某条路径上丢。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartState {
    /// 没有待处理的拉起 (还没失败过 / 已重新武装 / 已 reset)
    Idle,
    /// 等待退避到点后拉起。
    ///
    /// * `attempt` — 这是第几次拉起 (1-based), 决定退避档位
    /// * `until` — 到点时刻
    /// * `fired` — 这个窗口是否已经触发过 (UI 每帧都 `poll_ready`, 没有它
    ///   就会每帧拉起一次)。会话真的被拉起后由 [`Watcher::rearm`] 清掉。
    Waiting {
        attempt: u32,
        until: Instant,
        fired: bool,
    },
    /// 已经放弃 (超过最大次数) —— UI 亮红
    GivenUp { attempts: u32 },
    /// 不该拉起 (正常结束 / 需人工)
    Stopped { kind: ExitKind },
}

/// 拉起策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartPolicy {
    pub enabled: bool,
    pub max_attempts: u32,
    /// 退避序列 (秒)。用完之后用最后一项 (但不会发生 —— 到 max 就放弃了)。
    pub backoff_secs: Vec<u64>,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 3,
            backoff_secs: DEFAULT_BACKOFF_SECS.to_vec(),
        }
    }
}

impl RestartPolicy {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    /// 第 `attempt` 次尝试 (1-based) 之前要等多久。
    ///
    /// 超出序列长度时沿用最后一项 —— 但正常情况下到不了 (attempt 受
    /// `max_attempts` 限制)。空序列返回 0 (不等待), 避免 panic。
    pub fn backoff_for(&self, attempt: u32) -> Duration {
        if self.backoff_secs.is_empty() {
            return Duration::ZERO;
        }
        let idx = (attempt.saturating_sub(1) as usize).min(self.backoff_secs.len() - 1);
        Duration::from_secs(self.backoff_secs[idx])
    }
}

/// 一个会话的拉起状态机。
#[derive(Debug, Clone)]
pub struct Watcher {
    pub policy: RestartPolicy,
    state: RestartState,
    /// 连续排定过多少次拉起 (1-based 的"下一次是第几次")。
    ///
    /// 这是**唯一**决定退避档位与放弃时机的计数器, 且刻意**不随拉起成功清零**:
    /// 一个反复掉线的号必须越等越久 (5s → 15s → 45s) 并最终放弃。每次拉起都从
    /// 5 秒重来的话, 退避形同虚设 —— 那会变成每 5 秒踢一次服务器的重连风暴。
    /// 只有用户手动重启 ([`Self::reset`]) 才清零。
    attempts: u32,
    /// 累计重启成功次数 (诊断用)
    total_restarts: u32,
    /// 构造时传进来的 `enabled` 原值。
    ///
    /// [`Self::stop_manual`] 会把 `policy.enabled` 压成 `false`, 之后
    /// [`Self::resume`] 要还原成**用户原本的配置**而不是一律 `true` ——
    /// `--no-restart` 起的会话不该因为"停一次再启动"就获得自动拉起能力。
    policy_enabled_at_construction: bool,
}

impl Watcher {
    pub fn new(policy: RestartPolicy) -> Self {
        Self {
            policy_enabled_at_construction: policy.enabled,
            policy,
            state: RestartState::Idle,
            attempts: 0,
            total_restarts: 0,
        }
    }

    pub fn state(&self) -> &RestartState {
        &self.state
    }

    /// 已连续排定过多少次拉起 (0 = 还没失败过)。
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    pub fn policy(&self) -> &RestartPolicy {
        &self.policy
    }

    /// 运行期开关自动拉起 (用户切换"记住密码"时用)。
    ///
    /// 关掉时**不**动已有状态: 如果正在等待, 那个等待窗口仍然有效 ——
    /// 用户按开关的瞬间不该把已经排定的一次拉起吞掉。
    pub fn set_enabled(&mut self, enabled: bool) {
        self.policy.enabled = enabled;
    }

    pub fn total_restarts(&self) -> u32 {
        self.total_restarts
    }

    /// 会话结束了, 喂进来看该怎么处理。
    ///
    /// * `outcome` — `None` 表示"这次调用没有新的结局信息", 只用来推进状态
    ///   (幂等轮询走这条)。`Some` 表示真的结束了一次, 会重新判定要不要拉起。
    /// * `panicked` — task 是否 panic 结束 (来自 `JoinError::is_panic`)。
    ///   注意 `RunOutcome` 本身表达不了 panic (panic 的任务没有返回值),
    ///   所以这个信息必须由调用方分开传。
    /// * `now` — 当前时刻 (注入以便测试)
    ///
    /// 返回 `true` 表示"已排定一次重启", 调用方据此在到点后真正 spawn。
    ///
    /// # 两道守卫, 各自解决一个真问题
    ///
    /// 1. **未触发的窗口 → 直接返回 `false`** (幂等)。UI 每帧都调它, 没有
    ///    这条窗口会被每帧往后推, 退避永远走不到头。窗口过期时放行
    ///    (见 [`PENDING_WINDOW_STALE`])。
    /// 2. **已触发的窗口 → 先清掉再重新判定**。会话被拉起之后可能**正常
    ///    退出** (用户 `quit` / `--duration` 到点), 那时必须把等待清掉并落到
    ///    `Stopped`; 留着的话会话会被永远算作"还会再动", 主线程不肯退出,
    ///    UI 也一直显示一个不会发生的倒计时。
    pub fn on_exit(&mut self, outcome: Option<RunOutcome>, panicked: bool, now: Instant) -> bool {
        match &self.state {
            // 守卫 1: 还没触发的窗口
            RestartState::Waiting {
                fired: false,
                until,
                ..
            } => {
                if now < *until + PENDING_WINDOW_STALE {
                    return false;
                }
                // 过期了 → 当作"那次拉起没发生", 继续往下重新排定
            }
            // 守卫 2: 已触发的窗口已经完成使命, 结局由这次调用重新判定
            RestartState::Waiting { fired: true, .. } => {
                self.state = RestartState::Idle;
            }
            // 守卫 3: 用户手动停掉的会话, 无论它之后以什么方式结束都不该被拉起。
            //
            // 覆盖的是这条真实路径: 用户 stop → `quit` 指令送到 → bot 那边可能
            // 先是"连接已关闭"(`Disconnected`, 本该拉起), 然后才真正退出。
            // 没有这条守卫, 那一次 Disconnected 就会排定一次拉起, 把用户刚停掉
            // 的号在 5 秒后自己唤醒 —— UI 上表现为"停了又自己起来"。
            RestartState::Stopped {
                kind: ExitKind::Manual,
            } => return false,
            _ => {}
        }
        // 没有新的结局信息 → 只清理状态, 不排定新的拉起
        let Some(outcome) = outcome else {
            return false;
        };
        let kind = if panicked {
            ExitKind::Panic
        } else {
            ExitKind::from_outcome(outcome)
        };
        if !self.policy.enabled || !kind.should_restart() {
            self.state = RestartState::Stopped { kind };
            return false;
        }
        // 到上限了 (注意用跨拉起存活的 `attempts`, 不是当前窗口的 attempt)
        let done = self.attempts;
        if done >= self.policy.max_attempts {
            self.state = RestartState::GivenUp { attempts: done };
            return false;
        }
        let attempt = done + 1;
        self.attempts = attempt;
        self.state = RestartState::Waiting {
            attempt,
            until: now + self.policy.backoff_for(attempt),
            fired: false,
        };
        true
    }

    /// 到点了吗? 到点则**只返回一次** `true` (调用方去 spawn)。
    ///
    /// 幂等性是必须的: UI 每帧都调用它, 如果每次都返回 `true` 就会每帧重启。
    /// 触发后停在 `Waiting { fired: true }` (倒计时归零), 等会话真的再次结束
    /// (`on_exit`) 才推进到下一次退避。
    pub fn poll_ready(&mut self, now: Instant) -> bool {
        match &mut self.state {
            RestartState::Waiting { until, fired, .. } => {
                if *fired || now < *until {
                    return false;
                }
                *fired = true;
                self.total_restarts += 1;
                true
            }
            _ => false,
        }
    }

    /// 距下次拉起还有多久 (UI 显示倒计时)。非 `Waiting` 时返回 `None`。
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        match &self.state {
            RestartState::Waiting { until, .. } => Some(until.saturating_duration_since(now)),
            _ => None,
        }
    }

    /// 手动重启: 清零计数, 状态回到 `Idle`。
    ///
    /// **`Stopped { Manual }` 原样保留**: 那是用户刻意停掉的状态, 清零它是
    /// 反的 —— 下一次掉线就会被拉起。要重新开始就跑 [`Self::resume`]。
    pub fn reset(&mut self) {
        if let RestartState::Stopped {
            kind: ExitKind::Manual,
        } = self.state
        {
            return;
        }
        self.state = RestartState::Idle;
        self.attempts = 0;
    }

    /// 用户手动停掉本会话 (控制台 stop)。
    ///
    /// 关掉自动拉起 **并且**落到 `Stopped { Manual }` —— 光关掉 `enabled`
    /// 不够: 用户之后重新打开它 (或 `reset`) 时会按 `Stopped { kind }` 里的
    /// 原因重新判定, 而原因若是 `Disconnected` 就会立刻排定一次拉起。
    ///
    /// 计数一并清零: 用户主动停一次的语义是"从头开始", 下次掉线仍从第一档
    /// 退避起算。
    pub fn stop_manual(&mut self) {
        self.stop_manual_keep_attempts();
        self.attempts = 0;
    }

    /// 同 [`Self::stop_manual`], 但保留退避计数 (拉起失败时用)。
    pub fn stop_manual_keep_attempts(&mut self) {
        self.state = RestartState::Stopped {
            kind: ExitKind::Manual,
        };
        self.policy.enabled = false;
    }

    /// 从"手动停掉"恢复: 重新打开自动拉起, 状态回到 `Idle`。
    ///
    /// 恢复成**用户配置的原值**而不是一律打开: 用 `--no-restart` 或
    /// `RestartPolicy::disabled()` 起的会话, 不该因为"停了一次再启动"就
    /// 悄悄获得自动拉起能力。
    ///
    /// 返回是否真的从手动停止状态恢复了 —— 呼在别的状态上是 no-op。
    pub fn resume(&mut self) -> bool {
        if !matches!(
            self.state,
            RestartState::Stopped {
                kind: ExitKind::Manual
            }
        ) {
            return false;
        }
        self.state = RestartState::Idle;
        self.attempts = 0;
        self.policy.enabled = self.policy_enabled_at_construction;
        true
    }

    /// 是否处于"用户手动停掉"的状态 (UI 显示"已停止"而不是"需人工")。
    pub fn is_manually_stopped(&self) -> bool {
        matches!(
            self.state,
            RestartState::Stopped {
                kind: ExitKind::Manual
            }
        )
    }

    /// 会话真的被重新拉起之后调用: 结束当前窗口, 准备迎接下一次掉线。
    ///
    /// **必须清掉窗口, 不能只清 `fired`**: 清窗口才能让新的掉线登记进来
    /// (否则那个已到点的窗口会一直当"待触发"而挡住一切)。而尝试次数
    /// ([`Self::attempts`]) 不在这里动 —— 见它的文档: 退避必须跨拉起累加。
    ///
    /// 没在等待时是 no-op: 手动重启一个还在退避中的会话不该把等待吞掉。
    pub fn rearm(&mut self) {
        if matches!(self.state, RestartState::Waiting { fired: true, .. }) {
            self.state = RestartState::Idle;
        }
    }

    /// 已在等待拉起。
    pub fn is_waiting(&self) -> bool {
        matches!(self.state, RestartState::Waiting { .. })
    }

    /// 当前等待窗口是否已经触发过 (UI 把倒计时显示成"正在重连")。
    pub fn is_waiting_restart_fired(&self) -> bool {
        matches!(self.state, RestartState::Waiting { fired: true, .. })
    }

    /// 已放弃 (UI 亮红)。
    pub fn given_up(&self) -> bool {
        matches!(self.state, RestartState::GivenUp { .. })
    }

    /// 需要人工处理。
    ///
    /// 三类都算, 少任何一类都会让"要不要过去看一眼"这个提示漏掉一种情况:
    /// - `GivenUp` —— 试满上限放弃了
    /// - `Stopped { AuthFailed }` —— 登录失败 / 重连耗尽
    /// - `Stopped { Disconnected | Panic }` —— **本该拉起却没拉成**
    ///   (没有配置模板 / 自动拉起被关掉 / 拉起失败)。会话已经停了, 而没有任何
    ///   东西会再动它 —— 这正是最需要人看一眼的情况, 不能显示成"已结束"了事。
    ///
    /// `Stopped { Normal }` (用户 quit / duration 到点) 刻意**不算**: 那是用户
    /// 自己要停, 报警只会制造噪音。
    ///
    /// `Stopped { Manual }` 同样不算 —— 是用户自己按的 stop, 点名报警毫无意义。
    pub fn needs_attention(&self) -> bool {
        match &self.state {
            RestartState::GivenUp { .. } => true,
            RestartState::Stopped { kind } => {
                !matches!(kind, ExitKind::Normal | ExitKind::Manual)
            }
            _ => false,
        }
    }
}

// ---------------------------------------------------------------- 进程级句柄
//
// 放在这个模块 (而不是 `main.rs`) 有具体原因: TUI 是纯 bin crate, 测试用
// `#[path]` 把 `src/*.rs` 当模块 include 进来跑 —— 凡是测试要碰的东西都必须
// 住在 `src/` 的某个模块里, `main.rs` 本身 include 不了。
//
// 这三个东西都不属于"会话的数据模型" (那是 `SessionSet` 的事), 而是**进程级**
// 的接线: runtime handle / 退出条件 / 错误累计。分开放才能让 `SessionSet`
// 不依赖 tokio runtime。

/// 自动拉起相关计数 (渲染线程与主线程共享)。
///
/// * `spawned` — 累计重新拉起次数 (诊断用)
/// * `errors`  — 过程中出现的需要用户知道的错误 (非空 → 退出码 1)
#[derive(Clone, Default)]
pub struct RestartCounters {
    pub spawned: Arc<AtomicUsize>,
    pub errors: Arc<StdMutex<Vec<String>>>,
}

impl RestartCounters {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一条错误 (渲染线程调用)。
    pub fn push_error(&self, msg: impl Into<String>) {
        if let Ok(mut e) = self.errors.lock() {
            e.push(msg.into());
        }
    }

    /// 第一条错误 (主线程退出前取)。
    pub fn first_error(&self) -> Option<String> {
        self.errors.lock().ok().and_then(|e| e.first().cloned())
    }

    /// 有没有出过错。
    pub fn has_errors(&self) -> bool {
        self.errors.lock().map(|e| !e.is_empty()).unwrap_or(false)
    }

    /// 累计拉起次数。
    pub fn spawned_count(&self) -> usize {
        self.spawned.load(Ordering::SeqCst)
    }
}

/// 渲染线程驱动"自动拉起"所需的全部接线。
///
/// 为什么拉起放在渲染线程: 会话集合 (`SessionSet`) 归 `App` 所有, 而 `App`
/// 被渲染线程独占。让主线程去拉起就必须把集合共享出来 (加锁 / 通道), 白白
/// 引入竞争; 而"每帧看一眼有没有到点"本来就在渲染循环里, 顺手就做了。
#[derive(Clone)]
pub struct RestartCtx {
    /// 重新 spawn 会话用的 runtime 句柄。
    pub handle: tokio::runtime::Handle,
    /// 还在跑 (或正在等重连) 的会话数 —— 归零 + `all_done` 才是真的结束。
    pub keepalive: Arc<AtomicUsize>,
    /// 没有任何会话在跑、也没有任何会话在等退避 → 置位, 主线程据此收尾。
    pub all_done: Arc<AtomicBool>,
    pub counters: RestartCounters,
    /// 凭据存储: 档案里没写 `login.password` 时, 拉起从这里补密码。
    ///
    /// 用 `Arc<Mutex<..>>` 而不是直接持有: 渲染线程 (拉起) 与主线程 (启动时
    /// 记下向导输入的密码 / 退出时落盘) 都要碰它。
    pub creds: Arc<StdMutex<crate::credentials::Credentials>>,
}

impl RestartCtx {
    pub fn new(handle: tokio::runtime::Handle, policy: RestartPolicy) -> Self {
        let _ = policy; // 策略在建会话时已各自下发; 这里只做记录
        Self::with_credentials(
            handle,
            crate::credentials::Credentials::in_memory(crate::credentials::Remember::No),
        )
    }

    /// 带凭据存储构造。
    pub fn with_credentials(
        handle: tokio::runtime::Handle,
        creds: crate::credentials::Credentials,
    ) -> Self {
        Self {
            handle,
            keepalive: Arc::new(AtomicUsize::new(0)),
            all_done: Arc::new(AtomicBool::new(false)),
            counters: RestartCounters::new(),
            creds: Arc::new(StdMutex::new(creds)),
        }
    }

    /// 取凭据存储的克隆句柄 (渲染线程 / 主线程各一份)。
    pub fn creds(&self) -> Arc<StdMutex<crate::credentials::Credentials>> {
        self.creds.clone()
    }

    /// 当前还活着的会话数。
    pub fn keepalive(&self) -> usize {
        self.keepalive.load(Ordering::SeqCst)
    }

    /// 主线程该收尾了吗。
    pub fn is_all_done(&self) -> bool {
        self.all_done.load(Ordering::SeqCst)
    }

    /// 全部结束 (会话层在 `live == 0` 时调用)。
    pub fn mark_all_done(&self) {
        self.all_done.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn exit_kind_classification() {
        assert_eq!(ExitKind::from_outcome(RunOutcome::Quit), ExitKind::Normal);
        assert_eq!(
            ExitKind::from_outcome(RunOutcome::DurationElapsed),
            ExitKind::Normal
        );
        assert_eq!(
            ExitKind::from_outcome(RunOutcome::Failed),
            ExitKind::AuthFailed
        );
        assert_eq!(
            ExitKind::from_outcome(RunOutcome::ConnectFailed),
            ExitKind::Disconnected
        );
        assert_eq!(
            ExitKind::from_outcome(RunOutcome::ConnectionClosed),
            ExitKind::Disconnected
        );
    }

    /// 最重要的一条: 认证失败**绝不**自动拉起 (否则密码错会刷成风控)。
    #[test]
    fn auth_failure_is_never_restarted() {
        let mut w = Watcher::new(RestartPolicy::default());
        assert!(!w.on_exit(Some(RunOutcome::Failed), false, t0()));
        assert!(w.needs_attention(), "认证失败必须标记需人工");
        assert!(!w.is_waiting());
        assert!(!w.given_up(), "这不是'放弃', 是'不该试'");
    }

    #[test]
    fn normal_exit_is_never_restarted() {
        for o in [RunOutcome::Quit, RunOutcome::DurationElapsed] {
            let mut w = Watcher::new(RestartPolicy::default());
            assert!(!w.on_exit(Some(o), false, t0()));
            assert!(!w.is_waiting());
            assert!(!w.needs_attention(), "正常退出不该报警");
            assert_eq!(
                *w.state(),
                RestartState::Stopped {
                    kind: ExitKind::Normal
                }
            );
        }
    }

    #[test]
    fn panic_and_disconnect_are_restarted() {
        for (o, panicked) in [
            (RunOutcome::Quit, true), // panic 优先于 outcome
            (RunOutcome::ConnectionClosed, false),
            (RunOutcome::ConnectFailed, false),
        ] {
            let mut w = Watcher::new(RestartPolicy::default());
            assert!(
                w.on_exit(Some(o), panicked, t0()),
                "{o:?} panicked={panicked}"
            );
            assert!(w.is_waiting());
        }
    }

    #[test]
    fn panic_beats_normal_outcome() {
        // 任务 panic 时 `run` 的返回值不可信 —— panic 标志优先
        let mut w = Watcher::new(RestartPolicy::default());
        assert!(w.on_exit(Some(RunOutcome::Quit), true, t0()));
        assert!(w.is_waiting());
    }

    #[test]
    fn disabled_policy_never_restarts() {
        let mut w = Watcher::new(RestartPolicy::disabled());
        assert!(!w.on_exit(Some(RunOutcome::ConnectionClosed), true, t0()));
        assert!(!w.is_waiting());
    }

    #[test]
    fn backoff_sequence_follows_policy() {
        let p = RestartPolicy::default();
        assert_eq!(p.backoff_for(1), Duration::from_secs(5));
        assert_eq!(p.backoff_for(2), Duration::from_secs(15));
        assert_eq!(p.backoff_for(3), Duration::from_secs(45));
        // 超出序列沿用最后一项 (不会 panic)
        assert_eq!(p.backoff_for(99), Duration::from_secs(45));
    }

    #[test]
    fn backoff_for_zero_and_empty_policy() {
        let p = RestartPolicy::default();
        // attempt=0 不应 panic (saturating_sub)
        assert_eq!(p.backoff_for(0), Duration::from_secs(5));
        let empty = RestartPolicy {
            backoff_secs: vec![],
            ..RestartPolicy::default()
        };
        assert_eq!(empty.backoff_for(3), Duration::ZERO);
    }

    /// 退避时间真的按序列递增 (第一格最快, 后面变慢)。
    #[test]
    fn first_backoff_is_the_shortest() {
        let p = RestartPolicy::default();
        assert!(p.backoff_for(1) < p.backoff_for(2));
        assert!(p.backoff_for(2) < p.backoff_for(3));
    }

    #[test]
    fn restart_happens_once_per_backoff_window() {
        // 轮询多次不能重复重启 —— UI 每帧都会 poll
        let mut w = Watcher::new(RestartPolicy::default());
        let now = t0();
        assert!(w.on_exit(Some(RunOutcome::ConnectionClosed), false, now));
        // 还没到点
        assert!(!w.poll_ready(now));
        assert!(!w.poll_ready(now + Duration::from_secs(1)));
        // 到点: 只返回一次 true
        assert!(w.poll_ready(now + Duration::from_secs(6)));
        assert!(!w.poll_ready(now + Duration::from_secs(6)));
        assert!(!w.poll_ready(now + Duration::from_secs(7)));
    }

    /// 三次退避之后必须放弃 —— 不能无限重启。
    #[test]
    fn gives_up_after_max_attempts() {
        let mut w = Watcher::new(RestartPolicy::default());
        let mut now = t0();
        for i in 1..=3u32 {
            // 每次都以"断线"结束
            assert!(
                w.on_exit(Some(RunOutcome::ConnectionClosed), false, now),
                "第 {i} 次应排定重启"
            );
            let wait = w.remaining(now).expect("应在等待");
            assert!(wait > Duration::ZERO);
            now += wait + Duration::from_millis(1);
            assert!(w.poll_ready(now), "第 {i} 次应到点");
        }
        // 第 4 次结束: 已放弃
        assert!(!w.on_exit(Some(RunOutcome::ConnectionClosed), false, now));
        assert!(w.given_up());
        assert!(w.needs_attention());
        assert_eq!(w.total_restarts(), 3);
    }

    #[test]
    fn backoff_grows_across_attempts() {
        let mut w = Watcher::new(RestartPolicy::default());
        let now = t0();
        let mut waits = Vec::new();
        assert!(w.on_exit(Some(RunOutcome::ConnectionClosed), false, now));
        waits.push(w.remaining(now).unwrap());
        let mut cur = now;
        for _ in 0..2 {
            cur += *waits.last().unwrap() + Duration::from_millis(1);
            assert!(w.poll_ready(cur));
            assert!(w.on_exit(Some(RunOutcome::ConnectionClosed), false, cur));
            waits.push(w.remaining(cur).unwrap());
        }
        assert!(waits[0] < waits[1], "{waits:?}");
        assert!(waits[1] < waits[2], "{waits:?}");
    }

    #[test]
    fn reset_clears_the_give_up_state() {
        let mut w = Watcher::new(RestartPolicy::default());
        let mut now = t0();
        for _ in 0..3 {
            w.on_exit(Some(RunOutcome::ConnectionClosed), false, now);
            now += w.remaining(now).unwrap_or_default() + Duration::from_millis(1);
            w.poll_ready(now);
        }
        w.on_exit(Some(RunOutcome::ConnectionClosed), false, now);
        assert!(w.given_up());
        // 手动重启
        w.reset();
        assert!(!w.given_up());
        assert!(!w.needs_attention());
        assert_eq!(*w.state(), RestartState::Idle);
        // 之后又能排定重启了
        assert!(w.on_exit(Some(RunOutcome::ConnectionClosed), false, now));
    }

    #[test]
    fn remaining_is_none_when_not_waiting() {
        let mut w = Watcher::new(RestartPolicy::default());
        let now = t0();
        assert!(w.remaining(now).is_none(), "初始 Idle");
        w.on_exit(Some(RunOutcome::Failed), false, now);
        assert!(w.remaining(now).is_none(), "认证失败不等待");
    }

    #[test]
    fn remaining_counts_down_and_saturates() {
        let mut w = Watcher::new(RestartPolicy::default());
        let now = t0();
        w.on_exit(Some(RunOutcome::ConnectionClosed), false, now);
        let r0 = w.remaining(now).unwrap();
        let r1 = w.remaining(now + Duration::from_secs(2)).unwrap();
        assert!(r1 < r0);
        // 过点之后为 0 而不是 panic / 负数
        assert_eq!(
            w.remaining(now + Duration::from_secs(60)),
            Some(Duration::ZERO)
        );
    }

    // ── 窗口的收尾与重新武装 ────────────────────────────────────────

    /// 触发过的窗口必须能被"收尾": 会话被拉起之后**正常退出** (用户 quit),
    /// 不能还留着"在等重连"的状态 —— 那会算作"还会再动", 主线程不肯退出。
    #[test]
    fn a_normal_exit_after_a_restart_clears_the_window() {
        let mut w = Watcher::new(RestartPolicy::default());
        let t0 = Instant::now();
        assert!(w.on_exit(Some(RunOutcome::ConnectionClosed), false, t0));
        let at = t0 + w.remaining(t0).unwrap() + Duration::from_millis(1);
        assert!(w.poll_ready(at), "到点");
        assert!(w.is_waiting_restart_fired());
        // 拉起之后用户主动退出
        assert!(!w.on_exit(Some(RunOutcome::Quit), false, at));
        assert!(!w.is_waiting(), "正常退出必须把等待清掉");
        assert!(
            matches!(
                w.state(),
                RestartState::Stopped {
                    kind: ExitKind::Normal
                }
            ),
            "got {:?}",
            w.state()
        );
        assert!(!w.needs_attention(), "用户自己要停, 不该报警");
    }

    /// 触发过的窗口也必须能被**重新判定成可拉起**: 核心层把断线归约成
    /// `Failed` 时, 控制台层得接着重建会话, 而不是把已排定的拉起撤掉。
    #[test]
    fn a_disconnect_after_a_restart_schedules_the_next_attempt() {
        let mut w = Watcher::new(RestartPolicy::default());
        let t0 = Instant::now();
        assert!(w.on_exit(Some(RunOutcome::ConnectionClosed), false, t0));
        let first = w.remaining(t0).unwrap();
        let at = t0 + first + Duration::from_millis(1);
        assert!(w.poll_ready(at));
        // 新 bot 又断线
        assert!(w.on_exit(Some(RunOutcome::ConnectionClosed), false, at));
        assert!(w.is_waiting(), "必须排定下一次");
        assert!(!w.is_waiting_restart_fired(), "新窗口还没触发");
        let second = w.remaining(at).unwrap();
        assert!(
            second > first,
            "第二次退避必须更长: {first:?} vs {second:?}"
        );
        assert_eq!(w.attempts(), 2);
    }

    /// 尝试次数**不能**被拉起成功清零 —— 否则反复掉线的号会永远停在 5 秒
    /// 退避上, 而"最多 3 次然后放弃"也永远不会达成。
    #[test]
    fn attempt_counter_survives_restarts_so_backoff_grows() {
        let mut w = Watcher::new(RestartPolicy::default());
        let mut t = Instant::now();
        let mut waits = Vec::new();
        for i in 0..3u32 {
            assert!(
                w.on_exit(Some(RunOutcome::ConnectionClosed), false, t),
                "第 {i} 次"
            );
            waits.push(w.remaining(t).unwrap());
            t += *waits.last().unwrap() + Duration::from_millis(1);
            assert!(w.poll_ready(t), "第 {i} 次到点");
        }
        assert!(waits[0] < waits[1], "{waits:?}");
        assert!(waits[1] < waits[2], "{waits:?}");
        // 第 4 次失败 → 放弃 (max_attempts = 3)
        assert!(!w.on_exit(Some(RunOutcome::ConnectionClosed), false, t));
        assert!(w.given_up(), "跨多次拉起之后仍然必须能放弃");
        assert_eq!(w.total_restarts(), 3);
        assert_eq!(w.attempts(), 3);
    }

    #[test]
    fn rearm_clears_a_fired_window() {
        // `rearm` 是"手动拉起"路径的收尾钩子 (退出码/测试用); 正常路径下
        // 下一次 `on_exit` 自己就会清掉已触发的窗口。
        let mut w = Watcher::new(RestartPolicy::default());
        let t0 = Instant::now();
        assert!(w.on_exit(Some(RunOutcome::ConnectionClosed), false, t0));
        let at = t0 + w.remaining(t0).unwrap() + Duration::from_millis(1);
        assert!(w.poll_ready(at));
        w.rearm();
        assert!(!w.is_waiting_restart_fired());
        assert!(!w.is_waiting(), "已触发的窗口应被清成 Idle");
        assert_eq!(w.attempts(), 1, "尝试次数必须保留");
    }

    #[test]
    fn rearm_does_not_swallow_a_pending_unfired_wait() {
        // 手动重启一个还在退避中的会话: 不该把还没到点的等待吞掉
        let mut w = Watcher::new(RestartPolicy::default());
        let t0 = Instant::now();
        assert!(w.on_exit(Some(RunOutcome::ConnectionClosed), false, t0));
        let deadline = w.remaining(t0).unwrap();
        w.rearm();
        assert!(w.is_waiting(), "未触发的窗口不该被清掉");
        assert_eq!(w.remaining(t0).unwrap(), deadline);
        assert!(!w.poll_ready(t0), "不会被 rearm 提前触发");
    }

    #[test]
    fn rearm_is_a_noop_when_idle_or_given_up() {
        let mut w = Watcher::new(RestartPolicy::default());
        w.rearm();
        assert_eq!(*w.state(), RestartState::Idle);
        // 已放弃之后 rearm 不该悄悄复活它 (那要靠 reset)
        let mut t = Instant::now();
        for _ in 0..4 {
            w.on_exit(Some(RunOutcome::ConnectionClosed), false, t);
            t += Duration::from_secs(60);
            w.poll_ready(t);
        }
        assert!(w.given_up());
        w.rearm();
        assert!(w.given_up(), "rearm 不能把'已放弃'变成'可重试'");
    }

    #[test]
    fn poll_ready_is_false_for_all_non_waiting_states() {
        let now = t0();
        for outcome in [RunOutcome::Quit, RunOutcome::Failed] {
            let mut w = Watcher::new(RestartPolicy::default());
            w.on_exit(Some(outcome), false, now);
            assert!(!w.poll_ready(now + Duration::from_secs(999)), "{outcome:?}");
        }
        let mut idle = Watcher::new(RestartPolicy::default());
        assert!(!idle.poll_ready(now));
    }

    #[test]
    fn zero_max_attempts_gives_up_immediately() {
        let mut w = Watcher::new(RestartPolicy {
            max_attempts: 0,
            ..RestartPolicy::default()
        });
        assert!(!w.on_exit(Some(RunOutcome::ConnectionClosed), false, t0()));
        assert!(w.given_up());
    }

    #[test]
    fn alternates_between_restartable_and_auth_failure() {
        // 服务器重启 -> 拉起 -> 拉起后密码错 -> 停止并需人工
        let mut w = Watcher::new(RestartPolicy::default());
        let now = t0();
        assert!(w.on_exit(Some(RunOutcome::ConnectFailed), false, now));
        let n1 = now + w.remaining(now).unwrap() + Duration::from_millis(1);
        assert!(w.poll_ready(n1));
        assert!(!w.on_exit(Some(RunOutcome::Failed), false, n1));
        assert!(w.needs_attention());
        assert!(!w.is_waiting());
    }

    // ── 运行期开关 ──────────────────────────────────────────────────

    #[test]
    fn set_enabled_toggles_without_losing_a_pending_wait() {
        // 用户开关"记住密码"时不该把已经排定的一次拉起吞掉
        let mut w = Watcher::new(RestartPolicy::default());
        let now = t0();
        assert!(w.on_exit(Some(RunOutcome::ConnectionClosed), false, now));
        w.set_enabled(false);
        assert!(!w.policy().enabled);
        assert!(w.is_waiting(), "关掉开关不应清掉已有的等待窗口");
        assert!(
            w.poll_ready(now + Duration::from_secs(60)),
            "已排定的仍生效"
        );
        // 关掉之后新的结束不再排定
        w.reset();
        w.set_enabled(false);
        assert!(!w.on_exit(Some(RunOutcome::ConnectionClosed), false, now));
        assert!(!w.is_waiting());
    }

    #[test]
    fn disabled_policy_reports_stopped_not_given_up() {
        // 「没开自动拉起」与「重试太多次放弃了」是两件不同的事 —— 诊断时
        // 要能分清 (前者是配置, 后者是出了事)。
        let mut w = Watcher::new(RestartPolicy::disabled());
        assert!(!w.on_exit(Some(RunOutcome::ConnectionClosed), false, t0()));
        assert!(!w.given_up(), "关掉开关不等于放弃");
        assert!(matches!(
            w.state(),
            RestartState::Stopped {
                kind: ExitKind::Disconnected
            }
        ));
        // 但**确实需要人看一眼**: 会话因为掉线停了, 而没有东西会再动它。
        assert!(w.needs_attention(), "没拉起的掉线必须提示需人工");
    }

    /// 用户主动退出 (或 `--duration` 到点) 不该报警 —— 那是他自己要停。
    #[test]
    fn a_normal_stop_does_not_ask_for_attention() {
        for out in [RunOutcome::Quit, RunOutcome::DurationElapsed] {
            let mut w = Watcher::new(RestartPolicy::default());
            assert!(!w.on_exit(Some(out), false, t0()));
            assert!(!w.needs_attention(), "{out:?} 是用户自己要停, 不该报警");
        }
    }

    /// 认证失败要报警 (重试无意义, 只能人去改密码)。
    #[test]
    fn auth_failure_asks_for_attention() {
        let mut w = Watcher::new(RestartPolicy::default());
        assert!(!w.on_exit(Some(RunOutcome::Failed), false, t0()));
        assert!(w.needs_attention());
    }

    // ── 进程级句柄 (RestartCtx / RestartCounters) ────────────────────

    #[test]
    fn counters_start_empty_and_accumulate() {
        let c = RestartCounters::new();
        assert_eq!(c.spawned_count(), 0);
        assert!(!c.has_errors());
        assert!(c.first_error().is_none(), "没有错误时必须是 None");
        c.push_error("acc1: 档案未能加载");
        c.push_error("acc2: 另一个错");
        assert!(c.has_errors());
        // first_error 给的是**第一条** —— 主线程退出码看的是"有没有出过事",
        // 而人看的是最早那条原因 (`first_error` 比 `last` 更有诊断价值)。
        assert_eq!(c.first_error().as_deref(), Some("acc1: 档案未能加载"));
        c.spawned.fetch_add(3, Ordering::SeqCst);
        assert_eq!(c.spawned_count(), 3);
    }

    #[test]
    fn counters_clone_shares_state() {
        // 渲染线程与主线程各持一份 clone —— 必须是同一份数据
        let c = RestartCounters::new();
        let c2 = c.clone();
        c2.push_error("x");
        c2.spawned.fetch_add(1, Ordering::SeqCst);
        assert!(c.has_errors(), "clone 必须共享 errors");
        assert_eq!(c.spawned_count(), 1, "clone 必须共享 spawned");
    }

    #[test]
    fn ctx_keepalive_and_all_done_start_neutral() {
        // 没有 runtime 时也要能建 ctx (测试路径) —— 用 current handle 之外的
        // 办法: 直接起一个一次性 runtime。
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let ctx = RestartCtx::new(rt.handle().clone(), RestartPolicy::default());
        assert_eq!(ctx.keepalive(), 0);
        assert!(!ctx.is_all_done(), "刚建好时不能说'全部结束'");
        assert!(!ctx.counters.has_errors());
        ctx.keepalive.fetch_add(2, Ordering::SeqCst);
        assert_eq!(ctx.keepalive(), 2);
        ctx.mark_all_done();
        assert!(ctx.is_all_done());
        // clone 共享 (两个线程各持一份)
        let ctx2 = ctx.clone();
        ctx2.keepalive.fetch_sub(2, Ordering::SeqCst);
        assert_eq!(ctx.keepalive(), 0, "clone 必须共享 keepalive");
        assert!(ctx2.is_all_done());
    }

    // ── 手动启停 (阶段 8) ─────────────────────────────────────────────

    #[test]
    fn manual_stop_blocks_the_auto_restart_that_follows() {
        // 现场路径: 用户按 stop → 控制台发 `quit` → bot 那边**可能先报
        // "连接已关闭"**(Disconnected, 本该拉起) 然后才真的退出。
        // 没有守卫的话, 那一次 Disconnected 就在 5 秒后把它自己唤醒 ——
        // 用户看到的是"我停了它, 它自己又起来了"。
        let mut w = Watcher::new(RestartPolicy::default());
        w.stop_manual();
        assert!(w.is_manually_stopped());
        assert!(
            !w.on_exit(Some(RunOutcome::ConnectionClosed), false, Instant::now()),
            "手动停掉之后, 断线不该排定拉起"
        );
        assert!(!w.is_waiting(), "不能有任何等待窗口");
        assert!(w.is_manually_stopped(), "状态必须保持在'手动停'");
    }

    #[test]
    fn manual_stop_also_blocks_a_panic_restart() {
        // panic 是另一条路径 (kind 由 `panicked` 决定, 不走 from_outcome)。
        // 同样必须被手动停的守卫拦住。
        let mut w = Watcher::new(RestartPolicy::default());
        w.stop_manual();
        assert!(!w.on_exit(Some(RunOutcome::Quit), true, Instant::now()));
        assert!(!w.is_waiting());
        assert!(w.is_manually_stopped());
    }

    #[test]
    fn a_manually_stopped_session_does_not_ask_for_attention() {
        // 用户自己按的 stop, 点名报警毫无意义 —— "需人工"只留给不是用户造成的
        // 那几种情况。
        let mut w = Watcher::new(RestartPolicy::default());
        assert!(!w.needs_attention());
        w.stop_manual();
        assert!(
            !w.needs_attention(),
            "手动停掉的会话不该亮红说'需人工'"
        );
        // 对照: 认证失败才是"需人工"
        let mut w2 = Watcher::new(RestartPolicy::default());
        w2.on_exit(Some(RunOutcome::Failed), false, Instant::now());
        assert!(w2.needs_attention());
    }

    #[test]
    fn reset_does_not_revive_a_manually_stopped_session() {
        // `reset` 是"手动重启"(F3) 走的路径, 它会清掉 Stopped{Normal} —— 因为
        // 用户 quit 之后掉线照样该拉起。但 Stopped{Manual} **不能**被清:
        // 清了之后下一次掉线就会被自动拉起来, 用户按 stop 的意图被抹掉。
        let mut w = Watcher::new(RestartPolicy::default());
        w.stop_manual();
        w.reset();
        assert!(w.is_manually_stopped(), "reset 不该复活手动停掉的会话");
        assert!(!w.on_exit(Some(RunOutcome::ConnectionClosed), false, Instant::now()));

        // 对照: Stopped{Normal} 必须被 reset 清掉 (否则 F3 之后掉线不再拉起)
        let mut w2 = Watcher::new(RestartPolicy::default());
        w2.on_exit(Some(RunOutcome::Quit), false, Instant::now());
        assert!(!w2.is_manually_stopped());
        w2.reset();
        assert!(
            w2.on_exit(Some(RunOutcome::ConnectionClosed), false, Instant::now()),
            "正常退出 reset 之后掉线必须能排定拉起"
        );
    }

    #[test]
    fn resume_restores_the_policy_the_user_configured() {
        // `--no-restart` 起的会话不该因为"停一次再启动"就悄悄获得自动拉起能力。
        let mut w = Watcher::new(RestartPolicy::disabled());
        w.stop_manual();
        assert!(w.resume(), "从手动停恢复要返回 true");
        assert!(!w.policy().enabled, "--no-restart 必须仍然是关的");
        assert!(!w.is_manually_stopped());
        assert!(!w.on_exit(Some(RunOutcome::ConnectionClosed), false, Instant::now()));

        // 默认策略 (开着) 恢复之后必须真的能拉起
        let mut w2 = Watcher::new(RestartPolicy::default());
        w2.stop_manual();
        assert!(w2.resume());
        assert!(w2.policy().enabled);
        assert!(
            w2.on_exit(Some(RunOutcome::ConnectionClosed), false, Instant::now()),
            "恢复之后掉线要能排定拉起"
        );
    }

    #[test]
    fn resume_is_a_noop_on_any_other_state() {
        // 呼在别的状态上不该有任何副作用 —— 否则"按 start"会把一个正在退避的
        // 会话的计数清掉, 退避就形同虚设。
        let mut w = Watcher::new(RestartPolicy::default());
        assert!(!w.resume(), "Idle 上不该返回 true");
        let t0 = Instant::now();
        assert!(w.on_exit(Some(RunOutcome::ConnectionClosed), false, t0));
        let deadline = w.remaining(t0).unwrap();
        assert!(!w.resume(), "等待中不该返回 true");
        assert!(w.is_waiting(), "等待窗口不能被吞掉");
        assert_eq!(w.remaining(t0).unwrap(), deadline);
        assert_eq!(w.attempts(), 1, "计数不能被清");
    }

    #[test]
    fn stop_manual_keep_attempts_preserves_the_counter() {
        // 拉起失败时落到"已停止"要**保留**计数 —— 那是"还能不能再试"的依据。
        // 而用户主动停则清零 (语义是"从头开始")。
        let mut w = Watcher::new(RestartPolicy::default());
        w.on_exit(Some(RunOutcome::ConnectionClosed), false, Instant::now());
        assert_eq!(w.attempts(), 1);
        w.stop_manual_keep_attempts();
        assert_eq!(w.attempts(), 1, "拉起失败不该清计数");
        assert!(w.is_manually_stopped());
        assert!(!w.policy().enabled);

        let mut w2 = Watcher::new(RestartPolicy::default());
        w2.on_exit(Some(RunOutcome::ConnectionClosed), false, Instant::now());
        w2.stop_manual();
        assert_eq!(w2.attempts(), 0, "用户主动停 = 从头开始");
    }

    #[test]
    fn manual_stop_after_a_fired_window_still_wins() {
        // 顺序: 掉线 → 退避到点 (窗口 fired) → 用户在这时按 stop。
        // 之后 bot 真的结束报 `None`/Disconnected 都不该再排定拉起。
        let mut w = Watcher::new(RestartPolicy::default());
        let t0 = Instant::now();
        assert!(w.on_exit(Some(RunOutcome::ConnectionClosed), false, t0));
        let at = t0 + w.remaining(t0).unwrap() + Duration::from_millis(1);
        assert!(w.poll_ready(at), "退避到点");
        w.stop_manual();
        assert!(
            !w.on_exit(Some(RunOutcome::ConnectionClosed), false, at),
            "触发过的窗口被手动停掉之后也不能再拉起"
        );
        assert!(w.is_manually_stopped());
    }
}
