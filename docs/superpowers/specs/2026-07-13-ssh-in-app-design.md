# SSH in the App: Session Abstraction, Host Sidebar, Command Palette

**Date:** 2026-07-13
**Status:** approved

## Goal

A terminal tab can be a local shell or a remote SSH session. Hosts are stored in
`core-store` (SQLite + keyring), managed from a sidebar with an add-host form,
and reachable via a Cmd+K palette. Everything ships in one iteration.

## 1. Session abstraction (`app/src/session.rs`)

Trait lives in `app` — core crates stay independent; the future predictive-echo
layer becomes just another implementation.

```rust
pub trait Session: Send {
    /// Output stream; may only be taken once.
    fn take_reader(&mut self) -> Option<Box<dyn Read + Send>>;
    fn write_all(&mut self, data: &[u8]);
    fn resize(&mut self, rows: u16, cols: u16);
    fn kill(&mut self);
}
```

Implementations: `LocalSession` (wraps `core_pty::PtySession`) and
`RemoteSession` (wraps `core_ssh::SshSession`). Errors inside adapters are
logged (`eprintln!`), not propagated — a dead session surfaces as reader EOF.

`TerminalView` and `TerminalElement` switch from `Arc<Mutex<PtySession>>` to
`Arc<Mutex<Box<dyn Session>>>`. The reader-thread + notify model is unchanged.

## 2. Tab connection states

`SshSession::connect` blocks (TCP + auth) → runs on a background thread. The tab
appears immediately:

```
Connecting(label) → Live(session) | Failed(message)
```

`TerminalView` gains this state; render shows a status line for
Connecting/Failed and the grid for Live. Local tabs start directly in Live.
A Failed tab stays open (readable error) until closed by the user.

## 3. Layout & sidebar

`Workspace` renders flex-row: sidebar (fixed 200px) + column (tab bar +
terminal). Sidebar:

- Lists hosts from `core_store::Store`, cached `Vec<Host>`, re-read after
  add/delete.
- Click a host → new SSH tab (auth from keyring for password hosts, key path
  from the host row for key hosts).
- Hover “×” on a row → delete host + its keyring secret.
- “+ add host” button at the bottom → opens the add-host form.
- DB path: `~/Library/Application Support/project-t/store.db` (dir created on
  startup). Store open failure → sidebar shows an error line; local tabs still
  work.

## 4. Text input widget (`app/src/text_input.rs`)

gpui 0.2.2 has no built-in input. Minimal single-line editor:

- printable char insert, backspace, cursor block at end (no mid-line cursor
  movement, no selection, no IME),
- masked mode for passwords (renders `•`),
- focus-aware styling, placeholder text,
- pure editing state (`String` + handlers) kept separate from rendering so it
  is unit-testable.

Submit/cancel are handled by the parent (form/palette) via key events, not by
the widget.

## 5. Add-host form (`app/src/host_form.rs`)

Modal overlay, fields: label, host, port, user, auth toggle (password ⇄ key),
password (masked) or key path. Tab cycles focus, Enter saves, Esc cancels.
Validation: host/user non-empty, port parses as u16 (default 22), key path
non-empty when auth = key. On save: host row → SQLite, password → keyring
(password auth only). Store/keyring errors render as an error line inside the
form; the form stays open.

## 6. Command palette (`app/src/palette.rs`)

Cmd+K opens an overlay: text input + host list filtered by case-insensitive
substring match over label/host/user. ↑/↓ selects, Enter connects in a new tab,
Esc closes. Reuses the sidebar's cached host list.

## 7. Testing

- `session.rs`: adapter over a fake `Session` impl exercised by TerminalView
  logic is impractical to unit-test in gpui; adapters are thin — covered by the
  existing core-crate tests plus manual run.
- Pure logic gets unit tests: input editing state, form validation/parsing,
  palette filtering.
- Manual verification: local tab still works; add host → appears in sidebar;
  click → Connecting → Live against a real server; wrong password → Failed tab;
  Cmd+K filter + Enter connects; delete removes row and secret.

## Out of scope

known_hosts verification (core-ssh still accepts all keys), predictive echo,
host editing (delete + re-add instead), fuzzy ranking, multi-line input,
IME/selection in the input widget.
