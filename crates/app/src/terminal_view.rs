//! GPUI entity owning a terminal session: pumps session output into the VT and
//! drives repaints, translates keyboard/scroll into terminal input.

use std::io::Read;
use std::sync::{Arc, Mutex};

use core_ssh::SshConfig;
use core_vt::{GridPoint, GridSize, Side, Vt};
use futures::StreamExt;
use futures::channel::mpsc;
use gpui::{
    ClipboardItem, Context, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent,
    Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    Point, Render, ScrollDelta, ScrollWheelEvent, Styled, Window, div, rgb,
};

use crate::keys;
use crate::session::{LocalSession, RemoteSession, Session};
use crate::terminal_element::{
    CellLayout, FONT_SIZE, GridDims, LINE_HEIGHT_FACTOR, SharedLayout, TerminalElement,
};

const INITIAL_COLS: u16 = 100;
const INITIAL_ROWS: u16 = 30;
const SCROLLBACK_LINES: usize = 10_000;
/// Grid lines scrolled per mouse-wheel notch reported in line units.
const WHEEL_LINES_PER_NOTCH: f32 = 3.0;

pub type SharedSession = Arc<Mutex<Box<dyn Session>>>;

/// Where the tab is in its lifecycle. Local tabs start Live; SSH tabs pass
/// through Connecting on a background thread first.
enum TabState {
    Connecting(String),
    Live(SharedSession),
    Failed(String),
}

pub struct TerminalView {
    vt: Arc<Mutex<Vt>>,
    state: TabState,
    grid_dims: GridDims,
    layout: SharedLayout,
    /// True while the left button is held and dragging out a selection.
    selecting: bool,
    focus: FocusHandle,
}

impl TerminalView {
    fn base(state: TabState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let size = GridSize {
            columns: INITIAL_COLS as usize,
            screen_lines: INITIAL_ROWS as usize,
        };
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            vt: Arc::new(Mutex::new(Vt::new(size, SCROLLBACK_LINES))),
            state,
            grid_dims: Arc::new(Mutex::new((INITIAL_COLS, INITIAL_ROWS))),
            layout: Arc::new(Mutex::new(CellLayout::default())),
            selecting: false,
            focus,
        }
    }

    /// A tab running the user's local shell.
    pub fn new_local(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut view = Self::base(TabState::Connecting(String::new()), window, cx);
        match LocalSession::spawn(INITIAL_ROWS, INITIAL_COLS) {
            Ok(session) => view.go_live(Box::new(session), cx),
            Err(err) => view.state = TabState::Failed(format!("failed to spawn shell: {err}")),
        }
        view
    }

    /// A tab that failed before a session even existed (e.g. no stored secret).
    pub fn new_failed(message: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::base(TabState::Failed(message), window, cx)
    }

    /// A tab connecting to a remote host. Connect + auth happen on a background
    /// thread; the tab renders a status line until the shell is ready.
    pub fn new_ssh(
        label: String,
        config: SshConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let view = Self::base(TabState::Connecting(label), window, cx);

        let (tx, mut rx) = mpsc::unbounded();
        std::thread::spawn(move || {
            let _ = tx.unbounded_send(RemoteSession::connect(config));
        });
        cx.spawn(async move |this, cx| {
            if let Some(result) = rx.next().await {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(session) => this.go_live(Box::new(session), cx),
                        Err(err) => this.state = TabState::Failed(err.to_string()),
                    }
                    cx.notify();
                });
            }
        })
        .detach();

        view
    }

    /// Installs a ready session: takes its reader, starts the pump, flips state.
    fn go_live(&mut self, mut session: Box<dyn Session>, cx: &mut Context<Self>) {
        let Some(reader) = session.take_reader() else {
            self.state = TabState::Failed("session has no output stream".to_owned());
            return;
        };
        // Adopt the grid size the element last measured (the window may have
        // been resized while connecting).
        let (cols, rows) = *self.grid_dims.lock().expect("grid dims lock");
        session.resize(rows, cols);
        let session: SharedSession = Arc::new(Mutex::new(session));
        Self::spawn_reader(reader, self.vt.clone(), session.clone(), cx);
        self.state = TabState::Live(session);
    }

    fn session(&self) -> Option<&SharedSession> {
        match &self.state {
            TabState::Live(session) => Some(session),
            _ => None,
        }
    }

    /// Blocking OS thread reads session output and advances the VT; a foreground
    /// task awaits wake signals and repaints. The channel only signals "dirty";
    /// the VT itself is updated on the reader thread under its mutex.
    fn spawn_reader(
        mut reader: Box<dyn Read + Send>,
        vt: Arc<Mutex<Vt>>,
        session: SharedSession,
        cx: &mut Context<Self>,
    ) {
        let (tx, mut rx) = mpsc::unbounded::<()>();

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
                    session
                        .lock()
                        .expect("session lock")
                        .write_all(r.as_bytes());
                }
                if tx.unbounded_send(()).is_err() {
                    break;
                }
            }
        });

        cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        // Cmd shortcuts are handled by the app, not forwarded to the session.
        if keystroke.modifiers.platform {
            match keystroke.key.as_str() {
                "c" => self.copy(cx),
                "v" => self.paste(cx),
                _ => {}
            }
            return;
        }
        if let Some(bytes) = keys::encode(keystroke)
            && let Some(session) = self.session()
        {
            self.vt.lock().expect("vt lock").scroll_to_bottom();
            session.lock().expect("session lock").write_all(&bytes);
            cx.notify();
        }
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.vt.lock().expect("vt lock").selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
            && let Some(session) = self.session()
        {
            self.vt.lock().expect("vt lock").scroll_to_bottom();
            session
                .lock()
                .expect("session lock")
                .write_all(text.as_bytes());
            cx.notify();
        }
    }

    /// Maps a mouse position (window pixels) to a grid point and cell side,
    /// using the layout the element publishes each paint. `None` before the
    /// first paint.
    fn point_at(&self, pos: Point<Pixels>) -> Option<(GridPoint, Side)> {
        let layout = *self.layout.lock().expect("layout lock");
        if layout.cell_width <= 0.0 || layout.line_height <= 0.0 {
            return None;
        }
        let col_f = (f32::from(pos.x) - layout.origin_x).max(0.0) / layout.cell_width;
        let rel_y = (f32::from(pos.y) - layout.origin_y).max(0.0);
        let col = (col_f.floor() as usize).min(layout.cols.saturating_sub(1) as usize);
        let row = ((rel_y / layout.line_height).floor() as usize)
            .min(layout.rows.saturating_sub(1) as usize);
        let side = if col_f.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        let point = self.vt.lock().expect("vt lock").viewport_point(row, col);
        Some((point, side))
    }

    fn begin_selection(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some((point, side)) = self.point_at(pos) {
            self.selecting = true;
            self.vt
                .lock()
                .expect("vt lock")
                .start_selection(point, side);
            cx.notify();
        }
    }

    fn extend_selection(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        if self.selecting
            && let Some((point, side)) = self.point_at(pos)
        {
            self.vt
                .lock()
                .expect("vt lock")
                .update_selection(point, side);
            cx.notify();
        }
    }

    fn end_selection(&mut self) {
        self.selecting = false;
    }

    /// Terminates the session; called when the owning tab is closed so the
    /// reader thread hits EOF and exits.
    pub fn kill(&self) {
        if let Some(session) = self.session() {
            session.lock().expect("session lock").kill();
        }
    }

    /// Converts a wheel event into a number of grid lines (positive = up, into
    /// history). Pixel deltas are divided by the row height in pixels.
    fn wheel_lines(&self, event: &ScrollWheelEvent) -> i32 {
        match event.delta {
            ScrollDelta::Lines(delta) => (delta.y * WHEEL_LINES_PER_NOTCH).round() as i32,
            ScrollDelta::Pixels(delta) => {
                let row_height = f32::from(FONT_SIZE) * LINE_HEIGHT_FACTOR;
                (f32::from(delta.y) / row_height).round() as i32
            }
        }
    }

    /// On the normal screen the wheel moves the scrollback viewport; on the
    /// alternate screen (vim/less/htop) it is forwarded as arrow keys so the
    /// running app scrolls instead.
    fn scroll(&mut self, lines: i32, cx: &mut Context<Self>) {
        let alt_screen = self.vt.lock().expect("vt lock").is_alt_screen();
        if alt_screen {
            let Some(session) = self.session() else {
                return;
            };
            let seq: &[u8] = if lines > 0 { b"\x1b[A" } else { b"\x1b[B" };
            let mut input = Vec::with_capacity(seq.len() * lines.unsigned_abs() as usize);
            for _ in 0..lines.unsigned_abs() {
                input.extend_from_slice(seq);
            }
            session.lock().expect("session lock").write_all(&input);
        } else {
            self.vt.lock().expect("vt lock").scroll_display(lines);
        }
        cx.notify();
    }

    /// Status overlay for tabs that have no live grid yet.
    fn render_status(&self, text: String, color: u32) -> gpui::Div {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(color))
            .child(text)
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let container = div()
            .track_focus(&self.focus)
            .key_context("Terminal")
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                this.on_key(&ev.keystroke, cx);
            }))
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _window, cx| {
                let lines = this.wheel_lines(ev);
                if lines != 0 {
                    this.scroll(lines, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _window, cx| {
                    this.begin_selection(ev.position, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _window, cx| {
                if ev.pressed_button == Some(MouseButton::Left) {
                    this.extend_selection(ev.position, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseUpEvent, _window, _cx| {
                    this.end_selection();
                }),
            )
            .size_full()
            .bg(rgb(0x121216));

        match &self.state {
            TabState::Live(session) => container.child(TerminalElement::new(
                self.vt.clone(),
                session.clone(),
                self.grid_dims.clone(),
                self.layout.clone(),
            )),
            TabState::Connecting(label) => {
                container.child(self.render_status(format!("connecting to {label}…"), 0x888888))
            }
            TabState::Failed(err) => {
                container.child(self.render_status(format!("connection failed: {err}"), 0xcc6666))
            }
        }
    }
}
