# Project T

**A GPU-accelerated terminal with zero-latency SSH input, a built-in host manager and native tiling.**

![status](https://img.shields.io/badge/status-work%20in%20progress-orange)
![stage](https://img.shields.io/badge/stage-experiment-blueviolet)
![platform](https://img.shields.io/badge/platform-macOS-lightgrey?logo=apple)
![rust](https://img.shields.io/badge/rust-2024%20edition-b7410e?logo=rust)
![license](https://img.shields.io/badge/license-MIT-green)

> [!WARNING]
> **This is an experiment and a work in progress.** Nothing here is stable, APIs and file
> formats will change, and there are known security gaps (see [Known gaps](#known-gaps)).
> Do not use it for anything that matters yet.

### Why this exists

Project T is an attempt to build an alternative to [Warp](https://www.warp.dev) that keeps what
makes it pleasant and drops what gets in the way. It is also built almost entirely with AI coding
agents: the code, the plans and most of the debugging come from an agent-driven workflow, with a
human steering the direction and reviewing the result.

### The idea in short

- **Zero input latency over SSH.** Mosh-style predictive local echo: keystrokes show up instantly
  and get reconciled with what the server actually sends back.
- **Hosts as a first-class feature.** A sidebar with saved servers, click to connect, passwords
  in the OS keychain, never in a database.
- **Native GPU rendering.** The grid is painted with GPU primitives, no web views, no Electron.
- **Tiling built in.** Splits and tabs work the same locally and over SSH, no tmux required.
- **Your data stays yours.** Free export/import of hosts as plain JSON, no account, no
  subscription.
- **Core separate from UI.** Everything below the window is plain Rust, so the same core can
  later drive a Linux or Android front end.

---

## Status

The phase-1 MVP is feature-complete on macOS and is being stabilized.

| Area | State |
| --- | --- |
| Local shell (PTY), colors, block cursor, scrollback | ✅ works (`vim`, `htop`, `less`) |
| Resize with reflow, mouse selection, clipboard | ✅ works |
| Tabs and tiling panes | ✅ works (fixed 50/50 splits) |
| SSH sessions with predictive local echo | ✅ works |
| Host sidebar, add-host form, command palette | ✅ works |
| Hosts export/import (secret-free JSON) | ✅ works (CLI) |
| Host key verification | ❌ not yet |
| Autocomplete, command blocks, importers, tmux control mode, mosh | 🗓️ planned |
| Linux, then Android | 🗓️ planned |

## Getting started

Requirements:

- macOS with **full Xcode** (Command Line Tools alone are not enough) and the Metal toolchain:
  `xcodebuild -downloadComponent MetalToolchain`
- A recent stable Rust toolchain (edition 2024)

```sh
git clone https://github.com/darimanner/project-t.git
cd project-t
cargo run -p app --release
```

Hosts export/import:

```sh
cargo run -p app -- --export-hosts hosts.json
cargo run -p app -- --import-hosts hosts.json
```

Exports never contain passwords: they stay in the macOS keychain.

## Keybindings

| Keys | Action |
| --- | --- |
| `Cmd+T` | New tab |
| `Cmd+W` | Close pane, then tab |
| `Cmd+Shift+]` / `Cmd+Shift+[` | Next / previous tab |
| `Cmd+D` | Split pane right |
| `Cmd+Shift+D` | Split pane down |
| `Cmd+K` | Command palette |

## Architecture

The core knows nothing about rendering or UI. All `core-*` crates are pure Rust with no GUI
dependencies, so they can be reused on other platforms.

```
crates/
├── core-vt/        VT parsing and the screen grid (wraps alacritty_terminal)
├── core-pty/       local PTY: spawn a shell, read and write
├── core-ssh/       SSH via russh: sessions and channels
├── core-predict/   predictive local echo on top of core-ssh
├── core-store/     SQLite for hosts, OS keychain for secrets
├── render/         wgpu renderer behind `trait Renderer` (future Android backend)
└── app/            desktop app on GPUI: window, tabs, panes, sidebar, palette
```

Built on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), [alacritty_terminal](https://github.com/alacritty/alacritty),
[russh](https://github.com/Eugeny/russh), [portable-pty](https://github.com/wezterm/wezterm),
[rusqlite](https://github.com/rusqlite/rusqlite) and [keyring](https://github.com/hwchen/keyring-rs).

## Known gaps

- **SSH host keys are not verified.** Every host key is accepted, which leaves connections open to
  man-in-the-middle attacks. Only connect to hosts on networks you trust.
- Text inputs have no cursor movement, selection or IME yet.
- Splits are fixed at 50/50.
- Editing a host means deleting and re-adding it.

## Development

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

Agent instructions for this repo live in [AGENTS.md](AGENTS.md).

## License

[MIT](LICENSE) © 2026 Kostiantyn Durlov.

You are free to use, modify and fork this code. Keep the copyright notice and credit the original
author in your copies and forks.
