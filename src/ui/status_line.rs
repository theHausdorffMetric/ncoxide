use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::mode::Mode;
use crate::platform;

pub fn draw_status_line(f: &mut Frame, app: &App, area: Rect) {
    let mode = app.mode;
    let pane = app.active_pane_state();

    let mode_color = match mode {
        Mode::Normal => Color::Green,
        Mode::Select => Color::Yellow,
        Mode::Space => Color::Blue,
        Mode::Goto => Color::Magenta,
        Mode::Command => Color::Red,
        Mode::Input(_) => Color::Cyan,
        Mode::Finder => Color::LightBlue,
    };

    let mode_span = Span::styled(
        format!(" {} ", mode.label()),
        Style::default()
            .fg(Color::Black)
            .bg(mode_color)
            .add_modifier(Modifier::BOLD),
    );

    let path_span = Span::styled(
        format!("  {}  ", platform::display_path(&pane.cwd)),
        Style::default().fg(Color::White),
    );

    let sel_count = pane.selection_count();
    let sel_span = if sel_count > 0 {
        Span::styled(
            format!("│  {sel_count} selected  "),
            Style::default().fg(Color::Yellow),
        )
    } else {
        Span::raw("")
    };

    let preview_span = if app.preview_focused {
        Span::styled(
            "│  PRV  ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw("")
    };

    let count_span = Span::styled(
        format!("│  {} items  ", pane.entries.len()),
        Style::default().fg(Color::DarkGray),
    );

    // Show input buffer in command/input modes
    let input_span = if let Mode::Command = mode {
        Span::styled(
            format!("  :{}", app.input_buffer),
            Style::default().fg(Color::White),
        )
    } else if let Mode::Input(kind) = mode {
        let prefix = match kind {
            crate::mode::InputKind::Rename => "Rename: ",
            crate::mode::InputKind::Mkdir => "Mkdir: ",
            crate::mode::InputKind::Search => "/",
            crate::mode::InputKind::GlobSelect => "Glob: ",
            crate::mode::InputKind::CommandLine => ":",
        };
        Span::styled(
            format!("  {prefix}{}", app.input_buffer),
            Style::default().fg(Color::White),
        )
    } else {
        Span::raw("")
    };

    let line = Line::from(vec![
        mode_span,
        path_span,
        sel_span,
        preview_span,
        count_span,
        input_span,
    ]);
    let para = Paragraph::new(line).style(Style::default().bg(Color::Black));
    f.render_widget(para, area);
}
