use crossterm::event::{KeyCode, KeyEvent};

use super::{Action, InputKind};

pub fn handle_key(key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::ExitToNormal,

        // Navigation (same as normal)
        KeyCode::Char('j') | KeyCode::Down => Action::SelectExtendDown,
        KeyCode::Char('k') | KeyCode::Up => Action::SelectExtendUp,

        // Selection commands
        KeyCode::Char('v') => Action::ToggleSelect,
        KeyCode::Char('a') => Action::SelectAll,
        KeyCode::Char('n') => Action::InvertSelection,
        KeyCode::Char(';') => Action::DeselectAll,
        KeyCode::Char('*') => Action::EnterInput(InputKind::GlobSelect),

        // Act on selection
        KeyCode::Char(' ') => Action::EnterSpace,
        KeyCode::Char('d') => Action::DeleteSelected,
        KeyCode::Char('y') => Action::CopyToOther,

        // Pane switching
        KeyCode::Tab => Action::SwitchPane,
        KeyCode::Left => Action::FocusLeftPane,
        KeyCode::Right => Action::FocusRightPane,

        _ => Action::None,
    }
}
