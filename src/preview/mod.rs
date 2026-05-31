mod highlight;
mod index;
mod window;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

pub use index::LineCounter;
use window::FileWindow;

/// Progress of the background line-count for a large file.
#[derive(Debug, Clone, Copy)]
enum CountState {
    Unknown,
    Counting(u64),
    Total(u64),
}

/// Files at or below this size are fully loaded and syntax-highlighted; larger
/// files use the windowed plain-text reader so memory stays bounded.
const HIGHLIGHT_MAX: u64 = 2 * 1024 * 1024; // 2 MB
const BINARY_CHECK_SIZE: usize = 8192;

/// A single styled line from the preview (used for highlighted/small files).
#[derive(Debug, Clone)]
pub struct PreviewLine {
    pub spans: Vec<(String, Style)>,
}

/// Backing for a preview, chosen by file size in [`load_preview`].
#[derive(Debug)]
enum PreviewKind {
    Empty,
    /// Small file (or a status message): fully materialized, optionally
    /// highlighted, scrolled by line index.
    Loaded { lines: Vec<PreviewLine>, scroll: usize },
    /// Large file: windowed plain-text reader, never fully resident. The
    /// total line count is filled in by a background scan (see [`LineCounter`]).
    Windowed { window: FileWindow, count: CountState },
}

/// State for preview mode. Backed either by a fully-loaded line buffer (small
/// files) or a windowed reader (large files); both render through
/// [`PreviewState::render`].
#[derive(Debug)]
pub struct PreviewState {
    pub path: Option<PathBuf>,
    pub is_binary: bool,
    kind: PreviewKind,
}

impl Default for PreviewState {
    fn default() -> Self {
        PreviewState {
            path: None,
            is_binary: false,
            kind: PreviewKind::Empty,
        }
    }
}

impl PreviewState {
    /// A single-line status message (e.g. "[Directory]", errors), with `path`
    /// recorded so `update_preview` can dedupe by path.
    pub fn message(path: Option<PathBuf>, text: &str, color: Color) -> Self {
        PreviewState {
            path,
            is_binary: false,
            kind: PreviewKind::Loaded {
                lines: vec![PreviewLine {
                    spans: vec![(text.to_string(), Style::default().fg(color))],
                }],
                scroll: 0,
            },
        }
    }

    pub fn clear(&mut self) {
        self.path = None;
        self.is_binary = false;
        self.kind = PreviewKind::Empty;
    }

    /// Render the visible window as ratatui lines, including a line-number
    /// gutter. Owned (`'static`) so callers don't hold a borrow on `self`.
    pub fn render(&self, height: usize) -> Vec<Line<'static>> {
        let gutter = |n: Option<u64>| match n {
            Some(n) => Span::styled(format!("{n:>6} "), Style::default().fg(Color::DarkGray)),
            None => Span::styled("       ".to_string(), Style::default().fg(Color::DarkGray)),
        };
        match &self.kind {
            PreviewKind::Empty => Vec::new(),
            PreviewKind::Loaded { lines, scroll } => lines
                .iter()
                .enumerate()
                .skip(*scroll)
                .take(height)
                .map(|(i, pline)| {
                    let mut spans = vec![gutter(Some(i as u64 + 1))];
                    for (text, style) in &pline.spans {
                        spans.push(Span::styled(text.clone(), *style));
                    }
                    Line::from(spans)
                })
                .collect(),
            PreviewKind::Windowed { window, .. } => {
                let base = window.top_line_1based();
                window
                    .lines()
                    .iter()
                    .take(height)
                    .enumerate()
                    .map(|(i, text)| {
                        let num = base.map(|b| b + i as u64);
                        Line::from(vec![gutter(num), Span::raw(text.clone())])
                    })
                    .collect()
            }
        }
    }

    /// Whether this preview is a large, windowed file (worth a background
    /// line count).
    pub fn is_large(&self) -> bool {
        matches!(self.kind, PreviewKind::Windowed { .. })
    }

    /// Feed progress from a background [`LineCounter`] into the status display.
    pub fn set_line_count(&mut self, count: u64, complete: bool) {
        if let PreviewKind::Windowed { count: state, .. } = &mut self.kind {
            *state = if complete {
                CountState::Total(count)
            } else {
                CountState::Counting(count)
            };
        }
    }

    /// Status text for the title/footer: line position and (for large files)
    /// byte percentage through the file.
    pub fn status_text(&self) -> String {
        match &self.kind {
            PreviewKind::Empty => String::new(),
            PreviewKind::Loaded { lines, scroll } => {
                format!("line {}/{}", scroll + 1, lines.len().max(1))
            }
            PreviewKind::Windowed { window, count } => {
                let line = match window.top_line_1based() {
                    Some(l) => format!("line {l}"),
                    None => String::new(),
                };
                let total = match count {
                    CountState::Total(t) => format!("/{t}"),
                    CountState::Counting(c) => format!(" (~{c}…)"),
                    CountState::Unknown => String::new(),
                };
                format!("{line}{total} · {}%", window.byte_pct())
            }
        }
    }

    pub fn scroll_up(&mut self, amount: usize) {
        match &mut self.kind {
            PreviewKind::Loaded { scroll, .. } => *scroll = scroll.saturating_sub(amount),
            PreviewKind::Windowed { window, .. } => window.scroll_up(amount),
            PreviewKind::Empty => {}
        }
    }

    pub fn scroll_down(&mut self, amount: usize, height: usize) {
        match &mut self.kind {
            PreviewKind::Loaded { lines, scroll } => {
                *scroll = (*scroll + amount).min(lines.len().saturating_sub(1));
            }
            PreviewKind::Windowed { window, .. } => window.scroll_down(amount, height),
            PreviewKind::Empty => {}
        }
    }

    pub fn scroll_to_top(&mut self) {
        match &mut self.kind {
            PreviewKind::Loaded { scroll, .. } => *scroll = 0,
            PreviewKind::Windowed { window, .. } => window.scroll_to_top(),
            PreviewKind::Empty => {}
        }
    }

    pub fn scroll_to_bottom(&mut self, height: usize) {
        match &mut self.kind {
            PreviewKind::Loaded { lines, scroll } => {
                *scroll = lines.len().saturating_sub(height.max(1));
            }
            PreviewKind::Windowed { window, .. } => window.scroll_to_bottom(height),
            PreviewKind::Empty => {}
        }
    }
}

/// Detect if a file is likely binary by checking for null bytes and the ratio
/// of printable characters in the first 8 KB.
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
    if bytes.contains(&0) {
        return true;
    }
    let printable = bytes
        .iter()
        .filter(|&&b| b >= 0x20 || b == b'\n' || b == b'\r' || b == b'\t')
        .count();
    (printable as f64 / n as f64) < 0.85
}

/// Build a preview for `path`, choosing highlighting vs. windowed reading by
/// file size.
pub fn load_preview(path: &Path) -> PreviewState {
    load_preview_with_threshold(path, HIGHLIGHT_MAX)
}

fn load_preview_with_threshold(path: &Path, threshold: u64) -> PreviewState {
    let Ok(meta) = fs::metadata(path) else {
        return PreviewState::message(Some(path.to_path_buf()), "Cannot read file", Color::Red);
    };

    if is_binary(path) {
        let mut state =
            PreviewState::message(Some(path.to_path_buf()), "[Binary file]", Color::Red);
        state.is_binary = true;
        return state;
    }

    // Small enough: load fully and syntax-highlight (lossless source view).
    if meta.len() <= threshold
        && let Ok(content) = fs::read_to_string(path)
    {
        return PreviewState {
            path: Some(path.to_path_buf()),
            is_binary: false,
            kind: PreviewKind::Loaded {
                lines: highlight::highlight(&content, path),
                scroll: 0,
            },
        };
    }

    // Large file, or small-but-not-UTF-8: stream it with the windowed reader.
    match FileWindow::open(path) {
        Ok(window) => PreviewState {
            path: Some(path.to_path_buf()),
            is_binary: false,
            kind: PreviewKind::Windowed {
                window,
                count: CountState::Unknown,
            },
        },
        Err(_) => PreviewState::message(Some(path.to_path_buf()), "Cannot read file", Color::Red),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ncoxide_prev_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_binary_detection() {
        let dir = tmp("bin");
        fs::write(dir.join("text.txt"), "Hello, world!\nThis is text.\n").unwrap();
        assert!(!is_binary(&dir.join("text.txt")));

        let mut binary = b"ELF".to_vec();
        binary.push(0);
        binary.extend_from_slice(&[0xFF; 100]);
        fs::write(dir.join("binary.bin"), &binary).unwrap();
        assert!(is_binary(&dir.join("binary.bin")));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_small_file_is_loaded_and_highlighted() {
        let dir = tmp("small");
        let path = dir.join("hello.rs");
        fs::write(&path, "fn main() {\n    println!(\"Hello\");\n}\n").unwrap();
        let state = load_preview(&path);
        assert!(!state.is_binary);
        assert!(matches!(state.kind, PreviewKind::Loaded { .. }));
        assert_eq!(state.status_text(), "line 1/3");
        assert_eq!(state.render(10).len(), 3);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_large_file_uses_windowed_reader() {
        let dir = tmp("large");
        let path = dir.join("big.log");
        // Small file, but force the windowed path with a tiny threshold.
        let body: String = (0..100).map(|i| format!("log line {i}\n")).collect();
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();

        let state = load_preview_with_threshold(&path, 16);
        assert!(matches!(state.kind, PreviewKind::Windowed { .. }));
        let rendered = state.render(5);
        assert_eq!(rendered.len(), 5);
        assert!(state.status_text().contains('%'));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_binary_file_message() {
        let dir = tmp("binmsg");
        let path = dir.join("blob");
        let mut data = vec![0u8; 1000];
        data[0] = 0x7F;
        fs::write(&path, &data).unwrap();
        let state = load_preview(&path);
        assert!(state.is_binary);
        assert_eq!(state.render(1).len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_missing_file_message() {
        let state = load_preview(Path::new("/nonexistent/ncoxide/file"));
        assert!(!state.render(1).is_empty());
    }
}
