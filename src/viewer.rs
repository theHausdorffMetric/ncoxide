use std::io;
use std::path::Path;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::preview::{self, LineCounter, PreviewState};

/// Standalone full-screen file viewer. Read-only pager: scroll with
/// j/k/PgUp/PgDn, g/G for top/bottom, q to quit. Backed by the windowed
/// preview reader, so it opens arbitrarily large files with bounded memory.
pub fn view_file(path: &Path) -> crate::error::Result<()> {
    let mut state = preview::load_preview(path);
    // For large (windowed) files, count total lines in the background so the
    // status can show an accurate "line X / N". Aborts on drop (function exit).
    let counter = state
        .is_large()
        .then(|| LineCounter::spawn(path.to_path_buf()));

    enable_raw_mode().map_err(crate::error::NcError::Io)?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(crate::error::NcError::Io)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(crate::error::NcError::Io)?;

    loop {
        if let Some(c) = &counter {
            state.set_line_count(c.count(), c.is_done());
        }

        terminal
            .draw(|f| draw_viewer(f, &state, path))
            .map_err(crate::error::NcError::Io)?;

        // Poll so the view can refresh periodically (e.g. background line
        // counting) instead of blocking indefinitely on input.
        if !event::poll(Duration::from_millis(250)).map_err(crate::error::NcError::Io)? {
            continue;
        }

        if let Event::Key(key) = event::read().map_err(crate::error::NcError::Io)? {
            let page = terminal
                .size()
                .map(|s| s.height as usize)
                .unwrap_or(24)
                .saturating_sub(2);
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('j') | KeyCode::Down => state.scroll_down(1, page),
                KeyCode::Char('k') | KeyCode::Up => state.scroll_up(1),
                KeyCode::PageDown | KeyCode::Char(' ') => state.scroll_down(page, page),
                KeyCode::PageUp => state.scroll_up(page),
                KeyCode::Char('g') => state.scroll_to_top(),
                KeyCode::Char('G') => state.scroll_to_bottom(page),
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
    let title = format!(" {} — {} (q to quit) ", path.display(), state.status_text());

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    let visible_height = inner.height as usize;

    let para = Paragraph::new(state.render(visible_height)).block(block);
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
