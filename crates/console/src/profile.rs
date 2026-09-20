//! 配置档案 (profiles) 管理: 启动时选择要加载的配置文件。
//!
//! 目录约定: 工作目录下 `profiles/*.json` 为档案, 一份档案 = 一个
//! 服×账号 的完整独立配置 (login/tasks/rules/groups/hunt)。下划线开头
//! 的文件视为库文件, 不出现在选择列表。根目录旧 `config.json` 作为
//! 兜底条目追加在列表末尾。
//!
//! 交互: ↑↓ 选择 · Enter 确认 · N 新建(输入名称, 复制 config.default.json
//! 模板) · C 复制选中档案 · D 删除(再按一次确认) · Esc 退出程序。

use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

/// 档案目录名 (相对工作目录)。
pub const PROFILES_DIR: &str = "profiles";

/// 选择列表里的一条档案。
#[derive(Debug, Clone)]
pub struct ProfileEntry {
    /// 显示名 (文件名去 .json; 旧根文件固定 "config.json")。
    pub name: String,
    pub path: PathBuf,
    /// login 节摘要: "账号 @ ip" 或 "(无 login 节)"。
    pub summary: String,
}

/// 扫描 profiles/ 目录 + 根 config.json 兜底。按名称排序, 旧的根文件排最后。
pub fn scan() -> Vec<ProfileEntry> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(PROFILES_DIR) {
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|x| x.to_str())
                .unwrap_or_default()
                .to_string();
            // 下划线开头 = 库/共享文件, 不作为可启动档案展示
            if stem.starts_with('_') {
                continue;
            }
            let summary = summarize(&path);
            out.push(ProfileEntry {
                name: stem,
                path,
                summary,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    let legacy_path = PathBuf::from(openstory_bot::runtime_config::CONFIG_PATH);
    if legacy_path.exists() {
        out.push(ProfileEntry {
            name: "config.json".into(),
            path: legacy_path.clone(),
            summary: summarize(&legacy_path),
        });
    }
    out
}

/// 从文件的 login 节提取一行摘要 (含 data_dir 信息)。
fn summarize(path: &Path) -> String {
    let data_dir_hint = extract_data_dir(path);
    let login_hint = match openstory_bot::login::load(path) {
        Some(info) => {
            let acct = if info.account.is_empty() {
                "?"
            } else {
                &info.account
            };
            let ip = if info.ip.is_empty() { "?" } else { &info.ip };
            format!("{acct} @ {ip}")
        }
        None => "(无 login 节)".to_string(),
    };
    match data_dir_hint.as_deref() {
        Some(dir) => format!("{login_hint}  [{dir}]"),
        None => login_hint,
    }
}

/// 从 profile JSON 中提取 data_dir 字段。
fn extract_data_dir(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("data_dir")?.as_str().map(|s| s.to_string())
}

/// 选择屏动作结果。
#[derive(Debug, Clone, PartialEq)]
pub enum PickAction {
    /// 确认选择该档案路径。
    Select(PathBuf),
    /// **多选**确认: 用这些档案一起进控制台 (`Space` 勾选过)。
    ///
    /// 与 [`PickAction::Select`] 分开是刻意的 —— 两者进去之后的行为不同:
    /// 单选 = "就这个号, 连上"; 多选 = "把这几个号挂上, 我决定谁连"
    /// (会话全部停在「未启动」)。合成一个的话前端没法区分这两种意图。
    SelectMany(Vec<PathBuf>),
    /// Esc / Ctrl+C 退出程序。
    Cancel,
    /// 无动作 (继续等待输入)。
    None,
}

/// 选择屏的结束方式 (供 [`pick_many`] 的调用方区分"单选"与"多选")。
#[derive(Debug, Clone, PartialEq)]
pub enum PickOutcome {
    /// 单选一个档案 (`Enter` 且没有勾选任何项)。
    One(PathBuf),
    /// 多选 (`Space` 勾选后 `Enter`)。至少两个才有意义, 但一个也会走到这里
    /// (用户勾了一个再确认)。
    Many(Vec<PathBuf>),
    /// 用户取消。
    Cancel,
}

/// 选择屏状态机 (纯逻辑, 可单测; 渲染与事件循环在 draw/run 中)。
pub struct PickerState {
    pub entries: Vec<ProfileEntry>,
    pub cursor: usize,
    /// 已勾选的档案下标 (`Space` 勾选)。
    ///
    /// 用下标集合而不是路径集合: 列表会因为 N/C/D 而增删/排序, 路径集合在
    /// 删除一条之后就对不上了, 而"第几条"在增删时由下面的代码同步修正。
    ///
    /// 非空即代表"多选模式": `Enter` 会把它们一起带进控制台。
    pub marked: std::collections::BTreeSet<usize>,
    /// 输入模式: None = 列表浏览; Some(buf) = 正在输入新档案名。
    pub name_input: Option<String>,
    /// 输入模式语义: true = C 复制当前选中项, false = N 全新模板。
    pub pending_copy: bool,
    /// 删除二次确认: Some(index) = 已对第 index 项按下一次 D。
    pub delete_arm: Option<usize>,
    /// 底部状态提示 (错误/确认信息), 空则显示默认按键说明。
    pub status: String,
}

impl PickerState {
    pub fn new(entries: Vec<ProfileEntry>) -> Self {
        Self {
            entries,
            cursor: 0,
            marked: std::collections::BTreeSet::new(),
            name_input: None,
            pending_copy: false,
            delete_arm: None,
            status: String::new(),
        }
    }

    /// 已勾选的档案路径 (按列表顺序)。
    pub fn marked_paths(&self) -> Vec<PathBuf> {
        self.marked
            .iter()
            .filter_map(|&i| self.entries.get(i).map(|e| e.path.clone()))
            .collect()
    }

    /// 勾选/取消勾选光标所在项 (Space)。
    fn toggle_mark(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        if !self.marked.remove(&self.cursor) {
            self.marked.insert(self.cursor);
        }
        let n = self.marked.len();
        self.status = if n == 0 {
            "已取消全部勾选 (Enter = 只用光标这一项)".to_string()
        } else {
            format!("已勾选 {n} 项 · Enter 一起进入 (它们会停在「未启动」)")
        };
    }

    /// 全选 / 全不选 (A)。
    fn toggle_mark_all(&mut self) {
        if self.marked.len() == self.entries.len() {
            self.marked.clear();
            self.status = "已取消全部勾选".to_string();
        } else {
            self.marked = (0..self.entries.len()).collect();
            self.status = format!("已勾选全部 {} 项", self.entries.len());
        }
    }

    /// 处理一个按键, 返回动作。纯函数 (仅改自身状态)。
    pub fn on_key(&mut self, key: KeyEvent) -> PickAction {
        if key.kind == ratatui::crossterm::event::KeyEventKind::Release {
            return PickAction::None;
        }
        // Ctrl+C 任何模式直接退出
        if key
            .modifiers
            .contains(ratatui::crossterm::event::KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('c')
        {
            return PickAction::Cancel;
        }
        // 名称输入模式: 只处理文本编辑键
        if self.name_input.is_some() {
            return self.on_name_input_key(key);
        }
        match key.code {
            KeyCode::Esc => PickAction::Cancel,
            KeyCode::Up | KeyCode::Char('k') => {
                self.delete_arm = None;
                if !self.entries.is_empty() {
                    self.cursor = (self.cursor + self.entries.len() - 1) % self.entries.len();
                }
                self.status.clear();
                PickAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.delete_arm = None;
                if !self.entries.is_empty() {
                    self.cursor = (self.cursor + 1) % self.entries.len();
                }
                self.status.clear();
                PickAction::None
            }
            KeyCode::Enter => {
                self.delete_arm = None;
                if let Some(e) = self.entries.get(self.cursor) {
                    // 勾选过就一起带进去 (多开/管理形态); 否则只用光标这一项。
                    if !self.marked.is_empty() {
                        let paths = self.marked_paths();
                        if paths.is_empty() {
                            self.status = "勾选项已失效 (列表变了) — 请重新勾选".into();
                            return PickAction::None;
                        }
                        return PickAction::SelectMany(paths);
                    }
                    PickAction::Select(e.path.clone())
                } else {
                    self.status = "目录为空: 按 N 新建档案, 或 Esc 退出".into();
                    PickAction::None
                }
            }
            KeyCode::Char(' ') => {
                self.delete_arm = None;
                self.toggle_mark();
                PickAction::None
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.delete_arm = None;
                self.toggle_mark_all();
                PickAction::None
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.delete_arm = None;
                self.name_input = Some(String::new());
                self.pending_copy = false;
                self.status.clear();
                PickAction::None
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                self.delete_arm = None;
                if let Some(src) = self.entries.get(self.cursor) {
                    let base = src.name.trim_end_matches("_copy");
                    self.name_input = Some(format!("{base}_copy"));
                    self.pending_copy = true;
                    self.status.clear();
                }
                PickAction::None
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                // 注意: 此分支不能先清 delete_arm — 二次确认依赖上次按 D 留下的状态
                if let Some(e) = self.entries.get(self.cursor) {
                    let legacy = std::path::Path::new(openstory_bot::runtime_config::CONFIG_PATH);
                    if e.path == legacy {
                        self.status = "旧的根 config.json 不可在此删除".into();
                        return PickAction::None;
                    }
                    let idx = self.cursor;
                    if self.delete_arm == Some(idx) {
                        match std::fs::remove_file(&e.path) {
                            Ok(()) => {
                                self.status = format!("已删除 {}", e.name);
                                self.entries.remove(idx);
                                // 勾选集合是**下标**集合, 删掉一条之后必须跟着
                                // 修正: 否则"勾了 3 个"会变成勾着另外三个
                                // (删掉的那个位置后面的全部前移)。
                                self.marked = self
                                    .marked
                                    .iter()
                                    .filter(|&&i| i != idx)
                                    .map(|&i| if i > idx { i - 1 } else { i })
                                    .collect();
                                if self.cursor >= self.entries.len() {
                                    self.cursor = self.entries.len().saturating_sub(1);
                                }
                            }
                            Err(err) => self.status = format!("删除失败: {err}"),
                        }
                        self.delete_arm = None;
                    } else {
                        self.delete_arm = Some(idx);
                        self.status = format!("再按一次 D 确认删除 {}", e.name);
                    }
                }
                PickAction::None
            }
            _ => PickAction::None,
        }
    }

    /// 名称输入模式的按键 (N 新建 / C 复制 共用): Enter 落盘并选中, Esc 取消。
    fn on_name_input_key(&mut self, key: KeyEvent) -> PickAction {
        match key.code {
            KeyCode::Esc => {
                self.name_input = None;
                self.pending_copy = false;
                self.status.clear();
                PickAction::None
            }
            KeyCode::Enter => {
                let raw = self.name_input.take().unwrap_or_default();
                let is_copy_src = std::mem::take(&mut self.pending_copy);
                match validate_profile_name(&raw) {
                    Ok(stem) => {
                        let path = profile_path(&stem);
                        if is_copy_src {
                            if let Some(src) = self.entries.get(self.cursor) {
                                match std::fs::copy(&src.path, &path) {
                                    Ok(_) => {}
                                    Err(err) => {
                                        self.status = format!("复制失败: {err}");
                                        return PickAction::None;
                                    }
                                }
                            }
                        } else {
                            match create_from_template(&path) {
                                Ok(()) => {}
                                Err(err) => {
                                    self.status = format!("新建失败: {err}");
                                    return PickAction::None;
                                }
                            }
                        }
                        self.entries.push(ProfileEntry {
                            summary: summarize(&path),
                            name: stem.clone(),
                            path: path.clone(),
                        });
                        self.entries.sort_by(|a, b| a.name.cmp(&b.name));
                        // 排序会打乱下标 → 旧的勾选集合指向完全不同的档案。
                        // 清掉而不是"尽量修": 一个静默错位的勾选比没有勾选危险得多
                        // (用户以为在操作 A, 实际带进去的是 B)。
                        self.marked.clear();
                        self.cursor = self
                            .entries
                            .iter()
                            .position(|e| e.path == path)
                            .unwrap_or(0);
                        self.status.clear();
                        PickAction::Select(path)
                    }
                    Err(msg) => {
                        self.status = msg;
                        PickAction::None
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(buf) = self.name_input.as_mut() {
                    buf.pop();
                }
                PickAction::None
            }
            KeyCode::Char(c) => {
                if let Some(buf) = self.name_input.as_mut() {
                    if buf.chars().count() < 60 {
                        buf.push(c);
                    }
                }
                PickAction::None
            }
            _ => PickAction::None,
        }
    }

    /// 当前底部提示行内容。
    pub fn hint(&self) -> String {
        if !self.status.is_empty() {
            return format!(" {} ", self.status);
        }
        if self.name_input.is_some() {
            return " 输入档案名 · Enter 确认 · Esc 取消 ".to_string();
        }
        if self.entries.is_empty() {
            " 目录为空 — N 新建档案 · Esc 退出".to_string()
        } else {
            " Space 勾选(可多选) · A 全选 · Enter 进入 · ↑↓ 移动 · N 新建 · C 复制 · D 删除 · Esc 退出"
                .to_string()
        }
    }
}

/// 校验档案名: 非空、不含路径分隔符与非法字符、长度合理; 返回规范化后的 stem。
pub fn validate_profile_name(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("档案名不能为空".into());
    }
    if trimmed.chars().count() > 60 {
        return Err("档案名过长 (最多 60 字符)".into());
    }
    for ch in trimmed.chars() {
        if matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            return Err(format!("档案名不能包含字符: {ch}"));
        }
        if ch.is_control() {
            return Err("档案名不能包含控制字符".into());
        }
    }
    let stem = trimmed.strip_suffix(".json").unwrap_or(trimmed);
    if stem.is_empty() || stem.starts_with('_') {
        return Err("下划线开头的名字保留为库文件, 请换一个名字".into());
    }
    Ok(stem.to_string())
}

/// 档案完整路径 (profiles/<stem>.json)。
pub fn profile_path(stem: &str) -> PathBuf {
    Path::new(PROFILES_DIR).join(format!("{stem}.json"))
}

/// 用仓库模板 config.default.json 创建一份新档案; 缺模板时写最小骨架。
fn create_from_template(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    match std::fs::read_to_string("config.default.json") {
        Ok(text) => std::fs::write(path, text).map_err(|e| e.to_string()),
        Err(_) => std::fs::write(
            path,
            "{\n  \"tick_ms\": 50,\n  \"login\": {},\n  \"rules\": [],\n  \"tasks\": [],\n  \"groups\": []\n}\n",
        )
        .map_err(|e| e.to_string()),
    }
}

/// 渲染选择屏 (全屏居中面板)。
pub fn draw(f: &mut Frame, st: &mut PickerState) {
    let area = f.area();
    f.render_widget(Clear, area);
    let w = area.width.clamp(40, 64);
    let h = area.height.clamp(12, 18);
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let box_area = Rect::new(x, y, w, h);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" 选择配置档案 ")
        .title_style(Style::new().bold())
        .border_style(Style::new().fg(ratatui::style::Color::Yellow));
    let inner = block.inner(box_area);
    f.render_widget(block, box_area);

    let rows = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    if st.entries.is_empty() {
        let msg = vec![
            Line::from("profiles/ 目录还没有档案"),
            Line::from(Span::styled(
                "按 N 新建一个 (基于 config.default.json 模板)",
                Style::new().dim(),
            )),
        ];
        f.render_widget(Paragraph::new(msg).centered(), rows[0]);
    } else {
        let items: Vec<ListItem> = st
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let selected = i == st.cursor;
                let marked = st.marked.contains(&i);
                let marker = if selected { "▶ " } else { "  " };
                // 勾选标记用 [✓]: 光标 (▶) 与勾选是两件事 —— 光标只决定"按 N/C/D
                // 作用在谁身上", 勾选决定"Enter 带谁进去"。不区分的话用户会以为
                // 移动光标就等于改选择。
                let check = if marked { "[✓] " } else { "[ ] " };
                let legacy = e.name == "config.json";
                let title = if legacy {
                    format!("{marker}{check}{} (旧)", e.name)
                } else {
                    format!("{marker}{check}{}", e.name)
                };
                ListItem::new(vec![
                    Line::from(Span::styled(
                        title,
                        if selected {
                            Style::new().bold().fg(ratatui::style::Color::Yellow)
                        } else if marked {
                            Style::new().fg(ratatui::style::Color::LightGreen)
                        } else {
                            Style::new()
                        },
                    )),
                    Line::from(Span::styled(
                        format!("   {}", e.summary),
                        Style::new().dim(),
                    )),
                ])
            })
            .collect();
        let mut ls = ListState::default();
        ls.select(Some(st.cursor));
        f.render_stateful_widget(
            List::new(items).highlight_style(Style::new().add_modifier(Modifier::BOLD)),
            rows[0],
            &mut ls,
        );
    }

    if let Some(buf) = &st.name_input {
        f.render_widget(
            Paragraph::new(Line::from(format!("新档案名 > {buf}▌")))
                .style(Style::new().fg(ratatui::style::Color::LightGreen)),
            rows[1],
        );
    }
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(st.hint(), Style::new().dim()))),
        rows[2],
    );
}

/// 阻塞式选择循环: 直接读 crossterm 事件, 返回用户的选择结果。
/// 在 App/渲染线程启动之前运行, 不与其竞争输入。
/// 按需重绘: 只在初始帧和有事件后才 draw (空闲时不耗 CPU 重绘)。
///
/// 返回的是 [`PickOutcome`] 而不是裸路径 —— **单选与多选必须能区分开**:
/// 单选 = "就这个号, 连上"; 多选 = "把这几个号挂上, 我决定谁连"
/// (会话全部停在「未启动」)。合成一个返回值就没法区分这两种意图了。
pub fn pick_many(
    term: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
) -> PickOutcome {
    let mut st = PickerState::new(scan());
    let mut dirty = true;
    loop {
        if dirty {
            let _ = term.draw(|f| draw(f, &mut st));
            dirty = false;
        }
        if ratatui::crossterm::event::poll(std::time::Duration::from_millis(80)).unwrap_or(false) {
            while ratatui::crossterm::event::poll(std::time::Duration::ZERO).unwrap_or(false) {
                if let Ok(Event::Key(k)) = ratatui::crossterm::event::read() {
                    match st.on_key(k) {
                        PickAction::Select(p) => return PickOutcome::One(p),
                        PickAction::SelectMany(v) => return PickOutcome::Many(v),
                        PickAction::Cancel => return PickOutcome::Cancel,
                        PickAction::None => dirty = true,
                    }
                } else {
                    // 非按键事件 (Resize 等): 重绘适配新尺寸
                    dirty = true;
                }
            }
        }
    }
}

/// 逐档询问缺失的密码 (启动前的最后一步)。
///
/// # 为什么要在启动前问
///
/// 多开时每个档案的密码是独立的。缺密码的那个会话会在登录阶段失败并被标记
/// 「需人工」, 而自动拉起**又需要密码** —— 于是自动拉起对它永远无效, 用户
/// 却以为开着 (左栏还会亮红)。与其让他半夜发现某个号一直是停的, 不如启动前
/// 问一次。
///
/// 界面上先列出**全部**缺密码的档案, 再逐个问 —— 用户一眼就知道要输几个,
/// 而不是被一个个弹窗追着问。
///
/// 返回:
/// - `Ok(Some(map))` —— 用户逐档输入完了 (某一档留空 = 跳过那一档)
/// - `Ok(None)` —— 用户按 Esc 整体跳过 (不阻塞启动, 但调用方要提示后果)
/// - `Err(msg)` —— 终端出错
pub fn ask_passwords(
    term: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    profiles: &[String],
) -> Result<Option<Vec<(String, String)>>, String> {
    if profiles.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let mut out: Vec<(String, String)> = Vec::new();
    for (i, name) in profiles.iter().enumerate() {
        match prompt_one(term, name, i, profiles)? {
            Some(pw) => out.push((name.clone(), pw)),
            None => return Ok(None),
        }
    }
    Ok(Some(out))
}

/// 问一个档案的密码。返回 `None` = 用户按 Esc 放弃全部。
///
/// `all` 是这次要问的**全部**档案名 —— 列在界面上, 让用户知道还剩几个。
fn prompt_one(
    term: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    name: &str,
    idx: usize,
    all: &[String],
) -> Result<Option<String>, String> {
    let mut input = String::new();
    loop {
        let masked: String = "*".repeat(input.chars().count());
        term.draw(|f| {
            let h = 9u16.min(f.area().height);
            let area = centered(f.area(), 70, h);
            f.render_widget(Clear, area);
            let mut lines = vec![
                Line::from(Span::styled(
                    format!(" 需要密码的档案 ({} 个):", all.len()),
                    Style::new().fg(ratatui::style::Color::DarkGray),
                )),
                Line::from(Span::styled(
                    format!("   {}", all.join("  ")),
                    Style::new().fg(ratatui::style::Color::DarkGray),
                )),
                Line::from(Span::raw("")),
                Line::from(vec![
                    Span::styled(
                        format!(" {name} > "),
                        Style::new().fg(ratatui::style::Color::Green).bold(),
                    ),
                    Span::styled(
                        masked.clone(),
                        Style::new().fg(ratatui::style::Color::White),
                    ),
                ]),
                Line::from(Span::raw("")),
                Line::from(Span::styled(
                    " Enter 下一个 · 留空=跳过该档 · Esc 全部跳过",
                    Style::new().fg(ratatui::style::Color::DarkGray),
                )),
            ];
            // 终端很矮时先砍掉"清单"两行, 保住输入行
            while lines.len() > h.saturating_sub(2) as usize && lines.len() > 3 {
                lines.remove(0);
            }
            f.render_widget(
                Paragraph::new(lines).block(
                    Block::bordered()
                        .title(format!(" 密码 ({}/{}) ", idx + 1, all.len()))
                        .border_style(Style::new().fg(ratatui::style::Color::Magenta)),
                ),
                area,
            );
        })
        .map_err(|e| e.to_string())?;

        // 阻塞等一个按键 —— 这一步是启动前的独占阶段, 没有别的输入源
        if let Ok(Event::Key(k)) = ratatui::crossterm::event::read() {
            match k.code {
                KeyCode::Enter => return Ok(Some(input)),
                KeyCode::Esc => return Ok(None),
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) => input.push(c),
                _ => {}
            }
        }
    }
}

/// 把 `area` 居中成 `w` × `h` (越界时收窄到可用区域)。
fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn centered_clamps_to_the_available_area() {
        // 小终端下不能算出越界的矩形 (ratatui 会 panic)
        let small = Rect::new(0, 0, 20, 4);
        let r = centered(small, 64, 7);
        assert_eq!((r.width, r.height), (20, 4));
        let big = Rect::new(0, 0, 200, 60);
        let r = centered(big, 64, 7);
        assert_eq!((r.width, r.height), (64, 7));
        assert_eq!((r.x, r.y), (68, 26));
    }

    fn entry(name: &str) -> ProfileEntry {
        ProfileEntry {
            name: name.into(),
            path: PathBuf::from(format!("profiles/{name}.json")),
            summary: String::new(),
        }
    }

    #[test]
    fn validate_rejects_separators_and_reserved_prefix() {
        assert!(validate_profile_name("本地服_100000003").is_ok());
        assert!(validate_profile_name("a/b").is_err());
        assert!(validate_profile_name("a\\b").is_err());
        assert!(validate_profile_name("a:b").is_err());
        assert!(validate_profile_name("").is_err());
        assert!(validate_profile_name("   ").is_err());
        assert!(
            validate_profile_name("_lib").is_err(),
            "下划线开头保留为库文件"
        );
        // .json 后缀自动剥掉
        assert_eq!(validate_profile_name("x.json").unwrap(), "x");
    }

    #[test]
    fn navigation_and_select() {
        let mut st = PickerState::new(vec![entry("a"), entry("b")]);
        assert_eq!(st.cursor, 0);
        st.on_key(key(KeyCode::Down));
        assert_eq!(st.cursor, 1);
        st.on_key(key(KeyCode::Down));
        assert_eq!(st.cursor, 0, "循环滚动");
        st.on_key(key(KeyCode::Up));
        assert_eq!(st.cursor, 1, "向上循环");
        match st.on_key(key(KeyCode::Enter)) {
            PickAction::Select(p) => assert!(p.ends_with("b.json")),
            other => panic!("expected Select, got {other:?}"),
        }
    }

    // ── 多选 (阶段 9) ────────────────────────────────────────────────

    #[test]
    fn enter_without_marks_still_selects_one() {
        // 不勾选时 Enter 必须保持老行为 (单选进向导) —— 这是向后兼容的底线。
        let mut st = PickerState::new(vec![entry("a"), entry("b")]);
        assert!(st.marked.is_empty());
        match st.on_key(key(KeyCode::Enter)) {
            PickAction::Select(p) => assert!(p.ends_with("a.json")),
            other => panic!("不勾选时必须是单选: {other:?}"),
        }
    }

    #[test]
    fn space_marks_several_and_enter_returns_them_all() {
        // 勾选多个 → Enter 一次把它们全带进去 (多开管理器的入口)。
        let mut st = PickerState::new(vec![entry("a"), entry("b"), entry("c")]);
        st.on_key(key(KeyCode::Char(' ')));
        st.on_key(key(KeyCode::Down));
        st.on_key(key(KeyCode::Down));
        st.on_key(key(KeyCode::Char(' ')));
        assert_eq!(st.marked.len(), 2);
        match st.on_key(key(KeyCode::Enter)) {
            PickAction::SelectMany(v) => {
                assert_eq!(v.len(), 2, "{v:?}");
                assert!(v[0].ends_with("a.json"), "{v:?}");
                assert!(v[1].ends_with("c.json"), "{v:?}");
            }
            other => panic!("勾选后必须是多选: {other:?}"),
        }
    }

    #[test]
    fn space_toggles_off_again() {
        // 再按一次取消 —— 没有这条, 用户按错了就只能 Esc 重来。
        let mut st = PickerState::new(vec![entry("a"), entry("b")]);
        st.on_key(key(KeyCode::Char(' ')));
        assert_eq!(st.marked.len(), 1);
        st.on_key(key(KeyCode::Char(' ')));
        assert!(st.marked.is_empty(), "再按一次应取消勾选");
        // 全部取消之后 Enter 回到单选
        match st.on_key(key(KeyCode::Enter)) {
            PickAction::Select(_) => {}
            other => panic!("取消勾选后应回到单选: {other:?}"),
        }
    }

    #[test]
    fn select_all_toggles_everything() {
        let mut st = PickerState::new(vec![entry("a"), entry("b"), entry("c")]);
        st.on_key(key(KeyCode::Char('a')));
        assert_eq!(st.marked.len(), 3, "A 全选");
        st.on_key(key(KeyCode::Char('a')));
        assert!(st.marked.is_empty(), "再按 A 全不选");
    }

    #[test]
    fn navigation_does_not_change_the_marks() {
        // 光标 (▶) 决定 N/C/D 作用在谁身上; 勾选 ([✓]) 决定 Enter 带谁进去。
        // 移动光标绝不能改勾选 —— 否则"选了几个号"这件事会随手漂走。
        let mut st = PickerState::new(vec![entry("a"), entry("b"), entry("c")]);
        st.on_key(key(KeyCode::Char(' '))); // 勾 a
        st.on_key(key(KeyCode::Down));
        st.on_key(key(KeyCode::Down));
        assert_eq!(st.marked.len(), 1, "移动光标不该改勾选");
        assert!(st.marked.contains(&0));
        match st.on_key(key(KeyCode::Enter)) {
            PickAction::SelectMany(v) => assert_eq!(v.len(), 1),
            other => panic!("勾了一个也该走多选: {other:?}"),
        }
    }

    #[test]
    fn deleting_reindexes_the_marks() {
        // 勾选集合是**下标**集合, 删掉一条之后必须跟着修正 —— 不修的话
        // "勾了 a 和 c"会变成勾着 b 和 c (甚至指向不存在的下标)。
        let dir = std::env::temp_dir().join(format!("ost_pick_del_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mk = |n: &str| {
            let p = dir.join(format!("{n}.json"));
            std::fs::write(&p, "{}").unwrap();
            ProfileEntry {
                name: n.into(),
                path: p,
                summary: String::new(),
            }
        };
        let mut st = PickerState::new(vec![mk("a"), mk("b"), mk("c")]);
        st.on_key(key(KeyCode::Char(' '))); // 勾 a (下标 0)
        st.on_key(key(KeyCode::Down));
        st.on_key(key(KeyCode::Down));
        st.on_key(key(KeyCode::Char(' '))); // 勾 c (下标 2)
        assert_eq!(st.marked.iter().copied().collect::<Vec<_>>(), vec![0, 2]);
        // 删掉 b (下标 1) → c 前移到 1
        st.cursor = 1;
        st.on_key(key(KeyCode::Char('d')));
        st.on_key(key(KeyCode::Char('d')));
        assert_eq!(st.entries.len(), 2);
        assert_eq!(
            st.marked.iter().copied().collect::<Vec<_>>(),
            vec![0, 1],
            "c 的下标必须跟着前移"
        );
        match st.on_key(key(KeyCode::Enter)) {
            PickAction::SelectMany(v) => {
                assert!(v[0].ends_with("a.json"), "{v:?}");
                assert!(v[1].ends_with("c.json"), "{v:?}");
            }
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn esc_cancels_and_delete_needs_double_confirm() {
        let mut st = PickerState::new(vec![entry("a")]);
        assert_eq!(st.on_key(key(KeyCode::Esc)), PickAction::Cancel);
        // 第一次 D: 只待确认
        assert_eq!(st.on_key(key(KeyCode::Char('d'))), PickAction::None);
        assert_eq!(st.delete_arm, Some(0));
        // 移动解除待确认
        st.on_key(key(KeyCode::Down));
        assert_eq!(st.delete_arm, None);
    }

    fn tmp_file(tag: &str, name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ost_pick_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, "{}").unwrap();
        p
    }

    #[test]
    fn double_d_actually_deletes_the_file() {
        // 回归测试: 早先版本在 match 前无条件清空 delete_arm,
        // 导致第二次 D 永远走"首次按下"分支, 删除功能完全失效。
        let p1 = tmp_file("del", "x.json");
        let p2 = tmp_file("del", "y.json");
        let mk = |n: &str, p: &PathBuf| ProfileEntry {
            name: n.into(),
            path: p.clone(),
            summary: String::new(),
        };
        let mut st = PickerState::new(vec![mk("x", &p1), mk("y", &p2)]);
        st.on_key(key(KeyCode::Char('d')));
        assert_eq!(st.delete_arm, Some(0), "第一次按 D 进入待确认态");
        st.on_key(key(KeyCode::Char('d')));
        assert!(!p1.exists(), "第二次按 D 必须真正删除文件");
        assert_eq!(st.entries.len(), 1);
        assert_eq!(st.entries[0].name, "y", "列表移除被删项");
        assert_eq!(st.cursor, 0, "光标钳制到剩余项");
    }

    #[test]
    fn moving_between_entries_rearms_delete() {
        let p1 = tmp_file("rear", "x.json");
        let p2 = tmp_file("rear", "y.json");
        let mk = |n: &str, p: &PathBuf| ProfileEntry {
            name: n.into(),
            path: p.clone(),
            summary: String::new(),
        };
        let mut st = PickerState::new(vec![mk("x", &p1), mk("y", &p2)]);
        st.on_key(key(KeyCode::Char('d'))); // 对 x 待确认
        st.on_key(key(KeyCode::Down)); // 移到 y → 解除
        st.on_key(key(KeyCode::Char('d'))); // 对 y 首次按下 → 仅待确认
        assert!(p1.exists() && p2.exists(), "跨条目移动后不得误删");
        assert_eq!(st.delete_arm, Some(1));
        let _ = std::fs::remove_dir_all(p1.parent().unwrap());
        let _ = std::fs::remove_dir_all(p2.parent().unwrap());
    }

    #[test]
    fn legacy_config_json_is_not_deletable() {
        let mut st = PickerState::new(vec![ProfileEntry {
            name: "config.json".into(),
            path: PathBuf::from(openstory_bot::runtime_config::CONFIG_PATH),
            summary: String::new(),
        }]);
        st.on_key(key(KeyCode::Char('d')));
        assert!(!st.status.is_empty(), "必须提示不可删除");
        assert_eq!(st.delete_arm, None, "不进入删除确认态");
    }

    #[test]
    fn name_input_flow_n_and_c() {
        let mut st = PickerState::new(vec![entry("srv_acc")]);
        // N → 输入模式 (新建)
        st.on_key(key(KeyCode::Char('n')));
        assert!(st.name_input.is_some());
        assert!(!st.pending_copy);
        st.on_key(key(KeyCode::Esc));
        assert!(st.name_input.is_none());
        // C → 输入模式 (复制), 默认名 = 原名_copy
        st.on_key(key(KeyCode::Char('c')));
        assert_eq!(st.name_input.as_deref(), Some("srv_acc_copy"));
        assert!(st.pending_copy);
        // 输入非法字符 → Enter 时报错且留在输入外
        st.name_input = Some("bad/name".into());
        st.pending_copy = false;
        assert_eq!(st.on_key(key(KeyCode::Enter)), PickAction::None);
        assert!(st.status.contains("不能包含"), "报错提示: {}", st.status);
    }
}
