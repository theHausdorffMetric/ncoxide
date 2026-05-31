use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// Read buffer size for forward scans.
const READ_BUF: usize = 64 * 1024;
/// Block size for backward scans.
const BACK_CHUNK: usize = 64 * 1024;
/// Maximum bytes kept per displayed line. A line longer than this (e.g. a
/// newline-free multi-GB file) is truncated for display so it can never be
/// read into a single allocation — this is what keeps memory bounded on
/// small machines.
const MAX_LINE_BYTES: usize = 8 * 1024;
/// Number of lines buffered around the viewport. Covers any terminal height
/// while keeping resident memory tiny regardless of file size.
const WINDOW_LINES: usize = 512;

/// A windowed, low-memory reader over a (possibly huge) file.
///
/// Holds at most `WINDOW_LINES` decoded lines plus fixed scratch buffers, so
/// resident memory stays constant no matter how large the file is. Content is
/// decoded lossily (`from_utf8_lossy`) so binary-ish logs still render.
pub struct FileWindow {
    file: File,
    file_len: u64,
    /// Byte offset of the first buffered line.
    top_offset: u64,
    /// Absolute 0-based line number of the first buffered line, when known.
    /// Becomes `None` after a jump-to-bottom (no full scan in phase 1/2).
    top_line: Option<u64>,
    /// Buffered lines starting at `top_offset` (lossy-decoded, length-capped).
    buf: Vec<String>,
    /// True once the final line of the file is within the buffer.
    at_eof: bool,
}

impl std::fmt::Debug for FileWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileWindow")
            .field("file_len", &self.file_len)
            .field("top_offset", &self.top_offset)
            .field("top_line", &self.top_line)
            .field("buffered", &self.buf.len())
            .field("at_eof", &self.at_eof)
            .finish()
    }
}

impl FileWindow {
    /// Open `path` and buffer the first window of lines.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let file_len = file.metadata()?.len();
        let mut w = FileWindow {
            file,
            file_len,
            top_offset: 0,
            top_line: Some(0),
            buf: Vec::new(),
            at_eof: file_len == 0,
        };
        w.refill();
        Ok(w)
    }

    /// Currently buffered lines (the renderer slices `[0..height]`).
    pub fn lines(&self) -> &[String] {
        &self.buf
    }

    /// Absolute 1-based line number at the top of the view, if known.
    pub fn top_line_1based(&self) -> Option<u64> {
        self.top_line.map(|l| l + 1)
    }

    /// Position through the file as a percentage of bytes (0..=100).
    pub fn byte_pct(&self) -> u8 {
        if self.file_len == 0 {
            return 100;
        }
        ((self.top_offset.min(self.file_len) as f64 / self.file_len as f64) * 100.0).round() as u8
    }

    /// Re-read the window (`WINDOW_LINES` lines) starting at `top_offset`.
    /// Best-effort: on I/O error the previous buffer is retained.
    fn refill(&mut self) {
        if let Ok((lines, _next, eof)) = read_forward(&mut self.file, self.top_offset, WINDOW_LINES)
        {
            self.buf = lines;
            self.at_eof = eof;
        }
    }

    /// Scroll up by `amount` lines.
    pub fn scroll_up(&mut self, amount: usize) {
        for _ in 0..amount {
            if self.top_offset == 0 {
                self.top_line = Some(0);
                break;
            }
            match prev_line_start(&mut self.file, self.top_offset) {
                Ok(off) => {
                    self.top_offset = off;
                    self.top_line = self.top_line.map(|l| l.saturating_sub(1));
                    if off == 0 {
                        self.top_line = Some(0);
                    }
                }
                Err(_) => break,
            }
        }
        self.refill();
    }

    /// Scroll down by `amount` lines, clamping to the bottom of the file so the
    /// view never scrolls past the last line.
    pub fn scroll_down(&mut self, amount: usize, height: usize) {
        match read_forward(&mut self.file, self.top_offset, amount) {
            Ok((lines, next, _eof)) if lines.len() == amount => {
                // Don't advance if there is nothing left to show below.
                if let Ok((rest, _, _)) = read_forward(&mut self.file, next, 1)
                    && rest.is_empty()
                {
                    self.scroll_to_bottom(height);
                    return;
                }
                self.top_offset = next;
                self.top_line = self.top_line.map(|l| l + amount as u64);
                self.refill();
            }
            _ => self.scroll_to_bottom(height),
        }
    }

    /// Jump to the top of the file.
    pub fn scroll_to_top(&mut self) {
        self.top_offset = 0;
        self.top_line = Some(0);
        self.refill();
    }

    /// Jump to the bottom so the last `height` lines fill the view.
    pub fn scroll_to_bottom(&mut self, height: usize) {
        let mut off = self.file_len;
        // Walk back `height` line starts from EOF (the first step lands on the
        // start of the final content line, ignoring a trailing newline).
        for _ in 0..height.max(1) {
            if off == 0 {
                break;
            }
            match prev_line_start(&mut self.file, off) {
                Ok(o) => off = o,
                Err(_) => break,
            }
        }
        self.top_offset = off;
        // Absolute line number unknown without a full scan (see index.rs).
        self.top_line = if off == 0 { Some(0) } else { None };
        self.refill();
    }

    /// Position the view so `offset` (a line start) is the top line.
    fn seek_to(&mut self, offset: u64, line: Option<u64>) {
        self.top_offset = offset.min(self.file_len);
        self.top_line = line;
        self.refill();
    }

    /// Find the next/previous line satisfying `matches`, starting from the line
    /// after/before the current top, and reposition the view there. Returns
    /// `true` if a match was found. Streams one line at a time (bounded memory).
    pub fn search(&mut self, forward: bool, matches: impl Fn(&str) -> bool) -> bool {
        if forward {
            // Start just past the current top line.
            let Ok((top, mut off, top_eof)) = read_forward(&mut self.file, self.top_offset, 1)
            else {
                return false;
            };
            if top.is_empty() || top_eof {
                return false;
            }
            let mut line_no = self.top_line.map(|l| l + 1);
            loop {
                let Ok((ls, next, eof)) = read_forward(&mut self.file, off, 1) else {
                    return false;
                };
                let Some(text) = ls.first() else { break };
                if matches(text) {
                    self.seek_to(off, line_no);
                    return true;
                }
                if eof {
                    break;
                }
                off = next;
                line_no = line_no.map(|l| l + 1);
            }
            false
        } else {
            let mut off = self.top_offset;
            let mut line_no = self.top_line;
            while off > 0 {
                let Ok(prev) = prev_line_start(&mut self.file, off) else {
                    return false;
                };
                off = prev;
                line_no = line_no.map(|l| l.saturating_sub(1));
                if let Ok((ls, _, _)) = read_forward(&mut self.file, off, 1)
                    && let Some(text) = ls.first()
                    && matches(text)
                {
                    self.seek_to(off, line_no);
                    return true;
                }
            }
            false
        }
    }

    /// Jump so 1-based line `target` is at the top, reading forward from the
    /// supplied checkpoint `(offset, line)` (e.g. the nearest sparse-index
    /// entry, or `(0, 1)` to scan from the start). Bounded memory.
    pub fn goto_line(&mut self, target: u64, checkpoint: (u64, u64)) {
        let (mut off, mut line) = checkpoint; // `off` is the start of line `line` (1-based)
        let target = target.max(1);
        while line < target {
            let Ok((ls, next, eof)) = read_forward(&mut self.file, off, 1) else {
                break;
            };
            if ls.is_empty() {
                break; // past end of file
            }
            let prev = off;
            off = next;
            line += 1;
            if eof {
                // Just consumed the final line; clamp the view to it.
                off = prev;
                line -= 1;
                break;
            }
        }
        self.seek_to(off, Some(line.saturating_sub(1)));
    }
}

/// Append `data` to the current line buffer, capping stored bytes at
/// `MAX_LINE_BYTES` while still counting the true length.
fn append_capped(cur: &mut Vec<u8>, cur_total: &mut usize, data: &[u8]) {
    *cur_total += data.len();
    if cur.len() < MAX_LINE_BYTES {
        let take = (MAX_LINE_BYTES - cur.len()).min(data.len());
        cur.extend_from_slice(&data[..take]);
    }
}

/// Decode a (capped) line's bytes for display, stripping a trailing `\r`
/// (CRLF) and noting truncation when the real line was longer than the cap.
fn decode_line(cur: &[u8], cur_total: usize) -> String {
    let mut s = String::from_utf8_lossy(cur).into_owned();
    if s.ends_with('\r') {
        s.pop();
    }
    if cur_total > cur.len() {
        s.push_str(&format!(" …[+{} bytes]", cur_total - cur.len()));
    }
    s
}

/// Read up to `n` lines forward from byte `start`. Returns the decoded lines,
/// the byte offset just past the last returned line, and whether EOF was hit.
///
/// Lines are decoded lossily and capped at `MAX_LINE_BYTES`; the byte offset
/// always reflects true on-disk lengths so navigation stays exact.
pub(super) fn read_forward(
    file: &mut File,
    start: u64,
    n: usize,
) -> io::Result<(Vec<String>, u64, bool)> {
    if n == 0 {
        return Ok((Vec::new(), start, false));
    }
    file.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::with_capacity(READ_BUF, file);

    let mut lines: Vec<String> = Vec::with_capacity(n.min(WINDOW_LINES));
    let mut consumed: u64 = 0; // bytes of completed lines, from `start`
    let mut cur: Vec<u8> = Vec::new(); // current line (capped)
    let mut cur_total: usize = 0; // full current line length (excl. newline)
    let mut hit_eof = false;

    while lines.len() < n {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            hit_eof = true;
            // Flush a final line that had no trailing newline.
            if cur_total > 0 || !cur.is_empty() {
                lines.push(decode_line(&cur, cur_total));
                consumed += cur_total as u64;
            }
            break;
        }
        match available.iter().position(|&b| b == b'\n') {
            Some(pos) => {
                append_capped(&mut cur, &mut cur_total, &available[..pos]);
                lines.push(decode_line(&cur, cur_total));
                consumed += cur_total as u64 + 1; // + newline
                cur.clear();
                cur_total = 0;
                reader.consume(pos + 1);
            }
            None => {
                let len = available.len();
                append_capped(&mut cur, &mut cur_total, available);
                reader.consume(len);
            }
        }
    }

    Ok((lines, start + consumed, hit_eof))
}

/// Given the byte offset of a line start, return the byte offset of the
/// previous line's start (or 0 at the beginning of the file).
fn prev_line_start(file: &mut File, offset: u64) -> io::Result<u64> {
    if offset == 0 {
        return Ok(0);
    }
    // `offset - 1` is the newline terminating the previous line; find the
    // newline before that — the previous line starts just after it.
    let mut search_end = offset - 1; // exclusive upper bound
    let mut buf = vec![0u8; BACK_CHUNK];
    while search_end > 0 {
        let chunk_start = search_end.saturating_sub(BACK_CHUNK as u64);
        let len = (search_end - chunk_start) as usize;
        file.seek(SeekFrom::Start(chunk_start))?;
        file.read_exact(&mut buf[..len])?;
        if let Some(rel) = buf[..len].iter().rposition(|&b| b == b'\n') {
            return Ok(chunk_start + rel as u64 + 1);
        }
        search_end = chunk_start;
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(name: &str, data: &[u8]) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("ncoxide_win_{name}_{}", std::process::id()));
        let mut f = File::create(&path).unwrap();
        f.write_all(data).unwrap();
        path
    }

    #[test]
    fn test_window_open_and_scroll() {
        let body: String = (0..1000).map(|i| format!("line{i}\n")).collect();
        let path = write_tmp("scroll", body.as_bytes());

        let mut w = FileWindow::open(&path).unwrap();
        assert_eq!(w.lines()[0], "line0");
        assert_eq!(w.top_line_1based(), Some(1));

        w.scroll_down(10, 20);
        assert_eq!(w.lines()[0], "line10");
        assert_eq!(w.top_line_1based(), Some(11));

        w.scroll_up(5);
        assert_eq!(w.lines()[0], "line5");
        assert_eq!(w.top_line_1based(), Some(6));

        w.scroll_to_top();
        assert_eq!(w.lines()[0], "line0");
        assert_eq!(w.top_line_1based(), Some(1));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_window_bottom_shows_last_line() {
        let body: String = (0..1000).map(|i| format!("line{i}\n")).collect();
        let path = write_tmp("bottom", body.as_bytes());

        let mut w = FileWindow::open(&path).unwrap();
        w.scroll_to_bottom(20);
        // The last 20 lines should be buffered, ending on the final content line.
        assert_eq!(w.lines().last().unwrap(), "line999");
        assert_eq!(w.lines()[0], "line980");

        // Scrolling down at the bottom must not run off the end.
        w.scroll_down(50, 20);
        assert_eq!(w.lines().last().unwrap(), "line999");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_last_line_without_newline() {
        let path = write_tmp("nonl", b"alpha\nbeta\ngamma");
        let w = FileWindow::open(&path).unwrap();
        assert_eq!(w.lines(), &["alpha", "beta", "gamma"]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_empty_file() {
        let path = write_tmp("empty", b"");
        let w = FileWindow::open(&path).unwrap();
        assert!(w.lines().is_empty());
        assert_eq!(w.byte_pct(), 100);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_crlf_stripped() {
        let path = write_tmp("crlf", b"one\r\ntwo\r\n");
        let w = FileWindow::open(&path).unwrap();
        assert_eq!(w.lines(), &["one", "two"]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_long_line_capped() {
        // A single line far larger than the cap must be truncated, not loaded
        // whole, and must not break navigation to the following line.
        let mut data = vec![b'x'; MAX_LINE_BYTES * 4];
        data.push(b'\n');
        data.extend_from_slice(b"next\n");
        let path = write_tmp("longline", &data);

        let mut w = FileWindow::open(&path).unwrap();
        assert!(w.lines()[0].len() < MAX_LINE_BYTES + 64); // truncated for display
        assert!(w.lines()[0].contains("…[+"));
        assert_eq!(w.lines()[1], "next");

        w.scroll_down(1, 20);
        assert_eq!(w.lines()[0], "next");

        let _ = std::fs::remove_file(&path);
    }

    /// Large-file proof: opening and navigating a multi-hundred-MB file must
    /// be instant and correct because only a window is ever read — a full load
    /// would be slow and allocate the whole file. Run with
    /// `cargo test --release -- --ignored large_file`.
    #[test]
    #[ignore]
    fn test_large_file_windowed_navigation() {
        // ~256 MB of numbered lines, far past the highlight threshold.
        let path = std::env::temp_dir().join(format!("ncoxide_big_{}", std::process::id()));
        {
            let f = File::create(&path).unwrap();
            let mut w = std::io::BufWriter::new(f);
            let mut line: u64 = 0;
            let mut written: u64 = 0;
            while written < 256 * 1024 * 1024 {
                let s = format!("line {line}\n");
                w.write_all(s.as_bytes()).unwrap();
                written += s.len() as u64;
                line += 1;
            }
        }

        let mut win = FileWindow::open(&path).unwrap();
        assert_eq!(win.lines()[0], "line 0");

        win.scroll_down(1000, 40);
        assert_eq!(win.lines()[0], "line 1000");

        win.scroll_to_bottom(40);
        assert!(win.lines().last().unwrap().starts_with("line "));
        assert!(win.byte_pct() >= 99);

        win.scroll_to_top();
        assert_eq!(win.lines()[0], "line 0");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_lossy_invalid_utf8() {
        let path = write_tmp("lossy", b"good\n\xff\xfe bad\ntail\n");
        let w = FileWindow::open(&path).unwrap();
        assert_eq!(w.lines()[0], "good");
        assert!(w.lines()[1].contains("bad")); // invalid bytes replaced, no panic
        assert_eq!(w.lines()[2], "tail");
        let _ = std::fs::remove_file(&path);
    }
}
