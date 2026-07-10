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
    ParentElement, Render, Styled, Window, div, px, rgb,
};

use crate::keys;

const INITIAL_COLS: u16 = 100;
const INITIAL_ROWS: u16 = 30;
const SCROLLBACK_LINES: usize = 10_000;

pub struct TerminalView {
    vt: Arc<Mutex<Vt>>,
    pty: Arc<Mutex<PtySession>>,
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
        Self { vt, pty, focus }
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
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
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
