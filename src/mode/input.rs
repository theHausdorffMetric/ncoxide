use crossterm::event::{KeyCode, KeyEvent};

use super::Action;

pub fn handle_key(key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::InputCancel,
        KeyCode::Enter => Action::InputConfirm,
        KeyCode::Backspace => Action::InputBackspace,
        KeyCode::Char(c) if super::accepts_text(&key) => Action::InputChar(c),
        _ => Action::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn test_modified_chars_are_not_text() {
        let plain = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
        assert!(matches!(handle_key(plain), Action::InputChar('c')));

        let shifted = KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT);
        assert!(matches!(handle_key(shifted), Action::InputChar('C')));

        let ctrl = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(handle_key(ctrl), Action::None));

        let alt = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT);
        assert!(matches!(handle_key(alt), Action::None));
    }
}
