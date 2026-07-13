use std::path::Path;
use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

use super::PreviewLine;

/// Highlighting cost caps. Past either one, the rest of the file renders
/// unstyled: stopping for good (rather than per line) keeps syntect's parser
/// state from desyncing on the lines that follow, and a single overlong line
/// almost always means minified/generated content anyway.
const MAX_HIGHLIGHT_LINES: usize = 2_000;
const MAX_HIGHLIGHT_LINE_BYTES: usize = 4_096;

/// Shared syntect assets. Loading these is expensive (~100 ms); doing it per
/// preview made every cursor move onto a new file stutter (R6).
fn syntax_set() -> &'static SyntaxSet {
    static SS: OnceLock<SyntaxSet> = OnceLock::new();
    SS.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme_set() -> &'static ThemeSet {
    static TS: OnceLock<ThemeSet> = OnceLock::new();
    TS.get_or_init(ThemeSet::load_defaults)
}

/// Highlight an in-memory string into styled preview lines using syntect.
///
/// Only used for files small enough to load fully (see `HIGHLIGHT_MAX`);
/// large files are rendered as plain text by the windowed reader instead.
/// Content past the highlighting caps is returned as plain lines, so the
/// preview stays complete either way.
pub fn highlight(content: &str, path: &Path) -> Vec<PreviewLine> {
    let ss = syntax_set();
    let theme = &theme_set().themes["base16-ocean.dark"];

    let syntax = path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(|ext| ss.find_syntax_by_extension(ext))
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    let mut highlighter = syntect::easy::HighlightLines::new(syntax, theme);
    let mut plain_rest = false;

    content
        .lines()
        .enumerate()
        .map(|(i, line_str)| {
            if !plain_rest
                && (i >= MAX_HIGHLIGHT_LINES || line_str.len() > MAX_HIGHLIGHT_LINE_BYTES)
            {
                plain_rest = true;
            }
            if plain_rest {
                return PreviewLine {
                    spans: vec![(line_str.to_string(), Style::default())],
                };
            }

            let ranges = highlighter.highlight_line(line_str, ss).unwrap_or_default();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn is_plain(line: &PreviewLine) -> bool {
        line.spans
            .iter()
            .all(|(_, style)| *style == Style::default())
    }

    #[test]
    fn test_highlight_styles_source_lines() {
        let lines = highlight("fn main() {}\n", Path::new("x.rs"));
        assert_eq!(lines.len(), 1);
        assert!(!is_plain(&lines[0]), "Rust source should get styled spans");
    }

    #[test]
    fn test_highlight_line_cap_switches_to_plain() {
        let content: String = (0..MAX_HIGHLIGHT_LINES + 5)
            .map(|i| format!("let x{i} = {i};\n"))
            .collect();
        let lines = highlight(&content, Path::new("x.rs"));
        assert_eq!(lines.len(), MAX_HIGHLIGHT_LINES + 5, "no content dropped");
        assert!(!is_plain(&lines[0]));
        assert!(is_plain(&lines[MAX_HIGHLIGHT_LINES]), "past the cap: plain");
        assert!(is_plain(lines.last().unwrap()));
    }

    #[test]
    fn test_highlight_overlong_line_switches_to_plain() {
        let long = "x".repeat(MAX_HIGHLIGHT_LINE_BYTES + 1);
        let content = format!("fn a() {{}}\n{long}\nfn b() {{}}\n");
        let lines = highlight(&content, Path::new("x.rs"));
        assert_eq!(lines.len(), 3);
        assert!(!is_plain(&lines[0]), "line before the trigger stays styled");
        assert!(is_plain(&lines[1]), "overlong line renders plain");
        assert!(is_plain(&lines[2]), "highlighting stays off after trigger");
    }
}
