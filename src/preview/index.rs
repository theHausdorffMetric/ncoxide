use std::fs::File;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

const SCAN_BUF: usize = 64 * 1024;
/// One byte offset is recorded every `STRIDE` lines (a 40 M-line file →
/// ~40 K offsets ≈ 320 KB; bounded regardless of file size).
const STRIDE: u64 = 1000;

/// Background line scanner for large files. Counts total lines and records a
/// sparse line→byte-offset index, so the viewer can show an accurate
/// `line X / N` and jump to any line quickly — without holding the file in
/// memory and without blocking on open. The scan aborts promptly when dropped.
pub struct LineIndex {
    /// Lines counted so far (progressive lower bound until `is_done`).
    count: Arc<AtomicU64>,
    done: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    /// `offsets[k]` = byte offset of the start of line `k * STRIDE` (0-based).
    offsets: Arc<Mutex<Vec<u64>>>,
    handle: Option<JoinHandle<()>>,
}

impl LineIndex {
    pub fn spawn(path: PathBuf) -> Self {
        let count = Arc::new(AtomicU64::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let offsets = Arc::new(Mutex::new(vec![0u64])); // line 0 starts at byte 0

        let (c, d, s, o) = (count.clone(), done.clone(), stop.clone(), offsets.clone());
        let handle = std::thread::spawn(move || scan(&path, &c, &d, &s, &o));

        LineIndex {
            count,
            done,
            stop,
            offsets,
            handle: Some(handle),
        }
    }

    /// Lines counted so far. With `is_done`, this is the file's total.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }

    /// Nearest indexed checkpoint at or before 1-based `line`, as
    /// `(byte_offset, line_1based)` — a starting point to read forward from.
    /// Falls back to the start of the file.
    pub fn checkpoint_for(&self, line: usize) -> (u64, u64) {
        let target0 = (line.max(1) - 1) as u64; // 0-based
        let k = (target0 / STRIDE) as usize;
        let offsets = self.offsets.lock().unwrap();
        let k = k.min(offsets.len().saturating_sub(1));
        let offset = offsets.get(k).copied().unwrap_or(0);
        (offset, k as u64 * STRIDE + 1)
    }
}

impl Drop for LineIndex {
    fn drop(&mut self) {
        // Signal and detach — never join: a read blocked on a dead network
        // mount would freeze the UI thread (same policy as FinderWalk).
        self.stop.store(true, Ordering::Relaxed);
        drop(self.handle.take());
    }
}

fn scan(
    path: &PathBuf,
    count: &AtomicU64,
    done: &AtomicBool,
    stop: &AtomicBool,
    offsets: &Mutex<Vec<u64>>,
) {
    let Ok(file) = File::open(path) else {
        done.store(true, Ordering::Relaxed);
        return;
    };
    let mut reader = BufReader::with_capacity(SCAN_BUF, file);
    let mut buf = [0u8; SCAN_BUF];
    let mut newlines: u64 = 0;
    let mut pos: u64 = 0;
    let mut last_byte: Option<u8> = None;

    loop {
        if stop.load(Ordering::Relaxed) {
            return; // aborted: leave `done` false
        }
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                for (i, &b) in buf[..n].iter().enumerate() {
                    if b == b'\n' {
                        newlines += 1;
                        if newlines.is_multiple_of(STRIDE) {
                            // Start of the next line (= line `newlines`, 0-based).
                            offsets.lock().unwrap().push(pos + i as u64 + 1);
                        }
                    }
                }
                last_byte = Some(buf[n - 1]);
                pos += n as u64;
                count.store(newlines, Ordering::Relaxed);
            }
            Err(_) => break,
        }
    }

    // A final line without a trailing newline still counts as a line.
    let total = match last_byte {
        Some(b'\n') | None => newlines,
        Some(_) => newlines + 1,
    };
    count.store(total, Ordering::Relaxed);
    done.store(true, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(name: &str, data: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("ncoxide_idx_{name}_{}", std::process::id()));
        let mut f = File::create(&path).unwrap();
        f.write_all(data).unwrap();
        path
    }

    fn finished(path: PathBuf) -> LineIndex {
        let index = LineIndex::spawn(path);
        for _ in 0..100_000 {
            if index.is_done() {
                break;
            }
            std::thread::yield_now();
        }
        assert!(index.is_done());
        index
    }

    #[test]
    fn test_count_trailing_newline() {
        let path = write_tmp("trail", b"a\nb\nc\n");
        assert_eq!(finished(path.clone()).count(), 3);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_count_no_trailing_newline() {
        let path = write_tmp("notrail", b"a\nb\nc");
        assert_eq!(finished(path.clone()).count(), 3);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_count_empty() {
        let path = write_tmp("empty", b"");
        assert_eq!(finished(path.clone()).count(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_checkpoint_offsets() {
        // 5000 fixed-width lines "lineNNNN\n" (9 bytes each).
        let mut body = String::new();
        for i in 0..5000 {
            body.push_str(&format!("line{i:04}\n"));
        }
        let path = write_tmp("ckpt", body.as_bytes());
        let index = finished(path.clone());

        // Line 1 → start of file.
        assert_eq!(index.checkpoint_for(1), (0, 1));
        // Line 2001 falls in stride bucket k=2 → offset of line 2000 (0-based),
        // i.e. byte 2000 * 9, reported as 1-based line 2001.
        assert_eq!(index.checkpoint_for(2001), (2000 * 9, 2001));
        // A line within bucket 2 uses the same checkpoint.
        assert_eq!(index.checkpoint_for(2500), (2000 * 9, 2001));

        let _ = std::fs::remove_file(&path);
    }
}
