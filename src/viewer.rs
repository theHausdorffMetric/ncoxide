use std::io;
use std::path::Path;

use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::preview::{self, PreviewState};

/// Standalone full-screen file viewer. Read-only, scroll with j/k/PgUp/PgDn, q to quit.
pub fn view_file(path: &Path) -> crate::error::Result<()> {
    let mut state = preview::load_preview(path);

    enable_raw_mode().map_err(crate::error::NcError::Io)?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(crate::error::NcError::Io)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(crate::error::NcError::Io)?;

    loop {
        terminal
            .draw(|f| draw_viewer(f, &state, path))
            .map_err(crate::error::NcError::Io)?;

        if let Event::Key(key) = event::read().map_err(crate::error::NcError::Io)? {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('j') | KeyCode::Down => state.scroll_down(1),
                KeyCode::Char('k') | KeyCode::Up => state.scroll_up(1),
                KeyCode::PageDown | KeyCode::Char(' ') => {
                    let page = terminal
                        .size()
                        .map(|s| s.height as usize)
                        .unwrap_or(20)
                        .saturating_sub(4);
                    state.scroll_down(page);
                }
                KeyCode::PageUp => {
                    let page = terminal
                        .size()
                        .map(|s| s.height as usize)
                        .unwrap_or(20)
                        .saturating_sub(4);
                    state.scroll_up(page);
                }
                KeyCode::Char('g') => state.scroll = 0,
                KeyCode::Char('G') => {
                    state.scroll = state.total_lines.saturating_sub(1);
                }
                _ => {}
            }
        }
    }

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    Ok(())
}

fn draw_viewer(f: &mut ratatui::Frame, state: &PreviewState, path: &Path) {
    let area = f.area();
    let title = format!(
        " {} — line {}/{} (q to quit) ",
        path.display(),
        state.scroll + 1,
        state.total_lines
    );

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_view_file_nonexistent() {
        // Just verify load_preview handles missing files
        let state = preview::load_preview(Path::new("/nonexistent/file"));
        assert!(!state.lines.is_empty()); // Should have an error message line
    }
}
