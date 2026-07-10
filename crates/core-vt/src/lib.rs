//! VT parsing and screen grid model (wraps `alacritty_terminal`).
//!
//! [`Vt`] consumes raw bytes from a PTY/SSH stream and maintains the screen
//! state (grid of cells, cursor, modes). It knows nothing about rendering.

use std::sync::{Arc, Mutex};

// Re-exported so downstream crates (render) use the exact same version of the
// grid/cell types without declaring their own alacritty_terminal dependency.
pub use alacritty_terminal;

mod palette;
pub use palette::{DEFAULT_BG, DEFAULT_FG, resolve_color};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;

/// Viewport size in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    pub columns: usize,
    pub screen_lines: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// Collects events emitted by the terminal during parsing.
///
/// `Term` reports side effects (title changes, bell, responses that must be
/// written back to the PTY) through this listener; callers drain them via
/// [`Vt::drain_pty_writes`].
#[derive(Default, Clone)]
struct EventQueue(Arc<Mutex<Vec<Event>>>);

impl EventListener for EventQueue {
    fn send_event(&self, event: Event) {
        self.0.lock().expect("event queue poisoned").push(event);
    }
}

/// Terminal state machine: feed bytes in, read the grid out.
pub struct Vt {
    term: Term<EventQueue>,
    parser: Processor,
    events: EventQueue,
}

impl Vt {
    pub fn new(size: GridSize, scrollback_lines: usize) -> Self {
        let config = Config {
            scrolling_history: scrollback_lines,
            ..Config::default()
        };
        let events = EventQueue::default();
        let term = Term::new(config, &size, events.clone());
        Self {
            term,
            parser: Processor::new(),
            events,
        }
    }

    /// Feeds raw output bytes from the PTY/SSH stream into the state machine.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    pub fn resize(&mut self, size: GridSize) {
        self.term.resize(size);
    }

    /// Drains responses the terminal must send back to the PTY (e.g. cursor
    /// position reports). The caller is responsible for writing them.
    pub fn drain_pty_writes(&mut self) -> Vec<String> {
        let mut queue = self.events.0.lock().expect("event queue poisoned");
        let mut writes = Vec::new();
        queue.retain(|event| match event {
            Event::PtyWrite(text) => {
                writes.push(text.clone());
                false
            }
            // Other events (title, bell, clipboard) are not consumed yet.
            _ => true,
        });
        writes
    }

    pub fn cursor(&self) -> Point {
        self.term.grid().cursor.point
    }

    /// Scrolls the viewport through scrollback by `lines` (positive = up, into
    /// history). No-op on the alternate screen, which has no scrollback.
    pub fn scroll_display(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    /// Jumps the viewport back to the live bottom of the buffer.
    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    /// How many lines the viewport is scrolled up into scrollback (0 = bottom).
    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// Whether the alternate screen is active (vim, htop, less, …). Callers use
    /// this to route the mouse wheel to the app instead of scrolling history.
    pub fn is_alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// Full terminal state for the renderer (grid, cursor, colors, mode).
    pub fn term(&self) -> &Term<impl EventListener + use<>> {
        &self.term
    }

    /// Visible text of one viewport row, trailing whitespace trimmed.
    pub fn line_text(&self, row: usize) -> String {
        let grid = self.term.grid();
        let line = Line(row as i32);
        let text: String = (0..grid.columns())
            .map(|col| grid[line][Column(col)].c)
            .collect();
        text.trim_end().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: GridSize = GridSize {
        columns: 80,
        screen_lines: 24,
    };

    fn vt() -> Vt {
        Vt::new(SIZE, 1000)
    }

    #[test]
    fn plain_text_lands_in_grid() {
        let mut vt = vt();
        vt.advance(b"hello");
        assert_eq!(vt.line_text(0), "hello");
        assert_eq!(vt.cursor(), Point::new(Line(0), Column(5)));
    }

    #[test]
    fn crlf_moves_to_next_line() {
        let mut vt = vt();
        vt.advance(b"one\r\ntwo");
        assert_eq!(vt.line_text(0), "one");
        assert_eq!(vt.line_text(1), "two");
    }

    #[test]
    fn csi_cursor_movement() {
        let mut vt = vt();
        // Write 5 chars, move cursor 3 left (CUB).
        vt.advance(b"abcde\x1b[3D");
        assert_eq!(vt.cursor(), Point::new(Line(0), Column(2)));
    }

    #[test]
    fn sgr_sets_cell_color() {
        use alacritty_terminal::vte::ansi::{Color, NamedColor};

        let mut vt = vt();
        vt.advance(b"\x1b[31mX");
        let cell = &vt.term().grid()[Line(0)][Column(0)];
        assert_eq!(cell.c, 'X');
        assert_eq!(cell.fg, Color::Named(NamedColor::Red));
    }

    #[test]
    fn dsr_produces_pty_write() {
        let mut vt = vt();
        // Device Status Report: terminal must answer with the cursor position.
        vt.advance(b"\x1b[6n");
        let writes = vt.drain_pty_writes();
        assert_eq!(writes, vec!["\x1b[1;1R".to_owned()]);
        // Queue is drained.
        assert!(vt.drain_pty_writes().is_empty());
    }

    #[test]
    fn scrollback_reveals_history() {
        let mut vt = Vt::new(
            GridSize {
                columns: 20,
                screen_lines: 3,
            },
            1000,
        );
        // Push more lines than fit on screen so the first ones go to scrollback.
        // No trailing newline, so line9 stays on the bottom visible row.
        for i in 0..10 {
            if i > 0 {
                vt.advance(b"\r\n");
            }
            vt.advance(format!("line{i}").as_bytes());
        }
        // Seven lines pushed into scrollback (10 written, 3 on screen).
        assert_eq!(vt.display_offset(), 0);

        vt.scroll_display(5);
        assert_eq!(vt.display_offset(), 5);

        // Scrolling past the top clamps to the available history.
        vt.scroll_display(100);
        assert_eq!(vt.display_offset(), 7);

        vt.scroll_to_bottom();
        assert_eq!(vt.display_offset(), 0);
    }

    #[test]
    fn alt_screen_detection() {
        let mut vt = vt();
        assert!(!vt.is_alt_screen());
        // DECSET 1049: enter alternate screen buffer.
        vt.advance(b"\x1b[?1049h");
        assert!(vt.is_alt_screen());
        vt.advance(b"\x1b[?1049l");
        assert!(!vt.is_alt_screen());
    }

    #[test]
    fn resize_changes_dimensions() {
        let mut vt = vt();
        vt.advance(b"persist");
        vt.resize(GridSize {
            columns: 120,
            screen_lines: 40,
        });
        assert_eq!(vt.term().grid().columns(), 120);
        assert_eq!(vt.term().grid().screen_lines(), 40);
        assert_eq!(vt.line_text(0), "persist");
    }
}
