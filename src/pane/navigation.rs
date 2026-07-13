use super::PaneState;
use crate::error::Result;

impl PaneState {
    /// Switch to `new_cwd` and re-read the listing. On failure (unreadable or
    /// vanished directory) the pane is restored to its previous directory and
    /// the error is returned for the caller to surface — previously these
    /// failures were swallowed and looked like an empty directory (R11).
    fn change_dir(&mut self, new_cwd: std::path::PathBuf) -> Result<()> {
        let old_cwd = std::mem::replace(&mut self.cwd, new_cwd);
        let old_cursor = self.cursor;
        let old_scroll = self.scroll_offset;
        self.cursor = 0;
        self.scroll_offset = 0;
        match self.refresh() {
            Ok(()) => {
                self.prev_dir = Some(old_cwd);
                Ok(())
            }
            Err(e) => {
                self.cwd = old_cwd;
                self.cursor = old_cursor;
                self.scroll_offset = old_scroll;
                let _ = self.refresh(); // best-effort: restore the old listing
                Err(e)
            }
        }
    }
    pub fn cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn cursor_down(&mut self) {
        if !self.entries.is_empty() && self.cursor < self.entries.len() - 1 {
            self.cursor += 1;
        }
    }

    pub fn cursor_top(&mut self) {
        self.cursor = 0;
    }

    pub fn cursor_bottom(&mut self) {
        if !self.entries.is_empty() {
            self.cursor = self.entries.len() - 1;
        }
    }

    pub fn page_up(&mut self, page_size: usize) {
        self.cursor = self.cursor.saturating_sub(page_size);
    }

    pub fn page_down(&mut self, page_size: usize) {
        if !self.entries.is_empty() {
            self.cursor = (self.cursor + page_size).min(self.entries.len() - 1);
        }
    }

    pub fn half_page_up(&mut self, page_size: usize) {
        self.page_up(page_size / 2);
    }

    pub fn half_page_down(&mut self, page_size: usize) {
        self.page_down(page_size / 2);
    }

    /// Enter directory at cursor, or return path for file open.
    /// Returns `Ok(Some(path))` if it's a file (caller decides what to do).
    pub fn enter(&mut self) -> Result<Option<std::path::PathBuf>> {
        let Some(entry) = self.entries.get(self.cursor).cloned() else {
            return Ok(None);
        };
        if entry.is_dir {
            self.change_dir(entry.path)?;
            Ok(None)
        } else {
            Ok(Some(entry.path))
        }
    }

    /// Go to parent directory.
    pub fn go_parent(&mut self) -> Result<()> {
        let Some(parent) = self.cwd.parent().map(|p| p.to_path_buf()) else {
            return Ok(());
        };
        let old_name = self
            .cwd
            .file_name()
            .map(|n| n.to_string_lossy().to_string());
        self.change_dir(parent)?;
        // Try to position cursor on the directory we came from
        if let Some(name) = old_name
            && let Some(pos) = self.entries.iter().position(|e| e.name == name)
        {
            self.cursor = pos;
        }
        Ok(())
    }

    /// Navigate to an arbitrary path. A non-directory target errors like an
    /// unreadable one (previously it was silently ignored).
    pub fn goto(&mut self, path: std::path::PathBuf) -> Result<()> {
        self.change_dir(path)
    }

    /// Ensure scroll_offset keeps cursor visible.
    pub fn adjust_scroll(&mut self, visible_rows: usize) {
        if visible_rows == 0 {
            return;
        }
        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        }
        if self.cursor >= self.scroll_offset + visible_rows {
            self.scroll_offset = self.cursor - visible_rows + 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::PaneId;
    use std::fs;

    fn make_test_dir(name: &str) -> std::path::PathBuf {
        let tmp = std::env::temp_dir().join(format!("ncoxide_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        tmp
    }

    #[test]
    fn test_cursor_movement() {
        let tmp = make_test_dir("cursor");
        fs::create_dir_all(tmp.join("adir")).unwrap();
        fs::write(tmp.join("bfile.txt"), "hello").unwrap();
        fs::write(tmp.join("cfile.rs"), "world").unwrap();
        let mut pane = PaneState::new(PaneId::Left, tmp.clone());
        assert_eq!(
            pane.entries.len(),
            3,
            "expected 3 entries: {:?}",
            pane.entries.iter().map(|e| &e.name).collect::<Vec<_>>()
        );
        assert_eq!(pane.cursor, 0);
        pane.cursor_down();
        assert_eq!(pane.cursor, 1);
        pane.cursor_down();
        assert_eq!(pane.cursor, 2);
        pane.cursor_down(); // at end, should stay
        assert_eq!(pane.cursor, 2);
        pane.cursor_up();
        assert_eq!(pane.cursor, 1);
        pane.cursor_top();
        assert_eq!(pane.cursor, 0);
        pane.cursor_bottom();
        assert_eq!(pane.cursor, 2);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_enter_and_parent() {
        let tmp = make_test_dir("enter_parent");
        fs::create_dir_all(tmp.join("adir")).unwrap();
        fs::write(tmp.join("bfile.txt"), "hello").unwrap();
        fs::write(tmp.join("cfile.rs"), "world").unwrap();
        let mut pane = PaneState::new(PaneId::Left, tmp.clone());
        // First entry should be the directory (sorted first)
        assert!(pane.entries[0].is_dir);
        let result = pane.enter().unwrap();
        assert!(result.is_none()); // entered a directory
        assert!(pane.cwd.ends_with("adir"));

        pane.go_parent().unwrap();
        assert_eq!(pane.cwd, tmp);
        // Cursor should be on "adir"
        assert_eq!(pane.entries[pane.cursor].name, "adir");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_enter_file() {
        let tmp = make_test_dir("enter_file");
        fs::create_dir_all(tmp.join("adir")).unwrap();
        fs::write(tmp.join("bfile.txt"), "hello").unwrap();
        fs::write(tmp.join("cfile.rs"), "world").unwrap();
        let mut pane = PaneState::new(PaneId::Left, tmp.clone());
        // Move to a file entry
        pane.cursor = 1; // bfile.txt
        let result = pane.enter().unwrap();
        assert!(result.is_some());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_goto_unreadable_dir_errors_and_reverts() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = make_test_dir("nav_unreadable");
        fs::write(tmp.join("visible.txt"), "x").unwrap();
        let locked = tmp.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let mut pane = PaneState::new(PaneId::Left, tmp.clone());
        let before_prev = pane.prev_dir.clone();

        assert!(pane.goto(locked.clone()).is_err());
        assert_eq!(pane.cwd, tmp, "failed goto must keep the old cwd");
        assert!(
            pane.entries.iter().any(|e| e.name == "visible.txt"),
            "old listing restored"
        );
        assert_eq!(pane.prev_dir, before_prev, "prev_dir untouched on failure");

        // Non-directory target errors instead of being silently ignored.
        assert!(pane.goto(tmp.join("visible.txt")).is_err());
        assert_eq!(pane.cwd, tmp);

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        let _ = fs::remove_dir_all(&tmp);
    }
}
