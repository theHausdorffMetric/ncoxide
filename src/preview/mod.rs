mod filter;
mod highlight;
mod index;
mod search;
mod window;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

pub use filter::{FilterMatch, LineFilter};
pub use index::LineIndex;
pub use search::{Search, SearchKind};
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
    Loaded {
        lines: Vec<PreviewLine>,
        scroll: usize,
    },
    /// Large file: windowed plain-text reader, never fully resident. The
    /// total line count is filled in by a background scan (see [`LineCounter`]).
    Windowed {
        window: FileWindow,
        count: CountState,
    },
}

/// State for preview mode. Backed either by a fully-loaded line buffer (small
/// files) or a windowed reader (large files); both render through
/// [`PreviewState::render`].
#[derive(Debug)]
pub struct PreviewState {
    pub path: Option<PathBuf>,
    pub is_binary: bool,
    kind: PreviewKind,
    /// Active in-file search, used for match navigation and highlighting.
    active_search: Option<Search>,
}

impl Default for PreviewState {
    fn default() -> Self {
        PreviewState {
            path: None,
            is_binary: false,
            kind: PreviewKind::Empty,
            active_search: None,
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
            active_search: None,
        }
    }

    pub fn clear(&mut self) {
        self.path = None;
        self.is_binary = false;
        self.kind = PreviewKind::Empty;
        self.active_search = None;
    }

    /// Render the visible window as ratatui lines, including a line-number
    /// gutter. Owned (`'static`) so callers don't hold a borrow on `self`.
    pub fn render(&self, height: usize) -> Vec<Line<'static>> {
        let gutter = |n: Option<u64>| match n {
            Some(n) => Span::styled(format!("{n:>6} "), Style::default().fg(Color::DarkGray)),
            None => Span::styled("       ".to_string(), Style::default().fg(Color::DarkGray)),
        };
        // Build one line: gutter + content, applying search highlighting (only
        // for on-screen lines, so it stays cheap).
        let build = |num: Option<u64>, pieces: &[(String, Style)]| -> Line<'static> {
            let mut spans = vec![gutter(num)];
            spans.extend(self.highlight_pieces(pieces));
            Line::from(spans)
        };

        match &self.kind {
            PreviewKind::Empty => Vec::new(),
            PreviewKind::Loaded { lines, scroll } => lines
                .iter()
                .enumerate()
                .skip(*scroll)
                .take(height)
                .map(|(i, pline)| build(Some(i as u64 + 1), &pline.spans))
                .collect(),
            PreviewKind::Windowed { window, .. } => {
                let base = window.top_line_1based();
                window
                    .lines()
                    .iter()
                    .take(height)
                    .enumerate()
                    .map(|(i, text)| {
                        let pieces = [(text.clone(), Style::default())];
                        build(base.map(|b| b + i as u64), &pieces)
                    })
                    .collect()
            }
        }
    }

    /// Convert a line's styled pieces into spans, overlaying a highlight
    /// background on any ranges matched by the active search.
    fn highlight_pieces(&self, pieces: &[(String, Style)]) -> Vec<Span<'static>> {
        let plain: String = pieces.iter().map(|(t, _)| t.as_str()).collect();
        let ranges = match &self.active_search {
            Some(s) => s.match_ranges(&plain),
            None => Vec::new(),
        };
        if ranges.is_empty() {
            return pieces
                .iter()
                .map(|(t, st)| Span::styled(t.clone(), *st))
                .collect();
        }

        let hl = Style::default().bg(Color::Yellow).fg(Color::Black);
        let in_match = |abs: usize| ranges.iter().any(|(s, e)| abs >= *s && abs < *e);

        // Group consecutive chars sharing the same effective style into spans.
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut cur = String::new();
        let mut cur_style: Option<Style> = None;
        let mut abs = 0usize;
        for (text, base) in pieces {
            for ch in text.chars() {
                let style = if in_match(abs) { base.patch(hl) } else { *base };
                if cur_style != Some(style) {
                    if let Some(st) = cur_style {
                        spans.push(Span::styled(std::mem::take(&mut cur), st));
                    }
                    cur_style = Some(style);
                }
                cur.push(ch);
                abs += ch.len_utf8();
            }
        }
        if let Some(st) = cur_style {
            spans.push(Span::styled(cur, st));
        }
        spans
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

    /// Jump so 1-based `line` is at the top of the view. For windowed files,
    /// `checkpoint` is the nearest `(byte_offset, line_1based)` to read forward
    /// from (from [`LineIndex::checkpoint_for`]); pass `(0, 1)` to scan from the
    /// start.
    pub fn goto_line(&mut self, line: usize, checkpoint: (u64, u64)) {
        match &mut self.kind {
            PreviewKind::Loaded { lines, scroll } => {
                *scroll = line.saturating_sub(1).min(lines.len().saturating_sub(1));
            }
            PreviewKind::Windowed { window, .. } => window.goto_line(line as u64, checkpoint),
            PreviewKind::Empty => {}
        }
    }

    /// Set (or clear, when `query` is empty) the active in-file search.
    /// Returns `Err(message)` for an invalid regex.
    pub fn set_search(&mut self, query: &str, kind: SearchKind) -> Result<(), String> {
        self.active_search = if query.is_empty() {
            None
        } else {
            Some(Search::new(query, kind)?)
        };
        Ok(())
    }

    pub fn clear_search(&mut self) {
        self.active_search = None;
    }

    pub fn active_search(&self) -> Option<&Search> {
        self.active_search.as_ref()
    }

    /// Move to the next (`forward`) or previous matching line for the active
    /// search, wrapping at the ends. Returns `true` if a match was found.
    pub fn search_next(&mut self, forward: bool) -> bool {
        let Some(search) = self.active_search.clone() else {
            return false;
        };
        match &mut self.kind {
            PreviewKind::Loaded { lines, scroll } => {
                let n = lines.len();
                if n == 0 {
                    return false;
                }
                let mut i = *scroll;
                for _ in 0..n {
                    i = if forward {
                        (i + 1) % n
                    } else {
                        (i + n - 1) % n
                    };
                    if search.line_matches(&line_text(&lines[i])) {
                        *scroll = i;
                        return true;
                    }
                }
                false
            }
            PreviewKind::Windowed { window, .. } => {
                window.search(forward, |line| search.line_matches(line))
            }
            PreviewKind::Empty => false,
        }
    }
}

/// Plain text of a styled preview line (concatenated span text).
fn line_text(pline: &PreviewLine) -> String {
    pline.spans.iter().map(|(t, _)| t.as_str()).collect()
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

/// Cap on directory-preview entries; a tail line notes what was elided so
/// truncation is never silent.
const DIR_PREVIEW_MAX: usize = 1000;

/// Build a preview listing a directory's contents: directories first (blue,
/// bold, trailing `/`), then files, case-insensitively by name — mirroring
/// the pane's default order. Backed by `PreviewKind::Loaded`, so scrolling
/// and in-preview search work unchanged (F1).
pub fn load_dir_preview(path: &Path, show_hidden: bool) -> PreviewState {
    let read_dir = match fs::read_dir(path) {
        Ok(rd) => rd,
        Err(e) => {
            return PreviewState::message(
                Some(path.to_path_buf()),
                &format!("Cannot read directory: {e}"),
                Color::Red,
            );
        }
    };

    // (name, is_dir, is_symlink)
    let mut items: Vec<(String, bool, bool)> = Vec::new();
    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let file_type = entry.file_type();
        let is_symlink = file_type.as_ref().is_ok_and(|t| t.is_symlink());
        // Symlink dir-ness follows the target (mirrors FileEntry::from_path).
        let is_dir = match &file_type {
            Ok(t) if t.is_symlink() => entry.path().is_dir(),
            Ok(t) => t.is_dir(),
            Err(_) => false,
        };
        items.push((name, is_dir, is_symlink));
    }

    if items.is_empty() {
        return PreviewState::message(Some(path.to_path_buf()), "[empty]", Color::DarkGray);
    }

    items.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });

    let total = items.len();
    let mut lines: Vec<PreviewLine> = items
        .into_iter()
        .take(DIR_PREVIEW_MAX)
        .map(|(name, is_dir, is_symlink)| {
            let (text, style) = if is_dir {
                (
                    format!("{name}/"),
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                )
            } else if is_symlink {
                (name, Style::default().fg(Color::Magenta))
            } else {
                (name, Style::default())
            };
            PreviewLine {
                spans: vec![(text, style)],
            }
        })
        .collect();
    if total > DIR_PREVIEW_MAX {
        lines.push(PreviewLine {
            spans: vec![(
                format!("… {} more entries", total - DIR_PREVIEW_MAX),
                Style::default().fg(Color::DarkGray),
            )],
        });
    }

    PreviewState {
        path: Some(path.to_path_buf()),
        is_binary: false,
        kind: PreviewKind::Loaded { lines, scroll: 0 },
        active_search: None,
    }
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
            active_search: None,
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
            active_search: None,
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

    /// Plain text of each rendered line (gutter included).
    fn rendered(state: &PreviewState, h: usize) -> Vec<String> {
        state
            .render(h)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect()
    }

    #[test]
    fn test_search_next_loaded() {
        let dir = tmp("search_loaded");
        let path = dir.join("f.txt");
        fs::write(&path, "alpha\nbeta\nfind me\ngamma\nfind again\n").unwrap();
        let mut state = load_preview(&path);
        state.set_search("find", SearchKind::Literal).unwrap();

        assert!(state.search_next(true));
        assert!(rendered(&state, 1)[0].contains("find me"));
        assert!(state.search_next(true));
        assert!(rendered(&state, 1)[0].contains("find again"));
        // Wraps back to the first match.
        assert!(state.search_next(true));
        assert!(rendered(&state, 1)[0].contains("find me"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_search_next_windowed() {
        let dir = tmp("search_win");
        let path = dir.join("big.log");
        let mut body = String::new();
        for i in 0..200 {
            if i == 150 {
                body.push_str("the NEEDLE is here\n");
            } else {
                body.push_str(&format!("noise line {i}\n"));
            }
        }
        fs::write(&path, &body).unwrap();
        let mut state = load_preview_with_threshold(&path, 16); // force windowed
        state.set_search("needle", SearchKind::Literal).unwrap(); // smart-case insensitive

        assert!(state.search_next(true));
        assert!(rendered(&state, 1)[0].contains("NEEDLE"));
        assert_eq!(state.status_text().split(' ').nth(1), Some("151")); // 1-based line

        let _ = fs::remove_dir_all(&dir);
    }

    /// Large-file proof: search, goto, and fuzzy-filter over a ~128 MB file
    /// must stay bounded in memory (only windows/top-N are ever resident).
    /// Run with `cargo test --release -- --ignored large_file_search`.
    #[test]
    #[ignore]
    fn test_large_file_search_goto_filter() {
        use std::io::Write;
        let path = std::env::temp_dir().join(format!("ncoxide_bigsearch_{}", std::process::id()));
        let needle_line; // 1-based line number where the needle lives
        {
            let f = fs::File::create(&path).unwrap();
            let mut w = std::io::BufWriter::new(f);
            let mut line = 0u64;
            let mut written = 0u64;
            let mut needle_at = 0u64;
            while written < 128 * 1024 * 1024 {
                line += 1;
                let s = if written >= 100 * 1024 * 1024 && needle_at == 0 {
                    needle_at = line;
                    "UNIQUE_NEEDLE_XYZ marker line\n".to_string()
                } else {
                    format!("log entry number {line} with some filler text\n")
                };
                written += s.len() as u64;
                w.write_all(s.as_bytes()).unwrap();
            }
            needle_line = needle_at;
        }

        let mut state = load_preview_with_threshold(&path, 16); // windowed
        assert!(state.is_large());

        // Search finds the needle far into the file.
        state
            .set_search("UNIQUE_NEEDLE_XYZ", SearchKind::Literal)
            .unwrap();
        assert!(state.search_next(true));
        assert!(rendered(&state, 1)[0].contains("UNIQUE_NEEDLE_XYZ"));

        // Goto lands on an arbitrary line.
        state.goto_line(needle_line as usize, (0, 1));
        assert!(rendered(&state, 1)[0].contains("UNIQUE_NEEDLE_XYZ"));

        // Fuzzy filter finds it via a background scan (a full 128 MB pass takes
        // a few seconds, so wait on completion with a generous timeout).
        let filter = LineFilter::spawn(path.clone(), "uniqueneedle".to_string(), 200);
        for _ in 0..6000 {
            if filter.is_done() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(filter.is_done(), "filter scan did not finish in time");
        assert!(
            filter
                .results()
                .iter()
                .any(|m| m.text.contains("UNIQUE_NEEDLE_XYZ"))
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_search_highlights_matched_span() {
        let dir = tmp("highlight");
        let path = dir.join("f.txt");
        fs::write(&path, "hello world\n").unwrap();
        let mut state = load_preview(&path);
        state.set_search("world", SearchKind::Literal).unwrap();

        let lines = state.render(1);
        let hl: String = lines[0]
            .spans
            .iter()
            .filter(|s| s.style.bg == Some(Color::Yellow))
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(hl, "world", "only the match should be highlighted");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_dir_preview_sorts_dirs_first_and_respects_hidden() {
        let dir = tmp("dirprev");
        fs::create_dir_all(dir.join("zeta_dir")).unwrap();
        fs::write(dir.join("Alpha.txt"), "").unwrap();
        fs::write(dir.join("beta.txt"), "").unwrap();
        fs::write(dir.join(".hidden"), "").unwrap();

        let state = load_dir_preview(&dir, false);
        let lines = rendered(&state, 10);
        assert_eq!(lines.len(), 3, "hidden entry filtered: {lines:?}");
        assert!(lines[0].contains("zeta_dir/"), "dir first, with slash");
        assert!(lines[1].contains("Alpha.txt"), "case-insensitive order");
        assert!(lines[2].contains("beta.txt"));

        let state = load_dir_preview(&dir, true);
        let lines = rendered(&state, 10);
        assert_eq!(lines.len(), 4);
        assert!(lines[1].contains(".hidden"), "hidden shown when enabled");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_dir_preview_caps_with_tail_line() {
        let dir = tmp("dircap");
        for i in 0..DIR_PREVIEW_MAX + 5 {
            fs::write(dir.join(format!("f{i:05}.txt")), "").unwrap();
        }
        let state = load_dir_preview(&dir, false);
        let lines = rendered(&state, DIR_PREVIEW_MAX + 10);
        assert_eq!(lines.len(), DIR_PREVIEW_MAX + 1, "cap + tail line");
        assert!(
            lines.last().unwrap().contains("5 more entries"),
            "truncation is announced: {:?}",
            lines.last()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_dir_preview_empty_and_unreadable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("dirempty");
        let state = load_dir_preview(&dir, false);
        assert!(rendered(&state, 1)[0].contains("[empty]"));

        let locked = dir.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let state = load_dir_preview(&locked, false);
        assert!(rendered(&state, 1)[0].contains("Cannot read directory"));
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_goto_line_windowed_and_loaded() {
        let dir = tmp("goto");
        let body: String = (0..300).map(|i| format!("line{i}\n")).collect();

        let small = dir.join("small.txt");
        fs::write(&small, &body).unwrap();
        let mut state = load_preview(&small);
        state.goto_line(100, (0, 1));
        assert!(rendered(&state, 1)[0].contains("line99")); // 1-based 100 -> "line99"

        let big = dir.join("big.txt");
        fs::write(&big, &body).unwrap();
        let mut state = load_preview_with_threshold(&big, 16);
        state.goto_line(100, (0, 1));
        assert!(rendered(&state, 1)[0].contains("line99"));

        let _ = fs::remove_dir_all(&dir);
    }
}
