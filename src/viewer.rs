use std::io;
use std::path::Path;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::preview::{self, FilterMatch, LineFilter, LineIndex, PreviewState, SearchKind};

const FILTER_MAX: usize = 200;

/// Viewer input mode.
enum Mode {
    Normal,
    /// Typing a search query (`/`).
    Search {
        buf: String,
        kind: SearchKind,
    },
    /// Fuzzy line-filter (`&`): a query plus the background worker and its
    /// current best matches.
    Filter {
        buf: String,
        filter: Option<LineFilter>,
        results: Vec<FilterMatch>,
        cursor: usize,
    },
}

/// Standalone full-screen pager backed by the windowed preview reader, so it
/// opens arbitrarily large files with bounded memory.
///
/// Keys: `j/k`, `PgUp/PgDn`, `Space`; `g`/`G` top/bottom (or `NG` to jump to a
/// typed line number); `/` search (literal; `Ctrl-R` toggles regex), `n`/`N`
/// next/prev match; `q` quit.
pub fn view_file(path: &Path) -> crate::error::Result<()> {
    let mut state = preview::load_preview(path);
    // For large (windowed) files, scan total lines + a sparse offset index in
    // the background (accurate "line X / N" and fast goto). Aborts on drop.
    let index = state
        .is_large()
        .then(|| LineIndex::spawn(path.to_path_buf()));

    let mut mode = Mode::Normal;
    let mut goto_count: Option<usize> = None;
    let mut last_kind = SearchKind::Literal;
    let mut status_msg: Option<String> = None;

    enable_raw_mode().map_err(crate::error::NcError::Io)?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(crate::error::NcError::Io)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(crate::error::NcError::Io)?;

    loop {
        if let Some(i) = &index {
            state.set_line_count(i.count(), i.is_done());
        }
        // Pull the latest fuzzy-filter results from its background worker.
        if let Mode::Filter {
            filter: Some(f),
            results,
            cursor,
            ..
        } = &mut mode
        {
            *results = f.results();
            if *cursor >= results.len() {
                *cursor = results.len().saturating_sub(1);
            }
        }

        terminal
            .draw(|f| draw_viewer(f, &state, path, &mode, goto_count, status_msg.as_deref()))
            .map_err(crate::error::NcError::Io)?;

        // Poll so the view refreshes while the background scan progresses.
        if !event::poll(Duration::from_millis(250)).map_err(crate::error::NcError::Io)? {
            continue;
        }
        let Event::Key(key) = event::read().map_err(crate::error::NcError::Io)? else {
            continue;
        };

        let page = terminal
            .size()
            .map(|s| s.height as usize)
            .unwrap_or(24)
            .saturating_sub(3);

        match &mut mode {
            Mode::Normal => {
                status_msg = None;
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char(d @ '0'..='9') => {
                        let n = goto_count.unwrap_or(0);
                        goto_count = Some(n.saturating_mul(10) + (d as usize - '0' as usize));
                    }
                    KeyCode::Char('g') => match goto_count.take() {
                        Some(n) => goto(&mut state, index.as_ref(), n),
                        None => state.scroll_to_top(),
                    },
                    KeyCode::Char('G') => match goto_count.take() {
                        Some(n) => goto(&mut state, index.as_ref(), n),
                        None => state.scroll_to_bottom(page),
                    },
                    KeyCode::Char('j') | KeyCode::Down => state.scroll_down(1, page),
                    KeyCode::Char('k') | KeyCode::Up => state.scroll_up(1),
                    KeyCode::PageDown | KeyCode::Char(' ') => state.scroll_down(page, page),
                    KeyCode::PageUp => state.scroll_up(page),
                    KeyCode::Char('n') => {
                        state.search_next(true);
                    }
                    KeyCode::Char('N') => {
                        state.search_next(false);
                    }
                    KeyCode::Char('/') => {
                        mode = Mode::Search {
                            buf: String::new(),
                            kind: last_kind,
                        };
                    }
                    KeyCode::Char('&') => {
                        mode = Mode::Filter {
                            buf: String::new(),
                            filter: None,
                            results: Vec::new(),
                            cursor: 0,
                        };
                    }
                    _ => goto_count = None,
                }
            }
            Mode::Search { buf, kind } => match key.code {
                KeyCode::Esc => mode = Mode::Normal,
                KeyCode::Enter => {
                    last_kind = *kind;
                    match state.set_search(buf, *kind) {
                        Ok(()) => {
                            state.search_next(true);
                        }
                        Err(e) => status_msg = Some(e),
                    }
                    mode = Mode::Normal;
                }
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    *kind = kind.toggled();
                }
                KeyCode::Char(c) => buf.push(c),
                _ => {}
            },
            Mode::Filter {
                buf,
                filter,
                results,
                cursor,
            } => match key.code {
                KeyCode::Esc => mode = Mode::Normal,
                KeyCode::Enter => {
                    let line = results.get(*cursor).map(|m| m.line as usize);
                    if let Some(line) = line {
                        goto(&mut state, index.as_ref(), line);
                    }
                    mode = Mode::Normal;
                }
                KeyCode::Up => *cursor = cursor.saturating_sub(1),
                KeyCode::Down => {
                    if *cursor + 1 < results.len() {
                        *cursor += 1;
                    }
                }
                KeyCode::Backspace => {
                    buf.pop();
                    respawn_filter(buf, filter, results, cursor, path);
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    respawn_filter(buf, filter, results, cursor, path);
                }
                _ => {}
            },
        }
    }

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    Ok(())
}

/// Jump to 1-based `line`, using the sparse index checkpoint when available.
fn goto(state: &mut PreviewState, index: Option<&LineIndex>, line: usize) {
    let checkpoint = index.map(|i| i.checkpoint_for(line)).unwrap_or((0, 1));
    state.goto_line(line, checkpoint);
}

/// (Re)start the fuzzy-filter worker for the current query, or clear it when
/// the query is empty. Results stream in on subsequent loop ticks.
fn respawn_filter(
    buf: &str,
    filter: &mut Option<LineFilter>,
    results: &mut Vec<FilterMatch>,
    cursor: &mut usize,
    path: &Path,
) {
    *cursor = 0;
    results.clear();
    *filter = if buf.is_empty() {
        None
    } else {
        Some(LineFilter::spawn(
            path.to_path_buf(),
            buf.to_string(),
            FILTER_MAX,
        ))
    };
}

fn draw_viewer(
    f: &mut ratatui::Frame,
    state: &PreviewState,
    path: &Path,
    mode: &Mode,
    goto_count: Option<usize>,
    status_msg: Option<&str>,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(f.area());

    draw_content(f, state, path, chunks[0]);
    draw_footer(f, state, mode, goto_count, status_msg, chunks[1]);

    if let Mode::Filter {
        buf,
        results,
        cursor,
        ..
    } = mode
    {
        draw_filter_overlay(f, buf, results, *cursor);
    }
}

fn draw_filter_overlay(
    f: &mut ratatui::Frame,
    query: &str,
    results: &[FilterMatch],
    cursor: usize,
) {
    let area = f.area();
    let width = (area.width * 3 / 4).clamp(40, area.width);
    let height = 20u16.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect::new(x, y, width, height);
    f.render_widget(Clear, rect);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            " > ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(query.to_string()),
        Span::styled("_", Style::default().fg(Color::DarkGray)),
    ]));
    lines.push(Line::raw(""));

    let rows = height.saturating_sub(4) as usize;
    for (i, m) in results.iter().take(rows).enumerate() {
        let style = if i == cursor {
            Style::default()
                .fg(Color::White)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let marker = if i == cursor { ">" } else { " " };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{marker} {:>7} ", m.line),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(m.text.clone(), style),
        ]));
    }
    if results.is_empty() && !query.is_empty() {
        lines.push(Line::styled(
            "  (no matches yet…)",
            Style::default().fg(Color::DarkGray),
        ));
    }

    let block = Block::default()
        .title(" Fuzzy line-filter (Enter jump, Esc cancel) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn draw_content(f: &mut ratatui::Frame, state: &PreviewState, path: &Path, area: Rect) {
    let mut title = format!(" {} — {} ", path.display(), state.status_text());
    if let Some(s) = state.active_search() {
        title.push_str(&format!("[/{} {}] ", s.query(), s.kind().label()));
    }

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    let para = Paragraph::new(state.render(inner.height as usize)).block(block);
    f.render_widget(para, area);
}

fn draw_footer(
    f: &mut ratatui::Frame,
    state: &PreviewState,
    mode: &Mode,
    goto_count: Option<usize>,
    status_msg: Option<&str>,
    area: Rect,
) {
    let (text, style) = match mode {
        Mode::Search { buf, kind } => (
            format!(
                "/{buf}  [{}]  (Ctrl-R toggles, Enter search, Esc cancel)",
                kind.label()
            ),
            Style::default().fg(Color::Yellow),
        ),
        Mode::Filter { buf, results, .. } => (
            format!(
                "&{buf}  ({} matches)  (↑/↓ select, Enter jump, Esc cancel)",
                results.len()
            ),
            Style::default().fg(Color::Yellow),
        ),
        Mode::Normal => {
            if let Some(msg) = status_msg {
                (msg.to_string(), Style::default().fg(Color::Red))
            } else if let Some(n) = goto_count {
                (
                    format!(":{n}  (g/G to jump)"),
                    Style::default().fg(Color::Yellow),
                )
            } else {
                let hint = if state.active_search().is_some() {
                    "/ search  n/N next  NG goto  q quit"
                } else {
                    "/ search  NG goto line  g/G top/bottom  q quit"
                };
                (hint.to_string(), Style::default().fg(Color::DarkGray))
            }
        }
    };
    let para = Paragraph::new(text).style(style.add_modifier(Modifier::DIM));
    f.render_widget(para, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_view_file_nonexistent() {
        // load_preview handles missing files with a message line.
        let state = preview::load_preview(Path::new("/nonexistent/file"));
        assert!(!state.render(1).is_empty());
    }
}
