# Project T — GPU-accelerated terminal

Cross-platform terminal with native GPU rendering, built-in tiling (tmux-like), a smart
SSH manager with zero input latency (predictive echo), and free data export/sync.
Platforms in order: **macOS → Linux → (Windows) → Android**. Currently targeting **macOS**.

Full design docs live in `docs/` (Russian, git-ignored for now). Read `docs/README.md` as
the entry point; `docs/03-architecture.md` before writing code; `docs/phases/phase-0.md`
for the current phase. New plans go to `docs/plans/`.

## Core architectural rule (non-negotiable)

**The core knows nothing about rendering or UI.** All `core-*` crates are pure Rust
(no GUI deps) so they can be reused on Android later.

Dependency rules:
- `app → render, core-*`
- `core-predict → core-ssh, core-vt`
- `core-*` must never depend on `render` or `app`
- `render` is hidden behind `trait Renderer` (a direct Metal backend may be added later)

## Workspace layout

```
crates/
├── core-vt/        # VT parsing + screen grid model (wraps alacritty_terminal)
├── core-pty/       # local PTY: spawn shell, read/write (portable-pty)
├── core-ssh/       # SSH via russh: sessions, channels, port-forward
├── core-predict/   # predictive local echo (mosh-style) on top of core-ssh
├── core-store/     # SQLite (hosts, history) + OS keychain for secrets (keyring)
├── render/         # GPU grid renderer: wgpu + glyphon/cosmic-text, winit window
└── app/            # thin binary: window + wires render and core-* together
```

## Tech stack

- VT core: `alacritty_terminal` (fallback option: `libghostty-vt`)
- Desktop UI + rendering: **GPUI** (`gpui`, from crates.io) — the app renders the
  terminal grid with GPUI-native primitives (text system + quads), like Zed.
  Requires full Xcode + the Metal Toolchain component to build (`xcodebuild
  -downloadComponent MetalToolchain`), not just Command Line Tools.
- `render` crate (`wgpu` + `glyphon`/`cosmic-text`) is retained behind
  `trait Renderer` but NOT used by the app — it is the future Android backend
  (GPUI does not target Android). Do NOT hardcode Metal there.
- SSH: `russh` + custom predictive-echo layer; async runtime: `tokio`
- Storage: SQLite (`rusqlite`/`sqlx`); secrets: `keyring` (never store secrets in the DB, only references/metadata)

## Current status: Phase 1 (MVP) — feature-complete, stabilizing

The desktop app runs on GPUI: `vim`/`htop`/`less` work, colors + block cursor,
resize reflow, scrollback, selection + clipboard. Shell features: tabs
(`Cmd+T/W`, `Cmd+Shift+]`/`[`), tiling panes (`Cmd+D` right / `Cmd+Shift+D`
down; `Cmd+W` closes pane then tab), host sidebar (add-host modal form,
click-to-connect, delete), command palette (`Cmd+K`), hosts export/import
(`app --export-hosts` / `--import-hosts`, secret-free JSON).

SSH tabs run over `core-ssh` behind `trait Session` (`app/src/session.rs` —
local PTY and remote SSH are interchangeable byte sources) with a
Connecting → Live | Failed tab lifecycle and mosh-style predictive local echo
(`core-predict`) painted dimmed at the cursor. Host metadata lives in SQLite
(`~/Library/Application Support/project-t/store.db`), passwords in the OS
keychain — never in the DB.

Known phase-1 gaps: host keys are not verified (accept-all, MITM-open),
text inputs have no cursor movement/selection/IME, splits are fixed 50/50,
host editing = delete + re-add.

Next: phase-2 items (autocomplete, blocks, importers, tmux control mode,
mosh) or hardening the gaps above.

App module layout: `main` (GPUI boot) → `terminal_view` (session entity: PTY
reader task, key/scroll handling) → `terminal_element` (native GPUI grid paint)
→ `keys` (keystroke → terminal bytes).

## Conventions

- Rust edition 2024, `cargo fmt` + `cargo clippy` clean before committing.
- Tests live next to code (`#[cfg(test)]`) or in `tests/` for integration.
- Code, comments, commit messages: English only.
- Comments explain *why*, not *what*. No speculative abstractions (YAGNI) —
  except `trait Renderer`, which is a deliberate architectural seam.

## Commands

```sh
cargo build --workspace
cargo test --workspace
cargo run -p app          # launch the terminal window
cargo clippy --workspace --all-targets
```
