use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// Result from fuzzy matching a file path.
#[derive(Debug, Clone)]
pub struct FinderMatch {
    pub path: PathBuf,
    pub display_name: String,
    pub score: u32,
}

/// An entry collected by the walk: absolute path + root-relative display name.
type WalkedPath = (PathBuf, String);

/// The finder's directory walk, shared by the synchronous `Finder::find`
/// (tests, small trees) and the background [`FinderWalk`]. Depth-capped;
/// hidden entries are skipped except the walk root itself (R2).
fn walk_iter(root: &Path) -> impl Iterator<Item = WalkedPath> + '_ {
    walkdir::WalkDir::new(root)
        .max_depth(8)
        .into_iter()
        .filter_entry(|e| {
            // Skip hidden entries — but never the walk root itself
            // (depth 0), or a finder opened *inside* a hidden directory
            // (e.g. ~/.config) would yield no results at all.
            e.depth() == 0 || !e.file_name().to_str().is_some_and(|s| s.starts_with('.'))
        })
        .flatten()
        .filter_map(move |entry| {
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
            if rel.is_empty() {
                return None;
            }
            Some((path.to_path_buf(), rel.to_string()))
        })
}

/// Background directory walk feeding the finder (R7). The walk used to run
/// synchronously on every keystroke, freezing the UI for seconds under big
/// trees; now it runs once per finder session on a thread and keystrokes
/// only re-score the collected list. Same pattern as `preview::LineFilter`:
/// shared list + stop flag, aborts promptly on drop.
pub struct FinderWalk {
    paths: Arc<Mutex<Vec<WalkedPath>>>,
    done: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FinderWalk {
    pub fn spawn(root: PathBuf) -> Self {
        let paths = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));

        let (p, d, s) = (paths.clone(), done.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            // Push in small batches to keep lock hold times short.
            let mut batch = Vec::with_capacity(64);
            for item in walk_iter(&root) {
                if s.load(Ordering::Relaxed) {
                    return; // aborted: finder closed or new session started
                }
                batch.push(item);
                if batch.len() >= 64 {
                    p.lock().unwrap().append(&mut batch);
                }
            }
            p.lock().unwrap().append(&mut batch);
            d.store(true, Ordering::Relaxed);
        });

        FinderWalk {
            paths,
            done,
            stop,
            handle: Some(handle),
        }
    }

    /// Number of paths collected so far (cheap progress signal).
    pub fn count(&self) -> usize {
        self.paths.lock().unwrap().len()
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }

    /// Run `f` over the collected paths without cloning the list. The lock is
    /// held for the duration; the walker just stalls briefly if it collides.
    pub fn with_paths<T>(&self, f: impl FnOnce(&[WalkedPath]) -> T) -> T {
        let guard = self.paths.lock().unwrap();
        f(&guard)
    }
}

impl Drop for FinderWalk {
    fn drop(&mut self) {
        // Signal and detach — never join: the walker only checks the flag
        // between yielded entries, so a readdir blocked on a dead network
        // mount would otherwise freeze the UI thread on Esc/Enter. A
        // detached walker exits at its next flag check; its shared list is
        // dropped with the last Arc.
        self.stop.store(true, Ordering::Relaxed);
        drop(self.handle.take());
    }
}

/// Fuzzy file finder using nucleo-matcher (same engine as helix).
pub struct Finder {
    matcher: Matcher,
}

impl Default for Finder {
    fn default() -> Self {
        Self::new()
    }
}

impl Finder {
    pub fn new() -> Self {
        Finder {
            matcher: Matcher::new(Config::DEFAULT),
        }
    }

    /// Score an already-collected path list against `query` and return the
    /// best `max_results` matches, highest score first.
    pub fn find_in(
        &mut self,
        items: &[WalkedPath],
        query: &str,
        max_results: usize,
    ) -> Vec<FinderMatch> {
        if query.is_empty() {
            return Vec::new();
        }

        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();

        let mut results: Vec<FinderMatch> = items
            .iter()
            .filter_map(|(path, rel)| {
                let haystack = Utf32Str::new(rel, &mut buf);
                pattern
                    .score(haystack, &mut self.matcher)
                    .map(|score| FinderMatch {
                        path: path.clone(),
                        display_name: rel.clone(),
                        score,
                    })
            })
            .collect();

        results.sort_by_key(|m| std::cmp::Reverse(m.score));
        results.truncate(max_results);
        results
    }

    /// Walk `root` synchronously and score everything (tests and one-shot
    /// callers; the app uses [`FinderWalk`] + [`Finder::find_in`] instead).
    pub fn find(&mut self, root: &Path, query: &str, max_results: usize) -> Vec<FinderMatch> {
        if query.is_empty() {
            return Vec::new();
        }
        let items: Vec<WalkedPath> = walk_iter(root).collect();
        self.find_in(&items, query, max_results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_finder_basic() {
        let tmp = std::env::temp_dir().join(format!("ncoxide_finder_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("src")).unwrap();
        fs::write(tmp.join("src/main.rs"), "").unwrap();
        fs::write(tmp.join("src/lib.rs"), "").unwrap();
        fs::write(tmp.join("README.md"), "").unwrap();

        let mut finder = Finder::new();
        let results = finder.find(&tmp, "main", 10);
        assert!(!results.is_empty());
        assert!(results[0].display_name.contains("main"));

        let results = finder.find(&tmp, "rs", 10);
        assert!(results.len() >= 2);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_finder_works_inside_hidden_root() {
        // filter_entry also applies to the walk root; without the depth-0
        // guard a finder opened in a dot-directory finds nothing (R2).
        let tmp =
            std::env::temp_dir().join(format!(".ncoxide_finder_hidden_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("sub")).unwrap();
        fs::write(tmp.join("main.rs"), "").unwrap();
        fs::write(tmp.join("sub/other.rs"), "").unwrap();
        // Hidden entries *inside* the root stay excluded.
        fs::write(tmp.join(".secret.rs"), "").unwrap();

        let mut finder = Finder::new();
        let results = finder.find(&tmp, "rs", 10);
        let names: Vec<&str> = results.iter().map(|m| m.display_name.as_str()).collect();
        assert!(names.contains(&"main.rs"), "results: {names:?}");
        assert!(names.contains(&"sub/other.rs"), "results: {names:?}");
        assert!(!names.contains(&".secret.rs"), "results: {names:?}");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_finder_empty_query() {
        let mut finder = Finder::new();
        let results = finder.find(Path::new("/tmp"), "", 10);
        assert!(results.is_empty());
    }
}
