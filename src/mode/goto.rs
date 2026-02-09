use crossterm::event::{KeyCode, KeyEvent};

use super::Action;

pub fn handle_key(key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::ExitToNormal,

        KeyCode::Char('g') => Action::CursorTop,
        KeyCode::Char('e') => Action::CursorBottom,
        KeyCode::Char('h') => Action::GotoHome,
        KeyCode::Char('r') => Action::GotoRoot,
        KeyCode::Char('o') => Action::GotoOtherPane,
        KeyCode::Char('p') => Action::GotoPrevious,
        KeyCode::Char(c @ '1'..='9') => Action::GotoBookmark((c as u8 - b'1') as usize),

        _ => Action::ExitToNormal,
    }
}
