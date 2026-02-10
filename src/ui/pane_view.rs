use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table, TableState};

use crate::app::App;
use crate::pane::{PaneId, PaneState};
use crate::platform;
use crate::preview;

/// Draw both panes side-by-side.
pub fn draw_panes(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    if app.preview_active {
        // Active pane shows file list, inactive pane shows preview
        let (file_chunk, preview_chunk) = match app.active_pane {
            PaneId::Left => (chunks[0], chunks[1]),
            PaneId::Right => (chunks[1], chunks[0]),
        };
        draw_single_pane(f, app.active_pane_state(), !app.preview_focused, file_chunk);
        draw_preview_pane(f, &app.preview_state, app.preview_focused, preview_chunk);
    } else {
        draw_single_pane(f, &app.left_pane, app.active_pane == PaneId::Left, chunks[0]);
        draw_single_pane(f, &app.right_pane, app.active_pane == PaneId::Right, chunks[1]);
    }
}

fn draw_preview_pane(f: &mut Frame, state: &preview::PreviewState, is_focused: bool, area: Rect) {
    let title = match &state.path {
        Some(p) => format!(
            " [PREVIEW] {} ",
            p.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        ),
        None => " [PREVIEW] ".to_string(),
    };

    let border_color = if is_focused { Color::Cyan } else { Color::Magenta };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    let visible_height = inner.height as usize;

    let lines: Vec<Line> = state
        .lines
        .iter()
        .skip(state.scroll)
        .take(visible_height)
        .enumerate()
        .map(|(i, pline)| {
            let line_num = state.scroll + i + 1;
            let num_span = Span::styled(
                format!("{line_num:4} "),
                Style::default().fg(Color::DarkGray),
            );
            let mut spans = vec![num_span];
            for (text, style) in &pline.spans {
                spans.push(Span::styled(text.as_str(), *style));
            }
            Line::from(spans)
        })
        .collect();

    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, area);
}

fn draw_single_pane(f: &mut Frame, pane: &PaneState, is_active: bool, area: Rect) {
    let border_style = if is_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = format!(" {} ", platform::display_path(&pane.cwd));
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);

    // Build header
    let header = Row::new(vec!["Name", "Size", "Modified", "Perms"]).style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    // Build rows from entries
    let rows: Vec<Row> = pane
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let is_selected = pane.selected.get(i).copied().unwrap_or(false);
            let is_cursor = i == pane.cursor;

            let name_display = if entry.is_dir {
                format!("{}/", entry.name)
            } else {
                entry.name.clone()
            };

            let size_display = if entry.is_dir {
                "<DIR>".to_string()
            } else {
                platform::format_file_size(entry.size)
            };

            let time_display = entry
                .modified
                .map(platform::format_file_time)
                .unwrap_or_else(|| "---".to_string());

            let style = match (is_cursor && is_active, is_selected) {
                (true, true) => Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
                (true, false) => Style::default().bg(Color::DarkGray).fg(Color::White),
                (false, true) => Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
                (false, false) => {
                    if entry.is_dir {
                        Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)
                    } else if entry.is_symlink {
                        Style::default().fg(Color::Magenta)
                    } else if entry.permissions.contains('x') && !entry.is_dir {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::White)
                    }
                }
            };

            Row::new(vec![
                name_display,
                size_display,
                time_display,
                entry.permissions.clone(),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Min(20),
        Constraint::Length(10),
        Constraint::Length(16),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(Style::default());

    let mut state = TableState::default();
    if is_active && !pane.entries.is_empty() {
        state.select(Some(pane.cursor));
    }

    f.render_stateful_widget(table, area, &mut state);
}
