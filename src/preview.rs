use std::fs;
use std::io::Read;
use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

const MAX_PREVIEW_SIZE: u64 = 1024 * 1024; // 1MB
const MAX_PREVIEW_LINES: usize = 500;
const BINARY_CHECK_SIZE: usize = 8192;

/// State for preview mode.
#[derive(Debug)]
#[derive(Default)]
pub struct PreviewState {
    /// Lines to display (cached).
    pub lines: Vec<PreviewLine>,
    /// Current scroll offset.
    pub scroll: usize,
    /// Path being previewed.
    pub path: Option<std::path::PathBuf>,
    /// Whether the file is detected as binary.
    pub is_binary: bool,
    /// Number of total lines.
    pub total_lines: usize,
}

/// A single styled line from the preview.
#[derive(Debug, Clone)]
pub struct PreviewLine {
    pub spans: Vec<(String, Style)>,
}


impl PreviewState {
    pub fn clear(&mut self) {
        self.lines.clear();
        self.scroll = 0;
        self.path = None;
        self.is_binary = false;
        self.total_lines = 0;
    }

    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll = self.scroll.saturating_sub(amount);
    }

    pub fn scroll_down(&mut self, amount: usize) {
        if self.total_lines > 0 {
            self.scroll = (self.scroll + amount).min(self.total_lines.saturating_sub(1));
        }
    }
}

/// Detect if a file is likely binary by checking for null bytes
/// and the ratio of printable characters.
pub fn is_binary(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; BINARY_CHECK_SIZE];
    let Ok(n) = file.read(&mut buf) else {
        return false;
    };
    if n == 0 {
        return false;
    }

    let bytes = &buf[..n];
    // Any null byte → binary
    if bytes.contains(&0) {
        return true;
    }
    // Low ratio of printable characters → binary
    let printable = bytes
        .iter()
        .filter(|&&b| b >= 0x20 || b == b'\n' || b == b'\r' || b == b'\t')
        .count();
    (printable as f64 / n as f64) < 0.85
}

/// Load a file for preview with syntax highlighting.
pub fn load_preview(path: &Path) -> PreviewState {
    let mut state = PreviewState {
        path: Some(path.to_path_buf()),
        ..Default::default()
    };

    // Check file size
    if let Ok(meta) = fs::metadata(path)
        && meta.len() > MAX_PREVIEW_SIZE {
            state.lines.push(PreviewLine {
                spans: vec![(
                    format!("File too large to preview ({} bytes)", meta.len()),
                    Style::default().fg(Color::Red),
                )],
            });
            state.total_lines = 1;
            return state;
        }

    // Binary detection
    if is_binary(path) {
        state.is_binary = true;
        state.lines.push(PreviewLine {
            spans: vec![(
                "[Binary file]".to_string(),
                Style::default().fg(Color::Red),
            )],
        });
        state.total_lines = 1;
        return state;
    }

    // Read file
    let Ok(content) = fs::read_to_string(path) else {
        state.lines.push(PreviewLine {
            spans: vec![(
                "Cannot read file".to_string(),
                Style::default().fg(Color::Red),
            )],
        });
        state.total_lines = 1;
        return state;
    };

    // Try syntax highlighting
    let ss = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let theme = &ts.themes["base16-ocean.dark"];

    let syntax = path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(|ext| ss.find_syntax_by_extension(ext))
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    let mut highlighter = syntect::easy::HighlightLines::new(syntax, theme);

    for (i, line_str) in content.lines().enumerate() {
        if i >= MAX_PREVIEW_LINES {
            state.lines.push(PreviewLine {
                spans: vec![(
                    format!("... ({} more lines)", content.lines().count() - MAX_PREVIEW_LINES),
                    Style::default().fg(Color::DarkGray),
                )],
            });
            break;
        }

        let ranges = highlighter
            .highlight_line(line_str, &ss)
            .unwrap_or_default();

        let spans: Vec<(String, Style)> = ranges
            .iter()
            .map(|(style, text)| {
                let fg = Color::Rgb(
                    style.foreground.r,
                    style.foreground.g,
                    style.foreground.b,
                );
                let mut ratatui_style = Style::default().fg(fg);
                if style.font_style.contains(syntect::highlighting::FontStyle::BOLD) {
                    ratatui_style = ratatui_style.add_modifier(Modifier::BOLD);
                }
                if style.font_style.contains(syntect::highlighting::FontStyle::ITALIC) {
                    ratatui_style = ratatui_style.add_modifier(Modifier::ITALIC);
                }
                (text.to_string(), ratatui_style)
            })
            .collect();

        state.lines.push(PreviewLine { spans });
    }

    state.total_lines = state.lines.len();
    state
}

/// Convert a PreviewLine to a ratatui Line for rendering.
pub fn preview_line_to_ratatui(pline: &PreviewLine) -> Line<'_> {
    let spans: Vec<Span> = pline
        .spans
        .iter()
        .map(|(text, style)| Span::styled(text.as_str(), *style))
        .collect();
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_binary_detection() {
        let tmp = std::env::temp_dir().join(format!("ncoxide_binary_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        // Text file
        fs::write(tmp.join("text.txt"), "Hello, world!\nThis is text.\n").unwrap();
        assert!(!is_binary(&tmp.join("text.txt")));

        // Binary file (contains null bytes)
        let mut binary = Vec::new();
        binary.extend_from_slice(b"ELF");
        binary.push(0);
        binary.extend_from_slice(&[0xFF; 100]);
        fs::write(tmp.join("binary.bin"), &binary).unwrap();
        assert!(is_binary(&tmp.join("binary.bin")));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_preview_text() {
        let tmp = std::env::temp_dir().join(format!("ncoxide_preview_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        fs::write(tmp.join("hello.rs"), "fn main() {\n    println!(\"Hello\");\n}\n").unwrap();
        let state = load_preview(&tmp.join("hello.rs"));
        assert!(!state.is_binary);
        assert_eq!(state.total_lines, 3);
        assert!(!state.lines.is_empty());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_preview_binary() {
        let tmp = std::env::temp_dir().join(format!("ncoxide_prevbin_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let mut data = vec![0u8; 1000];
        data[0] = 0x7F; // ELF header byte
        fs::write(tmp.join("binary"), &data).unwrap();
        let state = load_preview(&tmp.join("binary"));
        assert!(state.is_binary);

        let _ = fs::remove_dir_all(&tmp);
    }
}
