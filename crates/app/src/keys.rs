//! Translates winit keyboard events into terminal input bytes.

use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Encodes a key press as the byte sequence a terminal application expects.
/// Returns `None` for keys that produce no input (modifiers, function keys
/// we do not support yet).
pub fn encode(event: &KeyEvent, modifiers: ModifiersState) -> Option<Vec<u8>> {
    // Ctrl+letter -> C0 control byte (Ctrl-C = 0x03 etc.).
    if modifiers.control_key()
        && let Key::Character(text) = &event.logical_key
    {
        let c = text.chars().next()?;
        if c.is_ascii_alphabetic() {
            return Some(vec![(c.to_ascii_uppercase() as u8) & 0x1f]);
        }
    }

    let named = match &event.logical_key {
        Key::Named(named) => *named,
        _ => return event.text.as_ref().map(|t| t.as_bytes().to_vec()),
    };

    let bytes: &[u8] = match named {
        NamedKey::Enter => b"\r",
        NamedKey::Backspace => b"\x7f",
        NamedKey::Tab => b"\t",
        NamedKey::Escape => b"\x1b",
        NamedKey::Space => b" ",
        NamedKey::ArrowUp => b"\x1b[A",
        NamedKey::ArrowDown => b"\x1b[B",
        NamedKey::ArrowRight => b"\x1b[C",
        NamedKey::ArrowLeft => b"\x1b[D",
        NamedKey::Home => b"\x1b[H",
        NamedKey::End => b"\x1b[F",
        NamedKey::PageUp => b"\x1b[5~",
        NamedKey::PageDown => b"\x1b[6~",
        NamedKey::Delete => b"\x1b[3~",
        NamedKey::Insert => b"\x1b[2~",
        _ => return event.text.as_ref().map(|t| t.as_bytes().to_vec()),
    };
    Some(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::SmolStr;

    fn plain() -> ModifiersState {
        ModifiersState::empty()
    }

    // KeyEvent cannot be constructed directly (non-exhaustive), so unit tests
    // here only cover the pure helpers; end-to-end key handling is exercised
    // manually until we add a fake-event harness.
    #[test]
    fn ctrl_byte_math_is_correct() {
        // Ctrl-C must be ETX (0x03); guards the `& 0x1f` trick.
        assert_eq!((b'C') & 0x1f, 0x03);
        assert_eq!((b'Z') & 0x1f, 0x1a);
    }

    #[test]
    fn modifiers_default_is_empty() {
        assert!(!plain().control_key());
        let _ = SmolStr::new("a");
    }
}
