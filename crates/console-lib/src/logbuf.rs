//! 日志环形缓冲 + 换行缓存 + 滚动窗口计算 (纯逻辑, 可单测)。

use std::collections::VecDeque;

use openstory_bot::emit::{Category, Event, Field, Level};
use unicode_width::UnicodeWidthChar;

pub const MAX_LINES: usize = 1000;

/// 一条日志 (可能含多行 — view 表格等整块输出)。
pub struct LogLine {
    pub ts: String,
    pub cat: Category,
    pub level: Level,
    pub text: String,
    /// 结构化字段 (前端自由渲染语言)
    pub fields: Vec<(Field, String)>,
    /// 按宽度缓存的换行结果
    wrapped: Option<(u16, Vec<String>)>,
    /// 中文渲染后的整条文本 (先渲染再折行, 避免折行截断解析)
    rendered: Option<String>,
    /// 缓存的显示行数 (与 wrapped 同步)
    display_line_count: usize,
}

impl LogLine {
    fn new(
        ts: String,
        cat: Category,
        level: Level,
        text: String,
        fields: Vec<(Field, String)>,
    ) -> Self {
        Self {
            ts,
            cat,
            level,
            text,
            fields,
            wrapped: None,
            rendered: None,
            display_line_count: 0,
        }
    }

    /// 中文渲染后的整条文本 (与 ui::log_line 原渲染链一致: 已知整句覆盖
    /// > 结构化字段 > 英文原文回退)。整条渲染后再折行, 避免折行把消息
    /// 切成两段导致首行解析残缺 (如 far seek 的 dist= 被切掉显示 `距离?px`)
    /// 和续行重复 tag + 英文残片。
    /// 多行事件 (hunt/group/task status) 逐行独立渲染 — zh_body 按行匹配。
    fn rendered_text(&self) -> String {
        let mut out: Vec<String> = Vec::new();
        for (i, line) in self.text.split('\n').enumerate() {
            let body = strip_tag(line);
            let rendered = match extract_tag(line) {
                Some(tag) => {
                    if let Some(zh) = crate::zh::zh_body(&tag, &body) {
                        zh
                    } else if i == 0 && !self.fields.is_empty() {
                        crate::zh::render_fields(&self.fields)
                    } else {
                        crate::enrich::enriched_body(&body)
                    }
                }
                None => crate::enrich::enriched_body(line),
            };
            out.push(rendered);
        }
        out.join("\n")
    }

    /// 换行后的显示行数 (带缓存; 宽度变化时重新换行)。
    /// 折行作用于中文渲染文本 (先渲染后折行)。
    pub fn wrap(&mut self, width: u16) -> usize {
        if let Some((w, lines)) = &self.wrapped {
            if *w == width {
                return lines.len();
            }
        }
        let rendered = self
            .rendered
            .clone()
            .unwrap_or_else(|| self.rendered_text());
        let lines = wrap_text(&rendered, width);
        let count = lines.len();
        self.rendered = Some(rendered);
        self.wrapped = Some((width, lines));
        self.display_line_count = count;
        count
    }

    /// 返回缓存的显示行数 (无缓存时返回 0)。
    pub fn cached_line_count(&self) -> usize {
        self.display_line_count
    }

    /// 取换行后的第 i 行 (须先调用 wrap; 行内容为渲染后的文本)。
    pub fn wrapped_line(&self, i: usize) -> &str {
        self.wrapped
            .as_ref()
            .and_then(|(_, l)| l.get(i))
            .map(|s| s.as_str())
            .unwrap_or("")
    }
}

/// 渲染窗口的一行。
pub struct WindowLine {
    pub ts: String,
    pub cat: Category,
    pub level: Level,
    /// 行首 tag (如 "[chat]"), 续行无
    pub tag: Option<String>,
    pub body: String,
    /// 首行原始正文 (渲染前的英文原文; 续行为空)。供 zh_hidden 等按原文匹配。
    pub raw_body: String,
}

/// 按显示宽度换行 (不截断单词; CJK 逐字)。
pub fn wrap_text(text: &str, width: u16) -> Vec<String> {
    let w = width as usize;
    if w == 0 {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let mut cur = String::new();
        let mut cur_w = 0usize;
        for ch in raw.chars() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            if cur_w + cw > w && cur_w > 0 {
                out.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            cur.push(ch);
            cur_w += cw;
        }
        out.push(cur);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

pub struct LogBuf {
    lines: VecDeque<LogLine>,
}

/// 渲染前缀预留宽度: 时间戳(8) + 空格(1) + 中文 tag 最大宽度(~7)。
/// window/total 按正文折行时预留, 避免渲染拼前缀后超宽被二次折行。
const PREFIX_W: u16 = 16;

impl Default for LogBuf {
    fn default() -> Self {
        Self {
            lines: VecDeque::new(),
        }
    }
}

impl LogBuf {
    pub fn push(&mut self, ev: Event) {
        let ts = format_time(ev.ts);
        self.lines
            .push_back(LogLine::new(ts, ev.category, ev.level, ev.text, ev.fields));
        while self.lines.len() > MAX_LINES {
            self.lines.pop_front();
        }
    }

    /// 清空全部日志 (`clear` 命令, 本地 UI 操作, 不影响 bot)。
    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// 往日志里塞一条**前端自己产生**的通知 (不是 bot 发的事件)。
    ///
    /// 用途: 自动拉起 / 手动重启 / 指令没送出去 这类"控制台的事"。它们必须
    /// 出现在**对应会话的日志里** —— 混进别的会话会让多开时的因果彻底看不清
    /// ("这个号为什么重连了"要能在它自己的日志里读到)。
    ///
    /// 走 `emit::Event::new` 而不是裸 `LogLine`: 时间戳 / 分类 / 中文渲染
    /// 三条链路与 bot 事件完全一致, 前端不需要第二套渲染。
    pub fn push_note(&mut self, level: Level, text: impl Into<String>) {
        self.push(Event::new(level, Category::System, text.into()));
    }

    /// O(1) 与另一个缓冲交换内容。
    ///
    /// 多会话 UI 用它把"当前选中会话的日志"换到渲染位置: 切换账号时只有三个
    /// `VecDeque` 指针交换, 不复制任何日志行 (1000 行 × N 个账号会是可观开销)。
    pub fn swap(&mut self, other: &mut LogBuf) {
        std::mem::swap(&mut self.lines, &mut other.lines);
    }

    /// 当前缓冲的行数 (不含折行展开; 用于诊断与测试)。
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// 过滤后全部显示行数 (利用缓存, 避免不必要的 wrap)。
    pub fn total_display_lines(&mut self, filter: Option<Category>, width: u16) -> usize {
        let wrap_w = width.saturating_sub(PREFIX_W);
        let mut total = 0;
        for l in &mut self.lines {
            if filter.map_or(true, |f| l.cat == f) {
                if l.cached_line_count() > 0
                    && l.wrapped.as_ref().map_or(false, |(w, _)| *w == wrap_w)
                {
                    total += l.cached_line_count();
                } else {
                    total += l.wrap(wrap_w);
                }
            }
        }
        total
    }

    /// 取可视窗口: 过滤后从底部往上数 `offset` 行, 显示 `height` 行。
    /// offset=0 表示贴底。
    /// 优化: 利用 cached_line_count 避免不必要的 wrap 调用,
    /// 只对可见范围内的条目做 wrap + render_line。
    pub fn window(
        &mut self,
        filter: Option<Category>,
        width: u16,
        height: usize,
        offset: usize,
    ) -> Vec<WindowLine> {
        if height == 0 {
            return Vec::new();
        }
        let wrap_w = width.saturating_sub(PREFIX_W);

        // 收集过滤后的条目, 同时获取/计算每条的显示行数 (利用缓存)。
        let mut filtered: Vec<(usize, &mut LogLine)> = self
            .lines
            .iter_mut()
            .enumerate()
            .filter(|(_, l)| filter.map_or(true, |f| l.cat == f))
            .collect();

        // 第一遍: 确保每条都已 wrap (利用缓存, 仅首次或宽度变化时重新计算)。
        let counts: Vec<usize> = filtered.iter_mut().map(|(_, l)| l.wrap(wrap_w)).collect();

        let total: usize = counts.iter().sum();
        if total <= height {
            // 全部可见, 直接渲染。
            let mut out = Vec::new();
            for ((_, l), &n) in filtered.iter().zip(&counts) {
                for i in 0..n {
                    out.push(render_line(l, i));
                }
            }
            return out;
        }

        // 计算可见行范围。
        let bottom_line = total.saturating_sub(1 + offset);
        let top_line = (bottom_line + 1).saturating_sub(height);

        // 前缀和: 找到 top_line 落在哪个条目中。
        let mut prefix: Vec<usize> = Vec::with_capacity(counts.len());
        let mut acc = 0usize;
        for &n in &counts {
            acc += n;
            prefix.push(acc);
        }

        // 二分查找 top_line 所在的条目索引。
        let start_idx = match prefix.binary_search(&top_line) {
            Ok(i) => i,  // top_line 恰好是某条目末尾, 从下一条开始
            Err(i) => i, // top_line 在第 i 条内部
        };

        // 从 start_idx 开始渲染, 直到填满 height 行。
        let mut out = Vec::with_capacity(height);
        let base = if start_idx > 0 {
            prefix[start_idx - 1]
        } else {
            0
        };
        let mut pos = base; // 当前行号

        for i in start_idx..filtered.len() {
            let (_, ref mut l) = filtered[i];
            let n = counts[i];
            let entry_start = pos;
            let entry_end = pos + n;
            let render_start = top_line.saturating_sub(entry_start);
            let render_end = (bottom_line + 1).saturating_sub(entry_start).min(n);
            for j in render_start..render_end {
                out.push(render_line(l, j));
                if out.len() >= height {
                    return out;
                }
            }
            pos = entry_end;
        }
        out
    }
}

fn render_line(l: &LogLine, i: usize) -> WindowLine {
    // tag 从原文提取: 折行作用的是渲染文本 (无 tag 前缀)。
    let first_tag = extract_tag(&l.text);
    WindowLine {
        ts: l.ts.clone(),
        cat: l.cat,
        level: l.level,
        // 续行无 tag: 折行只是长消息的物理分行, 重复渲染 tag 会把一条
        // 消息看成分裂的两条 (且续行为英文残片)。分类/过滤由 cat/raw_body 承担。
        tag: if i == 0 { first_tag } else { None },
        body: l.wrapped_line(i).to_string(),
        raw_body: if i == 0 {
            strip_tag(&l.text)
        } else {
            String::new()
        },
    }
}

/// 提取行首 tag: `[chat] ...` → `[chat]`。
pub fn extract_tag(text: &str) -> Option<String> {
    let t = text.trim_start();
    if let Some(rest) = t.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            return Some(format!("[{}]", &rest[..end]));
        }
    }
    None
}

/// 去掉行首 tag 后的正文 (trim 左侧空白)。
pub fn strip_tag(text: &str) -> String {
    let t = text.trim_start();
    if let Some(rest) = t.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let after = &rest[end + 1..];
            return after.trim_start().to_string();
        }
    }
    t.to_string()
}

fn format_time(ts: std::time::SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Local> = ts.into();
    dt.format("%H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(cat: Category, text: &str) -> Event {
        Event::new(Level::Info, cat, text.to_string())
    }

    #[test]
    fn wrap_ascii_and_cjk() {
        assert_eq!(wrap_text("hello world", 5), vec!["hello", " worl", "d"]);
        assert_eq!(wrap_text("你好世界", 4), vec!["你好", "世界"]);
        assert_eq!(wrap_text("a\nb", 10), vec!["a", "b"]);
    }

    #[test]
    fn cap_evicts_oldest() {
        let mut b = LogBuf::default();
        for i in 0..(MAX_LINES + 10) {
            b.push(ev(Category::Cmd, &format!("line {i}")));
        }
        // 窗口只保留最新的 MAX_LINES 条
        let w = b.window(None, 80, 10_000, 0);
        assert_eq!(w.len(), MAX_LINES);
    }

    #[test]
    fn window_pinned_shows_latest() {
        let mut b = LogBuf::default();
        for i in 1..=5 {
            b.push(ev(Category::Cmd, &format!("l{i}")));
        }
        let w = b.window(Some(Category::Cmd), 80, 3, 0);
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].body, "l3");
        assert_eq!(w[2].body, "l5");
    }

    #[test]
    fn window_offset_scrolls_up() {
        let mut b = LogBuf::default();
        for i in 1..=5 {
            b.push(ev(Category::Cmd, &format!("l{i}")));
        }
        let w = b.window(Some(Category::Cmd), 80, 3, 2);
        assert_eq!(w[0].body, "l1");
        assert_eq!(w[2].body, "l3");
    }

    #[test]
    fn filter_applies() {
        let mut b = LogBuf::default();
        b.push(ev(Category::Chat, "c1"));
        b.push(ev(Category::Notice, "n1"));
        b.push(ev(Category::Chat, "c2"));
        let w = b.window(Some(Category::Notice), 80, 10, 0);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].body, "n1");
        let w2 = b.window(None, 80, 10, 0);
        assert_eq!(w2.len(), 3);
    }

    #[test]
    fn all_filter_shows_latest_multiline_status() {
        let mut b = LogBuf::default();
        for i in 0..50 {
            b.push(ev(Category::Cmd, &format!("m{i}")));
        }
        b.push(ev(
            Category::Hunt,
            "[hunt] status=on mode=skill skill=1001005\n[hunt] pickup=500 enabled cooldown=700ms\n[hunt] range=70 teleport_delay=0ms\n[hunt] attack_max=6 skill_range=200 max_targets=6 mp_cost=7\n[hunt] pickup_filter=deny items=[2000000,2000001]\n[hunt] damage=auto(lvl^2/2) until=none",
        ));
        let w = b.window(None, 80, 10, 0);
        assert!(
            w.iter().any(|l| l.body.contains("状态")),
            "status must show in All tab: {:?}",
            w.iter().map(|l| &l.body).collect::<Vec<_>>()
        );
        assert!(
            w.iter().any(|l| l.body.contains("伤害")),
            "status tail must show: {:?}",
            w.iter().map(|l| &l.body).collect::<Vec<_>>()
        );
    }

    #[test]
    fn tag_extraction() {
        assert_eq!(extract_tag("[chat] 你好").as_deref(), Some("[chat]"));
        assert!(extract_tag("plain").is_none());
        assert_eq!(strip_tag("[chat] 你好"), "你好");
        assert_eq!(strip_tag("plain"), "plain");
        assert_eq!(
            extract_tag("[command error] x").as_deref(),
            Some("[command error]")
        );
    }

    #[test]
    fn multiline_entries_render_all_lines() {
        let mut b = LogBuf::default();
        b.push(ev(Category::View, "a\nb\nc"));
        let w = b.window(Some(Category::View), 80, 10, 0);
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].body, "a");
        assert_eq!(w[1].tag, None);
        assert_eq!(w[2].body, "c");
    }

    #[test]
    fn wrap_after_zh_render_keeps_message_whole() {
        // 回归: far seek 长消息在窄面板折行 — 先渲染后折行, 首行显示完整
        // 中文 (dist 不被切掉, 不出现 `距离?px`), 续行不重复 tag 也不是英文残片。
        let mut b = LogBuf::default();
        b.push(ev(
            Category::Hunt,
            "[teleport] far seek -> (300,400) dist=594px",
        ));
        let w = b.window(None, 30, 10, 0);
        assert!(w.len() >= 2, "must wrap: got {}", w.len());
        assert_eq!(w[0].tag.as_deref(), Some("[teleport]"));
        assert!(
            w[0].body.starts_with("就近寻怪传送"),
            "first line: {:?}",
            w[0].body
        );
        assert!(
            !w[0].body.contains('?'),
            "dist must not be cut off: {:?}",
            w[0].body
        );
        assert_eq!(w[1].tag, None, "wrap continuation must not repeat the tag");
        assert!(
            !w[1].body.contains("far seek"),
            "no raw English residue: {:?}",
            w[1].body
        );
        // 拼接后整条内容完整
        let joined: String = w.iter().map(|l| l.body.clone()).collect();
        assert!(joined.contains("距离594px"), "joined: {joined}");
        assert_eq!(w[0].raw_body, "far seek -> (300,400) dist=594px");
    }

    #[test]
    fn unrendered_lines_fallback_to_raw() {
        // 无已知 zh_body / 字段的消息: 首行是原文 (enriched), 续行无 tag。
        let mut b = LogBuf::default();
        b.push(ev(Category::Cmd, "[some_new_tag] hello world"));
        let w = b.window(None, 60, 10, 0);
        assert_eq!(w[0].tag.as_deref(), Some("[some_new_tag]"));
        assert!(w[0].body.contains("hello"));
        assert_eq!(w[0].raw_body, "hello world");
        for l in w.iter().skip(1) {
            assert_eq!(l.tag, None);
        }
    }
}
