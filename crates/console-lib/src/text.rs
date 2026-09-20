//! 文本度量与编辑 —— 显示宽度、字符边界、按显示宽折行。
//!
//! 中文是双宽字符, "按字节/字符" 与 "按屏幕列" 是三个不同的概念。所有涉及
//! 光标定位与换行的地方都必须走这里, 否则中文输入框的光标会漂移、日志折行
//! 会把内容推出屏幕。

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// 字符个数 (用于输入框光标索引 —— 光标是"第几个字符", 不是字节偏移)。
pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// 显示宽度 (屏幕列数): 中文/全角算 2, 其余算 1。
pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// 在第 `ci` 个字符处插入 (`ci` 超界则追加)。
pub fn insert_at(s: &mut String, ci: usize, c: char) {
    let mut chars: Vec<char> = s.chars().collect();
    chars.insert(ci.min(chars.len()), c);
    *s = chars.into_iter().collect();
}

/// 删除第 `ci` 个字符 (`ci` 超界则不动)。
pub fn remove_at(s: &mut String, ci: usize) {
    let mut chars: Vec<char> = s.chars().collect();
    if ci < chars.len() {
        chars.remove(ci);
    }
    *s = chars.into_iter().collect();
}

/// 取第 `ci` 个字符处的**字节**偏移 (切片/取前缀用; 保证不切开多字节字符)。
pub fn char_boundary(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map(|(i, _)| i).unwrap_or(s.len())
}

/// 按显示宽度折行 (不截断单词; CJK 逐字)。
///
/// 空行保留为空串 —— 调用方依赖"折行后行数与原文本行数对应"来定位滚动窗口。
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

/// 按显示宽度截断到最多 `width` 列 (CJK 不会被切成半个字)。
pub fn truncate_width(s: &str, width: usize) -> String {
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > width {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out
}

/// 右侧补空格到 `width` 显示列 (对齐用; 已超宽则原样返回)。
pub fn pad_width(s: &str, width: usize) -> String {
    let w = display_width(s);
    if w >= width {
        return s.to_string();
    }
    format!("{s}{}", " ".repeat(width - w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_width_counts_cjk_as_two() {
        assert_eq!(display_width("ab"), 2);
        assert_eq!(display_width("你好"), 4);
        assert_eq!(display_width("a你b"), 4);
        assert_eq!(display_width(""), 0);
        // 全角标点同样是双宽
        assert_eq!(display_width("，"), 2);
    }

    #[test]
    fn char_len_vs_display_width() {
        assert_eq!(char_len("你好"), 2);
        assert_eq!(display_width("你好"), 4);
        assert_eq!(char_len(""), 0);
    }

    #[test]
    fn insert_remove_cjk() {
        let mut s = String::from("你好");
        insert_at(&mut s, 1, '中');
        assert_eq!(s, "你中好");
        remove_at(&mut s, 1);
        assert_eq!(s, "你好");
        // 超界插入 = 追加
        insert_at(&mut s, 99, '!');
        assert_eq!(s, "你好!");
        // 超界删除 = 不动
        remove_at(&mut s, 99);
        assert_eq!(s, "你好!");
        remove_at(&mut s, 0);
        assert_eq!(s, "好!");
    }

    #[test]
    fn char_boundary_is_byte_offset() {
        let s = "a你b";
        assert_eq!(char_boundary(s, 0), 0);
        assert_eq!(char_boundary(s, 1), 1); // '你' 起于字节 1
        assert_eq!(char_boundary(s, 2), 4); // 'b' 起于字节 4
        assert_eq!(char_boundary(s, 3), 5); // 末尾
        assert_eq!(char_boundary(s, 99), 5); // 超界 = 全长
                                             // 任何偏移都必须落在字符边界上, 否则切片会 panic
        for i in 0..=char_len(s) {
            let b = char_boundary(s, i);
            assert!(s.is_char_boundary(b));
        }
    }

    #[test]
    fn wrap_respects_display_width() {
        // 宽度 4 = 两个中文
        let lines = wrap_text("你好世界", 4);
        assert_eq!(lines, vec!["你好", "世界"]);
        // 宽度 5: 第二行放不下第三个字
        let lines = wrap_text("你好世界", 5);
        assert_eq!(lines, vec!["你好", "世界"]);
    }

    #[test]
    fn wrap_preserves_explicit_newlines() {
        let lines = wrap_text("a\nb", 10);
        assert_eq!(lines, vec!["a", "b"]);
        // 空行保留 (调用方按行数定位滚动窗口)
        let lines = wrap_text("a\n\nb", 10);
        assert_eq!(lines, vec!["a", "", "b"]);
    }

    #[test]
    fn wrap_zero_width_is_safe() {
        // 宽度 0 曾会导致除零/死循环, 这里必须退化成单空行而不是 panic
        assert_eq!(wrap_text("你好", 0), vec![String::new()]);
        assert_eq!(wrap_text("", 0), vec![String::new()]);
    }

    #[test]
    fn wrap_empty_text() {
        assert_eq!(wrap_text("", 10), vec![String::new()]);
    }

    #[test]
    fn wrap_exact_fit_does_not_emit_trailing_empty() {
        // 正好放下时不应多出一个空行
        let lines = wrap_text("你好", 4);
        assert_eq!(lines, vec!["你好"]);
        let lines = wrap_text("ab", 2);
        assert_eq!(lines, vec!["ab"]);
    }

    #[test]
    fn truncate_never_splits_a_cjk_char() {
        assert_eq!(truncate_width("你好世界", 5), "你好"); // 5 列放不下第 3 个字
        assert_eq!(truncate_width("你好世界", 6), "你好世");
        assert_eq!(truncate_width("abcdef", 3), "abc");
        assert_eq!(truncate_width("你好", 0), "");
        assert_eq!(truncate_width("你好", 99), "你好");
    }

    #[test]
    fn pad_width_aligns_by_display_columns() {
        assert_eq!(pad_width("你好", 6), "你好  ");
        assert_eq!(pad_width("abc", 3), "abc");
        assert_eq!(pad_width("abcd", 3), "abcd"); // 已超宽, 原样
        assert_eq!(display_width(&pad_width("你好", 7)), 7);
    }
}
