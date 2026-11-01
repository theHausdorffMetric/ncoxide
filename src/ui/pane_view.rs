use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};
use ratatui_image::Image;

use crate::app::App;
use crate::pane::{PaneId, PaneState};
use crate::platform;
use crate::preview;
use crate::ui::Theme;
use crate::ui::layout::Regions;

/// Draw both panes side-by-side. `show_image` is false while an overlay is
/// up (see `ui::draw`).
pub fn draw_panes(f: &mut Frame, app: &App, theme: &Theme, regions: &Regions, show_image: bool) {
    if app.preview_active {
        // Active pane shows file list, inactive pane shows preview
        let (file_chunk, preview_chunk) = match app.active_pane {
            PaneId::Left => (regions.left, regions.right),
            PaneId::Right => (regions.right, regions.left),
        };
        // Compare mode: the locked picture takes the file list's half — the
        // largest two rectangles the screen has — and the live half keeps
        // following the cursor; titles and status line say where it is.
        if let Some(locked) = &app.locked_preview {
            draw_preview_pane(f, locked, "[LOCKED]", Color::Yellow, file_chunk, show_image);
        } else {
            draw_single_pane(
                f,
                app.active_pane_state(),
                !app.preview_focused,
                theme,
                file_chunk,
            );
        }
        let border = if app.preview_focused {
            theme.active_border
        } else {
            Color::Magenta
        };
        draw_preview_pane(
            f,
            &app.preview_state,
            "[PREVIEW]",
            border,
            preview_chunk,
            show_image,
        );
    } else {
        draw_single_pane(
            f,
            &app.left_pane,
            app.active_pane == PaneId::Left,
            theme,
            regions.left,
        );
        draw_single_pane(
            f,
            &app.right_pane,
            app.active_pane == PaneId::Right,
            theme,
            regions.right,
        );
    }
}

/// One preview half: `label` is the title tag (`[PREVIEW]` / `[LOCKED]`).
fn draw_preview_pane(
    f: &mut Frame,
    state: &preview::PreviewState,
    label: &str,
    border_color: Color,
    area: Rect,
    show_image: bool,
) {
    let name = state
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let status = state.status_text();
    let title = if status.is_empty() {
        format!(" {label} {name} ")
    } else {
        format!(" {label} {name} — {status} ")
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);

    // A picture encoded for exactly this inner size is drawn as pixels;
    // anything else (pending, stale size, overlay up) shows the text lines.
    if show_image
        && let Some(enc) = state.encoded_image()
        && enc.target == inner.as_size()
    {
        f.render_widget(block, area);
        // Aspect-fit leaves slack on one axis: centre the picture in it.
        let size = enc.protocol.size();
        let rect = Rect::new(
            inner.x + inner.width.saturating_sub(size.width) / 2,
            inner.y + inner.height.saturating_sub(size.height) / 2,
            size.width.min(inner.width),
            size.height.min(inner.height),
        );
        f.render_widget(Image::new(&enc.protocol), rect);
        return;
    }

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

    // Header and cells come from one column list, so they cannot disagree
    // when a narrow pane drops the trailing columns.
    let columns = visible_columns(area.width);
    let header = Row::new(columns.iter().map(|c| c.title()).collect::<Vec<_>>()).style(
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

            Row::new(
                columns
                    .iter()
                    .map(|c| c.cell(entry))
                    .collect::<Vec<String>>(),
            )
            .style(style)
        })
        .collect();

    let widths: Vec<Constraint> = columns.iter().map(|c| c.width()).collect();

    // Cursor highlighting is done via row styles above; scrolling via the
    // slice — no TableState needed.
    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}

/// One column of the file list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Name,
    Size,
    Modified,
    Perms,
}

/// The four columns ask for `20 + 10 + 16 + 10` cells plus borders and
/// spacing. A narrow pane — common once the divider moves — drops them from
/// the right so the name keeps its room: no `Perms` below 58 cells, no
/// `Modified` below 48, no `Size` below 32. `width` is the pane's outer
/// width (borders included).
pub fn visible_columns(width: u16) -> &'static [Column] {
    use Column::*;
    match width {
        0..32 => &[Name],
        32..48 => &[Name, Size],
        48..58 => &[Name, Size, Modified],
        _ => &[Name, Size, Modified, Perms],
    }
}

impl Column {
    fn title(self) -> &'static str {
        match self {
            Column::Name => "Name",
            Column::Size => "Size",
            Column::Modified => "Modified",
            Column::Perms => "Perms",
        }
    }

    fn width(self) -> Constraint {
        match self {
            Column::Name => Constraint::Min(20),
            Column::Size => Constraint::Length(10),
            Column::Modified => Constraint::Length(16),
            Column::Perms => Constraint::Length(10),
        }
    }

    fn cell(self, entry: &crate::pane::FileEntry) -> String {
        match self {
            Column::Name if entry.is_dir => format!("{}/", entry.name),
            Column::Name => entry.name.clone(),
            Column::Size if entry.is_dir => "<DIR>".to_string(),
            Column::Size => platform::format_file_size(entry.size),
            Column::Modified => entry
                .modified
                .map(platform::format_file_time)
                .unwrap_or_else(|| "---".to_string()),
            Column::Perms => entry.permissions.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_columns_drop_from_the_right_as_the_pane_narrows() {
        use Column::*;
        assert_eq!(visible_columns(80), &[Name, Size, Modified, Perms]);
        assert_eq!(visible_columns(58), &[Name, Size, Modified, Perms]);
        assert_eq!(visible_columns(57), &[Name, Size, Modified]);
        assert_eq!(visible_columns(48), &[Name, Size, Modified]);
        assert_eq!(visible_columns(47), &[Name, Size]);
        assert_eq!(visible_columns(36), &[Name, Size]);
        assert_eq!(visible_columns(32), &[Name, Size]);
        assert_eq!(visible_columns(31), &[Name]);
        assert_eq!(visible_columns(0), &[Name]);
        // Name is always first and always present.
        for w in 0..200 {
            assert_eq!(visible_columns(w)[0], Name, "{w}");
        }
    }
}
