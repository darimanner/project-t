//! Translates GPUI keystrokes into terminal input bytes.

use gpui::{Keystroke, Modifiers};

/// Encodes a keystroke as the byte sequence a terminal application expects.
/// Returns `None` for keys that produce no input.
pub fn encode(keystroke: &Keystroke) -> Option<Vec<u8>> {
    let m = &keystroke.modifiers;

    // Ctrl+letter -> C0 control byte (Ctrl-C = 0x03, etc.).
    if m.control && keystroke.key.len() == 1 {
        let c = keystroke.key.chars().next()?;
        if c.is_ascii_alphabetic() {
            return Some(vec![(c.to_ascii_uppercase() as u8) & 0x1f]);
        }
    }

    let named: &[u8] = match keystroke.key.as_str() {
        "enter" => b"\r",
        "backspace" => b"\x7f",
        "tab" => b"\t",
        "escape" => b"\x1b",
        "space" => b" ",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "pageup" => b"\x1b[5~",
        "pagedown" => b"\x1b[6~",
        "delete" => b"\x1b[3~",
        "insert" => b"\x1b[2~",
        // Not a named key: fall back to the typed character.
        _ => return keystroke.key_char.as_ref().map(|s| s.as_bytes().to_vec()),
    };
    Some(named.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ks(key: &str, key_char: Option<&str>, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.to_owned(),
            key_char: key_char.map(|s| s.to_owned()),
        }
    }

    fn plain() -> Modifiers {
        Modifiers::default()
    }

    fn ctrl() -> Modifiers {
        Modifiers {
            control: true,
            ..Modifiers::default()
        }
    }

    #[test]
    fn printable_char() {
        assert_eq!(encode(&ks("a", Some("a"), plain())), Some(b"a".to_vec()));
    }

    #[test]
    fn ctrl_c_is_etx() {
        assert_eq!(encode(&ks("c", Some("c"), ctrl())), Some(vec![0x03]));
    }

    #[test]
    fn enter_is_cr() {
        assert_eq!(encode(&ks("enter", None, plain())), Some(b"\r".to_vec()));
    }

    #[test]
    fn backspace_is_del() {
        assert_eq!(
            encode(&ks("backspace", None, plain())),
            Some(b"\x7f".to_vec())
        );
    }

    #[test]
    fn arrow_left_is_csi_d() {
        assert_eq!(encode(&ks("left", None, plain())), Some(b"\x1b[D".to_vec()));
    }

    #[test]
    fn escape_is_esc() {
        assert_eq!(encode(&ks("escape", None, plain())), Some(b"\x1b".to_vec()));
    }
}
