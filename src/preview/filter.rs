use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use super::window::read_forward;

/// One line matched by a fuzzy filter query.
#[derive(Debug, Clone)]
pub struct FilterMatch {
    /// 1-based line number.
    pub line: u64,
    pub text: String,
    pub score: u32,
}

/// Streams a file in the background, fuzzy-scoring each line against a query
/// (via the same `nucleo-matcher` engine as the file finder) and keeping the
/// top-N highest-scoring lines. Memory is O(N) regardless of file size; the
/// scan aborts promptly when dropped (e.g. the query changes).
pub struct LineFilter {
    results: Arc<Mutex<Vec<FilterMatch>>>,
    done: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl LineFilter {
    pub fn spawn(path: PathBuf, query: String, max_results: usize) -> Self {
        let results = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));

        let (r, d, s) = (results.clone(), done.clone(), stop.clone());
        let handle = std::thread::spawn(move || run(&path, &query, max_results, &r, &d, &s));

        LineFilter {
            results,
            done,
            stop,
            handle: Some(handle),
        }
    }

    /// Current best matches, highest score first.
    pub fn results(&self) -> Vec<FilterMatch> {
        self.results.lock().unwrap().clone()
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }
}

impl Drop for LineFilter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Snapshot a min-heap (smallest score on top) into a high-to-low result list.
fn snapshot(heap: &BinaryHeap<(Reverse<u32>, u64, String)>) -> Vec<FilterMatch> {
    let mut v: Vec<FilterMatch> = heap
        .iter()
        .map(|(Reverse(score), line, text)| FilterMatch {
            line: *line,
            text: text.clone(),
            score: *score,
        })
        .collect();
    v.sort_by_key(|m| Reverse(m.score));
    v
}

fn run(
    path: &PathBuf,
    query: &str,
    max_results: usize,
    results: &Mutex<Vec<FilterMatch>>,
    done: &AtomicBool,
    stop: &AtomicBool,
) {
    if query.is_empty() {
        done.store(true, Ordering::Relaxed);
        return;
    }
    let Ok(mut file) = File::open(path) else {
        done.store(true, Ordering::Relaxed);
        return;
    };

    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    // Min-heap keyed by Reverse(score): the lowest score sits on top so it can
    // be evicted once we exceed `max_results`.
    let mut heap: BinaryHeap<(Reverse<u32>, u64, String)> = BinaryHeap::new();
    let mut utf32 = Vec::new();

    let mut off = 0u64;
    let mut line = 0u64; // 0-based as we read
    const BATCH: usize = 256;
    loop {
        if stop.load(Ordering::Relaxed) {
            return; // aborted: query changed or viewer closed
        }
        let Ok((lines, next, eof)) = read_forward(&mut file, off, BATCH) else {
            break;
        };
        if lines.is_empty() {
            break;
        }
        for text in &lines {
            line += 1; // 1-based line number for this text
            if let Some(score) = pattern.score(Utf32Str::new(text, &mut utf32), &mut matcher) {
                heap.push((Reverse(score), line, text.clone()));
                if heap.len() > max_results {
                    heap.pop(); // drop the current lowest score
                }
            }
        }
        *results.lock().unwrap() = snapshot(&heap);
        if eof {
            break;
        }
        off = next;
    }

    *results.lock().unwrap() = snapshot(&heap);
    done.store(true, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn wait(filter: &LineFilter) {
        for _ in 0..100_000 {
            if filter.is_done() {
                return;
            }
            std::thread::yield_now();
        }
        panic!("filter did not finish");
    }

    #[test]
    fn test_fuzzy_filter_finds_and_ranks() {
        let path = std::env::temp_dir().join(format!("ncoxide_filt_{}", std::process::id()));
        let mut body = String::new();
        for i in 0..500 {
            body.push_str(&format!("noise {i}\n"));
        }
        body.push_str("the config loader\n"); // line 501
        body.push_str("unrelated\n");
        let mut f = File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();

        let filter = LineFilter::spawn(path.clone(), "cfgldr".to_string(), 200);
        wait(&filter);
        let results = filter.results();
        assert!(!results.is_empty());
        // The "config loader" line (fuzzy match for "cfgldr") should rank top.
        assert!(results[0].text.contains("config loader"));
        assert_eq!(results[0].line, 501);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_fuzzy_filter_top_n_bounded() {
        let path = std::env::temp_dir().join(format!("ncoxide_filt2_{}", std::process::id()));
        let mut body = String::new();
        for _ in 0..1000 {
            body.push_str("match line\n"); // all match "match"
        }
        File::create(&path).unwrap().write_all(body.as_bytes()).unwrap();

        let filter = LineFilter::spawn(path.clone(), "match".to_string(), 50);
        wait(&filter);
        assert_eq!(filter.results().len(), 50, "results capped at max_results");

        let _ = std::fs::remove_file(&path);
    }
}
