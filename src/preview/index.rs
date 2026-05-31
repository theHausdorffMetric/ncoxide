use std::fs::File;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

const SCAN_BUF: usize = 64 * 1024;

/// Counts the total number of lines in a (possibly huge) file on a background
/// thread, so the viewer can show an accurate `line X / N` without blocking on
/// open. Memory is bounded to a fixed scratch buffer; the scan aborts promptly
/// when the counter is dropped (i.e. the user navigates away).
pub struct LineCounter {
    /// Lines counted so far (progressive lower bound until `done`).
    count: Arc<AtomicU64>,
    done: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl LineCounter {
    pub fn spawn(path: PathBuf) -> Self {
        let count = Arc::new(AtomicU64::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));

        let (c, d, s) = (count.clone(), done.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            scan(&path, &c, &d, &s);
        });

        LineCounter {
            count,
            done,
            stop,
            handle: Some(handle),
        }
    }

    /// Lines counted so far. With `is_done()`, this is the file's total.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }
}

impl Drop for LineCounter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn scan(path: &PathBuf, count: &AtomicU64, done: &AtomicBool, stop: &AtomicBool) {
    let Ok(file) = File::open(path) else {
        done.store(true, Ordering::Relaxed);
        return;
    };
    let mut reader = BufReader::with_capacity(SCAN_BUF, file);
    let mut buf = [0u8; SCAN_BUF];
    let mut newlines: u64 = 0;
    let mut last_byte: Option<u8> = None;

    loop {
        if stop.load(Ordering::Relaxed) {
            return; // aborted: leave `done` false
        }
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                newlines += bytecount_newlines(&buf[..n]);
                last_byte = Some(buf[n - 1]);
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

fn bytecount_newlines(bytes: &[u8]) -> u64 {
    bytes.iter().filter(|&&b| b == b'\n').count() as u64
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

    fn wait_total(path: PathBuf) -> u64 {
        let counter = LineCounter::spawn(path);
        // Bounded spin until the (tiny, test-sized) scan completes.
        for _ in 0..10_000 {
            if counter.is_done() {
                break;
            }
            std::thread::yield_now();
        }
        assert!(counter.is_done());
        counter.count()
    }

    #[test]
    fn test_count_trailing_newline() {
        let path = write_tmp("trail", b"a\nb\nc\n");
        assert_eq!(wait_total(path.clone()), 3);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_count_no_trailing_newline() {
        let path = write_tmp("notrail", b"a\nb\nc");
        assert_eq!(wait_total(path.clone()), 3);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_count_empty() {
        let path = write_tmp("empty", b"");
        assert_eq!(wait_total(path.clone()), 0);
        let _ = std::fs::remove_file(&path);
    }
}
