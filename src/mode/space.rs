use crossterm::event::{KeyCode, KeyEvent};

use super::{Action, InputKind};

pub fn handle_key(key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::ExitToNormal,

        KeyCode::Char('c') => Action::CopyToOther,
        KeyCode::Char('m') => Action::MoveToOther,
        KeyCode::Char('d') => Action::DeleteSelected,
        KeyCode::Char('r') => Action::EnterInput(InputKind::Rename),
        KeyCode::Char('n') => Action::EnterInput(InputKind::Mkdir),
        KeyCode::Char('e') => Action::EditFile,
        KeyCode::Char('v') => Action::ViewFile,
        KeyCode::Char('s') => Action::EnterInput(InputKind::CommandLine), // sort submenu via command
        KeyCode::Char('f') => Action::EnterFinder,
        KeyCode::Char('i') => Action::ShowFileInfo,
        KeyCode::Char('?') => Action::ShowHelp,

        _ => Action::ExitToNormal,
    }
}
