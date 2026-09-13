use super::PaneState;

impl PaneState {
    /// Toggle selection on current entry.
    pub fn toggle_select(&mut self) {
        if let Some(sel) = self.selected.get_mut(self.cursor) {
            *sel = !*sel;
        }
    }

    /// Select all visible entries.
    pub fn select_all(&mut self) {
        self.selected.fill(true);
    }

    /// Deselect all.
    pub fn deselect_all(&mut self) {
        self.selected.fill(false);
    }

    /// Invert selection.
    pub fn invert_selection(&mut self) {
        for s in &mut self.selected {
            *s = !*s;
        }
    }

    /// Move cursor down and select (extend selection).
    pub fn select_extend_down(&mut self) {
        self.toggle_select();
        self.cursor_down();
    }

    /// Move cursor up and select (extend selection).
    pub fn select_extend_up(&mut self) {
        self.toggle_select();
        self.cursor_up();
    }

    /// Select entries matching a glob pattern.
    pub fn select_by_glob(&mut self, pattern: &str) {
        for (i, entry) in self.entries.iter().enumerate() {
            if glob_match(pattern, &entry.name)
                && let Some(s) = self.selected.get_mut(i)
            {
                *s = true;
            }
        }
    }
}

/// Simple glob matching: supports * and ? wildcards.
fn glob_match(pattern: &str, name: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    glob_match_inner(&pat, &name)
}

fn glob_match_inner(pat: &[char], name: &[char]) -> bool {
    match (pat.first(), name.first()) {
        (None, None) => true,
        (Some('*'), _) => {
            // Try matching zero chars, or consume one char from name
            glob_match_inner(&pat[1..], name)
                || (!name.is_empty() && glob_match_inner(pat, &name[1..]))
        }
        (Some('?'), Some(_)) => glob_match_inner(&pat[1..], &name[1..]),
        (Some(a), Some(b)) if a == b => glob_match_inner(&pat[1..], &name[1..]),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::{PaneId, PaneState};
    use std::fs;

    #[test]
    fn test_glob_match() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.rs", "lib.rs"));
        assert!(!glob_match("*.rs", "main.txt"));
        assert!(glob_match("test_*", "test_foo"));
        assert!(glob_match("?oo", "foo"));
        assert!(!glob_match("?oo", "foobar"));
        assert!(glob_match("*", "anything"));
    }

    #[test]
    fn test_selection_ops() {
        let tmp = std::env::temp_dir().join(format!("ncoxide_test_sel_ops_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("a.txt"), "").unwrap();
        fs::write(tmp.join("b.rs"), "").unwrap();
        fs::write(tmp.join("c.txt"), "").unwrap();

        let mut pane = PaneState::new(PaneId::Left, tmp.clone());
        assert_eq!(pane.selection_count(), 0);

        pane.select_all();
        assert_eq!(pane.selection_count(), 3);

        pane.invert_selection();
        assert_eq!(pane.selection_count(), 0);

        pane.select_by_glob("*.txt");
        assert_eq!(pane.selection_count(), 2);

        pane.deselect_all();
        assert_eq!(pane.selection_count(), 0);

        let _ = fs::remove_dir_all(&tmp);
    }
}
