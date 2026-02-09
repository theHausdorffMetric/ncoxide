use crossterm::event::{KeyCode, KeyEvent};

use super::Action;

pub fn handle_key(key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::ExitToNormal,
        KeyCode::Enter => Action::InputConfirm,
        KeyCode::Backspace => Action::InputBackspace,
        KeyCode::Char(c) => Action::InputChar(c),
        _ => Action::None,
    }
}

/// Parse and return an action for a command string.
pub fn execute_command(cmd: &str) -> Action {
    let parts: Vec<&str> = cmd.trim().splitn(2, ' ').collect();
    match parts.first().copied() {
        Some("q" | "quit") => Action::Quit,
        Some("sort") => match parts.get(1).copied() {
            Some("name") => Action::SetSort(crate::pane::SortBy::Name),
            Some("size") => Action::SetSort(crate::pane::SortBy::Size),
            Some("date") => Action::SetSort(crate::pane::SortBy::Date),
            Some("ext") => Action::SetSort(crate::pane::SortBy::Extension),
            _ => Action::None,
        },
        Some("filter") => {
            let pattern = parts.get(1).map(|s| s.to_string());
            Action::SetFilter(pattern)
        }
        Some("cd") => {
            if let Some(path) = parts.get(1) {
                Action::ExecuteCommand(format!("cd {path}"))
            } else {
                Action::GotoHome
            }
        }
        Some("set") => match parts.get(1).copied() {
            Some("show_hidden") => Action::ToggleHidden,
            _ => Action::None,
        },
        _ => Action::None,
    }
}
