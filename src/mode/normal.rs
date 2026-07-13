use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{Action, InputKind};

/// Tracks multi-key sequences like gg.
#[derive(Debug, Default)]
pub struct NormalState {
    /// True if the previous key was 'g' (waiting for second key).
    pub pending_g: bool,
}

pub fn handle_key(key: KeyEvent, state: &mut NormalState) -> Action {
    // If we have a pending 'g', handle the second key
    if state.pending_g {
        state.pending_g = false;
        return match key.code {
            KeyCode::Char('g') => Action::CursorTop,    // gg → top
            KeyCode::Char('e') => Action::CursorBottom, // ge → end
            KeyCode::Char('h') => Action::GotoHome,
            KeyCode::Char('r') => Action::GotoRoot,
            KeyCode::Char('o') => Action::GotoOtherPane,
            KeyCode::Char('p') => Action::GotoPrevious,
            KeyCode::Char(c @ '1'..='9') => Action::GotoBookmark((c as u8 - b'1') as usize),
            _ => Action::None,
        };
    }

    match key.code {
        // Quit
        KeyCode::Char('q') => Action::Quit,

        // Navigation
        KeyCode::Char('j') | KeyCode::Down => Action::CursorDown,
        KeyCode::Char('k') | KeyCode::Up => Action::CursorUp,
        KeyCode::Char('h') | KeyCode::Backspace => Action::ParentDir,
        KeyCode::Char('l') | KeyCode::Enter => Action::EnterDir,
        KeyCode::Char('G') => Action::CursorBottom,
        KeyCode::Char('g') => {
            state.pending_g = true;
            Action::None
        }
        KeyCode::PageUp => Action::PageUp,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::HalfPageDown,
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::HalfPageUp,

        // Pane switching
        KeyCode::Tab => Action::SwitchPane,
        KeyCode::Left => Action::FocusLeftPane,
        KeyCode::Right => Action::FocusRightPane,

        // Mode transitions
        KeyCode::Char('v') => Action::EnterSelect,
        KeyCode::Char(' ') => Action::EnterSpace,
        KeyCode::Char(':') => Action::EnterCommand,
        KeyCode::Char('/') => Action::EnterInput(InputKind::Search),

        // Quick actions
        KeyCode::Char('r') => Action::EnterInput(InputKind::Rename),
        KeyCode::Char('d') => Action::DeleteSelected,
        KeyCode::Char('y') => Action::CopyToOther,
        KeyCode::Char('.') => Action::ToggleHidden,
        KeyCode::Char('p') | KeyCode::Char('P') => Action::TogglePreview,
        KeyCode::Char('?') => Action::ShowHelp,

        // Shift+J/K: move + select
        KeyCode::Char('J') => Action::SelectExtendDown,
        KeyCode::Char('K') => Action::SelectExtendUp,

        KeyCode::Esc => Action::ExitToNormal,

        _ => Action::None,
    }
}
