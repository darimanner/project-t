# GPUI Terminal Mini-Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the winit window with a GPUI window hosting a single terminal pane that renders the grid with GPUI-native primitives, reusing `core-vt`/`core-pty`.

**Architecture:** Core and PTY→VT→repaint pipeline are unchanged. `app` becomes a GPUI application: a focusable `TerminalView` entity owns the session and drives repaint; a custom `TerminalElement` reads the grid from `Vt` and paints glyph runs + background/cursor quads via GPUI's text system and `paint_quad`. Color/palette resolution moves into `core-vt` so both the retained glyphon renderer and the new GPUI backend share it.

**Tech Stack:** Rust 2024, `gpui = "0.2.2"` (crates.io), `alacritty_terminal` (via `core-vt`), `portable-pty` (via `core-pty`).

## Global Constraints

- Rust edition 2024; `cargo fmt` + `cargo clippy --workspace --all-targets` must be clean before each commit.
- Code, comments, commit messages: English only.
- Prefix every cargo/rustc command with `export PATH="$HOME/.cargo/bin:$PATH"` — the tool shell does not have cargo on PATH.
- `core-*` crates must not depend on `render` or `app`, and must not pull in GPU/UI dependencies. `app → core-*` only (no dep on `render` after this work).
- The `render` crate stays in the workspace and keeps building/testing; it is the future Android backend, not dead code.
- GPUI is pre-1.0. When a GPUI API differs from what this plan shows, trust the compiler and the vendored Zed reference over this document. Reference: Zed repo at commit `7344a31ddc144f12bd8ee52328cfb9bd275c97f5`, files `crates/gpui/examples/hello_world.rs`, `crates/gpui/src/text_system.rs`, `crates/terminal_view/src/terminal_element.rs`.

---

## File Structure

- `crates/core-vt/src/palette.rs` — **new**: pure color/palette resolution (moved from `render`).
- `crates/core-vt/src/lib.rs` — **modify**: declare and re-export `palette`.
- `crates/render/src/color.rs` — **modify**: re-export palette from `core-vt`, keep only `srgb8_to_linear`.
- `crates/app/Cargo.toml` — **modify**: drop `winit`/`render`, add `gpui`.
- `crates/app/src/main.rs` — **rewrite**: GPUI boot + window + root view.
- `crates/app/src/terminal_view.rs` — **new**: focusable entity owning `Vt`+`PtySession`, reader task, event handlers.
- `crates/app/src/terminal_element.rs` — **new**: custom GPUI `Element` painting the grid.
- `crates/app/src/keys.rs` — **rewrite**: encode `gpui::Keystroke` → terminal bytes (pure, unit-tested).

---

## Task 1: Move color/palette resolution into `core-vt`

**Files:**
- Create: `crates/core-vt/src/palette.rs`
- Modify: `crates/core-vt/src/lib.rs` (add `mod palette; pub use palette::*;` near the top, after the crate docs)
- Modify: `crates/render/src/color.rs` (re-export palette, keep `srgb8_to_linear`)
- Modify: `crates/render/src/lib.rs` (imports of `color::resolve`/`DEFAULT_*` continue to work via re-export)

**Interfaces:**
- Produces (from `core_vt`):
  - `pub const DEFAULT_FG: [u8; 3]`
  - `pub const DEFAULT_BG: [u8; 3]`
  - `pub fn resolve_color(color: alacritty_terminal::vte::ansi::Color, overrides: &alacritty_terminal::term::color::Colors) -> [u8; 3]`
- Consumes: the current logic in `crates/render/src/color.rs` (`resolve`, `indexed`, `named`, `BASE16`).

- [ ] **Step 1: Write the failing test**

Add to the bottom of the new `crates/core-vt/src/palette.rs` (create the file with just this test first so it fails to compile against missing items):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::term::color::Colors;
    use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

    #[test]
    fn named_red_resolves() {
        let overrides = Colors::default();
        assert_eq!(resolve_color(Color::Named(NamedColor::Red), &overrides), [0xcc, 0x66, 0x66]);
    }

    #[test]
    fn truecolor_passes_through() {
        let overrides = Colors::default();
        let rgb = Rgb { r: 1, g: 2, b: 3 };
        assert_eq!(resolve_color(Color::Spec(rgb), &overrides), [1, 2, 3]);
    }

    #[test]
    fn indexed_grayscale_ramp() {
        let overrides = Colors::default();
        // index 232 is the darkest gray step (8,8,8).
        assert_eq!(resolve_color(Color::Indexed(232), &overrides), [8, 8, 8]);
    }

    #[test]
    fn override_wins_over_default() {
        let mut overrides = Colors::default();
        overrides[NamedColor::Red as usize] = Some(Rgb { r: 10, g: 20, b: 30 });
        assert_eq!(resolve_color(Color::Named(NamedColor::Red), &overrides), [10, 20, 30]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p core-vt palette 2>&1 | tail -20`
Expected: FAIL — `resolve_color`, `DEFAULT_FG`, `DEFAULT_BG` not found.

- [ ] **Step 3: Implement the palette module**

Populate `crates/core-vt/src/palette.rs` above the test module by moving the body of `crates/render/src/color.rs` — everything except `srgb8_to_linear`. Rename the public entry point `resolve` → `resolve_color`. Concretely, the file contains: `DEFAULT_FG`, `DEFAULT_BG`, `BASE16`, `fn indexed`, `fn named`, and `pub fn resolve_color`. Use fully-qualified `alacritty_terminal` paths (this crate re-exports it):

```rust
//! Terminal color resolution: ANSI colors -> sRGB, honoring runtime palette
//! overrides (OSC 4/10/11). Pure terminal semantics, no GPU dependency.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

pub const DEFAULT_FG: [u8; 3] = [0xd8, 0xd8, 0xd8];
pub const DEFAULT_BG: [u8; 3] = [0x12, 0x12, 0x16];

// ... BASE16, indexed(), named() unchanged from render/src/color.rs ...

/// Resolves a cell color to sRGB, preferring runtime palette overrides.
pub fn resolve_color(color: Color, overrides: &Colors) -> [u8; 3] {
    // ... body of the old `resolve`, unchanged ...
}
```

- [ ] **Step 4: Declare the module in `core-vt`**

In `crates/core-vt/src/lib.rs`, directly after the `pub use alacritty_terminal;` line, add:

```rust
mod palette;
pub use palette::{DEFAULT_BG, DEFAULT_FG, resolve_color};
```

- [ ] **Step 5: Point `render` at the moved code**

Replace the moved parts of `crates/render/src/color.rs` so the file becomes only:

```rust
//! Renderer-specific color helpers. Palette resolution lives in `core_vt`.

pub use core_vt::{DEFAULT_BG, DEFAULT_FG, resolve_color as resolve};

/// Converts an sRGB byte to a linear float for writing into an sRGB target.
pub fn srgb8_to_linear(c: u8) -> f32 {
    let s = c as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}
```

`crates/render/src/lib.rs` already calls `color::resolve(...)`, `color::DEFAULT_FG`, `color::DEFAULT_BG`, `color::srgb8_to_linear(...)` — all still resolve via the re-export, no changes needed there.

- [ ] **Step 6: Run tests to verify they pass**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p core-vt -p render 2>&1 | grep -E "test result|error" | head`
Expected: PASS for both crates; the 4 new palette tests pass and render's build is intact.

- [ ] **Step 7: Lint and commit**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo fmt --all && cargo clippy --workspace --all-targets 2>&1 | grep -cE "^(warning|error)"
git add -A && git commit -m "Move color palette resolution into core-vt (shared by both renderers)"
```
Expected: clippy count `0`.

---

## Task 2: GPUI boot — empty window replacing winit

**Files:**
- Modify: `crates/app/Cargo.toml`
- Rewrite: `crates/app/src/main.rs`
- (Temporarily) the `mod keys;` line is removed until Task 3 re-adds it.

**Interfaces:**
- Produces: a runnable `app` binary that opens a GPUI window with a placeholder dark view and stays alive.

- [ ] **Step 1: Swap dependencies**

Edit `crates/app/Cargo.toml` `[dependencies]` to:

```toml
[dependencies]
core-pty = { version = "0.1.0", path = "../core-pty" }
core-vt = { version = "0.1.0", path = "../core-vt" }
gpui = "0.2.2"
```

(Remove `render` and `winit`.)

- [ ] **Step 2: Minimal GPUI window**

Rewrite `crates/app/src/main.rs` to a placeholder root view. Boot API per `gpui/examples/hello_world.rs` — verify the exact boot symbol for 0.2.2 by compiling (it is `Application::new().run(...)` or `application().run(...)`; use whichever 0.2.2 exports):

```rust
//! Thin binary: GPUI window hosting the terminal pane.

use gpui::{
    App, Bounds, Context, IntoElement, ParentElement, Render, Styled, Window, WindowBounds,
    WindowOptions, div, px, rgb, size,
};

struct Root;

impl Render for Root {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x121216))
            .text_color(rgb(0xd8d8d8))
            .child("project-t")
    }
}

fn main() {
    gpui::Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(960.), px(640.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Root),
        )
        .unwrap();
        cx.activate(true);
    });
}
```

- [ ] **Step 3: Build**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build -p app 2>&1 | tail -5`
Expected: compiles. Fix imports/boot symbol against 0.2.2 if the compiler disagrees (this is the task that pins the boot API). First build pulls the gpui tree and is slow.

- [ ] **Step 4: Smoke test**

Run:
```bash
export PATH="$HOME/.cargo/bin:$PATH" && (./target/debug/app >/tmp/app-smoke.log 2>&1 & echo $! >/tmp/app.pid); sleep 4; kill -0 $(cat /tmp/app.pid) && echo ALIVE && kill $(cat /tmp/app.pid) || (echo DIED; cat /tmp/app-smoke.log)
```
Expected: `ALIVE` (a dark window with "project-t" appears).

- [ ] **Step 5: Lint and commit**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo fmt --all && cargo clippy -p app --all-targets 2>&1 | grep -cE "^(warning|error)"
git add -A && git commit -m "Boot app on GPUI with a placeholder window (drop winit)"
```
Expected: clippy count `0`.

---

## Task 3: Key encoding against `gpui::Keystroke`

**Files:**
- Rewrite: `crates/app/src/keys.rs`
- Modify: `crates/app/src/main.rs` (re-add `mod keys;`)

**Interfaces:**
- Consumes: `gpui::Keystroke { modifiers: gpui::Modifiers, key: String, key_char: Option<String> }`. `gpui::Modifiers` has bool fields `control`, `alt`, `shift`, `platform`, `function`.
- Produces: `pub fn encode(keystroke: &gpui::Keystroke) -> Option<Vec<u8>>` — terminal input bytes, or `None` when the key produces no input.

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/keys.rs`:

```rust
//! Translates GPUI keystrokes into terminal input bytes.

use gpui::{Keystroke, Modifiers};

/// Encodes a keystroke as the byte sequence a terminal application expects.
/// Returns `None` for keys that produce no input.
pub fn encode(keystroke: &Keystroke) -> Option<Vec<u8>> {
    todo!()
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
        Modifiers { control: true, ..Modifiers::default() }
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
        assert_eq!(encode(&ks("backspace", None, plain())), Some(b"\x7f".to_vec()));
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p app keys 2>&1 | tail -15`
Expected: FAIL — `todo!()` panics.

- [ ] **Step 3: Implement `encode`**

Replace the `todo!()` body. `Keystroke.key` holds GPUI's normalized key name (lowercase: `"enter"`, `"backspace"`, `"tab"`, `"escape"`, `"space"`, `"up"`, `"down"`, `"left"`, `"right"`, `"home"`, `"end"`, `"pageup"`, `"pagedown"`, `"delete"`, `"insert"`, and single characters like `"a"`).

```rust
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
```

- [ ] **Step 4: Re-add the module**

In `crates/app/src/main.rs`, add `mod keys;` near the top (with a `#[allow(dead_code)]` above it if the compiler warns — it is consumed in Task 4).

- [ ] **Step 5: Run tests to verify they pass**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p app keys 2>&1 | grep -E "test result|FAILED"`
Expected: PASS (6 tests). Note: if GPUI's key name for a modifier or key differs from the strings above, adjust the match arms to GPUI's actual names (verify against `gpui/src/platform/keystroke.rs`).

- [ ] **Step 6: Lint and commit**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo fmt --all && cargo clippy -p app --all-targets 2>&1 | grep -cE "^(warning|error)"
git add -A && git commit -m "Encode GPUI keystrokes into terminal bytes"
```
Expected: clippy count `0`.

---

## Task 4: `TerminalView` entity — pipeline first light (plain-text rendering)

Goal of this task: end-to-end pipeline (shell output visible, typing works) using **simple GPUI text** (one `div` child per grid row via `Vt::line_text`). The custom GPU element comes in Task 5. This is the natural reviewer gate: "the terminal works, but is not yet the real renderer".

**Files:**
- Create: `crates/app/src/terminal_view.rs`
- Modify: `crates/app/src/main.rs` (mount `TerminalView` as the root instead of `Root`)
- Modify: `crates/core-vt/src/lib.rs` (add a `visible_lines` helper — see Interfaces)

**Interfaces:**
- Consumes: `core_pty::{PtySession, SpawnConfig, Size}`, `core_vt::{Vt, GridSize}`, `crate::keys`.
- Produces:
  - `pub struct TerminalView` (a GPUI entity via `cx.new(...)`), with `TerminalView::new(window: &mut Window, cx: &mut Context<Self>) -> Self`.
  - `core_vt::Vt::visible_lines(&self) -> Vec<String>` — the on-screen rows top-to-bottom, each trimmed, honoring `display_offset`.

- [ ] **Step 1: Add `visible_lines` to `core-vt` with a failing test**

In `crates/core-vt/src/lib.rs`, add a test to the existing `tests` module:

```rust
#[test]
fn visible_lines_reads_viewport() {
    let mut vt = Vt::new(GridSize { columns: 20, screen_lines: 3 }, 100);
    vt.advance(b"a\r\nb\r\nc");
    assert_eq!(vt.visible_lines(), vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]);
}
```

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p core-vt visible_lines 2>&1 | tail -8`
Expected: FAIL — `visible_lines` not found.

- [ ] **Step 2: Implement `visible_lines`**

The existing `line_text(row)` indexes the grid by absolute line and ignores scroll. `visible_lines` must honor the viewport. Add to `impl Vt`:

```rust
/// Visible rows of the current viewport, top-to-bottom, trailing whitespace
/// trimmed. Honors scrollback position (`display_offset`).
pub fn visible_lines(&self) -> Vec<String> {
    use alacritty_terminal::index::{Column, Line};
    let grid = self.term.grid();
    let offset = grid.display_offset() as i32;
    let rows = grid.screen_lines();
    let cols = grid.columns();
    (0..rows)
        .map(|row| {
            let line = Line(row as i32 - offset);
            let text: String = (0..cols).map(|c| grid[line][Column(c)].c).collect();
            text.trim_end().to_owned()
        })
        .collect()
}
```

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p core-vt visible_lines 2>&1 | grep "test result"`
Expected: PASS.

- [ ] **Step 3: Write `TerminalView` (session + reader task + plain-text render)**

Create `crates/app/src/terminal_view.rs`. This mirrors the winit `State`/reader-thread logic but as a GPUI entity. Reader task updates `Vt` on a background thread and wakes the UI via the entity handle + `cx.notify()`. Exact async/notify wiring is verified by compilation against gpui 0.2.2; the shape:

```rust
use std::io::Read;
use std::sync::{Arc, Mutex};

use core_pty::{PtySession, Size, SpawnConfig};
use core_vt::{GridSize, Vt};
use gpui::{
    Context, FocusHandle, Focusable, IntoElement, ParentElement, Render, Styled, Window, div, px,
    rgb,
};

use crate::keys;

const INITIAL_COLS: u16 = 100;
const INITIAL_ROWS: u16 = 30;

pub struct TerminalView {
    vt: Arc<Mutex<Vt>>,
    pty: Arc<Mutex<PtySession>>,
    focus: FocusHandle,
}

impl TerminalView {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let size = GridSize { columns: INITIAL_COLS as usize, screen_lines: INITIAL_ROWS as usize };
        let vt = Arc::new(Mutex::new(Vt::new(size, 10_000)));
        let pty = PtySession::spawn(&SpawnConfig::user_shell(Size {
            rows: INITIAL_ROWS,
            cols: INITIAL_COLS,
        }))
        .expect("spawn shell");
        let reader = pty.reader().expect("clone pty reader");
        let pty = Arc::new(Mutex::new(pty));

        Self::spawn_reader(reader, vt.clone(), pty.clone(), cx);

        Self { vt, pty, focus: cx.focus_handle() }
    }

    fn spawn_reader(
        mut reader: Box<dyn Read + Send>,
        vt: Arc<Mutex<Vt>>,
        pty: Arc<Mutex<PtySession>>,
        cx: &mut Context<Self>,
    ) {
        // Background OS thread reads PTY output; each chunk advances the VT,
        // flushes VT->PTY responses, then wakes the view on the UI thread.
        let handle = cx.entity();
        let async_cx = cx.to_async();
        std::thread::spawn(move || {
            let mut buf = [0u8; 65536];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let responses = {
                    let mut vt = vt.lock().expect("vt lock");
                    vt.advance(&buf[..n]);
                    vt.drain_pty_writes()
                };
                for r in responses {
                    let _ = pty.lock().expect("pty lock").write_all(r.as_bytes());
                }
                // Wake the UI: schedule a notify on the foreground executor.
                let _ = handle.update(&async_cx, |_, cx| cx.notify());
            }
        });
    }

    fn on_key(&mut self, keystroke: &gpui::Keystroke, cx: &mut Context<Self>) {
        if let Some(bytes) = keys::encode(keystroke) {
            self.vt.lock().expect("vt lock").scroll_to_bottom();
            let _ = self.pty.lock().expect("pty lock").write_all(&bytes);
        }
        cx.notify();
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lines = self.vt.lock().expect("vt lock").visible_lines();
        div()
            .track_focus(&self.focus)
            .key_context("Terminal")
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _window, cx| {
                this.on_key(&ev.keystroke, cx);
            }))
            .size_full()
            .bg(rgb(0x121216))
            .text_color(rgb(0xd8d8d8))
            .font_family("monospace")
            .text_size(px(14.))
            .flex()
            .flex_col()
            .children(lines.into_iter().map(|l| div().child(l)))
    }
}
```

- [ ] **Step 4: Mount `TerminalView` as the root**

In `crates/app/src/main.rs`: add `mod terminal_view;`, remove the `Root` struct and its `Render` impl, and change `open_window` to focus and build the terminal view:

```rust
|window, cx| cx.new(|cx| terminal_view::TerminalView::new(window, cx))
```

Ensure the window focuses the view on open (call `window.focus(&view.focus_handle(cx))` or focus within `TerminalView::new`; verify the focus API against gpui 0.2.2).

- [ ] **Step 5: Build and smoke test**

Run:
```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo build -p app 2>&1 | tail -3 && (./target/debug/app >/tmp/app-smoke.log 2>&1 & echo $! >/tmp/app.pid); sleep 4; kill -0 $(cat /tmp/app.pid) && echo ALIVE && kill $(cat /tmp/app.pid) || (echo DIED; cat /tmp/app-smoke.log)
```
Expected: `ALIVE`. Manual check (run `cargo run -p app`): the shell prompt appears, typing `ls` + Enter shows output. Rendering is plain (no colors/cursor yet) — that is expected at this gate.

- [ ] **Step 6: Lint and commit**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo fmt --all && cargo test -p core-vt 2>&1 | grep "test result" && cargo clippy -p app -p core-vt --all-targets 2>&1 | grep -cE "^(warning|error)"
git add -A && git commit -m "Wire PTY->VT->GPUI pipeline with plain-text first light"
```
Expected: core-vt tests pass, clippy count `0`.

---

## Task 5: `TerminalElement` — GPUI-native grid rendering

Replace the plain-text `div` children with a custom GPUI `Element` that paints background quads, glyph runs, and the block cursor, using resolved colors.

**Files:**
- Create: `crates/app/src/terminal_element.rs`
- Modify: `crates/app/src/terminal_view.rs` (render returns `TerminalElement` instead of text `div`s; expose grid accessors the element needs)
- Modify: `crates/core-vt/src/lib.rs` if the element needs a cell iterator (add `pub fn each_cell<F: FnMut(usize, usize, &Cell)>(&self, f: F)` or expose `term()` — `term()` already exists and returns `&Term`, which the element can walk via `renderable_content()`).

**Interfaces:**
- Consumes:
  - `core_vt::Vt::term()` → `&Term<..>`; `renderable_content()` yields `display_iter` (cells with `point`, `cell.c`, `cell.fg`, `cell.bg`, `cell.flags`), `cursor` (`point`, `shape`), `display_offset`, `colors`.
  - `core_vt::{resolve_color, DEFAULT_FG, DEFAULT_BG}`.
  - `core_vt::alacritty_terminal::term::cell::Flags` (`BOLD`, `DIM`, `INVERSE`, `WIDE_CHAR_SPACER`), `::vte::ansi::CursorShape`.
- Pinned GPUI APIs (Zed `terminal_element.rs` @ `7344a31`):
  - Cell width: `window.text_system().em_advance(font_id, font_size) -> Pixels`, where `font_id = window.text_system().resolve_font(&font)`.
  - Line height: `font_size * line_height_factor` (use `1.25`).
  - Glyph run: `window.text_system().shape_line(text: SharedString, font_size: Pixels, runs: &[gpui::TextRun], force_cell_width: Option<Pixels>) -> ShapedLine`, then `shaped.paint(origin: Point<Pixels>, line_height: Pixels, gpui::TextAlign::Left, None, window, cx)`.
  - Background/cursor quad: `window.paint_quad(gpui::fill(Bounds::new(origin, size), color: Hsla))`.
  - `gpui::TextRun { len, font, color: Hsla, background_color: None, underline: None, strikethrough: None }`.
- Color conversion: `[u8;3]` sRGB → `gpui::Hsla` via `gpui::Rgba { r, g, b, a: 1.0 }.into()` (components 0.0–1.0).

- [ ] **Step 1: Implement the element skeleton**

Create `crates/app/src/terminal_element.rs`. Implement `gpui::Element` with `type RequestLayoutState = ()`, `type PrepaintState = ()` (compute layout inline in `paint` for the mini-port). Structure the `paint` method:

1. Read the `Vt` (locked) `renderable_content()`.
2. Compute `cell_width = em_advance(...)`, `line_height = font_size * 1.25`.
3. Group cells per row into style runs (fg, bold, dim), mirroring `build_frame` in `crates/render/src/lib.rs:build_frame` (same grouping/cursor/inverse logic). Reuse the algorithm; only the paint calls differ.
4. For each non-default background cell (and the cursor cell), `paint_quad(fill(...))`.
5. For each row's runs, `shape_line(...).paint(...)` at the row origin.
6. Cursor: block quad in fg color when `display_offset == 0` and shape is not `Hidden`; the glyph under it uses bg color (swap like `build_frame`).

Use `crates/render/src/lib.rs` `build_frame` as the reference algorithm and Zed `terminal_element.rs` (`BatchedTextRun::paint`, `LayoutRect::paint`) as the reference for the GPUI paint calls. Write the concrete code and let the compiler guide API details.

- [ ] **Step 2: Return the element from `TerminalView::render`**

In `terminal_view.rs`, replace the `.children(...)` text rendering with the focus/key wrapper containing the `TerminalElement`. Keep `track_focus`, `key_context`, and `on_key_down` on the wrapping `div`; place `TerminalElement` as its child sized `size_full()`. The element borrows the view's `Arc<Mutex<Vt>>` (clone the `Arc` into the element).

- [ ] **Step 3: Build and smoke test**

Run:
```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo build -p app 2>&1 | tail -5 && (./target/debug/app >/tmp/app-smoke.log 2>&1 & echo $! >/tmp/app.pid); sleep 4; kill -0 $(cat /tmp/app.pid) && echo ALIVE && kill $(cat /tmp/app.pid) || (echo DIED; cat /tmp/app-smoke.log)
```
Expected: `ALIVE`. Manual check (`cargo run -p app`): colors render, block cursor visible, `vim`/`htop` display correctly.

- [ ] **Step 4: Lint and commit**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo fmt --all && cargo clippy -p app --all-targets 2>&1 | grep -cE "^(warning|error)"
git add -A && git commit -m "Render terminal grid with a native GPUI element (colors, cursor)"
```
Expected: clippy count `0`.

---

## Task 6: Resize, DPI, and mouse-wheel scrollback — parity

**Files:**
- Modify: `crates/app/src/terminal_view.rs` (resize on element bounds change, wheel handler)
- Modify: `crates/app/src/terminal_element.rs` (report measured cell size back so the view can compute grid dimensions)

**Interfaces:**
- Consumes: `core_vt::Vt::{resize, scroll_display, scroll_to_bottom, is_alt_screen}`, `core_pty::PtySession::resize`, `core_pty::Size`, `core_vt::GridSize`.
- Produces: grid resize on window resize; wheel scrolls history (normal screen) or forwards arrows (alt screen); keypress already resets to bottom (Task 4).

- [ ] **Step 1: Resize the grid to the painted area**

When the element paints, it knows `bounds`, `cell_width`, `line_height`. Compute `cols = (bounds.width / cell_width).floor()`, `rows = (bounds.height / line_height).floor()` (min 2 each). If they differ from the current `Vt` size, call `vt.resize(GridSize{..})` and `pty.resize(Size{ rows, cols })`. Store the last-applied size on `TerminalView` to avoid resizing every frame. (Pass the view handle into the element or resize from the view using a shared `Arc<Mutex<(u16,u16)>>` — pick the simpler wiring that compiles.)

- [ ] **Step 2: Mouse-wheel handler on the wrapping `div`**

Add to the focus/key wrapper in `terminal_view.rs`:

```rust
.on_scroll_wheel(cx.listener(|this, ev: &gpui::ScrollWheelEvent, _window, cx| {
    // Convert the wheel delta to grid lines. Pixel deltas divide by line height;
    // line deltas map ~3 lines per notch. Verify ScrollDelta variants for 0.2.2.
    let lines = this.wheel_lines(ev);
    if lines != 0 {
        this.scroll(lines, cx);
    }
}))
```

And the methods:

```rust
impl TerminalView {
    fn scroll(&mut self, lines: i32, cx: &mut Context<Self>) {
        let alt = self.vt.lock().expect("vt lock").is_alt_screen();
        if alt {
            let seq: &[u8] = if lines > 0 { b"\x1b[A" } else { b"\x1b[B" };
            let mut input = Vec::new();
            for _ in 0..lines.unsigned_abs() {
                input.extend_from_slice(seq);
            }
            let _ = self.pty.lock().expect("pty lock").write_all(&input);
        } else {
            self.vt.lock().expect("vt lock").scroll_display(lines);
        }
        cx.notify();
    }
}
```

Implement `wheel_lines(&self, ev: &gpui::ScrollWheelEvent) -> i32` using the 0.2.2 `ScrollDelta` enum (line vs pixel); ~3 grid lines per notch for line deltas, `pixels.y / line_height` for pixel deltas.

- [ ] **Step 3: Build and manual verification**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build -p app 2>&1 | tail -3`
Expected: compiles. Manual (`cargo run -p app`):
- Resize the window → text reflows, prompt stays usable, `htop` fills the pane.
- Run a long command (`ls -la /usr/bin`), scroll the wheel up → older output appears; type a key → jumps back to the prompt.
- Open `less /etc/hosts`, scroll the wheel → `less` scrolls (alt-screen arrow forwarding).

- [ ] **Step 4: Lint and commit**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo fmt --all && cargo clippy --workspace --all-targets 2>&1 | grep -cE "^(warning|error)"
git add -A && git commit -m "Add resize and mouse-wheel scrollback to the GPUI terminal (parity)"
```
Expected: clippy count `0`.

---

## Task 7: Cleanup and parity sign-off

**Files:**
- Modify: `crates/core-vt/src/lib.rs` (remove the now-unused `line_text` if nothing references it, or keep only if a test uses it — check first)
- Modify: `CLAUDE.md` (update the "Current status" note: app now runs on GPUI; `render` retained as future Android backend)

- [ ] **Step 1: Dead-code check**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build --workspace 2>&1 | grep -i "never used\|dead_code" | head`
Expected: no warnings. If `line_text` is unused, remove it and its test; if a test still uses it, leave it.

- [ ] **Step 2: Full workspace test + lint**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace 2>&1 | grep -E "test result" && cargo clippy --workspace --all-targets 2>&1 | grep -cE "^(warning|error)"`
Expected: all test suites pass; clippy count `0`.

- [ ] **Step 3: Update CLAUDE.md status**

In `CLAUDE.md`, under "Current status", note: the desktop app now runs on GPUI (native grid rendering); the `render` crate (wgpu/glyphon) is retained as the future Android backend behind `trait Renderer`.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "Finalize GPUI mini-port: cleanup and status update"
```

---

## Self-Review Notes

- **Spec coverage:** parity criteria map to tasks — shell/vim/htop (T4–T5), keyboard (T3), colors/cursor (T5), resize/DPI (T6), scrollback + alt-screen wheel + reset-on-input (T4 key path, T6 wheel). Palette-to-core-vt (T1). `render` retained (T2 drops the app dep; crate untouched). App split into main/terminal_view/terminal_element/keys (T2–T5).
- **Dependency change vs spec:** the spec proposed a git pin for `gpui`; `gpui 0.2.2` is on crates.io, so the plan uses the published crate (strictly better for reproducibility). Recorded here as the one intentional deviation.
- **GPUI code honesty:** paint/boot/event APIs are pinned from Zed `7344a31`, but exact 0.2.2 signatures are verified by compilation in each task; where a symbol may differ (boot function, focus call, `ScrollDelta`/`Modifiers` field names), the step says to trust the compiler and the vendored reference.
