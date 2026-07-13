use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

pub fn draw_help(f: &mut Frame) {
    let area = f.area();
    let width = 60u16.min(area.width - 4);
    let height = (area.height - 4).min(30);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect::new(x, y, width, height);

    f.render_widget(Clear, rect);

    let key_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    let lines = vec![
        section("Navigation"),
        key_line("j/k, ↑/↓", "Move cursor up/down", key_style),
        key_line("h, Backspace", "Go to parent directory", key_style),
        key_line("l, Enter", "Enter directory / open file", key_style),
        key_line("gg / G", "Jump to top / bottom", key_style),
        key_line("Ctrl-d/u", "Half-page down/up", key_style),
        key_line("Tab, ←/→", "Switch pane", key_style),
        Line::raw(""),
        section("Modes"),
        key_line("v", "Select mode", key_style),
        key_line("Space", "Space menu (file ops)", key_style),
        key_line("g", "Goto mode", key_style),
        key_line(":", "Command mode", key_style),
        key_line("Esc", "Return to Normal mode", key_style),
        Line::raw(""),
        section("Actions"),
        key_line("d", "Delete file(s)", key_style),
        key_line("r", "Rename", key_style),
        key_line("y", "Yank (copy path)", key_style),
        key_line("p", "Paste", key_style),
        key_line(".", "Toggle hidden files", key_style),
        key_line("/", "Search/filter", key_style),
        key_line("q", "Quit", key_style),
        key_line("?", "Toggle this help", key_style),
    ];

    let block = Block::default()
        .title(" Help — press ? or Esc to close ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(para, rect);
}

fn section(title: &str) -> Line<'_> {
    Line::from(Span::styled(
        title,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
    ))
}

fn key_line<'a>(key: &'a str, desc: &'a str, key_style: Style) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("  {key:14}"), key_style),
        Span::raw(desc),
    ])
}
