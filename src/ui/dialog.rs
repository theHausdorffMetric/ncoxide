use std::path::PathBuf;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

/// Action to execute when a Confirm dialog is accepted.
#[derive(Debug, Clone)]
pub enum ConfirmAction {
    Delete {
        paths: Vec<PathBuf>,
    },
    OverwriteCopy {
        sources: Vec<PathBuf>,
        target: PathBuf,
    },
    OverwriteMove {
        sources: Vec<PathBuf>,
        target: PathBuf,
    },
    OverwriteRename {
        source: PathBuf,
        new_name: String,
    },
}

/// Dialog types the app can show.
#[derive(Debug, Clone)]
pub enum Dialog {
    Confirm {
        title: String,
        message: String,
        action: ConfirmAction,
    },
    Error {
        message: String,
    },
    Info {
        title: String,
        message: String,
    },
}

pub fn draw_dialog(f: &mut Frame, dialog: &Dialog) {
    let area = f.area();
    let (title, message, border_color) = match dialog {
        Dialog::Confirm { title, message, .. } => (title.as_str(), message.as_str(), Color::Yellow),
        Dialog::Error { message } => ("Error", message.as_str(), Color::Red),
        Dialog::Info { title, message } => (title.as_str(), message.as_str(), Color::Cyan),
    };

    let hint = match dialog {
        Dialog::Confirm { .. } => "\n\n[y]es / [n]o",
        Dialog::Error { .. } | Dialog::Info { .. } => "\n\nPress any key to dismiss",
    };

    let text = format!("{message}{hint}");
    let (width, height) = dialog_size(&text, area);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect::new(x, y, width, height);

    f.render_widget(Clear, rect);

    let block = Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let para = Paragraph::new(text).block(block).wrap(Wrap { trim: true });

    f.render_widget(para, rect);
}

/// Outer size for `text`: at least the classic 50×8, wide enough for the
/// longest line (so a URL stays on one row and terminals can make it a
/// link), tall enough for the wrapped lines, and never past the screen.
/// Saturating throughout: raw subtraction underflows on tiny terminals.
fn dialog_size(text: &str, area: Rect) -> (u16, u16) {
    let max_width = area.width.saturating_sub(4).max(1);
    let longest = text.lines().map(|l| l.chars().count()).max().unwrap_or(0);
    let width = (longest as u16)
        .saturating_add(4)
        .clamp(50.min(max_width), max_width);
    let inner_width = width.saturating_sub(2).max(1) as usize;
    let wrapped: usize = text
        .lines()
        .map(|l| l.chars().count().max(1).div_ceil(inner_width))
        .sum();
    let max_height = area.height.saturating_sub(2).max(1);
    let height = (wrapped as u16)
        .saturating_add(2)
        .clamp(8.min(max_height), max_height);
    (width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dialog_size_grows_for_long_lines_and_stays_on_screen() {
        let area = Rect::new(0, 0, 80, 24);
        // Short text: the classic size.
        assert_eq!(dialog_size("boom\n\nPress any key", area), (50, 8));
        // A 55-char URL gets a row of its own (width = len + border + pad).
        let url = "http://127.0.0.1:6269/0123456789abcdef0123456789abcdef/";
        let (w, h) = dialog_size(&format!("{url}\n\nhint"), area);
        assert_eq!(w as usize, url.len() + 4);
        assert_eq!(h, 8);
        // Many lines: taller, never past the screen.
        let tall = "x\n".repeat(40);
        assert_eq!(dialog_size(&tall, area).1, 22);
        // Longer than the screen is wide: capped, and wrapping is counted.
        let (w, h) = dialog_size(&"a".repeat(300), area);
        assert_eq!(w, 76);
        assert_eq!(h, 8); // 300 / 74 → 5 rows + border fits the minimum
        // Tiny terminals: no panic, nothing larger than the area.
        for (aw, ah) in [(1u16, 1u16), (3, 2), (6, 3), (20, 3)] {
            let (w, h) = dialog_size(url, Rect::new(0, 0, aw, ah));
            assert!(w <= aw.max(1) && h <= ah.max(1), "{aw}x{ah}: {w}x{h}");
        }
    }
}
