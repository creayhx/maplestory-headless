//! 冒险岛风格渲染: 状态栏 / 日志流 / 右侧信息面板 / 输入框 / 弹窗。

use openstory_bot::names;
use openstory_bot::state::Phase;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{display_width, App, Filter, Picker, STAGE_FINISHED, STAGE_RUNNING, STAGE_WIZARD};
use crate::wizard::Field;
use openstory_console_lib::command_spec;
use openstory_console_lib::logbuf::WindowLine;
use openstory_console_lib::text::char_boundary;
use openstory_console_lib::theme;

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();
    // 弹窗打开时整帧清屏再重绘: 底层日志每帧重渲染会覆盖弹窗边框列,
    // 而弹窗格子跨帧相同不被 diff 重发, 导致边框被日志内容吃掉
    // (如 F1 帮助换行行的左边框消失)。整帧 Clear 强制所有格子每帧重发。
    if app.help_open
        || !matches!(app.picker, Picker::None)
        || app.stage == STAGE_WIZARD
        || app.npc_pick_open
        || app.shop_pick_open
        || app.board_open
    {
        f.render_widget(Clear, area);
    }
    // 补全弹窗是布局占位 (最多 5 行), 不悬浮遮挡日志
    let popup_h = if app.comp.candidates.is_empty() {
        0
    } else {
        (app.comp.candidates.len().min(5) + 2) as u16
    };
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(popup_h),
        Constraint::Length(1),
        Constraint::Length(2),
    ])
    .split(area);

    render_status(f, rows[0], app);
    // 主区: 多会话时左侧多一条账号列表。单会话时 `side_w = 0`, 版面与原来
    // 逐像素一致 —— 这是阶段 5 "单档 == 今天的 TUI" 的版面保证。
    let side_w: u16 = if app.show_sidebar { 24 } else { 0 };
    if side_w > 0 {
        let cols = Layout::horizontal([
            Constraint::Length(side_w),
            Constraint::Min(20),
            Constraint::Length(50),
        ])
        .split(rows[1]);
        render_accounts(f, cols[0], app);
        render_log(f, cols[1], app);
        render_side(f, cols[2], app);
    } else {
        let cols = Layout::horizontal([Constraint::Min(20), Constraint::Length(50)]).split(rows[1]);
        render_log(f, cols[0], app);
        render_side(f, cols[1], app);
    }
    if popup_h > 0 {
        // 整条补全区先铺底色, 避免弹窗左右两侧黑屏
        f.render_widget(
            Paragraph::new(Line::from("")).style(Style::new().bg(theme::PANEL)),
            rows[2],
        );
        render_completion(f, rows[2], app);
    }
    render_hint(f, rows[3], app);
    render_input(f, rows[4], app);

    // 弹窗层
    match app.stage {
        STAGE_WIZARD => render_wizard(f, area, app),
        _ => {
            if !matches!(app.picker, Picker::None) {
                render_picker(f, area, &app.picker, app);
            } else if app.npc_pick_open {
                render_npc_pick(f, area, app);
            } else if app.shop_pick_open {
                render_shop_pick(f, area, app);
            }
        }
    }
    // 密码相关没有单独的弹窗: 手动模式下按 F4 而没密码时弹的就是上面那个
    // **登录向导** (账号/IP 已从该会话的档案预填, 焦点落在密码上)。
    // 复用同一个界面而不是另做一个小框 —— 用户已经认识它。
    if app.help_open {
        render_help(f, area, app);
    }
    if app.bag_open {
        render_bag(f, area, app);
    }
    // F2 看板: 与帮助同级, 但帮助优先 (F1 打开时盖在看板上)
    if app.board_open && !app.help_open {
        render_board(f, area, app);
    }

    // 光标
    match app.stage {
        STAGE_WIZARD => {
            if let Some(w) = &app.wizard {
                if let Some(pos) = wizard_cursor_pos(area, w) {
                    f.set_cursor_position(pos);
                }
            }
        }
        _ if matches!(app.picker, Picker::None) => {
            // 输入栏占 2 行: 边框在 y, 文字在 y+1
            let inner = rows[4];
            let text_row = inner.y + 1;
            let x = inner.x
                + 2
                + display_width(&app.input[..char_boundary(&app.input, app.cursor)]) as u16;
            f.set_cursor_position(Position::new(
                x.min(inner.right().saturating_sub(1)),
                text_row,
            ));
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- 状态栏

fn render_status(f: &mut Frame, area: Rect, app: &App) {
    let snap = &app.snap;
    let (phase_label, phase_color) = phase_chip(snap.phase);

    let mut spans: Vec<Span> = vec![
        Span::styled(
            " 冒险岛·OpenStory ",
            Style::new().fg(theme::PINK).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(
            format!("[{}]", phase_label),
            Style::new()
                .fg(Color::White)
                .bg(phase_color)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if snap.maxhp > 0 {
        spans.push(Span::raw("  血量 "));
        let ratio = (snap.hp as f64 / snap.maxhp as f64).clamp(0.0, 1.0);
        spans.push(Span::styled(
            theme::bar(ratio, 10, theme::HP),
            Style::new().fg(theme::HP),
        ));
        spans.push(Span::styled(
            format!(" {}/{}", snap.hp, snap.maxhp),
            Style::new().fg(theme::TEXT),
        ));
        spans.push(Span::raw("  蓝量 "));
        let mratio = (snap.mp as f64 / snap.maxmp as f64).clamp(0.0, 1.0);
        spans.push(Span::styled(
            theme::bar(mratio, 8, theme::MP),
            Style::new().fg(theme::MP),
        ));
        spans.push(Span::styled(
            format!(" {}/{}", snap.mp, snap.maxmp),
            Style::new().fg(theme::TEXT),
        ));
    }
    if snap.meso != 0 || snap.level > 0 {
        spans.push(Span::styled(
            format!("  金币 {} ", fmt_num(snap.meso)),
            Style::new().fg(theme::GOLD).add_modifier(Modifier::BOLD),
        ));
    }
    // 拾取过滤状态 (仅在启用过滤时出现): 仅捡/禁捡
    let pf = &snap.toggles;
    if pf.pickup_filter_mode == "allow" || pf.pickup_filter_mode == "deny" {
        let (label, color) = if pf.pickup_filter_mode == "allow" {
            ("仅捡", theme::GREEN)
        } else {
            ("禁捡", theme::RED)
        };
        spans.push(Span::styled(
            format!(" {label} "),
            Style::new()
                .fg(Color::Black)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let mut right: Vec<Span> = Vec::new();
    if snap.mapid != 0 {
        right.push(Span::styled(
            format!(" 地图 {} ", names::map_name(snap.mapid)),
            Style::new().fg(theme::BLUE),
        ));
        right.push(Span::styled(
            format!("({},{}) ", snap.pos.0, snap.pos.1),
            Style::new().fg(theme::DIM),
        ));
    }
    if app.dropped > 0 {
        right.push(Span::styled(
            format!(" 日志丢弃{} ", app.dropped),
            Style::new().fg(theme::RED),
        ));
    }
    right.push(Span::styled(
        format!(" {}", chrono::Local::now().format("%H:%M:%S")),
        Style::new().fg(theme::DIM),
    ));

    let line = Line::from(spans);
    let right_line = Line::from(right).alignment(Alignment::Right);
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::new().fg(theme::BORDER))
        .bg(theme::PANEL);
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(line), inner);
    f.render_widget(Paragraph::new(right_line), inner);
}

fn fmt_num(n: i32) -> String {
    if n < 0 {
        return n.to_string();
    }
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn phase_chip(p: Phase) -> (&'static str, Color) {
    match p {
        Phase::Disconnected => ("未连接", theme::DIM),
        Phase::Connecting => ("连接中", theme::BLUE),
        Phase::LoggingIn => ("登录中", theme::BLUE),
        Phase::GenderPick => ("选择性别", theme::PINK),
        Phase::WorldSelect => ("选择世界", theme::PINK),
        Phase::CharSelect => ("选择角色", theme::PINK),
        Phase::EnteringMap => ("进入地图", theme::YELLOW),
        Phase::InGame => ("游戏中", theme::GREEN),
        Phase::CashShop => ("商城中", theme::PURPLE),
    }
}

// ---------------------------------------------------------------- 日志流

fn render_log(f: &mut Frame, area: Rect, app: &mut App) {
    let tabs: Vec<Span> = Filter::ORDER
        .iter()
        .map(|f| {
            let label = f.label();
            if *f == app.filter {
                Span::styled(
                    format!(" {label} "),
                    Style::new()
                        .fg(Color::White)
                        .bg(theme::PINK)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(format!(" {label} "), Style::new().fg(theme::DIM))
            }
        })
        .collect();
    let title = Line::from(tabs);
    let block = Block::bordered()
        .border_style(Style::new().fg(theme::BORDER))
        .bg(theme::PANEL)
        .title(title)
        .title_style(Style::new().fg(theme::DIM));
    let inner = block.inner(area);
    f.render_widget(block, area);

    app.last_log_w = inner.width;
    app.last_log_h = inner.height as usize;

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if app.filter == crate::app::Filter::Status {
        return render_status_tab(f, inner, app);
    }

    if app.log.is_empty() {
        let banner = vec![
            Line::from(Span::styled(
                "╭──────────────────────────────────╮",
                Style::new().fg(theme::BORDER_DIM),
            ))
            .alignment(Alignment::Center),
            Line::from(Span::styled(
                "  欢迎使用 openstory-console",
                Style::new().fg(theme::PINK).add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
            Line::from(Span::styled(
                "  冒险岛 079 机器人控制台",
                Style::new().fg(theme::BORDER),
            ))
            .alignment(Alignment::Center),
            Line::from(Span::styled(
                "  底部输入框键入指令 (Tab 补全, ↑↓ 历史)",
                Style::new().fg(theme::DIM),
            ))
            .alignment(Alignment::Center),
            Line::from(Span::styled(
                "  F1 查看全部命令 · PgUp/PgDn 滚动日志",
                Style::new().fg(theme::DIM),
            ))
            .alignment(Alignment::Center),
            Line::from(Span::styled(
                "╰──────────────────────────────────╯",
                Style::new().fg(theme::BORDER_DIM),
            ))
            .alignment(Alignment::Center),
        ];
        let pad = inner.height as usize / 2;
        let mut lines = vec![Line::from(""); pad.saturating_sub(3)];
        lines.extend(banner);
        f.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let height = inner.height as usize;
    let mut avail = height;
    if app.scroll > 0 {
        avail = height.saturating_sub(1);
    }
    let win = app
        .log
        .window(app.filter.cat(), inner.width, avail, app.scroll);

    let mut lines: Vec<Line> = Vec::with_capacity(height);
    if app.scroll > 0 {
        lines.push(
            Line::from(Span::styled(
                format!(" ▲ 上翻 {} 行 — PgDn/↓/鼠标滚轮回到底部 ", app.scroll),
                Style::new().fg(theme::GOLD),
            ))
            .alignment(Alignment::Center),
        );
    }
    for w in win {
        if openstory_console_lib::zh::zh_hidden(w.tag.as_deref().unwrap_or(""), &w.raw_body) {
            continue;
        }
        lines.push(log_line(&w));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

// ---------------------------------------------------------------- 状态页

/// 「状态」页: 运行时 bot 状态 (左) + 内存 config 一览 (右), 诊断用。
fn render_status_tab(f: &mut Frame, area: Rect, app: &mut App) {
    let snap = &app.snap;
    let cols =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
    let left: Vec<Line> = snap
        .status_state
        .iter()
        .map(|l| Line::from(Span::styled(format!("  {l}"), Style::new().fg(theme::TEXT))))
        .collect();
    let right: Vec<Line> = snap
        .status_cfg
        .iter()
        .map(|l| Line::from(Span::styled(format!("  {l}"), Style::new().fg(theme::TEAL))))
        .collect();
    let title_left = Line::from(vec![Span::styled(
        " 运行时 ",
        Style::new().fg(theme::GREEN).add_modifier(Modifier::BOLD),
    )]);
    let title_right = Line::from(vec![Span::styled(
        " 配置(内存) ",
        Style::new().fg(theme::TEAL).add_modifier(Modifier::BOLD),
    )]);
    let bl = Block::bordered()
        .border_style(Style::new().fg(theme::BORDER_DIM))
        .title(title_left)
        .title_style(Style::new().fg(theme::DIM));
    let br = Block::bordered()
        .border_style(Style::new().fg(theme::BORDER_DIM))
        .title(title_right)
        .title_style(Style::new().fg(theme::DIM));
    let il = bl.inner(cols[0]);
    let ir = br.inner(cols[1]);
    f.render_widget(bl, cols[0]);
    f.render_widget(br, cols[1]);
    f.render_widget(Paragraph::new(left), il);
    f.render_widget(Paragraph::new(right), ir);
}

fn log_line(w: &WindowLine) -> Line<'static> {
    let mut spans: Vec<Span> = vec![Span::styled(
        format!("{} ", w.ts),
        Style::new().fg(theme::DIM),
    )];
    // body 已在 logbuf 折行前渲染成中文 (先渲染后折行): 这里只拼前缀,
    // 续行不重复 tag — 长消息折行后仍是一条消息, 不会出现重复 tag + 英文残片。
    match &w.tag {
        Some(tag) => {
            let zh_tag = openstory_console_lib::zh::translate_tag(tag);
            spans.push(Span::styled(zh_tag, theme::cat_style(w.cat)));
            spans.push(Span::styled(
                w.body.clone(),
                theme::body_style(w.level, &w.body),
            ));
        }
        None => {
            spans.push(Span::styled("    ", Style::default()));
            spans.push(Span::styled(
                w.body.clone(),
                theme::body_style(w.level, &w.body),
            ));
        }
    }
    Line::from(spans)
}

// ------------------------------------------------------------ 左侧账号列表

/// 左栏: N 个账号一行一条 (● 在线 / ○ 离线 / ✕ 已放弃)。
///
/// 只在多会话时出现 (`app.show_sidebar`), 单会话时整条不渲染, 保证单档版面
/// 与原来的 TUI 完全一致。
fn render_accounts(f: &mut Frame, area: Rect, app: &App) {
    let rows = app.board_rows();
    let selected = app.session_position().map(|(i, _)| i).unwrap_or(0);
    let mut lines: Vec<Line> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        // 状态点: ● 运行中 / ○ 已结束 / ↻ 等重连 / ✕ 需人工 / ◌ 未启动·已停止
        //
        // 顺序有讲究: "需人工"优先于一切 —— 一个放弃重连 (或根本没能拉起) 的
        // 会话在阶段上仍是"已结束", 但用户最需要看到的是"这个号没人管了"。
        let (dot, dot_color) = if r.needs_attention || r.start_failed {
            ("✕", theme::RED)
        } else if r.waiting_restart {
            ("↻", theme::YELLOW)
        } else if r.running {
            ("●", theme::GREEN)
        } else if r.never_started || r.manually_stopped {
            // 未启动与已停止用同一个点: 两者都是"没在跑, 等你去启动它",
            // 区别只在下一行的文字里 (一个是从没启动, 一个是用户停的)。
            ("◌", theme::DIM)
        } else if r.finished {
            ("○", theme::DIM)
        } else {
            ("◌", theme::YELLOW)
        };
        // 名称按可用宽度截断 (显示宽度, 中文算 2 列)
        let name_w = (area.width as usize).saturating_sub(4);
        let name = openstory_console_lib::text::truncate_width(&r.profile, name_w.max(4));
        let is_sel = i == selected;
        let name_style = if is_sel {
            Style::new().fg(theme::PINK).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme::TEXT)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{dot} "), Style::new().fg(dot_color)),
            Span::styled(name, name_style),
        ]));
        // 第二行: 阶段 / Lv / HP (选中项显示得更全)
        let s = &r.snap;
        let detail = if r.waiting_restart {
            // 倒计时每秒都在变 —— 用会话给的到点时刻现算, 不用缓存的秒数
            // (缓存的秒数会停在排定那一刻, 用户看到的是一个不动的数字)。
            let secs = r
                .restart_at
                .map(|at| {
                    at.saturating_duration_since(std::time::Instant::now())
                        .as_secs()
                })
                .unwrap_or(0);
            if secs == 0 {
                "  正在重连…".to_string()
            } else {
                format!("  {secs}s 后重连")
            }
        } else if r.gave_up {
            "  已放弃重连 — 需人工".to_string()
        } else if r.start_failed {
            // 用户按了启动但没起来 (没有密码 / 档案加载失败)。必须明确报出来 ——
            // 否则界面看起来和"从没启动过"一样, 用户会反复按同一个键。
            "  启动失败 — 需人工".to_string()
        } else if r.failed {
            "  登录失败 — 需人工".to_string()
        } else if r.needs_attention {
            // 掉线/崩溃结束, 但没有拉起 (没配置模板 / 关掉了自动拉起)
            "  已停止 — 需人工".to_string()
        } else if r.manually_stopped {
            "  已停止 (手动)".to_string()
        } else if r.never_started {
            "  未启动 — F4 启动".to_string()
        } else if !r.running && !r.finished {
            "  未启动".to_string()
        } else {
            match s.phase {
                // 左栏只回答"这是哪个角色": **角色名 + 角色 ID**。
                //
                // 等级/地图/血量属于详细信息, 左栏只有 22 列装不下 —— 它们会
                // 折成两行 ("Lv69 射手村集" / "3802/3802"), 把"一眼扫全部账号"
                // 这个用途挤没了。要看细节有 F2 看板与右侧角色面板。
                Phase::InGame => {
                    let avail = (area.width as usize).saturating_sub(4);
                    let id = s.my_cid.to_string();
                    let name_w = avail.saturating_sub(id.len() + 1);
                    let name = openstory_console_lib::text::truncate_width(&s.name, name_w);
                    if name.is_empty() {
                        // 还没收到角色名 (刚进游戏那一瞬间) —— 别显示一个孤零零
                        // 的 0, 那会被当成角色 ID。
                        format!("  {}", phase_zh_short(Phase::InGame))
                    } else {
                        format!("  {name} {id}")
                    }
                }
                other => format!("  {}", phase_zh_short(other)),
            }
        };
        // 需人工的红字必须真的红 —— 这是"要不要过去看一眼"的唯一线索
        let detail_color = if r.needs_attention || r.start_failed {
            theme::RED
        } else if r.waiting_restart {
            theme::YELLOW
        } else {
            theme::DIM
        };
        lines.push(Line::from(Span::styled(
            detail,
            Style::new().fg(detail_color),
        )));
        if r.dropped > 0 {
            lines.push(Line::from(Span::styled(
                format!("  日志丢弃 {}", r.dropped),
                Style::new().fg(theme::YELLOW),
            )));
        }
        // 重连过就把代数显示出来: "第 2 次重连" 与 "第 9 次" 是两件不同的事
        if r.generation > 0 && r.waiting_restart {
            lines.push(Line::from(Span::styled(
                format!("  第 {} 次重连", r.generation),
                Style::new().fg(theme::DIM),
            )));
        }
    }
    let block = Block::bordered()
        .border_style(Style::new().fg(theme::BORDER))
        .bg(theme::PANEL)
        .title(Span::styled(
            // 标题带"几个在等重连 / 几个没启动": 用户一眼就知道要不要展开看细节。
            //
            // 优先级给"重连中" —— 那是**正在发生**的事; "未启动"是静态状态,
            // 左栏每一行自己已经写了。
            match rows.iter().filter(|r| r.waiting_restart).count() {
                0 => match rows
                    .iter()
                    .filter(|r| r.never_started || r.manually_stopped)
                    .count()
                {
                    0 => format!(" 账号 {}/{} ", selected + 1, rows.len().max(1)),
                    n => format!(
                        " 账号 {}/{} · {n} 个未启动 ",
                        selected + 1,
                        rows.len().max(1)
                    ),
                },
                n => format!(
                    " 账号 {}/{} · {n} 个重连中 ",
                    selected + 1,
                    rows.len().max(1)
                ),
            },
            Style::new().fg(theme::PINK).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// 阶段短名 (左栏空间窄)。
fn phase_zh_short(p: Phase) -> &'static str {
    match p {
        Phase::Disconnected => "未连接",
        Phase::Connecting => "连接中",
        Phase::LoggingIn => "登录中",
        Phase::GenderPick => "选性别",
        Phase::WorldSelect => "选世界",
        Phase::CharSelect => "选角色",
        Phase::EnteringMap => "进图中",
        Phase::InGame => "游戏中",
        Phase::CashShop => "商城",
    }
}

// ------------------------------------------------------------ F2 看板页

/// 看板列: `(表头, 显示列宽, 是否右对齐)` —— 表头与数据行**共用**这一组常量。
///
/// # 为什么不能直接用 `format!("{:<22}", ..)`
///
/// Rust 的宽度参数数的是 `char` **个数**, 而一个汉字算 1 个 char 却占 **2** 列。
/// 于是表头 ("档案" = 2 字符 / 4 列) 与数据行 ("本地服_100000001" = 13 字符 /
/// 16 列) 补到同一个"字符宽度"后, **显示宽度并不相同** —— 每列都错开一点,
/// 整张表越往右越歪 (现场截图就是这个现象)。
///
/// 一律走 [`board_cell`] 的显示宽度补齐。
///
/// 另外 HP 原本表头写 `{:>6}` 而数据写 `{:>3}/{:<3}` (7 列), 本身就差一列;
/// 这里的宽度同时是"表头与数据共用"的唯一来源, 不会再出现这种两处各写一份。
const BOARD_COLS: [(&str, usize, bool); 11] = [
    ("档案", 22, false),
    ("阶段", 10, false),
    ("角色", 8, false),
    ("Lv", 4, true),
    ("EXP%", 6, true),
    ("HP", 11, true),
    ("频道", 5, true),
    ("地图", 14, false),
    ("打怪", 5, true),
    // 这一列的数据是"日志丢弃条数" (`r.dropped`), 不是重试次数 ——
    // 标题必须跟着数据走, 否则用户会以为那是重连次数。
    ("丢弃", 6, false),
    ("金币", 11, true),
];

/// 一个单元格: 先按**显示宽度**截断, 再按显示宽度补齐 (超宽则原样返回)。
fn board_cell(s: &str, width: usize, right: bool) -> String {
    use openstory_console_lib::text::{display_width, truncate_width};
    let t = truncate_width(s, width);
    let w = display_width(&t);
    if w >= width {
        return t;
    }
    let pad = " ".repeat(width - w);
    if right {
        format!("{pad}{t}")
    } else {
        format!("{t}{pad}")
    }
}

/// 拼一整行: 每列补齐到固定显示宽度, 列间一个空格, 行首行尾各一个空格。
///
/// 表头与数据行走同一个函数 → 两行的显示宽度必然相等, 列与列必然对齐。
fn board_line(cells: &[String]) -> String {
    let mut out = String::from(" ");
    for (c, (_, w, right)) in cells.iter().zip(BOARD_COLS.iter()) {
        out.push_str(&board_cell(c, *w, *right));
        out.push(' ');
    }
    out
}

/// F2 看板: N 行 × 全字段表格, 一眼扫全部账号。
///
/// 数据来自各会话的 `BotState` (零延迟、零协议)。
fn render_board(f: &mut Frame, area: Rect, app: &App) {
    let rows = app.board_rows();
    let header: Vec<String> = BOARD_COLS.iter().map(|(h, _, _)| (*h).to_string()).collect();
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        board_line(&header),
        Style::new().fg(theme::GOLD).add_modifier(Modifier::BOLD),
    ))];
    for r in rows.iter() {
        let s = &r.snap;
        let phase = if r.gave_up {
            "已放弃".to_string()
        } else if r.waiting_restart {
            let secs = r
                .restart_at
                .map(|at| {
                    at.saturating_duration_since(std::time::Instant::now())
                        .as_secs()
                })
                .unwrap_or(0);
            format!("{secs}s后重连")
        } else if r.failed {
            "需人工".to_string()
        } else if r.needs_attention {
            "已停止".to_string()
        } else if r.running {
            phase_zh_short(s.phase).to_string()
        } else if r.finished {
            "已结束".to_string()
        } else {
            "未启动".to_string()
        };
        let exp_pct = if s.level > 0 {
            openstory_bot::exptable::exp_remain(s.level as i32, s.exp as i64);
            // 只显示剩余经验, 百分比由 UI 自行估算意义不大
            format!("{:.0}", exp_ratio(s))
        } else {
            "-".to_string()
        };
        let map = openstory_console_lib::text::truncate_width(
            &openstory_bot::names::map_name_text(s.mapid),
            12,
        );
        let hunt = if s.toggles.hunt { "开" } else { "关" };
        let cells: Vec<String> = vec![
            r.profile.clone(),
            phase,
            s.name.clone(),
            s.level.to_string(),
            exp_pct,
            format!("{}/{}", s.hp, s.maxhp),
            s.channel.to_string(),
            map,
            hunt.to_string(),
            if r.dropped > 0 {
                r.dropped.to_string()
            } else {
                "-".to_string()
            },
            s.meso.to_string(),
        ];
        let line = board_line(&cells);
        let style = if r.selected {
            Style::new().fg(theme::PINK).add_modifier(Modifier::BOLD)
        } else if r.needs_attention {
            Style::new().fg(theme::RED)
        } else if r.waiting_restart {
            Style::new().fg(theme::YELLOW)
        } else if r.running {
            Style::new().fg(theme::TEXT)
        } else {
            Style::new().fg(theme::DIM)
        };
        lines.push(Line::from(Span::styled(line, style)));
    }
    let total = lines.len();
    let inner = area.height.saturating_sub(2) as usize;
    let max_scroll = total.saturating_sub(inner);
    let start = app.board_scroll.min(max_scroll);
    let end = (start + inner).min(total);
    let win: Vec<Line> = lines.drain(start..end).collect();
    let rect = centered(
        area,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(win).block(
            Block::bordered()
                .border_style(Style::new().fg(theme::PINK))
                .bg(theme::PANEL)
                .title(Span::styled(
                    format!(
                        " 看板 ({} 个账号 · F2/Esc 关闭 · ↑↓/PgUp/PgDn 滚动) ",
                        rows.len()
                    ),
                    Style::new().fg(theme::PINK).add_modifier(Modifier::BOLD),
                )),
        ),
        rect,
    );
}

/// 经验百分比 (0..100)。取不到升级所需经验时返回 0。
fn exp_ratio(s: &openstory_console_lib::snapshot::Snapshot) -> f64 {
    let remain = openstory_bot::exptable::exp_remain(s.level as i32, s.exp as i64);
    let total = remain.saturating_add(s.exp as i64);
    if total <= 0 {
        return 0.0;
    }
    (s.exp as f64 / total as f64) * 100.0
}

// ------------------------------------------------------------ 右侧信息面板

fn render_side(f: &mut Frame, area: Rect, app: &App) {
    let snap = &app.snap;
    let mut lines: Vec<Line> = Vec::new();

    section(&mut lines, "角色");
    if !snap.name.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  {}", snap.name),
            Style::new().fg(theme::PINK).add_modifier(Modifier::BOLD),
        )));
    }
    lines.push(Line::from(vec![
        Span::styled(
            format!("  等级 {} ", snap.level),
            Style::new().fg(theme::YELLOW).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "经验 {} ({}%)",
                fmt_num(snap.exp),
                openstory_bot::exptable::exp_pct(snap.level as i32, snap.exp as i64)
            ),
            Style::new().fg(theme::TEXT),
        ),
    ]));
    if snap.maxhp > 0 {
        let ratio = (snap.hp as f64 / snap.maxhp as f64).clamp(0.0, 1.0);
        lines.push(Line::from(vec![
            Span::styled("  血量 ", Style::new().fg(theme::DIM)),
            Span::styled(theme::bar(ratio, 14, theme::HP), Style::new().fg(theme::HP)),
            Span::styled(
                format!(" {}/{}", snap.hp, snap.maxhp),
                Style::new().fg(theme::TEXT),
            ),
        ]));
        let mratio = (snap.mp as f64 / snap.maxmp as f64).clamp(0.0, 1.0);
        lines.push(Line::from(vec![
            Span::styled("  蓝量 ", Style::new().fg(theme::DIM)),
            Span::styled(
                theme::bar(mratio, 14, theme::MP),
                Style::new().fg(theme::MP),
            ),
            Span::styled(
                format!(" {}/{}", snap.mp, snap.maxmp),
                Style::new().fg(theme::TEXT),
            ),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled(
            format!(" 金币 {} ", fmt_num(snap.meso)),
            Style::new().fg(theme::GOLD),
        ),
        Span::styled(format!("属性点 {} ", snap.ap), Style::new().fg(theme::TEXT)),
        Span::styled(format!("技能点 {}", snap.sp), Style::new().fg(theme::TEXT)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!(" 已学技能 {} ", snap.skills),
            Style::new().fg(theme::PURPLE),
        ),
        Span::styled(
            format!("已设置按键 {} ", snap.keymap),
            Style::new().fg(theme::TEAL),
        ),
        Span::styled(
            format!("角色ID {}", snap.my_cid),
            Style::new().fg(theme::DIM),
        ),
    ]));

    section(&mut lines, "地图");
    if snap.mapid != 0 {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(names::map_name(snap.mapid), Style::new().fg(theme::BLUE)),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled(
            format!("  频道 {} ", snap.channel),
            Style::new().fg(theme::BLUE).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("位置 ({},{})", snap.pos.0, snap.pos.1),
            Style::new().fg(theme::TEXT),
        ),
        Span::styled(
            format!("地面 {} ", snap.ground_y),
            Style::new().fg(theme::DIM),
        ),
        Span::styled(
            if snap.facing == 1 { "朝左" } else { "朝右" },
            Style::new().fg(theme::DIM),
        ),
    ]));

    section(&mut lines, "场景");
    lines.push(Line::from(vec![
        Span::raw(" "),
        kv("怪物", snap.counts.mobs, theme::ORANGE),
        Span::raw(" · "),
        kv("玩家", snap.counts.players, theme::BLUE),
        Span::raw(" · "),
        kv("掉落物", snap.counts.drops, theme::GOLD),
    ]));
    lines.push(Line::from(vec![
        Span::raw(" "),
        kv("NPC", snap.counts.npcs, theme::PINK),
        Span::raw(" · "),
        kv("放置物", snap.counts.reactors, theme::TEAL),
    ]));

    section(&mut lines, "挂机");
    let t = &snap.toggles;
    // 状态开关 (绿)
    lines.push(Line::from(vec![
        Span::raw(" "),
        toggle("打怪", t.hunt, theme::GREEN),
        toggle("放置物", t.hunt_reactor, theme::GREEN),
    ]));
    // 吸怪 (青绿): 开关/步长/间隔/每轮数量 一行
    lines.push(Line::from(vec![
        Span::raw(" "),
        toggle("吸怪", t.gather, theme::TEAL),
        Span::styled(" 步长 ", Style::new().fg(theme::DIM)),
        Span::styled(
            if t.gather_step <= 0 {
                "全拉".to_string()
            } else {
                format!("{}px", t.gather_step)
            },
            Style::new().fg(theme::TEAL),
        ),
        Span::styled(" 间隔 ", Style::new().fg(theme::DIM)),
        Span::styled(
            if t.gather_interval == 0 {
                "每tick".to_string()
            } else {
                format!("{}ms", t.gather_interval)
            },
            Style::new().fg(theme::TEAL),
        ),
        Span::styled(" 每轮 ", Style::new().fg(theme::DIM)),
        Span::styled(
            if t.gather_max == 0 {
                "不限".to_string()
            } else {
                format!("{}只", t.gather_max)
            },
            Style::new().fg(theme::TEAL),
        ),
    ]));
    // 攻击方式 (紫)
    let attack_mode = match t.attack_mode.as_str() {
        "skill" if t.attack_skill == 0 => "技能·普攻".to_string(),
        "skill" => format!("技能·{}", names::skill_name_text(t.attack_skill)),
        _ => "普攻".to_string(),
    };
    // 攻击方式 (紫); 目标数按模式取: 技能模式用技能目标数, 普攻用普攻数量
    let (target_label, targets) = match t.attack_mode.as_str() {
        "skill" => ("技能目标", t.skill_max_targets.to_string()),
        _ => ("普攻数量", t.attack_max_targets.to_string()),
    };
    lines.push(Line::from(vec![
        Span::styled("  攻击 ", Style::new().fg(theme::DIM)),
        Span::styled(
            attack_mode,
            Style::new().fg(theme::PURPLE).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" 攻击范围 ", Style::new().fg(theme::DIM)),
        Span::styled(
            format!("{}px", t.attack_range),
            Style::new().fg(theme::PURPLE),
        ),
        Span::styled(format!(" {target_label} "), Style::new().fg(theme::DIM)),
        Span::styled(targets, Style::new().fg(theme::PURPLE)),
    ]));
    let skill = if t.attack_skill > 0 {
        format!(
            "{} 范围{} 目标{} 蓝{} 段{}",
            names::skill_name_text(t.attack_skill),
            t.skill_range,
            t.skill_max_targets,
            t.skill_mp_cost,
            t.skill_hits
        )
    } else if t.attack_mode == "skill" {
        format!("无技能群发 段{}", t.skill_hits)
    } else {
        "无(普攻)".to_string()
    };
    lines.push(Line::from(vec![
        Span::styled("  冷却 ", Style::new().fg(theme::DIM)),
        Span::styled(
            format!("{}ms", t.attack_cooldown),
            Style::new().fg(theme::PURPLE),
        ),
        Span::styled(" 控制权 ", Style::new().fg(theme::DIM)),
        Span::styled(
            if t.attack_controller_only {
                "开"
            } else {
                "关"
            },
            Style::new().fg(theme::PURPLE),
        ),
        Span::styled(" 技能 ", Style::new().fg(theme::DIM)),
        Span::styled(skill, Style::new().fg(theme::PURPLE)),
    ]));
    // 移动参数 (青绿): 移动方式 = 瞬移/原地, 瞬移时附带冷却
    let (mode, cooldown) = if t.hunt_stand {
        (
            Span::styled(
                "原地",
                Style::new().fg(theme::TEAL).add_modifier(Modifier::BOLD),
            ),
            Span::styled("", Style::new().fg(theme::TEAL)),
        )
    } else {
        (
            Span::styled("瞬移", Style::new().fg(theme::TEAL)),
            Span::styled(
                format!("{}ms", t.teleport_delay),
                Style::new().fg(theme::TEAL),
            ),
        )
    };
    lines.push(Line::from(vec![
        Span::styled("  移动 ", Style::new().fg(theme::DIM)),
        mode,
        if t.hunt_stand {
            Span::styled("", Style::new().fg(theme::DIM))
        } else {
            Span::styled(" 冷却 ", Style::new().fg(theme::DIM))
        },
        cooldown,
    ]));
    // 拾取 (绿)
    let (pick_main, pick_extra) = if t.pickup_enabled {
        let main = if t.pickup_range <= 0 {
            "全图".to_string()
        } else {
            format!("{}px", t.pickup_range)
        };
        let extra = openstory_console_lib::snapshot::pickup_filter_zh(
            &t.pickup_filter_mode,
            &t.pickup_allow,
            &t.pickup_deny,
        );
        (main, extra)
    } else {
        ("关".to_string(), String::new())
    };
    lines.push(Line::from(vec![
        Span::styled("  拾取 ", Style::new().fg(theme::DIM)),
        Span::styled(
            pick_main,
            Style::new().fg(if t.pickup_enabled {
                theme::GREEN
            } else {
                theme::DIM
            }),
        ),
        if pick_extra.is_empty() {
            Span::styled("", Style::new().fg(theme::DIM))
        } else {
            Span::styled(format!(" · {pick_extra}"), Style::new().fg(theme::DIM))
        },
    ]));
    // 伤害 / 单段总伤害 / 自动停条件 (橙)
    let per_hit = match t.damage {
        Some(d) => d,
        None => {
            let lvl = snap.level as i32;
            lvl * lvl / 2
        }
    };
    let hits = t.skill_hits as i32;
    let total = per_hit * hits;
    let dmg = match t.damage {
        Some(d) => format!("固定 {d}"),
        None => "自动(等级²/2)".to_string(),
    };
    let until = t.until.clone().unwrap_or_else(|| "无".to_string());
    let total_span = if hits > 1 {
        vec![
            Span::styled(" ", Style::new().fg(theme::DIM)),
            Span::styled(format!("×{hits} = {total}"), Style::new().fg(theme::ORANGE)),
        ]
    } else {
        vec![]
    };
    let mut dmg_line = vec![
        Span::styled("  伤害 ", Style::new().fg(theme::DIM)),
        Span::styled(dmg, Style::new().fg(theme::ORANGE)),
    ];
    dmg_line.extend(total_span);
    dmg_line.push(Span::styled(" 自动停 ", Style::new().fg(theme::DIM)));
    dmg_line.push(Span::styled(until, Style::new().fg(theme::ORANGE)));
    lines.push(Line::from(dmg_line));
    // 自动重连 (开关/间隔/次数)
    let (rc, rcd, rcm) = t.reconnect;
    lines.push(Line::from(vec![
        Span::styled("  重连 ", Style::new().fg(theme::DIM)),
        Span::styled(
            if rc {
                format!(
                    "● {rcd}s ×{rcm}",
                    rcm = if rcm == 0 {
                        "∞".to_string()
                    } else {
                        rcm.to_string()
                    }
                )
            } else {
                "○ 关".to_string()
            },
            Style::new().fg(if rc { theme::GREEN } else { theme::DIM }),
        ),
    ]));

    section(&mut lines, "队伍 / 交易");
    lines.push(Line::from(vec![
        Span::styled("  队伍 ", Style::new().fg(theme::DIM)),
        match &snap.party {
            Some((pid, n, leader)) => Span::styled(
                format!("{pid} ({n}人, 队长 {leader})"),
                Style::new().fg(theme::GREEN),
            ),
            None => Span::styled("无", Style::new().fg(theme::DIM)),
        },
    ]));
    lines.push(Line::from(vec![
        Span::styled("  交易 ", Style::new().fg(theme::DIM)),
        match &snap.trade {
            Some(t) => Span::styled(t.clone(), Style::new().fg(theme::YELLOW)),
            None => Span::styled("无", Style::new().fg(theme::DIM)),
        },
    ]));
    if snap.shop_open {
        lines.push(Line::from(Span::styled(
            "  商店已开",
            Style::new().fg(theme::PINK),
        )));
    }
    if snap.dialog_open {
        lines.push(Line::from(Span::styled(
            "  NPC对话中",
            Style::new().fg(theme::PINK),
        )));
    }

    section(&mut lines, "任务 / 规则");
    if snap.task_stack.is_empty() {
        lines.push(Line::from(Span::styled(
            "  任务栈 无",
            Style::new().fg(theme::DIM),
        )));
    } else {
        for (i, (def, step, total)) in snap.task_stack.iter().enumerate() {
            let marker = if i == 0 { "→" } else { "  " };
            lines.push(Line::from(Span::styled(
                format!("  {marker} {def} ({step}/{total})"),
                Style::new().fg(theme::TEAL),
            )));
        }
    }
    let on: Vec<&str> = snap
        .rules
        .iter()
        .filter(|(b, _)| *b)
        .map(|(_, id)| id.as_str())
        .collect();
    let off = snap.rules.len() - on.len();
    lines.push(Line::from(vec![
        Span::styled(format!("  规则 "), Style::new().fg(theme::DIM)),
        Span::styled(format!("开{} ", on.len()), Style::new().fg(theme::GREEN)),
        Span::styled(format!("关{off}"), Style::new().fg(theme::DIM)),
        Span::styled(
            if on.is_empty() {
                "".into()
            } else {
                format!(" {}", on.join(","))
            },
            Style::new().fg(theme::GREEN),
        ),
    ]));
    if !snap.groups.is_empty() {
        let gs: Vec<String> = snap
            .groups
            .iter()
            .map(|(b, id)| format!("{} {}", if *b { "●" } else { "○" }, id))
            .collect();
        lines.push(Line::from(vec![
            Span::styled("  组 ", Style::new().fg(theme::DIM)),
            Span::styled(gs.join("  "), Style::new().fg(theme::TEAL)),
        ]));
    }

    if let Some((nx, points, items)) = snap.cs {
        section(&mut lines, "商城");
        lines.push(Line::from(Span::styled(
            format!(" 点券 {nx} · 点数 {points} · 物品 {items}"),
            Style::new().fg(theme::PURPLE),
        )));
    }

    section(&mut lines, "背包");
    let iv = &snap.inv;
    lines.push(Line::from(vec![
        kv("装备", iv.equip, theme::ORANGE),
        kv("消耗", iv.consume, theme::GREEN),
        kv("设置", iv.setup, theme::BLUE),
        kv("其他", iv.etc, theme::TEXT),
        kv("现金", iv.cash, theme::PINK),
        kv("已装备", iv.worn, theme::PURPLE),
    ]));

    let block = Block::bordered()
        .border_style(Style::new().fg(theme::BORDER))
        .bg(theme::PANEL)
        .title(Span::styled(
            " 角色信息 ",
            Style::new().fg(theme::PINK).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn section(lines: &mut Vec<Line>, name: &str) {
    lines.push(Line::from(Span::styled(
        format!("✦ {name}"),
        Style::new().fg(theme::PINK).add_modifier(Modifier::BOLD),
    )));
}

fn kv(label: &str, v: usize, color: Color) -> Span<'static> {
    Span::styled(format!(" {label} {v}"), Style::new().fg(color))
}

fn toggle(label: &str, on: bool, color: Color) -> Span<'static> {
    if on {
        Span::styled(
            format!(" {label} ●"),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(format!(" {label} ○"), Style::new().fg(theme::DIM))
    }
}

// ---------------------------------------------------------------- 底部

fn render_hint(f: &mut Frame, area: Rect, app: &App) {
    let hint = match app.stage {
        STAGE_WIZARD => " Tab/↑↓ 切换字段 · 输入内容 · Enter 连接 · Esc 退出".to_string(),
        _ if !matches!(app.picker, Picker::None) => " ↑↓ 选择 · Enter 确认 · Ctrl+C 退出".to_string(),
        STAGE_FINISHED => " 会话已结束 — PgUp/PgDn 回顾日志 · Ctrl+C 退出".to_string(),
        _ if app.show_sidebar => {
            // 多会话: 提示里补上多开专属操作 (单会话不显示, 保持原提示逐字不变)
            match app.session_position() {
                Some((i, n)) => format!(
                    " 账号 {i}/{n} · Alt+↑↓切换 · F4启停 · F3重启 · @all/@<档案> 批量 · F2看板 · ↑↓候选 · Tab补全 · Esc关提示 · PgUp/PgDn滚动 · F1帮助 · Ctrl+C退出",
                    i = i + 1
                ),
                None => " ↑↓候选 · Tab补全 · Esc关提示 · PgUp/PgDn滚动 · F1帮助 · Ctrl+C退出".to_string(),
            }
        }
        _ => " ↑↓候选 · Tab/Shift+Tab补全(空输入切过滤) · Esc关提示 · PgUp/PgDn滚动 · F1帮助 · Enter发送 · Ctrl+C退出".to_string(),
    };
    f.render_widget(
        Paragraph::new(Span::styled(hint, Style::new().fg(theme::DIM))).bg(theme::BG),
        area,
    );
}

fn render_input(f: &mut Frame, area: Rect, app: &App) {
    let prompt = Span::styled(
        "> ",
        Style::new().fg(theme::GREEN).add_modifier(Modifier::BOLD),
    );
    let input = Span::styled(app.input.clone(), Style::new().fg(theme::TEXT));
    let line = if app.input.is_empty()
        && app.comp.candidates.is_empty()
        && app.stage == STAGE_RUNNING
        && matches!(app.picker, Picker::None)
    {
        Line::from(vec![
            prompt,
            Span::styled(
                "输入指令, 如 view / hunt on / chat 你好 (Tab 补全)",
                Style::new().fg(theme::BORDER_DIM),
            ),
        ])
    } else {
        Line::from(vec![prompt, input])
    };
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(theme::BORDER))
        .bg(theme::PANEL);
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(line), inner);
}

fn render_completion(f: &mut Frame, area: Rect, app: &App) {
    let total = app.comp.candidates.len();
    if total == 0 {
        return;
    }
    // 可视窗口跟随选中项 (最多 5 行)
    let n = total.min(5);
    let idx = app.comp.idx % total;
    let start = if idx >= n {
        (idx + 1).saturating_sub(n).min(total.saturating_sub(n))
    } else {
        0
    };
    let mut lines: Vec<Line> = Vec::with_capacity(n);
    for (i, cand) in app.comp.candidates.iter().skip(start).take(n).enumerate() {
        let abs = start + i;
        if abs == idx {
            lines.push(Line::from(Span::styled(
                format!(" ▶ {}", cand.display),
                Style::new()
                    .fg(Color::White)
                    .bg(theme::PINK)
                    .add_modifier(Modifier::BOLD),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                format!("   {}", cand.display),
                Style::new().fg(theme::TEXT),
            )));
        }
    }
    let w = 44u16.min(area.width);
    let box_area = Rect {
        x: area.x + 1,
        y: area.y,
        width: w,
        height: area.height,
    };
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_style(Style::new().fg(theme::PINK))
                .bg(theme::PANEL)
                .title(Span::styled(
                    format!(" 补全 {}/{} ", idx + 1, total),
                    Style::new().fg(theme::PINK),
                )),
        ),
        box_area,
    );
}

// ---------------------------------------------------------------- 弹窗

fn render_wizard(f: &mut Frame, area: Rect, app: &App) {
    let Some(w) = &app.wizard else { return };
    let (rect, inner) = modal(area, 52, 12, " 登录向导 ");
    let mut lines: Vec<Line> = Vec::new();
    for fld in [
        Field::Ip,
        Field::Port,
        Field::Account,
        Field::Password,
        Field::AesKey,
    ] {
        let val = w.value(fld);
        let shown = if fld == Field::Password && !val.is_empty() {
            "●".repeat(val.chars().count())
        } else {
            val.to_string()
        };
        let style = if w.focus == fld {
            Style::new()
                .fg(Color::White)
                .bg(theme::PINK)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme::TEXT)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {}  ", fld.label()), Style::new().fg(theme::DIM)),
            Span::styled(format!(" {shown} "), style),
        ]));
    }
    if let Some(err) = &w.error {
        lines.push(Line::from(Span::styled(
            format!("  ⚠ {err}"),
            Style::new().fg(theme::RED).add_modifier(Modifier::BOLD),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  Tab/↑↓ 切换字段 · Enter 开始连接 · Esc 退出",
            Style::new().fg(theme::DIM),
        )));
        // 中文输入法会吞掉 Enter (用它上屏候选词), 用户在向导里就会觉得
        // "按回车没反应"。给一条替代路径, 并明确说出现在该按什么。
        lines.push(Line::from(Span::styled(
            "  输入法开着时 Enter 会被它吃掉 —— 切到英文再按, 或按 Ctrl+M",
            Style::new().fg(theme::DIM),
        )));
    }
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_style(Style::new().fg(theme::PINK))
                .bg(theme::PANEL)
                .title(Span::styled(
                    " 登录向导 · 冒险岛 ",
                    Style::new().fg(theme::PINK).add_modifier(Modifier::BOLD),
                )),
        ),
        rect,
    );
    let _ = inner;
}

fn wizard_cursor_pos(area: Rect, w: &crate::wizard::Wizard) -> Option<Position> {
    let (rect, _) = modal(area, 52, 12, " 登录向导 ");
    let row = match w.focus {
        Field::Ip => 0,
        Field::Port => 1,
        Field::Account => 2,
        Field::Password => 3,
        Field::AesKey => 4,
    };
    let val = w.value(w.focus);
    let shown = if w.focus == Field::Password && !val.is_empty() {
        "●".repeat(val.chars().count())
    } else {
        val.to_string()
    };
    // 字段行 = 边框(1) + "  "(2) + 标签 + "  "(2) + " "(1) + 内容
    let label_w = display_width(w.focus.label()) as u16;
    let x = rect.x + 6 + label_w + display_width(&shown) as u16;
    Some(Position::new(x, rect.y + 1 + row as u16))
}

fn render_picker(f: &mut Frame, area: Rect, picker: &Picker, app: &App) {
    let (title, rows, sel, prefix): (&str, Vec<String>, usize, Option<Vec<String>>) = match picker {
        Picker::Gender { sel } => (
            " 选择性别 ",
            vec!["男 (0)".into(), "女 (1)".into()],
            *sel,
            None,
        ),
        Picker::World { sel } => (
            " 选择世界 ",
            app.snap
                .worlds
                .iter()
                .map(|(wid, name, ch)| format!("{wid} · {name} ({ch} 频道)"))
                .collect(),
            *sel,
            None,
        ),
        Picker::Channel { world, sel } => {
            let n = app
                .snap
                .worlds
                .get(*world)
                .map(|(_, _, ch)| *ch as usize)
                .unwrap_or(1);
            (
                " 选择频道 ",
                // 1-based labels (频道 1..n); `login world` receives sel + 1
                (0..n).map(|i| format!("频道 {}", i + 1)).collect(),
                *sel,
                None,
            )
        }
        Picker::Char { sel } => (
            " 选择角色 ",
            app.snap
                .characters
                .iter()
                .map(|(id, name, job, lv)| format!("{id} · {name} · 等级 {lv} · 职业 {job}"))
                .collect(),
            *sel,
            None,
        ),
        Picker::None => return,
    };
    let _ = prefix;
    if rows.is_empty() {
        return;
    }
    let h = (rows.len() + 2) as u16;
    let w = 58u16.min(area.width.saturating_sub(2));
    let rect = centered(area, w, h);
    let mut lines: Vec<Line> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        if i == sel {
            lines.push(Line::from(Span::styled(
                format!(" ▶ {r}"),
                Style::new()
                    .fg(Color::White)
                    .bg(theme::PINK)
                    .add_modifier(Modifier::BOLD),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                format!("   {r}"),
                Style::new().fg(theme::TEXT),
            )));
        }
    }
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_style(Style::new().fg(theme::PINK))
                .bg(theme::PANEL)
                .title(Span::styled(
                    format!(" {title} "),
                    Style::new().fg(theme::PINK).add_modifier(Modifier::BOLD),
                )),
        ),
        rect,
    );
}

fn render_help(f: &mut Frame, area: Rect, app: &App) {
    let w = 76u16.min(area.width.saturating_sub(2));
    let h = 26u16.min(area.height.saturating_sub(2));
    let rect = centered(area, w, h);
    let mut lines: Vec<Line> = Vec::new();
    // 帮助内容完全由 command_spec 生成 —— 与补全同源, 因此永远不会漂移。
    for row in command_spec::help_rows() {
        // 第三列: 参数占位串 + 子命令摘录 (都由规格表自动生成)
        let mut detail = row.args.clone();
        if !row.subs.is_empty() {
            if !detail.is_empty() {
                detail.push(' ');
            }
            detail.push_str(&format!("[{}]", row.subs.join("/")));
        }
        if row.action_only {
            detail.push_str(" (仅规则/任务动作)");
        }
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {:<10} ", row.name),
                Style::new().fg(theme::GREEN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{:<18} ", row.usage), Style::new().fg(theme::TEXT)),
            Span::styled(detail, Style::new().fg(theme::DIM)),
        ]));
    }
    let inner = h as usize - 2; // 边框
    let total = lines.len();
    // 顶部对齐滚动：偏移 = 距顶部行数, 打开即顶部
    let max_scroll = total.saturating_sub(inner);
    let start = app.help_scroll.min(max_scroll);
    let end = (start + inner).min(total);
    let win = lines.drain(start..end).collect::<Vec<_>>();
    let title = format!(
        " 命令帮助 ({} 条, F1/Esc 关闭 · ↑↓/PgUp/PgDn/滚轮滚动) ",
        total
    );
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(win).wrap(Wrap { trim: false }).block(
            Block::bordered()
                .border_style(Style::new().fg(theme::PINK))
                .bg(theme::PANEL)
                .title(Span::styled(
                    title,
                    Style::new().fg(theme::PINK).add_modifier(Modifier::BOLD),
                )),
        ),
        rect,
    );
}

fn tab_color(tab: u8) -> Color {
    match tab {
        1 => theme::GREEN,
        2 => theme::BLUE,
        3 => theme::PURPLE,
        4 => theme::TEXT,
        5 => theme::PINK,
        _ => theme::DIM,
    }
}

/// 截断/补齐到指定显示宽度 (CJK 按 2 宽计)。
fn fit(s: &str, w: usize) -> String {
    let mut out = String::new();
    let mut len = 0usize;
    for ch in s.chars() {
        let cw = openstory_console_lib::text::display_width(&ch.to_string());
        if len + cw > w {
            break;
        }
        out.push(ch);
        len += cw;
    }
    while len < w {
        out.push(' ');
        len += 1;
    }
    out
}

fn render_bag(f: &mut Frame, area: Rect, app: &App) {
    let w = 96u16.min(area.width.saturating_sub(2));
    let h = area.height.saturating_sub(2);
    let rect = centered(area, w, h);
    let inv = &app.snap.inventory;
    let mut counts = [0usize; 5];
    for (tab, _, _, _) in inv {
        if (1..=5).contains(tab) {
            counts[*tab as usize - 1] += 1;
        }
    }
    let tabs = ["装备", "消耗", "设置", "其他", "现金"];
    let panel_names = ["全部", "装备", "消耗", "设置", "其他", "现金"];
    let panel = app.bag_panel.min(5);
    let items: Vec<&(u8, i16, i32, i16)> = if panel == 0 {
        inv.iter().collect()
    } else {
        inv.iter()
            .filter(|(tab, _, _, _)| *tab == panel as u8)
            .collect()
    };

    let mut lines: Vec<Line> = Vec::new();
    let mut stat = format!(" 物品 {} 件", inv.len());
    for (i, t) in tabs.iter().enumerate() {
        stat.push_str(&format!("  {t}={}", counts[i]));
    }
    lines.push(Line::from(vec![
        Span::styled(stat, Style::new().fg(theme::TEAL)),
        Span::styled(
            format!("  金币 {}", app.snap.meso),
            Style::new().fg(theme::YELLOW),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!(" 面板[{}]  {}件  模式[", panel_names[panel], items.len()),
            Style::new().fg(theme::PINK),
        ),
        Span::styled(
            if app.bag_mode == 1 {
                "卖出所有"
            } else {
                "卖出选中"
            },
            Style::new()
                .fg(if app.bag_mode == 1 {
                    theme::ORANGE
                } else {
                    theme::PINK
                })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "] (m 切换)  Tab/Shift+Tab 切面板 · Enter 卖出 · D 丢弃 · E 穿装",
            Style::new().fg(theme::PINK),
        ),
    ]));

    // 两列布局: 内容宽 = 面板宽 - 2 边框; 每列等宽, 行内交替放 (2i, 2i+1)。
    let inner_w = w.saturating_sub(2) as usize;
    let col_w = inner_w.saturating_sub(1) / 2;
    for (i, it) in items.iter().enumerate().step_by(2) {
        let left = fmt_bag_item(it, col_w);
        let right = items
            .get(i + 1)
            .map(|r| fmt_bag_item(r, col_w))
            .unwrap_or((fit("", col_w), theme::DIM));
        let mut l_span = Span::styled(left.0, Style::new().fg(left.1));
        let mut r_span = Span::styled(right.0, Style::new().fg(right.1));
        if i == app.bag_cursor {
            l_span = l_span.add_modifier(Modifier::REVERSED);
        }
        if i + 1 == app.bag_cursor {
            r_span = r_span.add_modifier(Modifier::REVERSED);
        }
        lines.push(Line::from(vec![l_span, r_span]));
    }
    if items.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "   (该分类为空)",
            Style::new().fg(theme::DIM),
        )]));
    }
    let inner = h as usize - 2;
    let total = lines.len();
    let max_scroll = total.saturating_sub(inner);
    // 光标行 (2 头部 + cursor/2, 两列一行 2 物品) 保持可见
    let mut start = app.bag_scroll.min(max_scroll);
    let cur_row = 2 + app.bag_cursor.min(items.len().saturating_sub(1)) / 2;
    if cur_row >= start + inner {
        start = cur_row.saturating_sub(inner) + 1;
    }
    if cur_row < start {
        start = cur_row;
    }
    let end = (start + inner).min(total);
    let win = lines.drain(start..end).collect::<Vec<_>>();
    let title = format!(
        " 背包 · {} ({} 件, Esc 关闭 · Tab 切面板 · 滚轮滚动) ",
        panel_names[panel],
        inv.len()
    );
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(win).wrap(Wrap { trim: false }).block(
            Block::bordered()
                .border_style(Style::new().fg(theme::TEAL))
                .bg(theme::PANEL)
                .title(Span::styled(
                    title,
                    Style::new().fg(theme::TEAL).add_modifier(Modifier::BOLD),
                )),
        ),
        rect,
    );
}

/// NPC 选项弹框: 服务器 sendSimple 菜单的快速选择界面 (npmenu 开启时自动弹出)。
/// ↑↓ 选择 · Enter 发送 `npc reply <id>` 并关闭 · Esc 收起 (不发送)。
/// 两列布局 (同 bag): 每行 2 个选项, 行内左右各一项, 光标项 REVERSED。
/// 滚动跟随在此完成: 按弹框实际可视行数保证光标可见, 并把窗口顶部写回
/// app.npc_pick_scroll (滚轮/移动共用同一份滚动状态, 不脱节)。
fn render_npc_pick(f: &mut Frame, area: Rect, app: &mut App) {
    let opts = &app.snap.npc_options;
    let w = 96u16.min(area.width.saturating_sub(2));
    // 两列: 数据行数 = ceil(n/2); 弹框高度 = 提示行 + 数据行 + 边框,
    // 上限 20 行 (过长占满屏幕且滚动脱节)。
    let data_rows = (opts.len() + 1) / 2;
    let h = ((data_rows + 3) as u16)
        .min(20)
        .min(area.height.saturating_sub(2));
    let rect = centered(area, w, h.max(5));
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![Span::styled(
        " 请选择 (↑↓ 选择 · Enter 确认 · Esc 收起) ",
        Style::new().fg(theme::PINK),
    )]));
    // 列宽: 边框内宽 - 1 空格分两列
    let inner_w = w.saturating_sub(2) as usize;
    let col_w = inner_w.saturating_sub(1) / 2;
    // 滚动: 数据行窗口 (提示行占 1 行)
    let inner = (h.saturating_sub(2) as usize).saturating_sub(1);
    let max_scroll = data_rows.saturating_sub(inner);
    let cur_row = app.npc_pick_cursor.min(opts.len().saturating_sub(1)) / 2;
    let mut start = app.npc_pick_scroll.min(max_scroll);
    if cur_row >= start + inner {
        start = cur_row.saturating_sub(inner) + 1;
    }
    if cur_row < start {
        start = cur_row;
    }
    app.npc_pick_scroll = start;
    let end = (start + inner).min(data_rows);
    for i in start..end {
        let left = opts.get(i * 2);
        let right = opts.get(i * 2 + 1);
        let mut l_span = match left {
            Some(&(id, ref label)) => npc_option_spans(id, label, col_w),
            None => Span::styled(fit("", col_w), Style::new().fg(theme::DIM)),
        };
        let mut r_span = match right {
            Some(&(id, ref label)) => npc_option_spans(id, label, col_w),
            None => Span::styled(fit("", col_w), Style::new().fg(theme::DIM)),
        };
        if i * 2 == app.npc_pick_cursor {
            l_span = l_span.add_modifier(Modifier::REVERSED);
        }
        if i * 2 + 1 == app.npc_pick_cursor {
            r_span = r_span.add_modifier(Modifier::REVERSED);
        }
        lines.push(Line::from(vec![l_span, r_span]));
    }
    if opts.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "  (无选项 — 服务器返回的不是菜单)",
            Style::new().fg(theme::DIM),
        )]));
    }
    let title = format!(
        " NPC 选项 · {} 项 {} ",
        opts.len(),
        if app.npmenu { "" } else { "(弹框已关)" }
    );
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::bordered()
                .border_style(Style::new().fg(theme::PINK))
                .bg(theme::PANEL)
                .title(Span::styled(
                    title,
                    Style::new().fg(theme::PINK).add_modifier(Modifier::BOLD),
                )),
        ),
        rect,
    );
}

/// 单个 NPC 选项的 span: 黄色 id + 正文标签, 整体 fit 到列宽 (CJK 2 宽)。
fn npc_option_spans(id: i32, label: &str, col_w: usize) -> Span<'static> {
    let id_txt = format!("{id:<6}");
    let lw = col_w.saturating_sub(openstory_console_lib::text::display_width(&id_txt));
    let label_txt = fit(label, lw);
    Span::styled(
        fit(&format!("{id_txt}{label_txt}"), col_w),
        Style::new().fg(theme::YELLOW),
    )
}

/// 商店购买弹框: 服务器 0x146 打开商店时自动弹出 (shop menu 默认开启)。
/// ↑↓ 选商品 · 数字输入数量 · Enter 购买 (`buy <itemid> <qty>`) ·
/// Esc 先清数量再收起。清单每次由 auto_shop_pick 在 0x146 到达时刷新。
fn render_shop_pick(f: &mut Frame, area: Rect, app: &mut App) {
    let items = &app.snap.shop_items;
    let w = 96u16.min(area.width.saturating_sub(2));
    // 两列: 数据行数 = ceil(n/2); 高度 = 提示行 + 数据行 + 边框, 上限 20
    let data_rows = (items.len() + 1) / 2;
    let h = ((data_rows + 3) as u16)
        .min(20)
        .min(area.height.saturating_sub(2));
    let rect = centered(area, w, h.max(5));
    let mut lines: Vec<Line> = Vec::new();
    let qty_hint = if app.shop_qty.is_empty() {
        " 请选择 (↑↓ · 数字输入数量 · Enter 购买 · Esc 收起) ".to_string()
    } else {
        format!(
            " 购买数量: {} (数字修改 · Backspace 删除 · Enter 购买 · Esc 取消) ",
            app.shop_qty
        )
    };
    lines.push(Line::from(Span::styled(
        qty_hint,
        Style::new().fg(if app.shop_qty.is_empty() {
            theme::PINK
        } else {
            theme::ORANGE
        }),
    )));
    // 列宽: 边框内宽 - 1 空格分两列
    let inner_w = w.saturating_sub(2) as usize;
    let col_w = inner_w.saturating_sub(1) / 2;
    // 滚动: 数据行窗口 (提示行占 1 行)
    let inner = (h.saturating_sub(2) as usize).saturating_sub(1);
    let max_scroll = data_rows.saturating_sub(inner);
    let cur_row = app.shop_pick_cursor.min(items.len().saturating_sub(1)) / 2;
    let mut start = app.shop_pick_scroll.min(max_scroll);
    if cur_row >= start + inner {
        start = cur_row.saturating_sub(inner) + 1;
    }
    if cur_row < start {
        start = cur_row;
    }
    app.shop_pick_scroll = start;
    let end = (start + inner).min(data_rows);
    for i in start..end {
        let left = items.get(i * 2).copied();
        let right = items.get(i * 2 + 1).copied();
        let mut l_span = match left {
            Some(it) => shop_item_spans(it, col_w),
            None => Span::styled(fit("", col_w), Style::new().fg(theme::DIM)),
        };
        let mut r_span = match right {
            Some(it) => shop_item_spans(it, col_w),
            None => Span::styled(fit("", col_w), Style::new().fg(theme::DIM)),
        };
        if i * 2 == app.shop_pick_cursor {
            l_span = l_span.add_modifier(Modifier::REVERSED);
        }
        if i * 2 + 1 == app.shop_pick_cursor {
            r_span = r_span.add_modifier(Modifier::REVERSED);
        }
        lines.push(Line::from(vec![l_span, r_span]));
    }
    if items.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "  (商店未打开或清单为空)",
            Style::new().fg(theme::DIM),
        )]));
    }
    let title = format!(
        " 商店 · {} 项 · 金币 {} {} ",
        items.len(),
        app.snap.meso,
        if app.shop_menu { "" } else { "(弹框已关)" }
    );
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::bordered()
                .border_style(Style::new().fg(theme::TEAL))
                .bg(theme::PANEL)
                .title(Span::styled(
                    title,
                    Style::new().fg(theme::TEAL).add_modifier(Modifier::BOLD),
                )),
        ),
        rect,
    );
}

/// 单个商店商品格式化为列宽 span: `itemid 名称 价格G`, 整体 fit。
fn shop_item_spans(it: (i32, i32, i16), col_w: usize) -> Span<'static> {
    let (itemid, price, _qty) = it;
    let id_txt = format!("{itemid:<7}");
    let name = openstory_bot::names::item_name_text(itemid);
    let price_txt = format!("{price}金");
    let lw = col_w
        .saturating_sub(openstory_console_lib::text::display_width(&id_txt))
        .saturating_sub(openstory_console_lib::text::display_width(&price_txt) + 1);
    let name_txt = fit(&name, lw);
    Span::styled(
        fit(&format!("{id_txt}{name_txt} {price_txt}"), col_w),
        Style::new().fg(theme::TEAL),
    )
}

/// 单件物品格式化为列宽文本: `slot=1 名称(itemid) qty=25`，超宽截断。
fn fmt_bag_item(it: &(u8, i16, i32, i16), w: usize) -> (String, Color) {
    let (tab, slot, itemid, qty) = *it;
    let name = openstory_bot::names::item_name_text(itemid);
    let qty_s = if tab == 1 {
        String::new()
    } else {
        format!(" qty={qty}")
    };
    let raw = format!("slot={slot:<2} {name}({itemid}){qty_s}");
    (fit(&raw, w), tab_color(tab))
}

fn modal(area: Rect, w: u16, h: u16, _title: &str) -> (Rect, Rect) {
    let rect = centered(area, w, h);
    (rect, rect)
}

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
    use crate::app::App;
    use openstory_bot::state::Phase;
    use openstory_console_lib::eventq::EventQueue;
    use openstory_console_lib::snapshot::Snapshot;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::sync::atomic::AtomicU8;
    use std::sync::{Arc, Mutex as StdMutex};

    #[test]
    fn fmt_num_groups() {
        assert_eq!(fmt_num(0), "0");
        assert_eq!(fmt_num(1234), "1,234");
        assert_eq!(fmt_num(12345678), "12,345,678");
        assert_eq!(fmt_num(-5), "-5");
    }

    #[test]
    fn board_cells_pad_by_display_width_not_char_count() {
        use openstory_console_lib::text::display_width;
        // 汉字是 1 个 char 但占 2 列 —— `format!("{:<8}", "档案")` 只补 6 个
        // 空格 (按 char 数), 显示出来是 10 列而不是 8。看板错位就是这么来的。
        assert_eq!(display_width(&board_cell("档案", 8, false)), 8);
        assert_eq!(display_width(&board_cell("本地服_100000001", 22, false)), 22);
        // ASCII 短值照常补齐
        assert_eq!(board_cell("abc", 6, false), "abc   ");
        // 右对齐: 内容贴着右边界
        assert_eq!(board_cell("69", 4, true), "  69");
        assert_eq!(display_width(&board_cell("3802/3802", 11, true)), 11);
        // 超宽按显示宽度截断, 不切半个汉字
        assert_eq!(display_width(&board_cell("本地服_100000001", 6, false)), 6);
        assert_eq!(board_cell("本地服_100000001", 6, false), "本地服");
    }

    #[test]
    fn board_header_and_data_rows_have_identical_width() {
        use openstory_console_lib::text::display_width;
        // 表头与数据行走同一个 `board_line` → 显示宽度必须完全一致, 逐列对齐。
        // 这正是现场那个"越往右越歪"的直接判据。
        let header: Vec<String> = BOARD_COLS.iter().map(|(h, _, _)| (*h).to_string()).collect();
        let data: Vec<String> = vec![
            "本地服_100000001".into(),
            "游戏中".into(),
            "Player".into(),
            "69".into(),
            "21".into(),
            "3802/3802".into(),
            "1".into(),
            "射手村集市".into(),
            "关".into(),
            "-".into(),
            "87441479".into(),
        ];
        let h = board_line(&header);
        let d = board_line(&data);
        assert_eq!(
            display_width(&h),
            display_width(&d),
            "表头与数据行宽度不一致 — 列会逐列错开\n表头: {h:?}\n数据: {d:?}"
        );
        // 每一列的**起始显示列**也必须相同 (宽度相同还不够: 中间某列偏了、
        // 后面又偏回来, 总宽仍然可能相等)。
        let starts = |cells: &[String]| -> Vec<usize> {
            let mut v = Vec::new();
            let mut col = 1usize; // 行首那个空格
            for (c, (_, w, _)) in cells.iter().zip(BOARD_COLS.iter()) {
                v.push(col);
                col += w + 1;
                let _ = c;
            }
            v
        };
        assert_eq!(starts(&header), starts(&data));
    }

    #[test]
    fn board_line_pads_every_column_to_its_declared_width() {
        use openstory_console_lib::text::display_width;
        // 逐列核对: 第 k 列的起点必须等于 1 + Σ(前面各列宽 + 1)。
        let cells: Vec<String> = vec![
            "本地服_100000001".into(),
            "游戏中".into(),
            "Player".into(),
            "69".into(),
            "21".into(),
            "3802/3802".into(),
            "1".into(),
            "射手村集市".into(),
            "关".into(),
            "-".into(),
            "87441479".into(),
        ];
        let line = board_line(&cells);
        let mut expected_start = 1usize;
        for (i, (_, w, _)) in BOARD_COLS.iter().enumerate() {
            // 按显示列切出第 i 列那一段, 断言它恰好是 w 个显示列。
            let mut seg = String::new();
            let mut col = 0usize;
            for ch in line.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if col >= expected_start && col < expected_start + *w {
                    seg.push(ch);
                }
                col += cw;
                if col >= expected_start + *w + 1 {
                    break;
                }
            }
            assert_eq!(
                display_width(&seg),
                *w,
                "第 {i} 列 ({}) 的实际宽度不对",
                BOARD_COLS[i].0
            );
            expected_start += w + 1;
        }
    }

    /// F1 帮助弹窗: 换行行也必须有左边框。
    #[test]
    fn help_wrap_keeps_left_border() {
        let queue = Arc::new(EventQueue::new(16));
        let state = Arc::new(tokio::sync::Mutex::new(
            openstory_bot::state::BotState::default(),
        ));
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let tx = Arc::new(StdMutex::new(tx));
        let stage = Arc::new(AtomicU8::new(crate::app::STAGE_RUNNING));
        let outcome = Arc::new(StdMutex::new(None));
        let mut app = App::new(
            queue,
            state,
            tx,
            None,
            stage,
            outcome,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            false,
            "127.0.0.1".into(),
            8484,
            String::new(),
            String::new(),
            String::new(),
        );
        let backend = TestBackend::new(130, 40);
        let mut term = Terminal::new(backend).unwrap();
        // 帧 1: 日志空
        term.draw(|f| render(f, &mut app)).unwrap();
        // 帧 2: 日志灌入内容 + 打开帮助 (模拟真实终端连续帧)
        app.help_open = true;
        for i in 0..40 {
            app.log.push(openstory_bot::emit::Event::new(
                openstory_bot::emit::Level::Info,
                openstory_bot::emit::Category::Cmd,
                format!("[view] line {i} with some long text to force wrapping and content on many rows"),
            ));
        }
        term.draw(|f| render(f, &mut app)).unwrap();
        {
            let buf = term.backend().buffer();
            let mut found = false;
            for y in 0..buf.area.height {
                let row_str: String = (0..buf.area.width)
                    .map(|xx| {
                        buf.cell((xx, y))
                            .map(|c| c.symbol())
                            .unwrap_or(" ")
                            .to_string()
                    })
                    .collect();
                // 标题行: 纯 ASCII 片段 + 左上角边框 (双宽汉字在 buffer 中
                // 会拆成带空格 cell, 不能直接 contains 中文)
                if row_str.contains("PgUp/PgDn") && (row_str.contains("┌") || row_str.contains("╭"))
                {
                    found = true;
                    break;
                }
            }
            assert!(found, "help box title missing after draw 2");
        }
        // 帧 3: 日志再变 (新消息到达, 模拟挂机刷屏)
        for i in 0..40 {
            app.log.push(openstory_bot::emit::Event::new(
                openstory_bot::emit::Level::Info,
                openstory_bot::emit::Category::Cmd,
                format!("[kill] more spam {i} line with long text to wrap around the help area"),
            ));
        }
        term.draw(|f| render(f, &mut app)).unwrap();
        let buf = term.backend().buffer();
        // 帮助弹窗: 130x40 布局下 centered(76x26) → 左框 x=27, 顶 y=7
        let bx: u16 = (130 - 76) / 2;
        let by: u16 = (40 - 26) / 2;
        // 顶部 5 行应有内容 (标题/首条命令), 否则帮助没渲染
        let mut has_content = false;
        for y in by..(by + 5) {
            for x in bx..(bx + 74) {
                let s = buf
                    .cell((x, y))
                    .map(|c| c.symbol())
                    .unwrap_or(" ")
                    .to_string();
                if !s.trim().is_empty() && s != "│" {
                    has_content = true;
                }
            }
        }
        assert!(has_content, "help box content missing");
        // 逐行检查左边框 (含换行产生的行) — 边框被日志吃掉则此处为日志内容
        let mut missing: Vec<u16> = Vec::new();
        for y in (by + 1)..(by + 25) {
            let s = buf
                .cell((bx, y))
                .map(|c| c.symbol())
                .unwrap_or(" ")
                .to_string();
            if !s.contains("│") {
                missing.push(y);
            }
        }
        assert!(
            missing.is_empty(),
            "left border missing on rows: {missing:?}"
        );
    }

    /// bag 背包界面: 打开后渲染物品行 (分组头 + 名称 + itemid + qty), 关闭后消失。
    #[test]
    fn bag_panel_lists_inventory() {
        let queue = Arc::new(EventQueue::new(16));
        let state = Arc::new(tokio::sync::Mutex::new(
            openstory_bot::state::BotState::default(),
        ));
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let tx = Arc::new(StdMutex::new(tx));
        let stage = Arc::new(AtomicU8::new(crate::app::STAGE_RUNNING));
        let outcome = Arc::new(StdMutex::new(None));
        let mut app = App::new(
            queue,
            state,
            tx,
            None,
            stage,
            outcome,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            false,
            "127.0.0.1".into(),
            8484,
            String::new(),
            String::new(),
            String::new(),
        );
        app.snap = Snapshot {
            inventory: vec![
                (1, 1, 1302000, 0),  // EQUIP
                (2, 3, 2000001, 25), // USE
                (2, 4, 2000002, 10), // USE
                (4, 7, 4000000, 1),  // ETC
            ],
            meso: 12345,
            ..Default::default()
        };
        app.bag_open = true;
        let backend = TestBackend::new(130, 40);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render(f, &mut app)).unwrap();
        let buf = term.backend().buffer();
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| {
                        buf.cell((x, y))
                            .map(|c| c.symbol())
                            .unwrap_or(" ")
                            .to_string()
                    })
                    .collect::<String>()
            })
            .collect();
        let all = rows.join("\n");
        assert!(all.contains("1302000"), "equip itemid missing: {all}");
        assert!(all.contains("2000001"), "use itemid missing");
        assert!(all.contains("qty=25"), "use qty missing");
        assert!(all.contains("slot=1"), "slot missing");
        assert!(all.contains("12345"), "meso missing");
        // 面板过滤: EQUIP 面板只显示装备
        app.bag_panel = 1;
        term.draw(|f| render(f, &mut app)).unwrap();
        let buf = term.backend().buffer();
        let rows_panel: String = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| {
                        buf.cell((x, y))
                            .map(|c| c.symbol())
                            .unwrap_or(" ")
                            .to_string()
                    })
                    .collect::<String>()
            })
            .collect();
        assert!(
            rows_panel.contains("1302000"),
            "equip missing in equip panel"
        );
        assert!(
            !rows_panel.contains("2000001"),
            "use item leaked into equip panel"
        );
        // 关闭后不再渲染背包内容
        app.bag_open = false;
        term.draw(|f| render(f, &mut app)).unwrap();
        let buf = term.backend().buffer();
        let rows2: String = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| {
                        buf.cell((x, y))
                            .map(|c| c.symbol())
                            .unwrap_or(" ")
                            .to_string()
                    })
                    .collect::<String>()
            })
            .collect();
        assert!(
            !rows2.contains("1302000"),
            "bag content visible after close"
        );
    }

    #[test]
    fn render_smoke_running() {
        let queue = Arc::new(EventQueue::new(16));
        let state = Arc::new(tokio::sync::Mutex::new(
            openstory_bot::state::BotState::default(),
        ));
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let tx = Arc::new(StdMutex::new(tx));
        let stage = Arc::new(AtomicU8::new(crate::app::STAGE_RUNNING));
        let outcome = Arc::new(StdMutex::new(None));
        let mut app = App::new(
            queue,
            state,
            tx,
            None,
            stage,
            outcome,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            false,
            "127.0.0.1".into(),
            8484,
            "acc".into(),
            "pass".into(),
            String::new(),
        );
        app.snap = Snapshot {
            phase: Phase::InGame,
            hp: 80,
            maxhp: 100,
            mp: 30,
            maxmp: 100,
            exp: 12345,
            level: 30,
            ap: 5,
            sp: 3,
            meso: 1234567,
            mapid: 104040000,
            pos: (120, 45),
            ground_y: 48,
            facing: 0,
            my_cid: 1,
            ..Snapshot::default()
        };
        app.input = "view".into();
        app.cursor = 4;
        app.comp.candidates = vec![openstory_console_lib::completion::Candidate {
            cmd: "view".into(),
            display: "view -- 查看状态".into(),
        }];
        app.comp.idx = 0;

        let backend = TestBackend::new(130, 40);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render(f, &mut app)).unwrap();
        let buf = term.backend().buffer();
        assert_eq!(buf.area.width, 130);
        assert_eq!(buf.area.height, 40);
    }

    /// 弹窗路径 (向导/选择器/帮助) 渲染不 panic。
    #[test]
    fn render_smoke_modals() {
        let queue = Arc::new(EventQueue::new(16));
        let state = Arc::new(tokio::sync::Mutex::new(
            openstory_bot::state::BotState::default(),
        ));
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let tx = Arc::new(StdMutex::new(tx));
        let stage = Arc::new(AtomicU8::new(crate::app::STAGE_WIZARD));
        let outcome = Arc::new(StdMutex::new(None));
        let mut app = App::new(
            queue,
            state,
            tx,
            None,
            stage,
            outcome,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            true,
            "127.0.0.1".into(),
            8484,
            String::new(),
            String::new(),
            String::new(),
        );
        let backend = TestBackend::new(130, 40);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render(f, &mut app)).unwrap();

        // 选择器弹窗
        app.stage = crate::app::STAGE_RUNNING;
        app.wizard = None;
        app.snap.phase = Phase::WorldSelect;
        app.snap.worlds = vec![(0, "蓝蜗牛".to_string(), 3), (1, "蘑菇仔".to_string(), 2)];
        app.picker = Picker::World { sel: 1 };
        term.draw(|f| render(f, &mut app)).unwrap();

        app.picker = Picker::Char { sel: 0 };
        app.snap.characters = vec![(1001, "小剑客".to_string(), 100, 30)];
        term.draw(|f| render(f, &mut app)).unwrap();

        app.picker = Picker::None;
        app.help_open = true;
        term.draw(|f| render(f, &mut app)).unwrap();
    }
}
