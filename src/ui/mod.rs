pub mod dialog;
pub mod help;
pub mod pane_view;
pub mod status_line;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::Color;

use crate::app::App;
use crate::config::ColorConfig;

/// Resolved file-list colors, parsed once per frame from [`ColorConfig`].
pub struct Theme {
    pub directory: Color,
    pub symlink: Color,
    pub executable: Color,
    pub selected: Color,
    pub cursor: Color,
    pub active_border: Color,
    pub inactive_border: Color,
}

/// Parse a color name/hex (`"blue"`, `"#1e1e2e"`), falling back to `default`
/// so an unset or invalid value keeps the built-in appearance.
fn parse_color(value: &str, default: Color) -> Color {
    value.parse().unwrap_or(default)
}

impl Theme {
    pub fn from_config(c: &ColorConfig) -> Self {
        Theme {
            directory: parse_color(&c.directory, Color::Blue),
            symlink: parse_color(&c.symlink, Color::Magenta),
            executable: parse_color(&c.executable, Color::Green),
            selected: parse_color(&c.selected, Color::Yellow),
            cursor: parse_color(&c.cursor, Color::DarkGray),
            active_border: parse_color(&c.active_border, Color::Cyan),
            inactive_border: parse_color(&c.inactive_border, Color::DarkGray),
        }
    }
}

/// Top-level draw function: lays out panes + status line.
pub fn draw(f: &mut Frame, app: &App) {
    let theme = Theme::from_config(&app.config.colors);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // panes area
            Constraint::Length(1), // status line
        ])
        .split(f.area());

    // Draw dual panes
    pane_view::draw_panes(f, app, &theme, chunks[0]);

    // Draw status line
    status_line::draw_status_line(f, app, chunks[1]);

    // Draw overlays (help, dialogs, space menu, finder)
    if app.show_help {
        help::draw_help(f);
    }

    if let Some(ref dialog) = app.dialog {
        dialog::draw_dialog(f, dialog);
    }

    if app.mode == crate::mode::Mode::Space {
        space_menu_overlay(f);
    }

    if app.mode == crate::mode::Mode::Finder {
        finder_overlay(f, app);
    }
}

fn space_menu_overlay(f: &mut Frame) {
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph};

    let items = [
        ("c", "Copy to other pane"),
        ("m", "Move to other pane"),
        ("d", "Delete"),
        ("r", "Rename"),
        ("n", "New directory"),
        ("e", "Edit with $EDITOR"),
        ("f", "Fuzzy find file"),
        ("s", "Sort menu"),
        ("i", "File info"),
        ("?", "Help"),
    ];

    let width = 32u16;
    let height = items.len() as u16 + 2;
    let area = f.area();
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect::new(x, y, width.min(area.width), height.min(area.height));

    f.render_widget(Clear, rect);

    let lines: Vec<Line> = items
        .iter()
        .map(|(key, desc)| {
            Line::from(vec![
                Span::styled(
                    format!(" {key} "),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" {desc}")),
            ])
        })
        .collect();

    let block = Block::default()
        .title(" Space Menu ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, rect);
}

fn finder_overlay(f: &mut Frame, app: &App) {
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph};

    let area = f.area();
    let width = (area.width * 3 / 4).max(40).min(area.width);
    let height = 22u16.min(area.height - 2);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect::new(x, y, width, height);

    f.render_widget(Clear, rect);

    let mut lines: Vec<Line> = Vec::new();

    // Input line
    lines.push(Line::from(vec![
        Span::styled(
            " > ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            &app.input_buffer,
            Style::default().fg(Color::White),
        ),
        Span::styled("_", Style::default().fg(Color::DarkGray)),
    ]));

    lines.push(Line::raw(""));

    // Results
    for (i, result) in app.finder_results.iter().enumerate().take(height as usize - 4) {
        let is_cursor = i == app.finder_cursor;
        let style = if is_cursor {
            Style::default()
                .fg(Color::White)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let indicator = if is_cursor { ">" } else { " " };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{indicator} "),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(&result.display_name, style),
        ]));
    }

    if app.finder_results.is_empty() && !app.input_buffer.is_empty() {
        lines.push(Line::styled(
            "  No matches",
            Style::default().fg(Color::DarkGray),
        ));
    }

    let block = Block::default()
        .title(" Find File (Esc to cancel) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, rect);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_from_default_config_matches_builtin_colors() {
        // The default ColorConfig names must parse to the historical built-in
        // colors so the unconfigured appearance is unchanged.
        let theme = Theme::from_config(&ColorConfig::default());
        assert_eq!(theme.directory, Color::Blue);
        assert_eq!(theme.symlink, Color::Magenta);
        assert_eq!(theme.executable, Color::Green);
        assert_eq!(theme.selected, Color::Yellow);
        assert_eq!(theme.cursor, Color::DarkGray);
        assert_eq!(theme.active_border, Color::Cyan);
        assert_eq!(theme.inactive_border, Color::DarkGray);
    }

    #[test]
    fn test_parse_color_falls_back_on_invalid() {
        assert_eq!(parse_color("not-a-real-color", Color::Red), Color::Red);
        assert_eq!(parse_color("green", Color::Red), Color::Green);
    }
}
