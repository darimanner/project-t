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
- GPU: `wgpu` (Metal on macOS via abstraction — do NOT hardcode Metal), text via `glyphon` + `cosmic-text`
- Window/input: `winit` (phase 0); desktop UI framework later: GPUI
- SSH: `russh` + custom predictive-echo layer; async runtime: `tokio`
- Storage: SQLite (`rusqlite`/`sqlx`); secrets: `keyring` (never store secrets in the DB, only references/metadata)

## Current status: Phase 0 — core prototype

Goal: prove that Rust core + wgpu render gives a fast, correct terminal.
Success = `vim`/`htop`/`less` run correctly in the window, input is responsive,
render holds high FPS under heavy output, core builds with no render/app deps.

Task order: workspace scaffold → core-pty → core-vt → render → app glue →
core-ssh prototype → core-predict prototype → core-store sketch.

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
