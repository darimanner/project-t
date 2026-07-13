//! Thin binary: GPUI window hosting the terminal pane.

mod host_form;
mod keys;
mod palette;
mod pane;
mod session;
mod terminal_element;
mod terminal_view;
mod text_input;
mod workspace;

use std::borrow::Cow;

use gpui::{App, AppContext, Bounds, KeyBinding, WindowBounds, WindowOptions, px, size};

use workspace::{
    CloseTab, NewTab, NextTab, PrevTab, SplitDown, SplitRight, TogglePalette, Workspace,
};

/// Bundled terminal font (SIL OFL 1.1; see assets/fonts/OFL.txt). Registering it
/// makes the family name resolvable via `gpui::font(FONT_FAMILY)`.
const FONT_REGULAR: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf");

fn main() {
    gpui::Application::new().run(|cx: &mut App| {
        cx.text_system()
            .add_fonts(vec![Cow::Borrowed(FONT_REGULAR), Cow::Borrowed(FONT_BOLD)])
            .expect("register bundled fonts");

        cx.bind_keys([
            KeyBinding::new("cmd-t", NewTab, None),
            KeyBinding::new("cmd-w", CloseTab, None),
            // On macOS gpui folds Shift into the character and drops the shift
            // modifier, so Cmd+Shift+] arrives as `cmd-}` — bind the glyph.
            KeyBinding::new("cmd-}", NextTab, None),
            KeyBinding::new("cmd-{", PrevTab, None),
            KeyBinding::new("cmd-k", TogglePalette, None),
            KeyBinding::new("cmd-d", SplitRight, None),
            KeyBinding::new("cmd-shift-d", SplitDown, None),
        ]);

        let bounds = Bounds::centered(None, size(px(960.), px(640.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
