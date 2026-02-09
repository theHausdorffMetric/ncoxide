use crossterm::event::{KeyCode, KeyEvent};

use super::Action;

pub fn handle_key(key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::InputCancel,
        KeyCode::Enter => Action::InputConfirm,
        KeyCode::Backspace => Action::InputBackspace,
        KeyCode::Char(c) => Action::InputChar(c),
        _ => Action::None,
    }
}
