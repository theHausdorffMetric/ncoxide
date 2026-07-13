use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

use super::PreviewLine;

/// Highlight an in-memory string into styled preview lines using syntect.
///
/// Only used for files small enough to load fully (see `HIGHLIGHT_MAX`);
/// large files are rendered as plain text by the windowed reader instead.
pub fn highlight(content: &str, path: &Path) -> Vec<PreviewLine> {
    let ss = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let theme = &ts.themes["base16-ocean.dark"];

    let syntax = path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(|ext| ss.find_syntax_by_extension(ext))
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    let mut highlighter = syntect::easy::HighlightLines::new(syntax, theme);

    content
        .lines()
        .map(|line_str| {
            let ranges = highlighter
                .highlight_line(line_str, &ss)
                .unwrap_or_default();
            let spans = ranges
                .iter()
                .map(|(style, text)| {
                    let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
                    let mut ratatui_style = Style::default().fg(fg);
                    if style
                        .font_style
                        .contains(syntect::highlighting::FontStyle::BOLD)
                    {
                        ratatui_style = ratatui_style.add_modifier(Modifier::BOLD);
                    }
                    if style
                        .font_style
                        .contains(syntect::highlighting::FontStyle::ITALIC)
                    {
                        ratatui_style = ratatui_style.add_modifier(Modifier::ITALIC);
                    }
                    (text.to_string(), ratatui_style)
                })
                .collect();
            PreviewLine { spans }
        })
        .collect()
}
