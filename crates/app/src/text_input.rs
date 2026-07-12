//! Minimal single-line text input for forms and the palette.
//!
//! gpui 0.2.2 ships no input widget, so this is ours. Editing state is a pure
//! struct ([`InputState`]) so it is unit-testable; [`render_input`] draws it as
//! a bordered line with placeholder, masking, and an end-of-line cursor.
//!
//! `ponytail: no selection, no IME, no mid-line cursor — insert at end and
//! backspace only. Upgrade path: track a cursor index in InputState and handle
//! left/right/home/end when a real need appears.`

use gpui::{Div, Keystroke, ParentElement, Styled, div, px, rgb};

/// Pure editing state for a single-line input.
#[derive(Default)]
pub struct InputState {
    text: String,
    /// Render as bullets (password fields).
    pub masked: bool,
}

impl InputState {
    /// A password-style input rendering bullets.
    pub fn new_masked() -> Self {
        Self {
            masked: true,
            ..Default::default()
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }

    /// Applies a keystroke. Returns true if the text changed (caller repaints).
    /// Enter/Escape/Tab are the parent's business and are ignored here.
    pub fn on_key(&mut self, keystroke: &Keystroke) -> bool {
        if keystroke.modifiers.platform || keystroke.modifiers.control {
            return false;
        }
        match keystroke.key.as_str() {
            "backspace" => self.text.pop().is_some(),
            "space" => {
                self.text.push(' ');
                true
            }
            _ => {
                if let Some(ch) = &keystroke.key_char
                    && !ch.is_empty()
                    && !ch.chars().any(char::is_control)
                {
                    self.text.push_str(ch);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// What the widget displays: bullets when masked, otherwise the text.
    fn display(&self) -> String {
        if self.masked {
            "\u{2022}".repeat(self.text.chars().count())
        } else {
            self.text.clone()
        }
    }
}

/// Draws one input line. `focused` controls the border highlight and cursor.
pub fn render_input(state: &InputState, placeholder: &str, focused: bool) -> Div {
    let display = state.display();
    let empty = display.is_empty();

    let mut line = div()
        .flex()
        .flex_row()
        .items_center()
        .px_2()
        .h(px(28.))
        .rounded_sm()
        .border_1()
        .border_color(if focused {
            rgb(0x5588cc)
        } else {
            rgb(0x33333c)
        })
        .bg(rgb(0x1a1a20))
        .text_color(if empty { rgb(0x666666) } else { rgb(0xd8d8d8) })
        .child(if empty {
            placeholder.to_owned()
        } else {
            display
        });

    if focused {
        line = line.child(div().w(px(7.)).h(px(16.)).ml_px().bg(rgb(0xd8d8d8)));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Modifiers;

    fn key(key: &str, key_char: Option<&str>) -> Keystroke {
        Keystroke {
            modifiers: Modifiers::default(),
            key: key.to_owned(),
            key_char: key_char.map(str::to_owned),
        }
    }

    #[test]
    fn typing_appends_chars() {
        let mut s = InputState::default();
        assert!(s.on_key(&key("a", Some("a"))));
        assert!(s.on_key(&key("b", Some("b"))));
        assert_eq!(s.text(), "ab");
    }

    #[test]
    fn space_and_backspace() {
        let mut s = InputState::default();
        s.on_key(&key("a", Some("a")));
        s.on_key(&key("space", Some(" ")));
        s.on_key(&key("b", Some("b")));
        assert_eq!(s.text(), "a b");
        assert!(s.on_key(&key("backspace", None)));
        assert_eq!(s.text(), "a ");
    }

    #[test]
    fn backspace_on_empty_reports_no_change() {
        let mut s = InputState::default();
        assert!(!s.on_key(&key("backspace", None)));
    }

    #[test]
    fn named_and_modified_keys_are_ignored() {
        let mut s = InputState::default();
        assert!(!s.on_key(&key("enter", Some("\n"))));
        assert!(!s.on_key(&key("escape", None)));
        let mut cmd = key("a", Some("a"));
        cmd.modifiers.platform = true;
        assert!(!s.on_key(&cmd));
        assert_eq!(s.text(), "");
    }

    #[test]
    fn masked_display_uses_bullets() {
        let mut s = InputState {
            masked: true,
            ..Default::default()
        };
        s.on_key(&key("a", Some("a")));
        s.on_key(&key("b", Some("b")));
        assert_eq!(s.display(), "\u{2022}\u{2022}");
        assert_eq!(s.text(), "ab"); // real value intact
    }
}
