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

/// Handles `--export-hosts [path]` / `--import-hosts <path>` and returns true
/// when the process should exit instead of opening a window. Hosts travel as
/// secret-free JSON; passwords stay in the keychain and must be re-entered.
fn handle_cli(args: &[String]) -> bool {
    let open_store = || {
        let path = workspace::store_path().expect("resolve store path");
        core_store::Store::open(path).expect("open host store")
    };
    if let Some(i) = args.iter().position(|a| a == "--export-hosts") {
        let path = args
            .get(i + 1)
            .cloned()
            .unwrap_or_else(|| "hosts.json".to_owned());
        let json = open_store().export_hosts_json().expect("export hosts");
        std::fs::write(&path, json).expect("write export file");
        println!("hosts exported to {path}");
        return true;
    }
    if let Some(i) = args.iter().position(|a| a == "--import-hosts") {
        let Some(path) = args.get(i + 1) else {
            eprintln!("usage: app --import-hosts <path>");
            return true;
        };
        let json = std::fs::read_to_string(path).expect("read import file");
        let count = open_store().import_hosts_json(&json).expect("import hosts");
        println!("imported {count} host(s); passwords must be re-entered");
        return true;
    }
    false
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if handle_cli(&args) {
        return;
    }

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
