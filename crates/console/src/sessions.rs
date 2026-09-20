//! 会话集合: N 个 [`BotSession`] + 当前选中 + 批量目标解析。
//!
//! UI 只通过这里访问会话, 因此"作用于谁"这件事只有一处实现。
//!
//! # 指令目标语法
//!
//! ```text
//! hunt on                   → 当前选中会话
//! @serverA_100000003 hunt on  → 指定档案 (可用前缀, 如 @serverA)
//! @serverA hunt on            → 档案名前缀命中该服的全部会话
//! @all hunt on              → 所有会话
//! ```
//!
//! 解析与发送分离 ([`SessionSet::resolve`] 返回命中列表, 再由调用方决定是否
//! 真的发), 这样 UI 可以先显示"将命中 N 个会话"再确认。

#![allow(dead_code)] // 阶段 4 落地, 阶段 5 接入 UI 后自然消除

use std::path::PathBuf;

use crate::session::{BotSession, SessionSpec};

/// 控制台自己执行的内建指令 (不发给 bot)。
///
/// 与 [`Target`] 正交: `@all stop` 是"对全部会话执行 stop", `stop` 本身是
/// 内建指令、`@all` 是目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Builtin {
    /// 启动目标会话 (`start`)
    Start,
    /// 停止目标会话 (`stop`)
    Stop,
    /// 挂上一个新账号 (`profiles add <路径>`) —— 只对当前选中会话有意义, 不需要目标
    AddProfile(PathBuf),
    /// 摘掉一个账号 (`profiles remove <档案名>`)
    RemoveProfile(String),
    /// 列出已挂上的账号与各自状态 (`profiles list`)
    ListProfiles,
}

impl Builtin {
    /// 给用户看的说明 (日志里那句"结果如何")。
    pub fn describe(&self) -> &'static str {
        match self {
            Builtin::Start => "启动",
            Builtin::Stop => "停止",
            Builtin::AddProfile(_) => "挂上账号",
            Builtin::RemoveProfile(_) => "摘掉账号",
            Builtin::ListProfiles => "账号列表",
        }
    }

    /// 是否按目标批量执行。`start` / `stop` 是; 增删账号不是 (它们针对的是
    /// 集合本身, 不是某个会话)。
    pub fn is_per_session(&self) -> bool {
        matches!(self, Builtin::Start | Builtin::Stop)
    }
}

/// 按空白切词, 但**尊重双引号** —— `profiles add "profiles/a b.json"` 里的
/// 空格属于路径本身。
///
/// 为什么需要它: 档案路径可能带空格 (中文名 + 空格很常见)。用
/// `split_whitespace` 会把路径切成两半, 于是"加了 a 但报文件不存在"。
fn split_quoted(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in s.chars() {
        match c {
            '"' => in_quote = !in_quote,
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 一条指令的目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// 当前选中的会话。
    Selected,
    /// 全部会话。
    All,
    /// 档案名前缀 (大小写不敏感)。`@serverA` 命中 `serverA_*`。
    Prefix(String),
}

impl Target {
    /// 从首 token 解析目标。返回 `None` 表示这不是一个目标前缀 (没有 `@`)。
    pub fn parse(token: &str) -> Option<Target> {
        let rest = token.strip_prefix('@')?;
        if rest.eq_ignore_ascii_case("all") {
            return Some(Target::All);
        }
        if rest.is_empty() {
            // 裸 `@` 视为"全部"? 不 —— 语义不明时应报错而不是猜。
            return None;
        }
        Some(Target::Prefix(rest.to_string()))
    }

    /// 给用户看的说明。
    pub fn describe(&self) -> String {
        match self {
            Target::Selected => "当前会话".to_string(),
            Target::All => "全部会话".to_string(),
            Target::Prefix(p) => format!("档案名前缀 @{p}"),
        }
    }
}

/// 把一行输入拆成 `(目标, 余下指令)`。
///
/// - 没有 `@` 前缀 → `(Selected, 整行)`
/// - 有 `@xxx` → `(目标, 去掉首 token 的余下部分)`
/// - `@` 单独出现 (空目标) → `Err` (语义不明, 不能猜)
///
/// 注意 `@` 也可能出现在指令中间 (如充值口令), 因此只认**首 token**。
pub fn split_target(line: &str) -> Result<(Target, String), String> {
    // 两端空白都规整掉: 尾部空白不影响指令语义, 留着只会让日志/回显变得
    // 难以比对 (「操作/UI 行为与 TUI 一致」的验收里这点很关键)。
    let trimmed = line.trim();
    let Some(first) = trimmed.split_whitespace().next() else {
        return Ok((Target::Selected, String::new()));
    };
    if !first.starts_with('@') {
        return Ok((Target::Selected, trimmed.to_string()));
    }
    let Some(target) = Target::parse(first) else {
        return Err(format!("目标 `{first}` 无效 (写法: @all / @<档案名前缀>)"));
    };
    let rest = trimmed[first.len()..].trim().to_string();
    Ok((target, rest))
}

/// 启动一个会话所需的全部分布式句柄。
///
/// 提成别名是为了让 spawn_one 与调用点不必重复写五层泛型。
///
/// 顺序与字段含义 (改这里必须同时改 `BotSession::arcs`, 编译器会抓):
/// 0. 事件队列 (该会话的事件只进这里 —— G1)
/// 1. 游戏状态
/// 2. 阶段原子量
/// 3. 结束原因槽
/// 4. panic 标志
/// 5. `JoinHandle` 存放位
pub type SessionArcs = (
    std::sync::Arc<openstory_console_lib::eventq::EventQueue>,
    std::sync::Arc<tokio::sync::Mutex<openstory_bot::state::BotState>>,
    std::sync::Arc<std::sync::atomic::AtomicU8>,
    std::sync::Arc<std::sync::Mutex<Option<openstory_bot::runtime::RunOutcome>>>,
    crate::session::PanicFlag,
    crate::session::JoinSlot,
);

/// 多开: 由全局配置 + **该档案自己的** `login` 节构造一个会话配置。
///
/// 存在的理由是把"每个号用自己的账号"收成一处。从前启动路径里有几份手写的
/// `let mut c = config.clone(); ...; c.apply_login(&info)`, 而 `apply_login`
/// 是**填空缺**语义 —— 全局 config 里的账号属于上一个号, 于是每个会话都继承了
/// 它, 两个号登成同一个 (服务器把两边来回踢)。
///
/// 语义细节见 [`openstory_bot::config::Config::apply_login_for_profile`]。
pub fn session_template(
    base: &openstory_bot::config::Config,
    path: &std::path::Path,
) -> openstory_bot::config::Config {
    let mut c = base.clone();
    c.config_paths = vec![path.display().to_string()];
    if let Some(info) = openstory_bot::login::load(path) {
        c.apply_login_for_profile(&info);
    }
    c
}

/// 一批会话。
pub struct SessionSet {
    sessions: Vec<BotSession>,
    selected: usize,
    /// 每个会话的指令接收端 (与 sessions 同序)。
    ///
    /// 存成集合的一部分而不是散落在各处: 接收端属于会话, 谁启动就取谁的。
    /// spawn_all 会把它们移进各自的 future。
    rxs: Vec<tokio::sync::mpsc::Receiver<String>>,
    /// 该写进**渲染缓冲**的事件 (选中会话的日志在 `App` 那边, 不在集合里)。
    ///
    /// 见 [`SessionSet::drain_all`]: 不攒起来的话, 选中会话的新日志会写进一个
    /// 已经被换出去的旧缓冲, 永远不上屏。
    render_pending: Vec<openstory_bot::emit::Event>,
    /// 选中会话的日志缓冲是否已被外部接管 (渲染位置)。
    ///
    /// `App` 在挂上集合时开启: 它用 `swap` 把选中会话的 `LogBuf` 换到渲染位置,
    /// 于是那个会话留在这里的 `s.log` 是一份**被换出去的旧缓冲**, 往里写的东西
    /// 永远不上屏。开启后 [`Self::drain_all`] / [`Self::note_at`] 会把选中会话的
    /// 内容攒进 `render_pending`, 由 `App::drain_all_sessions` flush 回渲染缓冲。
    ///
    /// 独立使用 `SessionSet` 的调用方 (联调测试 / 远端驱动) 保持关闭 —— 那时没有
    /// "渲染位置"这回事, 一切照旧写进各自的 `s.log`。
    render_taken_over: bool,
}

impl SessionSet {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            selected: 0,
            rxs: Vec::new(),
            render_pending: Vec::new(),
            render_taken_over: false,
        }
    }

    /// 按描述列表建立会话 (尚未启动)。
    ///
    /// 每个会话的指令接收端存进集合自身 ([`Self::spawn_all`] 会消费它们);
    /// 需要自己驱动某个会话时用 [`Self::take_receiver`]。
    pub fn from_specs(specs: &[SessionSpec]) -> Self {
        Self::from_specs_with_policy(specs, crate::watcher::RestartPolicy::disabled())
    }

    /// 同 [`Self::from_specs`], 但指定自动拉起策略。
    pub fn from_specs_with_policy(
        specs: &[SessionSpec],
        policy: crate::watcher::RestartPolicy,
    ) -> Self {
        let mut set = Self::new();
        for spec in specs {
            let (s, rx) = BotSession::new_with_policy(
                spec.name.clone(),
                vec![spec.config_path.clone()],
                policy.clone(),
            );
            set.sessions.push(s);
            set.rxs.push(rx);
        }
        set
    }

    /// 取出第 `i` 个会话的指令接收端 (取出后 [`Self::spawn_all`] 不再能启动它)。
    ///
    /// 给"自己驱动单个会话"的场景用 (测试、或者将来远端把指令源换成
    /// WebSocket)。正常情况下用 [`Self::spawn_all`]。
    pub fn take_receiver(&mut self, i: usize) -> Option<tokio::sync::mpsc::Receiver<String>> {
        if i < self.rxs.len() {
            Some(std::mem::replace(
                &mut self.rxs[i],
                tokio::sync::mpsc::channel::<String>(1).1,
            ))
        } else {
            None
        }
    }

    /// 取出全部指令接收端 (与 `sessions` 同序)。
    pub fn pull_receivers(&mut self) -> Vec<tokio::sync::mpsc::Receiver<String>> {
        (0..self.rxs.len())
            .map(|i| std::mem::replace(&mut self.rxs[i], tokio::sync::mpsc::channel::<String>(1).1))
            .collect()
    }

    /// 直接塞入已有会话 (测试与远端驱动用)。
    pub fn from_sessions(sessions: Vec<BotSession>) -> Self {
        Self {
            sessions,
            selected: 0,
            rxs: Vec::new(),
            render_pending: Vec::new(),
            render_taken_over: false,
        }
    }

    /// 声明"选中会话的日志缓冲已由外部接管" (见字段说明)。
    ///
    /// `App::attach_sessions` 调用它; 独立使用集合的调用方不要调。
    pub fn set_render_taken_over(&mut self, on: bool) {
        self.render_taken_over = on;
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &BotSession> {
        self.sessions.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut BotSession> {
        self.sessions.iter_mut()
    }

    /// 当前选中会话的下标 (空集合时为 0)。
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// 当前选中会话 (空集合时为 `None`)。
    pub fn selected(&self) -> Option<&BotSession> {
        self.sessions.get(self.selected)
    }

    pub fn selected_mut(&mut self) -> Option<&mut BotSession> {
        self.sessions.get_mut(self.selected)
    }

    /// 选中第 `i` 个 (越界则不动)。
    pub fn select(&mut self, i: usize) {
        if i < self.sessions.len() {
            self.selected = i;
        }
    }

    /// 相对移动选择 (`d` 可为负; 循环)。
    pub fn select_move(&mut self, d: i32) {
        let n = self.sessions.len();
        if n == 0 {
            return;
        }
        let cur = self.selected as i32;
        self.selected = (cur + d).rem_euclid(n as i32) as usize;
    }

    /// 按档案名精确查找。
    pub fn find(&self, name: &str) -> Option<usize> {
        self.sessions.iter().position(|s| s.profile == name)
    }

    /// 按档案名前缀查找全部命中 (大小写不敏感)。
    ///
    /// 前缀也匹配**精确名** —— `@serverA_100000003` 只命中它自己。
    pub fn find_prefix(&self, prefix: &str) -> Vec<usize> {
        let p = prefix.to_ascii_lowercase();
        self.sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| s.profile.to_ascii_lowercase().starts_with(&p))
            .map(|(i, _)| i)
            .collect()
    }

    /// 解析目标为命中的会话下标列表 (始终有序, 不含重复)。
    ///
    /// 前缀无命中时返回空 —— 调用方据此提示"没有会话匹配", 不能默默发给
    /// 当前选中会话 (那是最容易误操作的行为)。
    pub fn resolve(&self, target: &Target) -> Vec<usize> {
        match target {
            Target::Selected => {
                if self.sessions.is_empty() {
                    Vec::new()
                } else {
                    vec![self.selected]
                }
            }
            Target::All => (0..self.sessions.len()).collect(),
            Target::Prefix(p) => self.find_prefix(p),
        }
    }

    /// 全部在线 (运行中) 的会话下标。
    pub fn running(&self) -> Vec<usize> {
        self.sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_running())
            .map(|(i, _)| i)
            .collect()
    }

    /// 把所有会话的事件队列取进各自的日志缓冲。返回本次新增事件总数。
    ///
    /// 每帧调用一次; 因为每个会话有自己的队列, 日志天然分离, 不需要按来源
    /// 打标签再过滤。
    ///
    /// # 选中会话为什么走 `render_pending`
    ///
    /// 渲染位置只有一份缓冲 (`App.log`)。切换账号时它用 `swap` 把**选中会话**
    /// 的日志换过去, 于是那个会话留在集合里的 `s.log` 变成了一份**被换出去的
    /// 旧缓冲** —— 往里写的东西永远不上屏。
    ///
    /// 现场症状 (用户报的"Alt+↑↓ 切换时日志偶尔空白, 再切一下又出来"):
    /// 选中会话的日志停在切换那一刻, 屏幕上要么是旧内容要么是空的, 只有再切
    /// 一次才会把攒下的内容换出来。
    ///
    /// 所以选中会话的事件攒进 `render_pending`, 由 `App` flush 进它那份渲染
    /// 缓冲 (见 [`Self::take_render_pending`])。
    pub fn drain_all(&mut self) -> usize {
        let sel = self.selected;
        let route = self.render_taken_over;
        let mut n = 0;
        for (i, s) in self.sessions.iter_mut().enumerate() {
            let (evs, _) = s.take_queued();
            n += evs.len();
            if route && i == sel {
                self.render_pending.extend(evs);
            } else {
                for ev in evs {
                    s.push_log(ev);
                }
            }
        }
        n
    }

    /// 取走"该写进渲染缓冲"的事件 (见 [`Self::drain_all`] 的说明)。
    pub fn take_render_pending(&mut self) -> Vec<openstory_bot::emit::Event> {
        std::mem::take(&mut self.render_pending)
    }

    /// 往第 `i` 个会话的日志里写一行。
    ///
    /// **必须用这个而不是 `s.note(..)`**: 选中会话的活动缓冲在渲染位置,
    /// 直接写 `s.note` 会让这一行永远不上屏 (同 [`Self::drain_all`])。
    pub fn note_at(
        &mut self,
        i: usize,
        level: openstory_bot::emit::Level,
        text: impl Into<String>,
    ) {
        if self.render_taken_over && i == self.selected {
            self.render_pending.push(openstory_bot::emit::Event::new(
                level,
                openstory_bot::emit::Category::System,
                text.into(),
            ));
            return;
        }
        if let Some(s) = self.sessions.get_mut(i) {
            s.note(level, text);
        }
    }

    /// 累计丢弃事件数 (全部会话求和)。
    pub fn dropped_total(&self) -> u64 {
        self.sessions.iter().map(|s| s.dropped()).sum()
    }

    /// 向指定下标集合发送指令。返回 `(成功数, 失败项)`。
    ///
    /// 失败项带上会话名与原因 —— 批量下发时"哪个账号没收到"必须可见。
    pub fn send_to(&self, idxs: &[usize], cmd: &str) -> (usize, Vec<(String, String)>) {
        let mut ok = 0usize;
        let mut errs = Vec::new();
        for &i in idxs {
            match self.sessions.get(i) {
                Some(s) => match s.send(cmd) {
                    Ok(()) => ok += 1,
                    Err(e) => errs.push((s.profile.clone(), e)),
                },
                None => errs.push((format!("#{i}"), "会话不存在".to_string())),
            }
        }
        (ok, errs)
    }

    /// 解析目标 + 下发, 一步完成。返回 `(命中数, 成功数, 失败项)`。
    pub fn dispatch(&self, target: &Target, cmd: &str) -> (usize, usize, Vec<(String, String)>) {
        let idxs = self.resolve(target);
        let (ok, errs) = self.send_to(&idxs, cmd);
        (idxs.len(), ok, errs)
    }

    /// 目标相关的控制台内建指令 —— 它们**不发给 bot**, 由控制台自己执行。
    ///
    /// 三类:
    /// - `start` / `stop` —— 手动启停 ([`Self::start_session`] / [`Self::stop_session`])
    /// - `profiles add <路径>` / `profiles remove <档案名>` / `profiles list`
    ///   —— 运行时增删账号
    ///
    /// 为什么必须在这里拦下来而不是塞给 bot: bot 根本不认识这些词, 发过去
    /// 只会回一句"未知指令", 而用户要的是"把这个号连上/断开/挂上/摘掉"。
    pub fn builtin_for_target(cmd: &str) -> Option<Builtin> {
        let t = cmd.trim();
        match t {
            "start" => return Some(Builtin::Start),
            "stop" => return Some(Builtin::Stop),
            _ => {}
        }
        // `profiles ...` / `accounts ...`: 账号管理。
        let rest = t
            .strip_prefix("profiles")
            .or_else(|| t.strip_prefix("accounts"))?
            .trim();
        // 用**一个**分词器读完, 别一处 `split_whitespace` 一处再取第 N 个 ——
        // 那样带空格的引号路径会被两种解析法切得不一样 (实测踩到)。
        let tokens = split_quoted(rest);
        match tokens.first().map(|s| s.as_str()) {
            // 裸 `profiles` = 列出 (与 `profiles list` 同义)
            None | Some("list") | Some("ls") => Some(Builtin::ListProfiles),
            Some("add") | Some("open") => {
                // 空路径 = 解析成一个必然失败的动作, 让错误信息告诉用户怎么用
                let path = tokens.get(1).cloned().unwrap_or_default();
                Some(Builtin::AddProfile(PathBuf::from(path)))
            }
            Some("remove") | Some("rm") | Some("del") => Some(Builtin::RemoveProfile(
                tokens.get(1).cloned().unwrap_or_default(),
            )),
            // 未知子命令: 不认, 交给 bot (报了"未知指令"也比静默好)
            Some(_) => None,
        }
    }

    /// 对目标集合执行内建指令。返回 `(命中数, 成功数, 失败项)`。
    ///
    /// 失败项带会话名与原因 —— 批量 `@all start` 时"哪个号没起来、为什么"
    /// 必须逐条可见, 否则用户只能看到一个总数。
    ///
    /// 增删账号类指令 (`profiles ...`) **不按目标执行**: 它们作用于集合本身。
    /// 那种情况下 `(hit, ok)` 记作 `(1, 0/1)`, 详情由调用方从返回值之外的
    /// 渠道写日志 (见 `App::run_builtin_on_selected`)。
    pub fn dispatch_builtin(
        &mut self,
        handle: &tokio::runtime::Handle,
        target: &Target,
        builtin: Builtin,
        creds: Option<&crate::credentials::Credentials>,
    ) -> (usize, usize, Vec<(String, String)>) {
        if !builtin.is_per_session() {
            // 集合级动作: 目标对它没意义 (增删的是"有哪些账号")
            return (0, 0, Vec::new());
        }
        let idxs = self.resolve(target);
        let mut ok = 0usize;
        let mut errs = Vec::new();
        for &i in &idxs {
            let name = match self.sessions.get(i) {
                Some(s) => s.profile.clone(),
                None => {
                    errs.push((format!("#{i}"), "会话不存在".to_string()));
                    continue;
                }
            };
            let r = match builtin {
                Builtin::Start => self.start_session(handle, i, creds),
                Builtin::Stop => self.stop_session(i),
                // 上面已经拦掉了; 这里只是把穷尽匹配写完整
                Builtin::AddProfile(_) | Builtin::RemoveProfile(_) | Builtin::ListProfiles => {
                    continue;
                }
            };
            match r {
                Ok(_) => ok += 1,
                Err(e) => errs.push((name, e)),
            }
        }
        (idxs.len(), ok, errs)
    }

    /// 解析一行输入并按目标下发。
    ///
    /// 返回 `Ok((命中数, 成功数, 失败项))`, 或 `Err` (目标语法错误 /
    /// 没有会话命中 —— 后者是 `Err` 而不是"发 0 条", 因为静默不发送会让用户
    /// 以为指令生效了)。
    pub fn dispatch_line(
        &self,
        line: &str,
    ) -> Result<(usize, usize, Vec<(String, String)>), String> {
        let (target, cmd) = split_target(line)?;
        if cmd.is_empty() {
            return Err("没有指令内容".to_string());
        }
        if self.sessions.is_empty() {
            return Err("没有会话".to_string());
        }
        let idxs = self.resolve(&target);
        if idxs.is_empty() {
            return Err(format!("{} 没有匹配的会话", target.describe()));
        }
        let (ok, errs) = self.send_to(&idxs, &cmd);
        Ok((idxs.len(), ok, errs))
    }

    /// 全部档案名 (UI 左栏列表用)。
    pub fn profile_names(&self) -> Vec<&str> {
        self.sessions.iter().map(|s| s.profile.as_str()).collect()
    }

    /// 全部档案路径 (持久化/重启用)。
    pub fn config_paths(&self) -> Vec<PathBuf> {
        self.sessions
            .iter()
            .flat_map(|s| s.config_paths.iter().cloned())
            .collect()
    }

    // ── 自动拉起 ────────────────────────────────────────────────────

    /// 登记所有"刚刚结束"的会话, 返回 `(下标, 给用户看的一行说明)`。
    ///
    /// 每帧调用; 幂等 (见 [`BotSession::note_exit`])。
    pub fn note_exits(&mut self, now: std::time::Instant) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        for (i, s) in self.sessions.iter_mut().enumerate() {
            if s.note_exit(now) {
                let wait = s.restart_remaining(now).unwrap_or_default();
                out.push((i, format!("掉线, {} 秒后自动拉起", wait.as_secs())));
            }
        }
        out
    }

    /// 真正执行到点的拉起。返回 `(被拉起的下标, 失败原因列表)`。
    ///
    /// 需要 runtime `handle` 才能起 task, 因此由持有 runtime 的一侧
    /// (main / App) 每帧调用一次。
    ///
    /// `creds` 是控制台的凭据存储: 档案里没写 `login.password` 时, 拉起要从
    /// 这里补上密码 —— 否则新会话会在登录阶段失败, 而那种失败被判定为
    /// "认证问题, 不该重试", 于是自动拉起等于没生效。
    ///
    /// 拉不起来的会话**不**留在"等待"状态里空转: 没有配置模板 (档案从来没
    /// 加载成功) 就直接标记停止 —— 否则 UI 会永远显示一个不会发生的倒计时。
    pub fn advance_watchers(
        &mut self,
        handle: &tokio::runtime::Handle,
        now: std::time::Instant,
        creds: Option<&crate::credentials::Credentials>,
    ) -> (Vec<usize>, Vec<String>) {
        let mut spawned: Vec<usize> = Vec::new();
        let mut errs: Vec<String> = Vec::new();
        for i in 0..self.sessions.len() {
            if !self.sessions[i].poll_restart(now) {
                continue;
            }
            let profile = self.sessions[i].profile.clone();
            let pw = creds.and_then(|c| c.password_for(&profile));
            let Some(cfg) = self.sessions[i].begin_respawn(pw) else {
                self.sessions[i].set_restart_enabled(false);
                errs.push(format!("{profile}: 档案未能加载, 放弃自动拉起"));
                continue;
            };
            let (tx, rx) = tokio::sync::mpsc::channel::<String>(crate::session::CMD_CHANNEL_CAP);
            let arcs = self.sessions[i].arcs();
            let _jh = spawn_one(handle, cfg, rx, arcs);
            self.sessions[i].replace_cmd_tx(tx);
            spawned.push(i);
        }
        (spawned, errs)
    }

    /// 手动重启第 `i` 个会话 (用户按 F3): 清零退避并立刻拉起。
    ///
    /// 返回 `Err` 说明重启没发生 (会话不存在 / 没有配置模板)。
    pub fn manual_restart(
        &mut self,
        handle: &tokio::runtime::Handle,
        i: usize,
        now: std::time::Instant,
        creds: Option<&crate::credentials::Credentials>,
    ) -> Result<(), String> {
        let Some(s) = self.sessions.get_mut(i) else {
            return Err("会话不存在".to_string());
        };
        let profile = s.profile.clone();
        s.reset_watcher();
        if !s.task_finished() {
            // 还在跑: 只是清了退避状态, 不做别的
            return Ok(());
        }
        let pw = creds.and_then(|c| c.password_for(&profile));
        let Some(cfg) = s.begin_respawn(pw) else {
            return Err(format!("{profile}: 档案未能加载"));
        };
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(crate::session::CMD_CHANNEL_CAP);
        let arcs = s.arcs();
        let _jh = spawn_one(handle, cfg, rx, arcs);
        s.replace_cmd_tx(tx);
        let _ = now;
        Ok(())
    }

    /// 手动启动第 `i` 个会话 (用户按 `S`, 或 `@all start`)。
    ///
    /// 三种情况分别处理:
    /// - **已经在跑 / 正在等重连** → `Err`(不重复 spawn: 那会拿同一个账号
    ///   登录两次, 服务器会踢掉先来的那个)。
    /// - **没有可用密码** → `Err`。手动模式的价值就是"想连的时候才连", 而
    ///   那时多半还没输过密码; 不报错的话用户会看到一个没有任何反应的 start。
    /// - **正常** → 清手动停止状态 + 立刻 spawn。
    pub fn start_session(
        &mut self,
        handle: &tokio::runtime::Handle,
        i: usize,
        creds: Option<&crate::credentials::Credentials>,
    ) -> Result<&'static str, String> {
        let Some(s) = self.sessions.get_mut(i) else {
            return Err("会话不存在".to_string());
        };
        let profile = s.profile.clone();
        if !s.prepare_start() {
            return Err(format!("{profile}: 已经在运行 (或在等重连)"));
        }
        let pw = creds.and_then(|c| c.password_for(&profile)).map(str::to_string);
        // 模板里的密码优先级最高 (档案里写的 / 向导输过的 / 上一次拉起补上的)
        let has_pw = s
            .template()
            .map(|c| !c.password.is_empty())
            .unwrap_or(false)
            || pw.as_deref().map(|p| !p.is_empty()).unwrap_or(false);
        if !has_pw {
            // 回滚: 不能留下一个"看着像要启动"其实起不来的状态
            s.stop_manual_keep_attempts();
            s.mark_start_failed();
            return Err(format!(
                "{profile}: 没有可用密码 —— 选中它按 F4 补输, 或用 --password / 档案的 login.password / 凭据存储"
            ));
        }
        let Some(cfg) = s.begin_respawn(pw.as_deref()) else {
            s.stop_manual_keep_attempts();
            s.mark_start_failed();
            return Err(format!("{profile}: 档案未能加载 (没有配置模板)"));
        };
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(crate::session::CMD_CHANNEL_CAP);
        let arcs = s.arcs();
        let _jh = spawn_one(handle, cfg, rx, arcs);
        s.replace_cmd_tx(tx);
        // 新的 task 起来了 → 清掉"正在收尾"闩 (它是唯一能清这件事的事件)
        s.note_respawned();
        Ok("已启动")
    }

    /// 手动停止第 `i` 个会话 (用户按 `S`, 或 `@all stop`)。
    ///
    /// 两步, 顺序不能反:
    /// 1. 先落状态 (`mark_stopped_by_user`) —— 它关掉自动拉起并把结束原因
    ///    标成 `Manual`, 于是 bot 随后的退出不会被当成"掉线"而排定一次拉起。
    /// 2. 再发 `quit` 让 bot 走正常退出路径。
    ///
    /// 卡死的兜底: `quit` 送不出去 (通道满 / bot 已经不收指令) 时直接 abort
    /// task。**abort 之前状态已经落好了**, 所以这次 abort 不会被误判成崩溃。
    ///
    /// 返回 `Err` 表示没什么可停的 (从未启动的会话)。
    ///
    /// "已经结束"的会话**算可停**: 它可能正亮着"需人工", 而用户的意图就是
    /// "这个号我不要了, 别再提示我"。这种情况必须让它落到"已停止 (手动)" ——
    /// 否则用户按了 stop 界面却继续亮红, 看起来像按键没生效。
    pub fn stop_session(&mut self, i: usize) -> Result<&'static str, String> {
        // `note_at` 要 `&mut self`, 所以会话那笔可变借用必须在调用前结束 ——
        // 下面用块把 `s` 的借用收窄。
        let (profile, stuck) = {
            let Some(s) = self.sessions.get_mut(i) else {
                return Err("会话不存在".to_string());
            };
            let profile = s.profile.clone();
            // 先判定"有没有什么可停的", **再**动状态。
            //
            // 顺序要紧: `mark_stopped_by_user` 会清掉结束原因 (那是"用户说我
            // 不要了"的语义), 之后再想区分"从未启动"和"跑过已结束"就没有依据
            // 了 —— 两者都会变成"阶段向导 + 没有 outcome"。
            let nothing_to_stop = s.never_started() && !s.is_running() && !s.is_waiting_restart();
            if nothing_to_stop {
                return Err(format!("{profile}: 本来就没在运行"));
            }
            s.mark_stopped_by_user();
            // 还在跑的: 发 `quit` 让它走正常退出路径。
            //
            // 判据用 `has_task()` 而不是 `is_running()`: 阶段标志是 task
            // **内部**置的, 所以刚 spawn 完的那一瞬间 `is_running()` 还是
            // false —— 用它会让"刚启动就停止"这条路径不发 quit, 于是任务一直
            // 挂到超时。
            let stuck = s.has_task() && s.send("quit").is_err();
            if stuck {
                // 卡死的兜底: 指令送不进去就直接 abort。**状态已经落好了**,
                // 所以这次 abort 不会被误判成崩溃。
                s.abort_task();
            }
            (profile, stuck)
        };
        let _ = profile;
        // 用 `note_at` (而不是 `s.note`): 选中会话的活动缓冲在渲染位置。
        self.note_at(
            i,
            openstory_bot::emit::Level::Info,
            "已停止 (手动) —— 自动拉起已关闭; 按 F4 可重新启动",
        );
        if stuck {
            self.note_at(
                i,
                openstory_bot::emit::Level::Err,
                "quit 指令送不进去 (bot 可能卡住) —— 已强制取消该会话的任务",
            );
        }
        Ok("已停止")
    }

    /// 是否所有会话都被用户手动停掉 (含从未启动的)。
    ///
    /// 手动模式下用它回答"现在还有没有号在跑" —— 全是停的就没什么可看的了。
    pub fn all_stopped_by_user(&self) -> bool {
        !self.sessions.is_empty()
            && self
                .sessions
                .iter()
                .all(|s| s.is_manually_stopped() || s.never_started())
    }

    /// 有多少会话从未启动 (手动模式的左栏显示)。
    pub fn never_started_count(&self) -> usize {
        self.sessions.iter().filter(|s| s.never_started()).count()
    }

    /// 是否还有"从未启动"的会话 —— 手动模式下**阻止程序自己收尾**的唯一依据。
    ///
    /// 语义: 一个从没启动过的会话是"随时可能起来"的, 所以那一刻不算"全部完事"。
    /// 手动模式 (`--no-autostart`) 下用户可能先启动一个试试, 剩下几个等他决定;
    /// 没有这条, 第一个会话一结束 (`all_done` 置位) 整个程序就退出了, 剩下的
    /// 号根本没机会被启动。
    pub fn has_never_started(&self) -> bool {
        self.sessions.iter().any(|s| s.never_started())
    }

    // ── 运行时增删账号 ──────────────────────────────────────────────

    /// 运行时挂上一个新账号 (控制台 `profiles add <路径>`)。
    ///
    /// 新会话**停在「未启动」**上, 由用户按 F4 决定什么时候连 —— 加一个账号
    /// 不该顺手把它登上去 (那正是"管理"与"自动登录"的区别)。
    ///
    /// 从文件读出配置当模板是必须的: 没有模板的会话在 start 时会被拒
    /// (`档案未能加载`), 用户会看到一个按了没反应的按钮。
    ///
    /// 返回新会话的下标。
    pub fn add_profile(&mut self, path: PathBuf) -> Result<usize, String> {
        if !path.is_file() {
            return Err(format!("{}: 文件不存在", path.display()));
        }
        if self.sessions.iter().any(|s| s.config_paths.contains(&path)) {
            return Err(format!("{}: 已经挂上了", path.display()));
        }
        let profile = BotSession::profile_from_path(&path);
        if self.sessions.iter().any(|s| s.profile == profile) {
            return Err(format!("{profile}: 已有同名档案在列表里"));
        }
        // 配置能读出来才加 —— 加进去一个永远启动不了的会话是纯噪音
        let mut cfg = openstory_bot::config::Config::parse_partial(&[
            "--config".to_string(),
            path.display().to_string(),
        ])
        .map_err(|e| format!("{profile}: 配置读不出来 ({e})"))?;
        cfg.config_paths = vec![path.display().to_string()];
        if let Some(info) = openstory_bot::login::load(&path) {
            // 与启动路径同一条语义: 账号以档案为准 (这里的 `cfg` 是新解析出来的,
            // 本来就没有继承账号, 但保持"所有模板都这么建"的一致性)。
            cfg.apply_login_for_profile(&info);
        }
        let (mut s, rx) = BotSession::new(profile, vec![path]);
        s.set_template(cfg);
        self.sessions.push(s);
        // 接收端存进集合 —— 下一次 `pull_receivers()` 会把它交给启动方
        // (用户按 F4 时 `start_session` 自己建新通道, 这个仅作占位)。
        self.rxs.push(rx);
        Ok(self.sessions.len() - 1)
    }

    /// 运行时摘掉一个账号 (控制台 `profiles remove <档案名>`)。
    ///
    /// **只允许摘"静了"的会话** (从没启动 / 已结束 / 手动停 / 启动失败):
    /// 摘掉一个正在跑的会话等于把它连根拔起 —— bot task 会变成孤儿继续跑,
    /// 而界面上再也看不到它, 用户无从停止。要摘就先 `stop`。
    ///
    /// 返回 `(下标, 档案名)`。
    pub fn remove_profile(&mut self, name: &str) -> Result<(usize, String), String> {
        let idx = self
            .sessions
            .iter()
            .position(|s| s.profile == name)
            .ok_or_else(|| format!("{name}: 没有这个档案"))?;
        if !self.sessions[idx].is_settled() {
            return Err(format!(
                "{name}: 还在运行 —— 先 stop 它再摘 (否则它会变成看不见的孤儿进程)"
            ));
        }
        let s = self.sessions.remove(idx);
        // rxs 与 sessions **同序**, 必须一起动 —— 错位之后 `pull_receivers`
        // 会把某个会话的接收端交给另一个会话。
        if idx < self.rxs.len() {
            self.rxs.remove(idx);
        }
        if self.selected >= self.sessions.len() {
            self.selected = self.sessions.len().saturating_sub(1);
        }
        Ok((idx, s.profile))
    }

    /// 按档案名找下标。
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.sessions.iter().position(|s| s.profile == name)
    }

    /// 全部档案名 + 当前状态 (控制台 `profiles list`)。
    pub fn status_lines(&self) -> Vec<String> {
        self.sessions
            .iter()
            .map(|s| {
                let st = if s.is_manually_stopped() {
                    "已停止(手动)"
                } else if s.start_failed() {
                    "启动失败"
                } else if s.never_started() {
                    "未启动"
                } else if s.is_running() {
                    "运行中"
                } else if s.is_waiting_restart() {
                    "等重连"
                } else if s.is_finished() {
                    "已结束"
                } else {
                    "?"
                };
                format!("  {} — {st}", s.profile)
            })
            .collect()
    }

    /// 有多少会话正在等自动拉起。
    pub fn waiting_restart(&self) -> usize {
        self.sessions
            .iter()
            .filter(|s| s.is_waiting_restart())
            .count()
    }

    /// 有多少会话已放弃自动拉起 / 需人工介入。
    pub fn needing_attention(&self) -> usize {
        self.sessions.iter().filter(|s| s.needs_attention()).count()
    }

    /// 还"活着"的会话数: 在跑的 + 正在等退避重连的。
    ///
    /// 这是退出条件的核心量, 判据是 [`BotSession::is_settled`] 的反面 ——
    /// "还会自己动"才算活着。用 `is_running()` 是不行的: 一个已结束但还在等
    /// 重连的会话阶段是 `FINISHED`, 但它**还会再动**。
    ///
    /// 两种"静了"的会话必须被排除, 否则 `keepalive` 永远不为零、程序永远不退出:
    /// - **从未启动** (手动模式的初始状态)
    /// - **启动失败** (用户按了 start 但没起来 —— 没有任何东西会再推动它)
    pub fn live_count(&self) -> usize {
        self.sessions.iter().filter(|s| !s.is_settled()).count()
    }

    /// 一个还会自己动的会话都没有 (且集合非空) → 主线程可以收尾。
    pub fn all_stopped(&self) -> bool {
        !self.is_empty() && self.live_count() == 0
    }
}

impl Default for SessionSet {
    fn default() -> Self {
        Self::new()
    }
}

/// 启动一个会话的 bot: 在自己的 emitter 作用域里跑 `runtime::run`, 结束后
/// 写回 `stage` / `outcome`。
///
/// **独立函数而不是 `SessionSet` 的方法**: 启动 bot 需要 `'static` 的数据
/// (tokio task 不能借用会话), 而会话集合本身要交给 UI 线程。把所需的 `Arc`
/// 在这里取好、连同接收端一起 move 进 task, 两边就都能拿到自己需要的东西 ——
/// 不需要共享 `SessionSet`, 也不需要 unsafe。
///
/// 用调用方给的 `handle` 而不是 `tokio::spawn`: 主线程不在 runtime 上下文里
/// (它负责界面与退出), 直接 `tokio::spawn` 会 panic "no reactor running"。
///
/// 返回的是**监督任务**的 handle: 它内部等 bot 任务, 因此
/// `is_finished()` 只有 bot 真的结束才为真, 而 `JoinError::is_panic()`
/// 能在 bot panic 时把 `panic_flag` 置位 (panic 的任务没有返回值,
/// `RunOutcome` 表达不了这件事)。
#[allow(clippy::too_many_arguments)]
pub fn spawn_one(
    handle: &tokio::runtime::Handle,
    cfg: openstory_bot::config::Config,
    rx: tokio::sync::mpsc::Receiver<String>,
    arcs: SessionArcs,
) -> tokio::task::JoinHandle<openstory_bot::runtime::RunOutcome> {
    let (queue, state, stage, outcome, panicked, join_slot) = arcs;
    let emitter: std::sync::Arc<dyn openstory_bot::emit::Emitter> = queue;
    spawn_with_emitter_and(
        handle,
        cfg,
        rx,
        (state, stage, outcome, panicked, join_slot),
        emitter,
    )
}

/// 同 [`spawn_one`], 但注入自定义 emitter (测试用来制造 panic / 捕获事件)。
pub fn spawn_with_emitter(
    handle: &tokio::runtime::Handle,
    cfg: openstory_bot::config::Config,
    rx: tokio::sync::mpsc::Receiver<String>,
    arcs: SessionArcs,
    emitter: std::sync::Arc<dyn openstory_bot::emit::Emitter>,
) -> tokio::task::JoinHandle<openstory_bot::runtime::RunOutcome> {
    let (_queue, state, stage, outcome, panicked, join_slot) = arcs;
    spawn_with_emitter_and(
        handle,
        cfg,
        rx,
        (state, stage, outcome, panicked, join_slot),
        emitter,
    )
}

/// bot task 结束原因的存放位 (会话与监督任务共享)。
pub type OutcomeSlot = std::sync::Arc<std::sync::Mutex<Option<openstory_bot::runtime::RunOutcome>>>;

/// 一个 bot task 的收尾结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupervisorResult {
    /// 结束原因 (panic / abort 归约为 `Quit` —— 两者都没有正常返回值)。
    pub outcome: openstory_bot::runtime::RunOutcome,
    /// task 是否 panic 结束。
    ///
    /// 必须单独记: panic 的任务没有返回值, `RunOutcome` 表达不了这件事。
    /// 而"崩溃"与"用户主动退出"的处置完全相反 (前者要拉起, 后者绝不),
    /// 所以这个信息丢了就等于把崩溃当成正常退出 —— 那时自动拉起永远不会
    /// 因为崩溃而触发。
    pub panicked: bool,
}

/// 把 bot task 的 `Result` 归约成 [`SupervisorResult`]。
///
/// 抽成纯函数 (而不是写在 `spawn` 的闭包里) 是为了**能直接测**: panic 分支
/// 没法用真 bot 稳定复现 (要制造 panic 就得让 bot 崩, 而崩法本身依赖环境),
/// 而它恰好是最关键的一条 —— 判错了会把崩溃当正常退出, 自动拉起形同虚设。
pub fn classify_join(
    r: Result<openstory_bot::runtime::RunOutcome, tokio::task::JoinError>,
) -> SupervisorResult {
    match r {
        Ok(o) => SupervisorResult {
            outcome: o,
            panicked: false,
        },
        Err(e) => SupervisorResult {
            // abort (退出流程) 与 panic 都没有返回值, 都归约为 Quit;
            // 区别由 `panicked` 承载。
            outcome: openstory_bot::runtime::RunOutcome::Quit,
            panicked: e.is_panic(),
        },
    }
}

/// 写入收尾结果。**先写原因, 再置 `FINISHED`**。
///
/// 顺序是契约的一部分: 任何看到 `FINISHED` 的读者 (UI / 拉起逻辑) 都必须
/// 能立刻读到 `outcome`。反过来先置位就会有一个窗口让读者看到
/// "已结束但没有原因", 从而把一次断线误判成"说不清的结束"。
///
/// 用 `unwrap_or_else(|e| e.into_inner())` 而不是 `unwrap()`: bot 的 future
/// 里 panic 会毒化这把锁, 而这里**正是** panic 之后的收尾路径 —— 用 `unwrap`
/// 会在收尾时二次 panic, 把整个进程 abort (多开时一个号崩带走全部号)。
pub fn write_back(
    outcome_slot: &OutcomeSlot,
    stage: &std::sync::Arc<std::sync::atomic::AtomicU8>,
    res: SupervisorResult,
) {
    let mut slot = outcome_slot.lock().unwrap_or_else(|e| e.into_inner());
    *slot = Some(res.outcome);
    drop(slot);
    stage.store(
        crate::session::STAGE_FINISHED,
        std::sync::atomic::Ordering::SeqCst,
    );
}

#[allow(clippy::type_complexity)]
fn spawn_with_emitter_and(
    handle: &tokio::runtime::Handle,
    cfg: openstory_bot::config::Config,
    rx: tokio::sync::mpsc::Receiver<String>,
    inner_arcs: (
        std::sync::Arc<tokio::sync::Mutex<openstory_bot::state::BotState>>,
        std::sync::Arc<std::sync::atomic::AtomicU8>,
        OutcomeSlot,
        crate::session::PanicFlag,
        crate::session::JoinSlot,
    ),
    emitter: std::sync::Arc<dyn openstory_bot::emit::Emitter>,
) -> tokio::task::JoinHandle<openstory_bot::runtime::RunOutcome> {
    let (state, stage, outcome, panicked, join_slot) = inner_arcs;
    stage.store(
        crate::session::STAGE_RUNNING,
        std::sync::atomic::Ordering::SeqCst,
    );
    handle.spawn(async move {
        let mut rx = rx;
        let inner = tokio::spawn(openstory_bot::emit::with_session_emitter(
            emitter,
            async move { openstory_bot::runtime::run(cfg, &mut rx, state).await },
        ));
        // 把 abort 句柄入槽: 会话层据此能取消一个卡死的 bot, 而"结束了吗"
        // 由监督任务写 `stage` 回答 —— 那比轮询 `JoinHandle::is_finished()`
        // 更可靠 (写回一定发生在置位之前)。
        if let Ok(mut slot) = join_slot.lock() {
            *slot = Some(inner.abort_handle());
        }
        let res = classify_join(inner.await);
        if res.panicked {
            panicked.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        write_back(&outcome, &stage, res);
        res.outcome
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// 建集合 + 把每个会话的接收端取出来交给测试 (模拟"已经在跑的 bot")。
    fn set(names: &[&str]) -> (SessionSet, Vec<mpsc::Receiver<String>>) {
        let specs: Vec<SessionSpec> = names
            .iter()
            .map(|n| SessionSpec::new(format!("profiles/{n}.json")))
            .collect();
        let mut s = SessionSet::from_specs(&specs);
        let rxs = (0..s.len()).filter_map(|i| s.take_receiver(i)).collect();
        (s, rxs)
    }

    // ── 目标语法 ────────────────────────────────────────────────────

    #[test]
    fn parse_target_forms() {
        assert_eq!(Target::parse("@all"), Some(Target::All));
        assert_eq!(Target::parse("@ALL"), Some(Target::All));
        assert_eq!(Target::parse("@All"), Some(Target::All));
        assert_eq!(
            Target::parse("@serverA"),
            Some(Target::Prefix("serverA".into()))
        );
        assert_eq!(Target::parse("hunt"), None, "无 @ 不是目标");
        assert_eq!(Target::parse("@"), None, "裸 @ 语义不明");
    }

    #[test]
    fn split_target_keeps_command_for_bare_lines() {
        let (t, c) = split_target("hunt on").unwrap();
        assert_eq!(t, Target::Selected);
        assert_eq!(c, "hunt on");
        // 前后空白被规整
        let (t, c) = split_target("   hunt on   ").unwrap();
        assert_eq!(t, Target::Selected);
        assert_eq!(c, "hunt on");
    }

    #[test]
    fn split_target_strips_the_target_token() {
        let (t, c) = split_target("@serverA hunt on").unwrap();
        assert_eq!(t, Target::Prefix("serverA".into()));
        assert_eq!(c, "hunt on");
        let (t, c) = split_target("@all   chat 你好").unwrap();
        assert_eq!(t, Target::All);
        assert_eq!(c, "chat 你好");
    }

    #[test]
    fn split_target_rejects_bare_at() {
        let err = split_target("@ hunt on").unwrap_err();
        assert!(err.contains("无效"), "got {err}");
    }

    #[test]
    fn split_target_only_looks_at_the_first_token() {
        // 指令中间的 @ 不属于目标 (例如聊天内容里的 @某人)
        let (t, c) = split_target("chat @someone 你好").unwrap();
        assert_eq!(t, Target::Selected);
        assert_eq!(c, "chat @someone 你好");
    }

    #[test]
    fn split_target_empty_line() {
        let (t, c) = split_target("").unwrap();
        assert_eq!(t, Target::Selected);
        assert!(c.is_empty());
        let (t, c) = split_target("   ").unwrap();
        assert_eq!(t, Target::Selected);
        assert!(c.is_empty());
    }

    // ── 集合与选择 ──────────────────────────────────────────────────

    #[test]
    fn selection_starts_at_zero_and_moves_cyclically() {
        let (mut s, _r) = set(&["a", "b", "c"]);
        assert_eq!(s.selected_index(), 0);
        s.select_move(1);
        assert_eq!(s.selected_index(), 1);
        s.select_move(-1);
        assert_eq!(s.selected_index(), 0);
        // 向前越界回卷
        s.select_move(-1);
        assert_eq!(s.selected_index(), 2);
        s.select_move(1);
        assert_eq!(s.selected_index(), 0);
    }

    #[test]
    fn select_out_of_range_is_ignored() {
        let (mut s, _r) = set(&["a", "b"]);
        s.select(5);
        assert_eq!(s.selected_index(), 0, "越界选中不应改动");
        s.select(1);
        assert_eq!(s.selected_index(), 1);
    }

    #[test]
    fn select_move_on_empty_set_is_a_noop() {
        let mut s = SessionSet::new();
        s.select_move(1);
        s.select_move(-1);
        assert_eq!(s.selected_index(), 0);
        assert!(s.selected().is_none());
        assert!(s.is_empty());
    }

    // ── 目标解析 ────────────────────────────────────────────────────

    #[test]
    fn resolve_selected_all_and_prefix() {
        let (s, _r) = set(&["serverA_1", "serverA_2", "serverB_1"]);
        assert_eq!(s.resolve(&Target::Selected), vec![0]);
        assert_eq!(s.resolve(&Target::All), vec![0, 1, 2]);
        assert_eq!(s.resolve(&Target::Prefix("serverA".into())), vec![0, 1]);
        assert_eq!(s.resolve(&Target::Prefix("serverB".into())), vec![2]);
        // 前缀也命中精确名 -> 只命中自己
        assert_eq!(s.resolve(&Target::Prefix("serverA_1".into())), vec![0]);
    }

    #[test]
    fn resolve_prefix_is_case_insensitive() {
        let (s, _r) = set(&["ServerA_1"]);
        assert_eq!(s.resolve(&Target::Prefix("serverA".into())), vec![0]);
        assert_eq!(s.resolve(&Target::Prefix("SERVERA".into())), vec![0]);
    }

    #[test]
    fn resolve_unknown_prefix_is_empty_not_selected() {
        // 关键安全行为: 前缀无命中必须是"空", 绝不能退化成"发给当前选中会话"
        let (mut s, _r) = set(&["a", "b"]);
        s.select(1);
        assert!(s.resolve(&Target::Prefix("nope".into())).is_empty());
    }

    #[test]
    fn resolve_on_empty_set_is_empty() {
        let s = SessionSet::new();
        assert!(s.resolve(&Target::Selected).is_empty());
        assert!(s.resolve(&Target::All).is_empty());
    }

    // ── 下发 ────────────────────────────────────────────────────────

    #[test]
    fn dispatch_line_to_selected_only() {
        let (mut s, mut rxs) = set(&["a", "b"]);
        s.select(1);
        let (hit, ok, errs) = s.dispatch_line("hunt on").unwrap();
        assert_eq!((hit, ok), (1, 1));
        assert!(errs.is_empty());
        assert!(rxs[0].try_recv().is_err(), "未选中的会话不应收到");
        assert_eq!(rxs[1].try_recv().unwrap(), "hunt on");
    }

    #[test]
    fn dispatch_line_to_all_reaches_every_session() {
        let (s, mut rxs) = set(&["a", "b", "c"]);
        let (hit, ok, errs) = s.dispatch_line("@all hunt on").unwrap();
        assert_eq!((hit, ok), (3, 3));
        assert!(errs.is_empty());
        for rx in rxs.iter_mut() {
            assert_eq!(rx.try_recv().unwrap(), "hunt on");
        }
    }

    #[test]
    fn dispatch_line_to_prefix_reaches_the_matching_group() {
        let (s, mut rxs) = set(&["serverA_1", "serverA_2", "serverB_1"]);
        let (hit, ok, _) = s.dispatch_line("@serverA reactor on").unwrap();
        assert_eq!((hit, ok), (2, 2));
        assert_eq!(rxs[0].try_recv().unwrap(), "reactor on");
        assert_eq!(rxs[1].try_recv().unwrap(), "reactor on");
        assert!(rxs[2].try_recv().is_err(), "不同服的会话不应收到");
    }

    #[test]
    fn dispatch_line_rejects_unmatched_prefix() {
        let (s, mut rxs) = set(&["a"]);
        let err = s.dispatch_line("@nope hunt on").unwrap_err();
        assert!(err.contains("没有匹配"), "got {err}");
        assert!(rxs[0].try_recv().is_err(), "无命中时不能发给任何人");
    }

    #[test]
    fn dispatch_line_rejects_empty_command() {
        let (s, _r) = set(&["a"]);
        assert!(s
            .dispatch_line("@all")
            .unwrap_err()
            .contains("没有指令内容"));
        assert!(s.dispatch_line("   ").unwrap_err().contains("没有指令内容"));
    }

    #[test]
    fn dispatch_line_rejects_empty_set() {
        let s = SessionSet::new();
        let err = s.dispatch_line("hunt on").unwrap_err();
        assert!(err.contains("没有会话"), "got {err}");
    }

    #[test]
    fn dispatch_line_reports_per_session_failures() {
        // 一个会话的 bot 已结束 -> 批量下发必须报告"哪个没收到"
        let (s, rxs) = set(&["a", "b"]);
        drop(rxs); // 全部结束
        let (hit, ok, errs) = s.dispatch_line("@all hunt on").unwrap();
        assert_eq!((hit, ok), (2, 0));
        assert_eq!(errs.len(), 2);
        assert_eq!(errs[0].0, "a");
        assert_eq!(errs[1].0, "b");
    }

    #[test]
    fn dispatch_line_partial_failure_is_visible() {
        let (s, mut rxs) = set(&["a", "b"]);
        // 只丢弃 b 的接收端
        let b_rx = rxs.pop().unwrap();
        drop(b_rx);
        let (hit, ok, errs) = s.dispatch_line("@all hunt on").unwrap();
        assert_eq!((hit, ok), (2, 1));
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].0, "b");
        assert_eq!(rxs[0].try_recv().unwrap(), "hunt on");
    }

    #[test]
    fn send_to_handles_bad_index() {
        let (s, _r) = set(&["a"]);
        let (ok, errs) = s.send_to(&[0, 9], "hunt on");
        assert_eq!(ok, 1);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].1.contains("不存在"));
    }

    // ── 元信息 ──────────────────────────────────────────────────────

    #[test]
    fn find_is_exact_and_find_prefix_is_not() {
        let (s, _r) = set(&["ab", "abc"]);
        assert_eq!(s.find("ab"), Some(0));
        assert_eq!(s.find("a"), None, "find 是精确匹配");
        assert_eq!(s.find_prefix("a"), vec![0, 1]);
    }

    #[test]
    fn profile_names_and_running() {
        let (s, _r) = set(&["a", "b"]);
        assert_eq!(s.profile_names(), vec!["a", "b"]);
        assert!(s.running().is_empty(), "未启动时没有运行中的会话");
        s.iter().next().unwrap().stage.store(
            crate::session::STAGE_RUNNING,
            std::sync::atomic::Ordering::SeqCst,
        );
        assert_eq!(s.running(), vec![0]);
    }

    #[test]
    fn config_paths_are_collected_in_order() {
        let (s, _r) = set(&["a", "b"]);
        let ps = s.config_paths();
        assert_eq!(ps.len(), 2);
        assert!(ps[0].ends_with("a.json"));
        assert!(ps[1].ends_with("b.json"));
    }

    #[test]
    fn dispatch_by_target_struct() {
        let (s, mut rxs) = set(&["a", "b"]);
        let (hit, ok, _) = s.dispatch(&Target::Prefix("b".into()), "chat hi");
        assert_eq!((hit, ok), (1, 1));
        assert!(rxs[0].try_recv().is_err());
        assert_eq!(rxs[1].try_recv().unwrap(), "chat hi");
    }

    // ── 监督任务收尾 (classify_join / write_back) ───────────────────
    //
    // 这两条是纯函数, 因此可以**精确**测到 panic 分支 —— 用真 bot 制造
    // panic 需要先让它崩, 而崩法依赖环境 (不可靠)。判错这一条会把崩溃当成
    // 正常退出, 于是自动拉起永远不会因为崩溃触发。

    #[test]
    fn classify_join_passes_through_a_normal_outcome() {
        for o in [
            openstory_bot::runtime::RunOutcome::Quit,
            openstory_bot::runtime::RunOutcome::Failed,
            openstory_bot::runtime::RunOutcome::ConnectFailed,
            openstory_bot::runtime::RunOutcome::ConnectionClosed,
            openstory_bot::runtime::RunOutcome::DurationElapsed,
        ] {
            let r = classify_join(Ok(o));
            assert_eq!(r.outcome, o, "正常返回值必须原样传递");
            assert!(!r.panicked);
        }
    }

    #[tokio::test]
    async fn classify_join_flags_a_panic() {
        let h = tokio::spawn(async { panic!("boom") });
        let err = h.await.unwrap_err();
        let r = classify_join(Err(err));
        assert!(r.panicked, "panic 必须被识别出来");
        assert_eq!(
            r.outcome,
            openstory_bot::runtime::RunOutcome::Quit,
            "panic 没有返回值, 归约为 Quit (区别由 panicked 承载)"
        );
    }

    #[tokio::test]
    async fn classify_join_treats_abort_as_a_clean_stop() {
        // 退出流程会 abort 任务 —— 那是我们自己的动作, 不能算崩溃
        // (否则退出时每个会话都会排队等一次"自动拉起")。
        let h: tokio::task::JoinHandle<()> = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        });
        h.abort();
        let err = h.await.unwrap_err();
        let r = classify_join(Err(err));
        assert!(!r.panicked, "abort 不是崩溃");
        assert_eq!(r.outcome, openstory_bot::runtime::RunOutcome::Quit);
    }

    #[test]
    fn write_back_sets_the_outcome_before_the_stage() {
        let (s, _rx) = BotSession::new("acc1", vec![]);
        s.stage.store(
            crate::session::STAGE_RUNNING,
            std::sync::atomic::Ordering::SeqCst,
        );
        write_back(
            &s.outcome,
            &s.stage,
            SupervisorResult {
                outcome: openstory_bot::runtime::RunOutcome::ConnectionClosed,
                panicked: false,
            },
        );
        assert_eq!(
            s.outcome(),
            Some(openstory_bot::runtime::RunOutcome::ConnectionClosed)
        );
        assert!(s.task_finished(), "写回之后必须置 FINISHED");
    }

    #[test]
    fn write_back_survives_a_poisoned_outcome_lock() {
        // 收尾路径**正是** panic 之后走的那条 —— 用一个会 panic 的持锁者
        // 毒化它, 再验证 write_back 仍然能写完 (用 unwrap 的话这里会二次
        // panic, 多开时一个号崩会带走整个进程)。
        let (s, _rx) = BotSession::new("acc1", vec![]);
        let slot = s.outcome.clone();
        let victim = slot.clone();
        let _ = std::thread::spawn(move || {
            let _g = victim.lock().unwrap();
            panic!("毒化这把锁");
        })
        .join();
        assert!(slot.is_poisoned(), "前置条件: 锁应已被毒化");
        write_back(
            &slot,
            &s.stage,
            SupervisorResult {
                outcome: openstory_bot::runtime::RunOutcome::ConnectFailed,
                panicked: true,
            },
        );
        assert!(s.task_finished(), "毒化的锁也必须能完成收尾");
        assert_eq!(
            s.outcome(),
            Some(openstory_bot::runtime::RunOutcome::ConnectFailed)
        );
    }

    // ── 集合级的拉起调度 (advance_watchers / note_exits) ────────────

    fn fast_policy() -> crate::watcher::RestartPolicy {
        crate::watcher::RestartPolicy {
            enabled: true,
            max_attempts: 3,
            backoff_secs: vec![0],
        }
    }

    /// 造一个"刚刚断线"的会话集合 (带模板, 因此能真的拉起)。
    fn ended_set(names: &[&str]) -> SessionSet {
        let specs: Vec<SessionSpec> = names
            .iter()
            .map(|n| SessionSpec::new(format!("profiles/{n}.json")))
            .collect();
        let mut s = SessionSet::from_specs_with_policy(&specs, fast_policy());
        for i in 0..s.len() {
            let sess = s.iter_mut().nth(i).unwrap();
            sess.set_template(openstory_bot::config::Config {
                ip: "127.0.0.1".to_string(),
                port: 1,
                account: "t".to_string(),
                password: "t".to_string(),
                ..Default::default()
            });
            sess.stage.store(
                crate::session::STAGE_FINISHED,
                std::sync::atomic::Ordering::SeqCst,
            );
            if let Ok(mut o) = sess.outcome.lock() {
                *o = Some(openstory_bot::runtime::RunOutcome::ConnectionClosed);
            }
        }
        s
    }

    #[test]
    fn note_exits_reports_only_newly_ended_sessions() {
        let mut s = ended_set(&["a", "b"]);
        let t = std::time::Instant::now();
        let first = s.note_exits(t);
        assert_eq!(first.len(), 2, "两个会话都应被登记");
        assert_eq!(first[0].0, 0);
        assert_eq!(first[1].0, 1);
        assert!(first[0].1.contains("秒后自动拉起"), "got {}", first[0].1);
        // 幂等: 再来一次不该重复登记
        assert!(s.note_exits(t).is_empty(), "note_exits 必须幂等");
        assert_eq!(s.waiting_restart(), 2);
    }

    #[test]
    fn one_ended_session_does_not_mark_others() {
        let mut s = ended_set(&["a", "b"]);
        // 只有 a 结束了
        s.iter_mut().nth(1).unwrap().stage.store(
            crate::session::STAGE_RUNNING,
            std::sync::atomic::Ordering::SeqCst,
        );
        let out = s.note_exits(std::time::Instant::now());
        assert_eq!(out.len(), 1, "只有结束的那个该被登记");
        assert_eq!(out[0].0, 0);
        assert_eq!(s.waiting_restart(), 1);
    }

    #[test]
    fn needing_attention_counts_given_up_and_failed_sessions() {
        let mut s = ended_set(&["a", "b", "c"]);
        assert_eq!(s.needing_attention(), 0);
        // b: 认证失败
        {
            let b = s.iter_mut().nth(1).unwrap();
            if let Ok(mut o) = b.outcome.lock() {
                *o = Some(openstory_bot::runtime::RunOutcome::Failed);
            }
        }
        assert_eq!(s.needing_attention(), 1, "登录失败要计入需人工");
        // b 再走一次登记 (Failed → Stopped, 仍然 needs_attention)
        s.note_exits(std::time::Instant::now());
        assert_eq!(s.needing_attention(), 1);
        assert_eq!(s.waiting_restart(), 2, "a 与 c 在等拉起");
    }

    #[test]
    fn live_count_counts_running_and_waiting_only() {
        let mut s = ended_set(&["a", "b"]);
        assert_eq!(s.live_count(), 0, "都结束了且还没排定 → 0");
        assert!(s.all_stopped());
        s.note_exits(std::time::Instant::now());
        assert_eq!(s.live_count(), 2, "等退避也算活着 (还会再动)");
        assert!(!s.all_stopped());
        // a 被拉起跑起来了, b 还在等退避 → 两个都算活着
        {
            let a = s.iter_mut().nth(0).unwrap();
            assert!(a.begin_respawn(None).is_some());
            a.stage.store(
                crate::session::STAGE_RUNNING,
                std::sync::atomic::Ordering::SeqCst,
            );
        }
        assert_eq!(s.live_count(), 2, "跑着的也算活着");
        // a 跑起来之后正常退出 (不是掉线) → 不该再算活着; b 还在等重连
        {
            let a = s.iter_mut().nth(0).unwrap();
            a.reset_watcher(); // 手动拉起路径的收尾 (真实路径由 begin_respawn 做)
            if let Ok(mut o) = a.outcome.lock() {
                *o = Some(openstory_bot::runtime::RunOutcome::Quit);
            }
            a.stage.store(
                crate::session::STAGE_FINISHED,
                std::sync::atomic::Ordering::SeqCst,
            );
        }
        assert_eq!(s.live_count(), 1, "结束且不重连的不算活着");
        assert!(!s.all_stopped(), "b 还在等重连, 不能说全部停了");
    }

    #[test]
    fn all_stopped_is_false_for_an_empty_set() {
        // 空集合不是"全部停止" —— 否则主线程会立刻收尾, 连向导都来不及显示
        let s = SessionSet::new();
        assert!(!s.all_stopped());
        assert_eq!(s.live_count(), 0);
    }

    #[test]
    fn advance_watchers_reports_a_session_without_a_template() {
        // 没有模板 = 档案从未加载成功 → 不能拉起, 而且要**关掉**自动拉起,
        // 否则 UI 会一直显示一个不会发生的倒计时。
        let specs = vec![SessionSpec::new("profiles/ghost.json")];
        let mut s = SessionSet::from_specs_with_policy(&specs, fast_policy());
        {
            let sess = s.iter_mut().next().unwrap();
            sess.stage.store(
                crate::session::STAGE_FINISHED,
                std::sync::atomic::Ordering::SeqCst,
            );
            if let Ok(mut o) = sess.outcome.lock() {
                *o = Some(openstory_bot::runtime::RunOutcome::ConnectionClosed);
            }
        }
        s.note_exits(std::time::Instant::now());
        // 到点之后 (backoff = 0)
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let later = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let (spawned, errs) = s.advance_watchers(rt.handle(), later, None);
        assert!(spawned.is_empty(), "没模板不该拉起");
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("档案未能加载"), "got {}", errs[0]);
        assert!(
            !s.iter().next().unwrap().restart_enabled(),
            "拉不起来之后必须关掉自动拉起"
        );
    }

    #[test]
    fn manual_restart_on_a_running_session_only_clears_the_counter() {
        // 一个正常工作的号不该被"手动重启"踢下线 —— 只清计数
        let mut s = ended_set(&["a"]);
        {
            let a = s.iter_mut().next().unwrap();
            a.stage.store(
                crate::session::STAGE_RUNNING,
                std::sync::atomic::Ordering::SeqCst,
            );
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert!(s
            .manual_restart(rt.handle(), 0, std::time::Instant::now(), None)
            .is_ok());
        let a = s.iter().next().unwrap();
        assert!(a.is_running(), "还在跑的会话不该被重启掉");
        assert!(!a.gave_up_restart());
        assert_eq!(a.generation(), 0, "没有真的重新拉起, 代数不该变");
    }

    #[test]
    fn manual_restart_rejects_a_bad_index() {
        let mut s = ended_set(&["a"]);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = s
            .manual_restart(rt.handle(), 9, std::time::Instant::now(), None)
            .unwrap_err();
        assert!(err.contains("不存在"), "got {err}");
    }

    #[test]
    fn manual_restart_reports_a_missing_template() {
        let specs = vec![SessionSpec::new("profiles/ghost.json")];
        let mut s = SessionSet::from_specs_with_policy(&specs, fast_policy());
        s.iter_mut().next().unwrap().stage.store(
            crate::session::STAGE_FINISHED,
            std::sync::atomic::Ordering::SeqCst,
        );
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = s
            .manual_restart(rt.handle(), 0, std::time::Instant::now(), None)
            .unwrap_err();
        assert!(err.contains("档案未能加载"), "got {err}");
    }

    // ── 凭据存储与拉起 (密码双模式) ──────────────────────────────────

    /// 拉起时必须用**凭据存储里**的密码, 而不是模板里那个 (可能是空的)。
    ///
    /// 这是"记住密码"模式的全部意义: 档案里没写密码时, 自动拉起要靠它才能
    /// 登录成功; 补不上就会在登录阶段失败并被判定为"认证问题, 不该重试" ——
    /// 于是自动拉起对它永远无效, 用户却以为开着。
    #[tokio::test]
    async fn manual_restart_uses_the_credential_store_password() {
        let mut s = ended_set(&["acc0"]);
        // 模板里是错的密码 (模拟"档案里没写 login 节")
        {
            let sess = s.iter_mut().next().unwrap();
            let mut cfg = sess.template().unwrap();
            cfg.password = "WRONG".into();
            sess.set_template(cfg);
        }
        let mut creds =
            crate::credentials::Credentials::in_memory(crate::credentials::Remember::No);
        creds.record("acc0", "t", "RIGHT");
        s.manual_restart(
            &tokio::runtime::Handle::current(),
            0,
            std::time::Instant::now(),
            Some(&creds),
        )
        .unwrap();
        let sess = s.iter().next().unwrap();
        assert_eq!(sess.generation(), 1, "应已拉起");
        assert_eq!(
            sess.template().unwrap().password,
            "RIGHT",
            "拉起必须用凭据存储里的密码"
        );
    }

    #[tokio::test]
    async fn restart_keeps_the_template_password_when_the_store_has_none() {
        // 存储里没有 → 不能把模板里的密码清掉 (档案里的 login 节是用户写的,
        // 优先级更高)
        let mut s = ended_set(&["acc0"]);
        {
            let sess = s.iter_mut().next().unwrap();
            let mut cfg = sess.template().unwrap();
            cfg.password = "FROM_PROFILE".into();
            sess.set_template(cfg);
        }
        let creds = crate::credentials::Credentials::in_memory(crate::credentials::Remember::No);
        s.manual_restart(
            &tokio::runtime::Handle::current(),
            0,
            std::time::Instant::now(),
            Some(&creds),
        )
        .unwrap();
        assert_eq!(
            s.iter().next().unwrap().template().unwrap().password,
            "FROM_PROFILE"
        );
    }

    #[tokio::test]
    async fn advance_watchers_uses_the_credential_store_password() {
        // 自动拉起那条路径也要补密码 (与手动重启是两条独立的路径, 都得测)
        let mut s = ended_set(&["acc0"]);
        {
            let sess = s.iter_mut().next().unwrap();
            let mut cfg = sess.template().unwrap();
            cfg.password = String::new();
            sess.set_template(cfg);
        }
        s.note_exits(std::time::Instant::now());
        let mut creds =
            crate::credentials::Credentials::in_memory(crate::credentials::Remember::No);
        creds.record("acc0", "t", "STORED_PW");
        let later = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let (spawned, errs) =
            s.advance_watchers(&tokio::runtime::Handle::current(), later, Some(&creds));
        assert_eq!(spawned, vec![0], "应被拉起 ({errs:?})");
        assert_eq!(
            s.iter().next().unwrap().template().unwrap().password,
            "STORED_PW"
        );
    }

    #[tokio::test]
    async fn restart_without_credentials_still_spawns() {
        // 没有密码时**照样**拉起 (也许服务器不校验), 失败之后由核心层返回
        // `Failed` 并标记「需人工」—— 这里验的是"没密码不会让拉起本身被拒绝"。
        let mut s = ended_set(&["acc0"]);
        {
            let sess = s.iter_mut().next().unwrap();
            let mut cfg = sess.template().unwrap();
            cfg.password = String::new();
            sess.set_template(cfg);
        }
        s.note_exits(std::time::Instant::now());
        let later = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let (spawned, errs) = s.advance_watchers(&tokio::runtime::Handle::current(), later, None);
        assert_eq!(spawned, vec![0], "没凭据也必须拉起 ({errs:?})");
        assert!(s
            .iter()
            .next()
            .unwrap()
            .template()
            .unwrap()
            .password
            .is_empty());
    }

    // ── 手动启停 (阶段 8) ─────────────────────────────────────────────

    /// 造一个"从未启动"的集合 (手动模式的初始状态), 每个会话都有模板+密码。
    fn fresh_set(names: &[&str]) -> SessionSet {
        let specs: Vec<SessionSpec> = names
            .iter()
            .map(|n| SessionSpec::new(format!("profiles/{n}.json")))
            .collect();
        let mut s = SessionSet::from_specs_with_policy(&specs, fast_policy());
        for i in 0..s.len() {
            let sess = s.iter_mut().nth(i).unwrap();
            sess.set_template(openstory_bot::config::Config {
                ip: "127.0.0.1".to_string(),
                port: 1,
                account: "t".to_string(),
                password: "t".to_string(),
                ..Default::default()
            });
        }
        s
    }

    #[test]
    fn a_fresh_set_starts_entirely_unstarted() {
        // 手动模式的初始状态: 一个都不许算"活着", 否则程序永远不会退出。
        let s = fresh_set(&["a", "b"]);
        assert_eq!(s.live_count(), 0, "从未启动的会话不算活着");
        assert!(s.all_stopped(), "全都是未启动 → 可以收尾");
        assert!(s.has_never_started(), "但还有号没动过 → 不许收尾");
        assert_eq!(s.never_started_count(), 2);
        assert!(s.all_stopped_by_user(), "全是未启动");
    }

    #[tokio::test]
    async fn start_session_spawns_and_marks_it_started() {
        let mut s = fresh_set(&["a", "b"]);
        let r = s.start_session(&tokio::runtime::Handle::current(), 0, None);
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(s.iter().next().unwrap().generation(), 1, "必须真的 spawn 了");
        assert!(!s.iter().next().unwrap().never_started());
        assert_eq!(s.never_started_count(), 1, "只启动了第一个");
        assert_eq!(s.live_count(), 1);
    }

    #[tokio::test]
    async fn start_session_refuses_a_second_time() {
        // 同一个账号被登录两次 = 服务器踢掉先来的那个。必须挡住。
        let mut s = fresh_set(&["a"]);
        assert!(s
            .start_session(&tokio::runtime::Handle::current(), 0, None)
            .is_ok());
        let r = s.start_session(&tokio::runtime::Handle::current(), 0, None);
        assert!(r.is_err(), "第二次启动必须被拒: {r:?}");
        assert_eq!(s.iter().next().unwrap().generation(), 1, "代数不该再加");
    }

    #[tokio::test]
    async fn start_session_without_a_password_reports_it_instead_of_failing_silently() {
        // 手动模式的价值是"想连的时候才连", 而那时多半还没输过密码。
        // 静默失败的话用户会对着一个没反应的界面反复按键。
        let mut s = fresh_set(&["a"]);
        {
            let sess = s.iter_mut().next().unwrap();
            let mut cfg = sess.template().unwrap();
            cfg.password = String::new();
            sess.set_template(cfg);
        }
        let r = s.start_session(&tokio::runtime::Handle::current(), 0, None);
        let e = r.unwrap_err();
        assert!(e.contains("没有可用密码"), "错误必须说清原因: {e}");
        // 失败要落到"已停止 + 启动失败" —— **不能**留在"从未启动"
        let sess = s.iter().next().unwrap();
        assert!(sess.start_failed(), "必须标记启动失败");
        assert!(sess.is_manually_stopped());
        assert!(
            !s.has_never_started(),
            "启动失败之后不能再算'从未启动' —— 否则程序永远不肯退出"
        );
        // 而且没有 spawn 出来
        assert_eq!(sess.generation(), 0);
    }

    #[tokio::test]
    async fn start_session_uses_a_stored_password() {
        let mut s = fresh_set(&["a"]);
        {
            let sess = s.iter_mut().next().unwrap();
            let mut cfg = sess.template().unwrap();
            cfg.password = String::new();
            sess.set_template(cfg);
        }
        let mut creds =
            crate::credentials::Credentials::in_memory(crate::credentials::Remember::No);
        creds.record("a", "t", "STORED_PW");
        let r = s.start_session(&tokio::runtime::Handle::current(), 0, Some(&creds));
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(
            s.iter().next().unwrap().template().unwrap().password,
            "STORED_PW"
        );
    }

    #[tokio::test]
    async fn stop_session_then_start_again_works() {
        // 停 → 启 是手动模式最常用的一对操作, 中间不能有死锁。
        let mut s = fresh_set(&["a"]);
        let h = tokio::runtime::Handle::current();
        assert!(s.start_session(&h, 0, None).is_ok());
        assert!(s.stop_session(0).is_ok());
        let sess = s.iter().next().unwrap();
        assert!(sess.is_manually_stopped());
        assert!(!sess.restart_enabled(), "stop 必须关掉自动拉起");

        // bot 收尾 (真实路径里由监督任务置位)
        sess.stage.store(
            crate::session::STAGE_FINISHED,
            std::sync::atomic::Ordering::SeqCst,
        );
        let r = s.start_session(&h, 0, None);
        assert!(r.is_ok(), "收尾之后必须能重新启动: {r:?}");
        assert_eq!(s.iter().next().unwrap().generation(), 2);
        assert!(!s.iter().next().unwrap().is_manually_stopped());
    }

    #[test]
    fn stop_session_refuses_a_never_started_session() {
        // 没跑过的会话按 stop 没有意义 —— 报错而不是假装成功。
        let mut s = fresh_set(&["a"]);
        let e = s.stop_session(0).unwrap_err();
        assert!(e.contains("本来就没在运行"), "got {e}");
        // 而且不能因为我们探了一下就把它变成"已停止"
        assert!(s.iter().next().unwrap().never_started(), "状态必须没变");
        assert!(s.has_never_started());
    }

    #[tokio::test]
    async fn stop_session_accepts_a_finished_session_that_needs_attention() {
        // 已结束但亮着"需人工"的会话: 用户的 stop 意图是"这个号我不要了,
        // 别再提示我"。必须让它落到"已停止" —— 否则按了没反应, 界面继续亮红。
        let mut s = ended_set(&["a"]);
        {
            // 用"登录失败"制造"需人工": 断线只会在正常退避里等 (那不算需人工)
            let sess = s.iter_mut().next().unwrap();
            if let Ok(mut o) = sess.outcome.lock() {
                *o = Some(openstory_bot::runtime::RunOutcome::Failed);
            }
        }
        let t = std::time::Instant::now();
        s.note_exits(t);
        assert!(s.iter().next().unwrap().needs_attention(), "前置: 需人工");
        let r = s.stop_session(0);
        assert!(r.is_ok(), "已结束的会话应该能停: {r:?}");
        let sess = s.iter().next().unwrap();
        assert!(sess.is_manually_stopped());
        assert!(!sess.needs_attention(), "停掉之后不该继续亮红");
        assert!(!sess.is_waiting_restart(), "等待窗口也要清掉");
        // 也确认它不会在下一轮被拉起
        let (spawned, errs) = s.advance_watchers(
            &tokio::runtime::Handle::current(),
            t + std::time::Duration::from_secs(120),
            None,
        );
        assert!(spawned.is_empty(), "{spawned:?} {errs:?}");
    }

    #[tokio::test]
    async fn a_manually_stopped_session_is_never_respawned_by_the_watcher() {
        // 端到端: 停掉 → bot 报"连接已关闭" → 退避到点 → **不该**被拉起。
        let mut s = fresh_set(&["a"]);
        let h = tokio::runtime::Handle::current();
        assert!(s.start_session(&h, 0, None).is_ok());
        assert!(s.stop_session(0).is_ok());
        // bot 用它自己的方式结束 (abort/断开都会归约成这个)
        {
            let sess = s.iter_mut().next().unwrap();
            sess.stage.store(
                crate::session::STAGE_FINISHED,
                std::sync::atomic::Ordering::SeqCst,
            );
            if let Ok(mut o) = sess.outcome.lock() {
                *o = Some(openstory_bot::runtime::RunOutcome::ConnectionClosed);
            }
        }
        let t = std::time::Instant::now();
        assert!(s.note_exits(t).is_empty(), "手动停掉的不该被登记为待拉起");
        let later = t + std::time::Duration::from_secs(60);
        let (spawned, errs) = s.advance_watchers(&h, later, None);
        assert!(spawned.is_empty(), "绝不能自己起来: {spawned:?} {errs:?}");
        assert_eq!(s.iter().next().unwrap().generation(), 1, "代数不该变");
    }

    #[tokio::test]
    async fn dispatch_builtin_starts_and_stops_by_target() {
        let mut s = fresh_set(&["serverA_a", "serverA_b", "serverB_c"]);
        let h = tokio::runtime::Handle::current();
        // 只启动 serverA_ 前缀的两个
        let (hit, ok, errs) = s.dispatch_builtin(
            &h,
            &Target::Prefix("serverA".into()),
            Builtin::Start,
            None,
        );
        assert_eq!((hit, ok), (2, 2), "{errs:?}");
        assert_eq!(s.never_started_count(), 1, "serverB_c 应仍是未启动");
        // 全部停掉
        let (hit, ok, errs) = s.dispatch_builtin(&h, &Target::All, Builtin::Stop, None);
        // serverB_c 从没启动 → 它那条是"本来就没在运行", 属于可预期的失败
        assert_eq!(hit, 3);
        assert_eq!(ok, 2, "{errs:?}");
        assert_eq!(errs.len(), 1);
        assert!(errs[0].0.contains("serverB_c"), "失败项要带会话名");
    }

    #[test]
    fn builtin_for_target_only_recognises_start_and_stop() {
        assert_eq!(
            SessionSet::builtin_for_target("start"),
            Some(Builtin::Start)
        );
        assert_eq!(SessionSet::builtin_for_target("stop"), Some(Builtin::Stop));
        assert_eq!(
            SessionSet::builtin_for_target("  start  "),
            Some(Builtin::Start),
            "前后空白要容忍"
        );
        // 别的词都是普通指令, 必须原样发给 bot
        assert_eq!(SessionSet::builtin_for_target("starts"), None);
        assert_eq!(SessionSet::builtin_for_target("hunt on"), None);
        assert_eq!(SessionSet::builtin_for_target("start extra"), None);
        assert_eq!(SessionSet::builtin_for_target(""), None);
    }

    // ── 运行时增删账号 (阶段 9) ───────────────────────────────────────

    #[test]
    fn builtin_parses_the_profiles_command_family() {
        use Builtin::*;
        assert_eq!(
            SessionSet::builtin_for_target("profiles"),
            Some(ListProfiles),
            "裸 profiles = 列出"
        );
        assert_eq!(
            SessionSet::builtin_for_target("profiles list"),
            Some(ListProfiles)
        );
        assert_eq!(
            SessionSet::builtin_for_target("accounts ls"),
            Some(ListProfiles),
            "别名"
        );
        assert_eq!(
            SessionSet::builtin_for_target("profiles add profiles/x.json"),
            Some(AddProfile(std::path::PathBuf::from("profiles/x.json")))
        );
        assert_eq!(
            SessionSet::builtin_for_target("profiles add \"profiles/a b.json\""),
            Some(AddProfile(std::path::PathBuf::from("profiles/a b.json"))),
            "带空格的路径用引号包起来"
        );
        assert_eq!(
            SessionSet::builtin_for_target("profiles remove x"),
            Some(RemoveProfile("x".to_string()))
        );
        assert_eq!(
            SessionSet::builtin_for_target("profiles rm x"),
            Some(RemoveProfile("x".to_string()))
        );
        // 缺参数 → 解析成必然失败的动作, 由错误信息告诉用户怎么用
        assert_eq!(
            SessionSet::builtin_for_target("profiles add"),
            Some(AddProfile(std::path::PathBuf::new()))
        );
        assert_eq!(
            SessionSet::builtin_for_target("profiles remove"),
            Some(RemoveProfile(String::new()))
        );
        // 未知子命令不认: 交给 bot 报"未知指令", 比静默好
        assert_eq!(SessionSet::builtin_for_target("profiles frobnicate"), None);
        // 只有 start/stop 是按会话批量执行的
        assert!(Start.is_per_session() && Stop.is_per_session());
        assert!(!ListProfiles.is_per_session());
        assert!(!AddProfile(std::path::PathBuf::new()).is_per_session());
    }

    /// 造一个真实的档案文件 (加账号要能从盘上读出来)。
    fn write_profile(dir: &std::path::Path, name: &str) -> PathBuf {
        let p = dir.join(format!("{name}.json"));
        std::fs::write(
            &p,
            r#"{"tick_ms":50,"login":{"account":"999","ip":"127.0.0.1","port":8484}}"#,
        )
        .unwrap();
        p
    }

    #[test]
    fn each_profile_gets_its_own_account_not_the_inherited_one() {
        // 现场 bug (用户报的"两个号相互重连、不停挤兑"):
        //
        // `--profiles a.json b.json`, 两个会话的模板都从**全局 config** 克隆,
        // 而那份 config 的账号来自第一个档案 (或 config.json)。`apply_login`
        // 是填空缺语义, 见 account 非空就跳过 → 两个会话都登 A。
        //
        // 症状: 选中第二个号按 F4, 向导预填的是 A 的账号; 提交后同一个号被登
        // 两次, 服务器把两边来回踢。
        let dir = std::env::temp_dir().join(format!("ost_tmpl_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.json");
        let b = dir.join("b.json");
        std::fs::write(
            &a,
            r#"{"login":{"account":"100000001","ip":"127.0.0.1","port":8484}}"#,
        )
        .unwrap();
        std::fs::write(
            &b,
            r#"{"login":{"account":"100000002","ip":"127.0.0.1","port":8484}}"#,
        )
        .unwrap();

        // 全局 config: 先按第一个档案填好账号 (lib.rs 的启动流程就是这么做的)
        let mut global = openstory_bot::config::Config::parse_partial(&[]).unwrap();
        global.account = "100000001".into();

        let ta = session_template(&global, &a);
        let tb = session_template(&global, &b);
        assert_eq!(ta.account, "100000001", "第一个号用 A");
        assert_eq!(
            tb.account, "100000002",
            "第二个号必须用**它自己档案**的账号, 不能继承 A"
        );
        // 两个模板必须指向各自的档案 (reload / hunt save 的写回目标)
        assert!(ta.config_paths.last().unwrap().ends_with("a.json"));
        assert!(tb.config_paths.last().unwrap().ends_with("b.json"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_profile_without_a_login_section_still_loads() {
        // 档案里没有 login 节 → 模板照常可建 (账号留空, 由向导/凭据存储补),
        // 不能因为"读不到 login"就 panic 或丢掉模板。
        let dir = std::env::temp_dir().join(format!("ost_tmpl2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bare.json");
        std::fs::write(&p, r#"{"tick_ms":50}"#).unwrap();

        let global = openstory_bot::config::Config::parse_partial(&[]).unwrap();
        let t = session_template(&global, &p);
        assert_eq!(t.account, "", "没有 login 节就没有账号");
        assert!(
            t.config_paths.last().unwrap().ends_with("bare.json"),
            "模板仍要指向该档案"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_profile_hangs_it_up_unstarted() {
        let dir = std::env::temp_dir().join(format!("ost_add_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = write_profile(&dir, "newacc");

        let mut s = fresh_set(&["a"]);
        assert_eq!(s.len(), 1);
        let idx = s.add_profile(path.clone()).expect("应该能挂上");
        assert_eq!(idx, 1);
        assert_eq!(s.len(), 2);
        let added = s.iter().nth(1).unwrap();
        assert_eq!(added.profile, "newacc");
        assert!(
            added.never_started(),
            "新挂上的账号必须是「未启动」—— 加账号不该顺手把它登上去"
        );
        assert!(
            added.template().is_some(),
            "必须从文件读出模板, 否则按 F4 会被拒 (档案未能加载)"
        );
        assert_eq!(added.template().unwrap().account, "999");
        assert_eq!(s.never_started_count(), 2, "两个都是从没启动");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_profile_rejects_the_same_file_twice() {
        let dir = std::env::temp_dir().join(format!("ost_add2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = write_profile(&dir, "dup");

        let mut s = fresh_set(&["a"]);
        assert!(s.add_profile(path.clone()).is_ok());
        let e = s.add_profile(path.clone()).unwrap_err();
        assert!(e.contains("已经挂上了"), "got {e}");
        assert_eq!(s.len(), 2, "不该重复挂同一个文件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_profile_rejects_a_missing_file() {
        // 挂一个不存在的文件是纯噪音: 它会永远启动不了
        let mut s = fresh_set(&["a"]);
        let e = s
            .add_profile(PathBuf::from("profiles/definitely_not_here_12345.json"))
            .unwrap_err();
        assert!(e.contains("文件不存在"), "got {e}");
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn remove_profile_refuses_a_running_session() {
        // 摘掉一个正在跑的会话 = bot task 变孤儿继续跑, 而界面上再也看不到它。
        let mut s = fresh_set(&["a", "b"]);
        {
            let a = s.iter_mut().next().unwrap();
            a.stage.store(
                crate::session::STAGE_RUNNING,
                std::sync::atomic::Ordering::SeqCst,
            );
        }
        let e = s.remove_profile("a").unwrap_err();
        assert!(e.contains("先 stop"), "错误要告诉用户怎么办: {e}");
        assert_eq!(s.len(), 2, "不许摘掉");

        // 停掉之后可以摘
        assert!(s.stop_session(0).is_ok());
        s.iter_mut().next().unwrap().stage.store(
            crate::session::STAGE_FINISHED,
            std::sync::atomic::Ordering::SeqCst,
        );
        let (idx, name) = s.remove_profile("a").expect("停掉之后应该能摘");
        assert_eq!((idx, name.as_str()), (0, "a"));
        assert_eq!(s.len(), 1);
        assert_eq!(s.iter().next().unwrap().profile, "b");
    }

    #[test]
    fn remove_profile_keeps_the_receivers_aligned() {
        // `rxs` 与 `sessions` 必须**同序** —— 错位之后把某个会话的接收端交给
        // 另一个会话, 指令就会发到错误的账号上 (而且是静默的)。
        let mut s = fresh_set(&["a", "b", "c"]);
        assert_eq!(s.rxs.len(), 3);
        s.remove_profile("b").unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s.rxs.len(), 2, "接收端数量必须跟着变");
        // 用"发一条指令, 从接收端读回来"验证没有错位
        s.iter().next().unwrap().send("for-a").unwrap();
        let mut rxs = s.pull_receivers();
        assert_eq!(rxs[0].try_recv().unwrap(), "for-a", "第 0 个接收端属于 a");
        assert!(rxs[1].try_recv().is_err(), "b 的接收端不该收到");
        assert_eq!(s.iter().nth(1).unwrap().profile, "c");
    }

    #[test]
    fn remove_profile_clamps_the_selection() {
        let mut s = fresh_set(&["a", "b"]);
        s.select(1);
        s.remove_profile("b").unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s.selected_index(), 0, "选中的被摘掉后要落到最后一个");
    }

    #[test]
    fn remove_profile_reports_an_unknown_name() {
        let mut s = fresh_set(&["a"]);
        let e = s.remove_profile("nope").unwrap_err();
        assert!(e.contains("没有这个档案"), "got {e}");
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn status_lines_cover_every_state() {
        // `profiles list` 是用户看"现在都挂了谁、各是什么状态"的唯一入口,
        // 每种状态都要有明确文字 (显示 "?" 等于没说)。
        let mut s = fresh_set(&["fresh", "stopped", "dead"]);
        {
            let st = s.iter_mut().nth(1).unwrap();
            st.stage.store(
                crate::session::STAGE_RUNNING,
                std::sync::atomic::Ordering::SeqCst,
            );
            st.mark_stopped_by_user();
        }
        {
            let d = s.iter_mut().nth(2).unwrap();
            d.stage.store(
                crate::session::STAGE_FINISHED,
                std::sync::atomic::Ordering::SeqCst,
            );
        }
        let lines = s.status_lines();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("未启动"), "{}", lines[0]);
        assert!(lines[1].contains("已停止"), "{}", lines[1]);
        assert!(lines[2].contains("已结束"), "{}", lines[2]);
        assert!(
            !lines.iter().any(|l| l.contains('?')),
            "每种状态都要有文字: {lines:?}"
        );
    }
}
