//! 冒险岛风格配色 (源自 `client/docs/UI Design Token规范.md` 玻璃拟态粉色系),
//! 适配深色终端可读性。

use openstory_bot::emit::{Category, Level};
use ratatui::style::{Color, Style};

pub const BG: Color = Color::Rgb(255, 244, 230); // cream
pub const PANEL: Color = Color::Rgb(255, 251, 243);
pub const BORDER: Color = Color::Rgb(74, 55, 40); // #4A3728
pub const BORDER_DIM: Color = Color::Rgb(170, 148, 124);
pub const TEXT: Color = Color::Rgb(90, 70, 54); // #5A4636
pub const DIM: Color = Color::Rgb(168, 146, 122);
pub const PINK: Color = Color::Rgb(206, 92, 140);
pub const BLUE: Color = Color::Rgb(70, 128, 198);
pub const YELLOW: Color = Color::Rgb(186, 146, 28);
pub const GREEN: Color = Color::Rgb(82, 158, 92);
pub const ORANGE: Color = Color::Rgb(208, 118, 38);
pub const PURPLE: Color = Color::Rgb(136, 106, 208);
pub const TEAL: Color = Color::Rgb(38, 138, 128);
pub const RED: Color = Color::Rgb(206, 52, 52);
pub const GOLD: Color = Color::Rgb(168, 128, 38);
pub const HP: Color = Color::Rgb(214, 76, 76);
pub const MP: Color = Color::Rgb(66, 128, 216);

pub fn cat_label(cat: Category) -> &'static str {
    match cat {
        Category::System => "系统",
        Category::Cmd => "命令",
        Category::Chat => "聊天",
        Category::Notice => "公告",
        Category::Npc => "NPC",
        Category::Hunt => "打怪",
        Category::Rule => "规则",
        Category::Group => "组",
        Category::Task => "任务",
        Category::View => "查看",
        Category::Packet => "封包",
        Category::Error => "错误",
    }
}

pub fn cat_color(cat: Category) -> Color {
    match cat {
        Category::System => BORDER,
        Category::Cmd => BLUE,
        Category::Chat => GREEN,
        Category::Notice => YELLOW,
        Category::Npc => PINK,
        Category::Hunt => ORANGE,
        Category::Rule => TEAL,
        Category::Group => GOLD,
        Category::Task => TEAL,
        Category::View => PURPLE,
        Category::Packet => DIM,
        Category::Error => RED,
    }
}

pub fn cat_style(cat: Category) -> Style {
    Style::default().fg(cat_color(cat))
}

pub fn body_style(level: Level, text: &str) -> Style {
    if level == Level::Err {
        return Style::default().fg(RED);
    }
    // [report] 行内嵌 [ERR]/[WARN] 级别着色
    if text.contains("[ERR]") || text.contains("[error]") {
        Style::default().fg(RED)
    } else if text.contains("[WARN]") || text.contains("[warn]") {
        Style::default().fg(ORANGE)
    } else {
        Style::default().fg(TEXT)
    }
}

/// 冒险岛式血条: `██████░░░░` — filled blocks + dim track, ratio colors.
pub fn bar(ratio: f64, width: usize, color: Color) -> String {
    let w = width.max(1);
    let filled = ((ratio.clamp(0.0, 1.0)) * w as f64).round() as usize;
    let mut s = String::with_capacity(w * 3);
    for _ in 0..filled {
        s.push('█');
    }
    for _ in filled..w {
        s.push('░');
    }
    let _ = color;
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_fills_proportionally() {
        assert_eq!(bar(1.0, 10, HP), "██████████");
        assert_eq!(bar(0.0, 10, HP), "░░░░░░░░░░");
        assert_eq!(bar(0.5, 10, HP), "█████░░░░░");
        assert_eq!(bar(0.33, 10, HP), "███░░░░░░░");
    }
}
