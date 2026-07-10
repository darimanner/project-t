# GPUI terminal mini-port — design

**Date:** 2026-07-11
**Status:** Approved (design), pending spec review
**Scope:** First sub-project of the phase-1 desktop shell.

## Goal

Replace the winit window with a GPUI window hosting a **single terminal pane**,
reusing the existing Rust core (`core-vt`, `core-pty`) and rendering the grid
with **GPUI-native primitives** (no glyphon/wgpu in the app path). This proves
the core↔GPUI integration before any chrome (tabs, tiling, sidebar, palette,
blocks) is added.

## Success criteria (parity with the current winit app, on GPUI)

- User's login shell launches; `vim`, `htop`, `less` work correctly.
- Keyboard input: printable chars, Ctrl-letters (C0), arrows (CSI),
  Enter/Backspace/Tab/Esc, Home/End/PageUp/PageDown/Delete/Insert.
- Colors: named/16, indexed/256, truecolor; bold, dim, inverse; block cursor.
- Window resize recomputes the grid and resizes the PTY; DPI/scale change
  recomputes font metrics.
- Scrollback via mouse wheel; wheel forwarded as arrow keys on the alternate
  screen; any keypress resets the viewport to the bottom.

### Out of scope (later iterations)

Tabs, tiling panes, connection sidebar, command palette, Warp-like blocks,
mouse text selection / copy / paste, cursor blink, focus-hollow cursor,
damage-tracking optimization, bundled fonts, themes.

## Rendering approach: GPUI-native (chosen)

Render the grid using GPUI's own paint API, mirroring how Zed's terminal is
built (`terminal_element.rs`: `alacritty_terminal` + GPUI). GPUI owns the GPU
(Metal on macOS). Rejected alternatives:

- **Embed our wgpu surface as a GPUI element** — GPUI has no first-class
  external-surface element; would fight pre-1.0 internals. High risk.
- **Stay on winit, hand-build chrome** — huge scope, contradicts the stack
  decision in `docs/02-tech-stack.md`.

The `trait Renderer` seam built in phase 0 is exactly what makes swapping the
glyphon backend for a GPUI backend a localized change.

## Architecture

Core and data pipeline are unchanged. Only the window/render layer in `app`
changes.

```
crates/
├── core-vt/     # + color/palette module moved in (see below)
├── core-pty/    # unchanged
├── render/      # glyphon/wgpu — stays in workspace, app no longer depends on it
└── app/         # GPUI application (replaces winit)
```

### `render` crate is retained, not deleted

`app` stops depending on `render`, but the crate stays. Rationale is strategic,
not sentimental: **GPUI does not target Android**, and the Android backend
(phase 4) will need an owned wgpu renderer. The glyphon/wgpu layer is that
future backend and a second concrete `trait Renderer` impl. It keeps building
and its tests keep running.

### `app` module layout

- `main.rs` — boots the GPUI application, opens the window, mounts the root view.
- `terminal_view.rs` — GPUI entity (focusable) owning session state; handles
  key/scroll/resize events; drives repaint.
- `terminal_element.rs` — custom GPUI `Element`; reads the grid from `Vt` and
  paints glyph runs + background/cursor quads.
- `keys.rs` — encodes a normalized key (keysym + modifiers) into terminal bytes.

Responsibility boundaries: `terminal_element` knows only "how to paint a grid",
`terminal_view` knows "session state and events", the core knows "what is in
the grid". Each is understandable and (where practical) testable in isolation.

### Color/palette moves to `core-vt`

Resolving a cell `Color` (+ runtime OSC overrides) to an sRGB `[u8; 3]`, the
default fg/bg, and the xterm-256 base palette are **pure terminal semantics**
with no GPU dependency. They move from `render/src/color.rs` into a `core-vt`
module so both renderers (glyphon and GPUI) share one implementation (DRY).

Renderer-specific bits stay in each renderer: `render` keeps `srgb8_to_linear`
(glyphon writes into a linear sRGB target); the GPUI backend consumes sRGB
bytes directly and converts to `gpui::Rgba`. `core-vt` gains unit tests for
palette resolution.

## Input, threading, data flow

Same pipeline as today (PTY → VT → repaint), re-expressed in GPUI's model.

- **State.** `TerminalView` is a GPUI entity owning `Arc<Mutex<Vt>>` and
  `Arc<Mutex<PtySession>>`, plus the current grid size.
- **PTY read loop.** A background task reads output, calls `vt.advance(bytes)`,
  drains VT→PTY responses (cursor-position reports, already implemented) and
  writes them back, then schedules a foreground update on the entity handle and
  calls `cx.notify()` to trigger a repaint. This replaces winit's
  `EventLoopProxy::send_event`.
- **Keyboard.** `terminal_view` receives GPUI key events; `keys::encode` maps a
  normalized key to bytes (Ctrl-letter → C0, arrows → CSI, named keys, printable
  text). Before writing, `vt.scroll_to_bottom()`.
- **Mouse wheel.** Normal screen → `vt.scroll_display(±lines)` + repaint; alt
  screen → forward as arrow keys to the PTY.
- **Resize.** GPUI reports the element's pixel size → divide by cell size →
  `vt.resize` + `pty.resize`. DPI/scale change recomputes font metrics.
- **Focus.** `terminal_view` is focusable so it receives keyboard input.

Flow: `key/wheel → keys/scroll → pty.write / vt.scroll → (PTY echo) → reader
task → vt.advance → cx.notify → element.paint`.

## Dependencies

- Add `gpui` as a git dependency pinned to a specific Zed commit
  (`gpui = { git = "https://github.com/zed-industries/zed", rev = "<pin>" }`).
  The exact commit is chosen during planning and verified to build on macOS.
- Remove `winit` from `app`. `render`'s dependencies are unchanged.

## Error handling

- Window/GPUI startup failures are unrecoverable at boot → `expect` (matches the
  current app). PTY spawn failure likewise fails fast for the mini-port; a
  friendlier in-terminal error surface is a later concern.

## Testing

- `core-vt`: existing tests plus new unit tests for palette/color resolution.
- `keys::encode`: pure unit tests over the normalized-key representation (no GPU
  or window needed).
- GPUI rendering: not unit-tested (needs a GPU/window); validated by a smoke run
  (process stays alive) and manual dogfooding against `vim`/`htop`/`less`.

## Risks

- **GPUI is pre-1.0.** Paint APIs and event types may differ from memory; exact
  signatures are pinned from Zed's `terminal_element.rs` during planning rather
  than guessed. The git pin insulates us from upstream churn until we choose to
  bump it.
- **Build time.** Depending on Zed's `gpui` pulls a large dependency tree and
  lengthens the first build; mitigated by the pin and cargo caching.
