//! Thin binary: GPUI window hosting the terminal pane.

mod keys;
mod terminal_element;
mod terminal_view;

use std::borrow::Cow;

use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};

use terminal_view::TerminalView;

/// Bundled terminal font (SIL OFL 1.1; see assets/fonts/OFL.txt). Registering it
/// makes the family name resolvable via `gpui::font(FONT_FAMILY)`.
const FONT_REGULAR: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
const FONT_BOLD: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf");

fn main() {
    gpui::Application::new().run(|cx: &mut App| {
        cx.text_system()
            .add_fonts(vec![Cow::Borrowed(FONT_REGULAR), Cow::Borrowed(FONT_BOLD)])
            .expect("register bundled fonts");

        let bounds = Bounds::centered(None, size(px(960.), px(640.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| TerminalView::new(window, cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
