use std::path::{Path, PathBuf};

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// Result from fuzzy matching a file path.
#[derive(Debug, Clone)]
pub struct FinderMatch {
    pub path: PathBuf,
    pub display_name: String,
    pub score: u32,
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

    /// Find files matching the query in the given directory (recursive, up to max_depth).
    pub fn find(&mut self, root: &Path, query: &str, max_results: usize) -> Vec<FinderMatch> {
        if query.is_empty() {
            return Vec::new();
        }

        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);

        let mut results: Vec<FinderMatch> = Vec::new();
        let mut buf = Vec::new();

        // Walk directory tree
        for entry in walkdir::WalkDir::new(root)
            .max_depth(8)
            .into_iter()
            .filter_entry(|e| {
                // Skip hidden directories
                !e.file_name()
                    .to_str()
                    .is_some_and(|s| s.starts_with('.'))
            })
            .flatten()
        {
            let path = entry.path();

            // Get relative path for display/matching
            let rel = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy();

            if rel.is_empty() {
                continue;
            }

            let haystack = Utf32Str::new(&rel, &mut buf);
            if let Some(score) = pattern.score(haystack, &mut self.matcher) {
                results.push(FinderMatch {
                    path: path.to_path_buf(),
                    display_name: rel.to_string(),
                    score,
                });
            }
        }

        // Sort by score descending
        results.sort_by(|a, b| b.score.cmp(&a.score));
        results.truncate(max_results);
        results
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
    fn test_finder_empty_query() {
        let mut finder = Finder::new();
        let results = finder.find(Path::new("/tmp"), "", 10);
        assert!(results.is_empty());
    }
}
