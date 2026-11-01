use crossterm::event::{KeyCode, KeyEvent};

use super::Action;

pub fn handle_key(key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::ExitToNormal,
        KeyCode::Enter => Action::InputConfirm,
        KeyCode::Backspace => Action::InputBackspace,
        KeyCode::Char(c) if super::accepts_text(&key) => Action::InputChar(c),
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
        // `:web` starts the compare page (or shows its URL again);
        // `:web stop` shuts the server down.
        Some("web") => match parts.get(1).map(|s| s.trim()) {
            None | Some("") => Action::OpenWeb,
            Some("stop" | "off" | "close") => Action::CloseWeb,
            _ => Action::None,
        },
        // `:split 30` puts the divider at 30 % (left half); `:split` alone
        // returns to the configured value. Anything else is a no-op.
        Some("split") => match parts.get(1).map(|s| s.trim()) {
            None | Some("") => Action::SetSplit(None),
            Some(n) => match n.parse::<u16>() {
                Ok(n)
                    if (crate::ui::layout::SPLIT_MIN..=crate::ui::layout::SPLIT_MAX)
                        .contains(&n) =>
                {
                    Action::SetSplit(Some(n))
                }
                _ => Action::None,
            },
        },
        _ => Action::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execute_command_parses_web_and_friends() {
        assert!(matches!(execute_command("web"), Action::OpenWeb));
        assert!(matches!(execute_command("  web  "), Action::OpenWeb));
        assert!(matches!(execute_command("web stop"), Action::CloseWeb));
        assert!(matches!(execute_command("web off"), Action::CloseWeb));
        assert!(matches!(execute_command("web whatever"), Action::None));
        assert!(matches!(execute_command("q"), Action::Quit));
        assert!(matches!(
            execute_command("sort size"),
            Action::SetSort(crate::pane::SortBy::Size)
        ));
        assert!(matches!(execute_command("nonsense"), Action::None));
    }

    #[test]
    fn test_execute_command_parses_split() {
        assert!(matches!(execute_command("split"), Action::SetSplit(None)));
        assert!(matches!(execute_command("split  "), Action::SetSplit(None)));
        assert!(matches!(
            execute_command("split 30"),
            Action::SetSplit(Some(30))
        ));
        assert!(matches!(
            execute_command("split 90"),
            Action::SetSplit(Some(90))
        ));
        assert!(matches!(execute_command("split 7"), Action::None));
        assert!(matches!(execute_command("split 95"), Action::None));
        assert!(matches!(execute_command("split abc"), Action::None));
        assert!(matches!(execute_command("split -5"), Action::None));
    }
}
