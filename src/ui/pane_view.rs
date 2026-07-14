use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};

use crate::app::App;
use crate::pane::{PaneId, PaneState};
use crate::platform;
use crate::preview;
use crate::ui::Theme;

/// Draw both panes side-by-side.
pub fn draw_panes(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
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
        draw_single_pane(
            f,
            app.active_pane_state(),
            !app.preview_focused,
            theme,
            file_chunk,
        );
        draw_preview_pane(
            f,
            &app.preview_state,
            app.preview_focused,
            theme,
            preview_chunk,
        );
    } else {
        draw_single_pane(
            f,
            &app.left_pane,
            app.active_pane == PaneId::Left,
            theme,
            chunks[0],
        );
        draw_single_pane(
            f,
            &app.right_pane,
            app.active_pane == PaneId::Right,
            theme,
            chunks[1],
        );
    }
}

fn draw_preview_pane(
    f: &mut Frame,
    state: &preview::PreviewState,
    is_focused: bool,
    theme: &Theme,
    area: Rect,
) {
    let name = state
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let status = state.status_text();
    let title = if status.is_empty() {
        format!(" [PREVIEW] {name} ")
    } else {
        format!(" [PREVIEW] {name} — {status} ")
    };

    let border_color = if is_focused {
        theme.active_border
    } else {
        Color::Magenta
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    let visible_height = inner.height as usize;

    let para = Paragraph::new(state.render(visible_height)).block(block);
    f.render_widget(para, area);
}

fn draw_single_pane(f: &mut Frame, pane: &PaneState, is_active: bool, theme: &Theme, area: Rect) {
    let border_style = if is_active {
        Style::default().fg(theme.active_border)
    } else {
        Style::default().fg(theme.inactive_border)
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

    // Build rows for the visible slice only (R8): a directory with tens of
    // thousands of entries must not cost a Row (with jiff formatting and
    // several Strings) per entry per frame. The event loop keeps
    // `scroll_offset` tracking the cursor via `adjust_scroll`; the clamp
    // below keeps standalone renders (tests) correct too.
    let visible = area.height.saturating_sub(3) as usize; // borders + header
    let mut offset = pane.scroll_offset.min(pane.entries.len().saturating_sub(1));
    if pane.cursor < offset {
        offset = pane.cursor;
    } else if visible > 0 && pane.cursor >= offset + visible {
        offset = pane.cursor + 1 - visible;
    }

    let rows: Vec<Row> = pane
        .entries
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
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
                    .bg(theme.cursor)
                    .fg(theme.selected)
                    .add_modifier(Modifier::BOLD),
                (true, false) => Style::default().bg(theme.cursor).fg(Color::White),
                (false, true) => Style::default()
                    .fg(theme.selected)
                    .add_modifier(Modifier::BOLD),
                (false, false) => {
                    if entry.is_dir {
                        Style::default()
                            .fg(theme.directory)
                            .add_modifier(Modifier::BOLD)
                    } else if entry.is_symlink {
                        Style::default().fg(theme.symlink)
                    } else if entry.permissions.contains('x') && !entry.is_dir {
                        Style::default().fg(theme.executable)
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

    // Cursor highlighting is done via row styles above; scrolling via the
    // slice — no TableState needed.
    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}
