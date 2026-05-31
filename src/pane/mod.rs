pub mod navigation;
pub mod operations;
pub mod selection;

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::error::{NcError, Result};
use crate::platform;

/// Sort criteria for file entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SortBy {
    Name,
    Size,
    Date,
    Extension,
}

impl SortBy {
    /// Parse a sort key from a config/command string. Returns `None` for
    /// unrecognized values so the caller can keep its existing default.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "name" => Some(SortBy::Name),
            "size" => Some(SortBy::Size),
            "date" => Some(SortBy::Date),
            "ext" | "extension" => Some(SortBy::Extension),
            _ => None,
        }
    }
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SortDirection {
    Ascending,
    Descending,
}

/// A single file/directory entry in a pane.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub is_hidden: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub permissions: String,
}

impl FileEntry {
    pub fn from_path(path: &Path) -> Result<Self> {
        let link_meta = fs::symlink_metadata(path).map_err(NcError::Io)?;
        let is_symlink = link_meta.is_symlink();

        // For symlinks, follow once to resolve target type/size. On failure
        // (e.g. a dangling symlink) fall back to the link's own metadata and
        // treat it as a non-directory rather than reusing the link metadata
        // as if it were the target.
        let (is_dir, size, modified) = if is_symlink {
            match fs::metadata(path) {
                Ok(target) => (target.is_dir(), target.len(), target.modified().ok()),
                Err(_) => (false, link_meta.len(), link_meta.modified().ok()),
            }
        } else {
            (link_meta.is_dir(), link_meta.len(), link_meta.modified().ok())
        };

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());

        let is_hidden = name.starts_with('.');

        Ok(FileEntry {
            name,
            path: path.to_path_buf(),
            is_dir,
            is_symlink,
            is_hidden,
            size,
            modified,
            permissions: platform::format_permissions(&link_meta),
        })
    }

    /// Extension for sorting purposes.
    pub fn extension(&self) -> &str {
        self.path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
    }
}

/// Identifies which pane (left or right).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneId {
    Left,
    Right,
}

impl PaneId {
    pub fn other(self) -> PaneId {
        match self {
            PaneId::Left => PaneId::Right,
            PaneId::Right => PaneId::Left,
        }
    }
}

/// State for a single pane.
#[derive(Debug)]
pub struct PaneState {
    pub id: PaneId,
    pub cwd: PathBuf,
    pub entries: Vec<FileEntry>,
    pub cursor: usize,
    pub scroll_offset: usize,
    pub selected: Vec<bool>,
    pub sort_by: SortBy,
    pub sort_dir: SortDirection,
    pub show_hidden: bool,
    pub filter: Option<String>,
    /// Previous directory for `goto previous` (g-p)
    pub prev_dir: Option<PathBuf>,
}

impl PaneState {
    pub fn new(id: PaneId, path: PathBuf) -> Self {
        let mut pane = PaneState {
            id,
            cwd: path.clone(),
            entries: Vec::new(),
            cursor: 0,
            scroll_offset: 0,
            selected: Vec::new(),
            sort_by: SortBy::Name,
            sort_dir: SortDirection::Ascending,
            show_hidden: false,
            filter: None,
            prev_dir: None,
        };
        let _ = pane.refresh();
        pane
    }

    /// Re-read directory contents, apply filter/sort.
    pub fn refresh(&mut self) -> Result<()> {
        self.entries.clear();

        let read_dir = fs::read_dir(&self.cwd).map_err(NcError::Io)?;

        for entry in read_dir.flatten() {
            let path = entry.path();
            if let Ok(fe) = FileEntry::from_path(&path) {
                // Filter hidden files
                if !self.show_hidden && fe.is_hidden {
                    continue;
                }
                // Apply text filter
                if let Some(ref f) = self.filter {
                    let pattern = f.to_lowercase();
                    if !fe.name.to_lowercase().contains(&pattern) {
                        continue;
                    }
                }
                self.entries.push(fe);
            }
        }

        self.sort_entries();

        // Reset selection vector
        self.selected = vec![false; self.entries.len()];

        // Clamp cursor
        if self.entries.is_empty() {
            self.cursor = 0;
        } else if self.cursor >= self.entries.len() {
            self.cursor = self.entries.len() - 1;
        }

        Ok(())
    }

    fn sort_entries(&mut self) {
        let sort_by = self.sort_by;
        let ascending = self.sort_dir == SortDirection::Ascending;

        self.entries.sort_by(|a, b| {
            // Directories always come first
            match (a.is_dir, b.is_dir) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                _ => {}
            }

            let cmp = match sort_by {
                SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortBy::Size => a.size.cmp(&b.size),
                SortBy::Date => a.modified.cmp(&b.modified),
                SortBy::Extension => {
                    let ext_cmp = a.extension().to_lowercase().cmp(&b.extension().to_lowercase());
                    if ext_cmp == Ordering::Equal {
                        a.name.to_lowercase().cmp(&b.name.to_lowercase())
                    } else {
                        ext_cmp
                    }
                }
            };

            if ascending { cmp } else { cmp.reverse() }
        });
    }

    /// Current entry under cursor, if any.
    pub fn current_entry(&self) -> Option<&FileEntry> {
        self.entries.get(self.cursor)
    }

    /// Number of selected entries.
    pub fn selection_count(&self) -> usize {
        self.selected.iter().filter(|&&s| s).count()
    }

    /// Get paths of all selected entries, or current entry if none selected.
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        let selected: Vec<PathBuf> = self
            .selected
            .iter()
            .enumerate()
            .filter(|(_, s)| **s)
            .filter_map(|(i, _)| self.entries.get(i))
            .map(|e| e.path.clone())
            .collect();

        if selected.is_empty() {
            if let Some(entry) = self.current_entry() {
                vec![entry.path.clone()]
            } else {
                vec![]
            }
        } else {
            selected
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_pane_state_new() {
        let tmp = std::env::temp_dir().join("ncoxide_test_pane");
        let _ = fs::create_dir_all(&tmp);
        fs::write(tmp.join("file_a.txt"), "hello").unwrap();
        fs::write(tmp.join("file_b.rs"), "world").unwrap();
        fs::create_dir_all(tmp.join("subdir")).unwrap();

        let pane = PaneState::new(PaneId::Left, tmp.clone());
        assert!(!pane.entries.is_empty());
        // Directories should come first
        assert!(pane.entries[0].is_dir);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_sort_by_name() {
        let tmp = std::env::temp_dir().join("ncoxide_test_sort");
        let _ = fs::create_dir_all(&tmp);
        fs::write(tmp.join("zeta.txt"), "").unwrap();
        fs::write(tmp.join("alpha.txt"), "").unwrap();
        fs::write(tmp.join("middle.txt"), "").unwrap();

        let pane = PaneState::new(PaneId::Left, tmp.clone());
        // All are files, sorted alphabetically
        let names: Vec<&str> = pane.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha.txt", "middle.txt", "zeta.txt"]);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_hidden_files_filtered() {
        let tmp = std::env::temp_dir().join("ncoxide_test_hidden");
        let _ = fs::create_dir_all(&tmp);
        fs::write(tmp.join(".hidden"), "").unwrap();
        fs::write(tmp.join("visible.txt"), "").unwrap();

        let pane = PaneState::new(PaneId::Left, tmp.clone());
        assert_eq!(pane.entries.len(), 1);
        assert_eq!(pane.entries[0].name, "visible.txt");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_selected_paths_default() {
        let tmp = std::env::temp_dir().join("ncoxide_test_sel");
        let _ = fs::create_dir_all(&tmp);
        fs::write(tmp.join("file.txt"), "").unwrap();

        let pane = PaneState::new(PaneId::Left, tmp.clone());
        let paths = pane.selected_paths();
        assert_eq!(paths.len(), 1);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_sort_by_parse() {
        assert_eq!(SortBy::parse("name"), Some(SortBy::Name));
        assert_eq!(SortBy::parse("size"), Some(SortBy::Size));
        assert_eq!(SortBy::parse("date"), Some(SortBy::Date));
        assert_eq!(SortBy::parse("ext"), Some(SortBy::Extension));
        assert_eq!(SortBy::parse("extension"), Some(SortBy::Extension));
        assert_eq!(SortBy::parse("bogus"), None);
    }
}
