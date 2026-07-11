//! GPUI entity owning a terminal session: pumps PTY output into the VT and
//! drives repaints, translates keyboard/scroll into terminal input.

use std::io::Read;
use std::sync::{Arc, Mutex};

use core_pty::{PtySession, Size, SpawnConfig};
use core_vt::{GridSize, Vt};
use futures::StreamExt;
use futures::channel::mpsc;
use gpui::{
    Context, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent, Keystroke,
    ParentElement, Render, ScrollDelta, ScrollWheelEvent, Styled, Window, div, rgb,
};

use crate::keys;
use crate::terminal_element::{FONT_SIZE, GridDims, LINE_HEIGHT_FACTOR, TerminalElement};

const INITIAL_COLS: u16 = 100;
const INITIAL_ROWS: u16 = 30;
const SCROLLBACK_LINES: usize = 10_000;
/// Grid lines scrolled per mouse-wheel notch reported in line units.
const WHEEL_LINES_PER_NOTCH: f32 = 3.0;

pub struct TerminalView {
    vt: Arc<Mutex<Vt>>,
    pty: Arc<Mutex<PtySession>>,
    grid_dims: GridDims,
    focus: FocusHandle,
}

impl TerminalView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let size = GridSize {
            columns: INITIAL_COLS as usize,
            screen_lines: INITIAL_ROWS as usize,
        };
        let vt = Arc::new(Mutex::new(Vt::new(size, SCROLLBACK_LINES)));
        let pty = PtySession::spawn(&SpawnConfig::user_shell(Size {
            rows: INITIAL_ROWS,
            cols: INITIAL_COLS,
        }))
        .expect("spawn shell");
        let reader = pty.reader().expect("clone pty reader");
        let pty = Arc::new(Mutex::new(pty));

        Self::spawn_reader(reader, vt.clone(), pty.clone(), cx);

        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            vt,
            pty,
            grid_dims: Arc::new(Mutex::new((INITIAL_COLS, INITIAL_ROWS))),
            focus,
        }
    }

    /// Blocking OS thread reads PTY output and advances the VT; a foreground
    /// task awaits wake signals and repaints. The channel only signals "dirty";
    /// the VT itself is updated on the reader thread under its mutex.
    fn spawn_reader(
        mut reader: Box<dyn Read + Send>,
        vt: Arc<Mutex<Vt>>,
        pty: Arc<Mutex<PtySession>>,
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
                    let _ = pty.lock().expect("pty lock").write_all(r.as_bytes());
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
        if let Some(bytes) = keys::encode(keystroke) {
            self.vt.lock().expect("vt lock").scroll_to_bottom();
            let _ = self.pty.lock().expect("pty lock").write_all(&bytes);
            cx.notify();
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
            let seq: &[u8] = if lines > 0 { b"\x1b[A" } else { b"\x1b[B" };
            let mut input = Vec::with_capacity(seq.len() * lines.unsigned_abs() as usize);
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

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
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
            .size_full()
            .bg(rgb(0x121216))
            .child(TerminalElement::new(
                self.vt.clone(),
                self.pty.clone(),
                self.grid_dims.clone(),
            ))
    }
}
