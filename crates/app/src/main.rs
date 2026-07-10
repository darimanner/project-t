//! Thin binary: GPUI window hosting the terminal pane.

mod keys;
mod terminal_view;

use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};

use terminal_view::TerminalView;

fn main() {
    gpui::Application::new().run(|cx: &mut App| {
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
