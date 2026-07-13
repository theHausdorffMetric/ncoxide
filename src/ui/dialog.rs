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
    // saturating_sub: raw subtraction underflows (panics) on tiny terminals.
    let width = 50u16.min(area.width.saturating_sub(4));
    let height = 8u16.min(area.height.saturating_sub(2));
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
