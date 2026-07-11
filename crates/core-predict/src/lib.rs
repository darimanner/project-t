//! Predictive local echo (mosh-style).
//!
//! When typing over a high-latency link, waiting for the remote shell to echo
//! each keystroke feels laggy. [`Predictor`] optimistically records the printable
//! characters the user types and exposes them as a *predicted suffix* the renderer
//! can draw at the cursor immediately, before the server confirms. As the server's
//! echo arrives it is reconciled against the predictions.
//!
//! `ponytail: deliberately simple — single-line, front-of-queue confirmation, and
//! it flushes all predictions on any divergence. Escape sequences in server output
//! are skipped so they don't count as divergence, but there is no cursor tracking,
//! no epochs, and no timing/confidence like real mosh. Upgrade path: track cursor
//! position from the VT (core-vt) and predict cursor motion, keyed to round-trip
//! timing.`

use std::collections::VecDeque;

const BACKSPACE: u8 = 0x08;
const DEL: u8 = 0x7f;
const ESC: u8 = 0x1b;

fn is_printable(b: u8) -> bool {
    (0x20..=0x7e).contains(&b)
}

/// Where the server-output scanner is within a VT escape sequence. Persisted
/// across calls because output arrives in arbitrary chunks.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum EscState {
    #[default]
    Normal,
    /// Saw ESC; the next byte selects the sequence type.
    Esc,
    /// Inside a CSI (`ESC [ … final`) sequence.
    Csi,
}

/// Tracks keystrokes predicted to echo but not yet confirmed by the server.
#[derive(Debug, Default)]
pub struct Predictor {
    pending: VecDeque<u8>,
    esc: EscState,
}

impl Predictor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records predictions for locally-typed input. The caller still forwards the
    /// original bytes to the server unchanged; this only updates the overlay.
    ///
    /// Printable ASCII is predicted to echo; backspace retracts the last
    /// prediction; anything else (Enter, control sequences) ends the current
    /// prediction run, since its echo is no longer predictable.
    pub fn on_input(&mut self, bytes: &[u8]) {
        for &b in bytes {
            match b {
                _ if is_printable(b) => self.pending.push_back(b),
                BACKSPACE | DEL => {
                    self.pending.pop_back();
                }
                _ => self.pending.clear(),
            }
        }
    }

    /// Reconciles the predicted suffix against bytes received from the server.
    /// Each text byte matching the front of the queue confirms that prediction; a
    /// printable byte that diverges means we mispredicted, so we flush. Bytes
    /// inside an escape sequence are skipped entirely.
    pub fn on_server_output(&mut self, bytes: &[u8]) {
        for &b in bytes {
            match self.esc {
                EscState::Normal => {
                    if b == ESC {
                        self.esc = EscState::Esc;
                        continue;
                    }
                    match self.pending.front() {
                        Some(&front) if front == b => {
                            self.pending.pop_front();
                        }
                        Some(_) if is_printable(b) => {
                            // Server printed something other than predicted.
                            self.pending.clear();
                        }
                        // Control bytes / no pending: neither confirm nor diverge.
                        _ => {}
                    }
                }
                EscState::Esc => {
                    // `ESC [` opens a CSI; any other second byte is a short
                    // two-byte sequence that ends here.
                    self.esc = if b == b'[' {
                        EscState::Csi
                    } else {
                        EscState::Normal
                    };
                }
                EscState::Csi => {
                    // CSI ends at a final byte in 0x40..=0x7e.
                    if (0x40..=0x7e).contains(&b) {
                        self.esc = EscState::Normal;
                    }
                }
            }
        }
    }

    /// The bytes to render at the cursor as unconfirmed local echo.
    pub fn predicted(&self) -> Vec<u8> {
        self.pending.iter().copied().collect()
    }

    pub fn has_predictions(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Drops all predictions and resets escape parsing (e.g. on reflow, screen
    /// switch, or disconnect).
    pub fn clear(&mut self) {
        self.pending.clear();
        self.esc = EscState::Normal;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn predicted_str(p: &Predictor) -> String {
        String::from_utf8(p.predicted()).unwrap()
    }

    #[test]
    fn printable_input_is_predicted() {
        let mut p = Predictor::new();
        p.on_input(b"ls");
        assert_eq!(predicted_str(&p), "ls");
        assert!(p.has_predictions());
    }

    #[test]
    fn server_echo_confirms_front_to_back() {
        let mut p = Predictor::new();
        p.on_input(b"ls");
        p.on_server_output(b"l");
        assert_eq!(predicted_str(&p), "s");
        p.on_server_output(b"s");
        assert_eq!(predicted_str(&p), "");
        assert!(!p.has_predictions());
    }

    #[test]
    fn divergent_echo_flushes_predictions() {
        let mut p = Predictor::new();
        p.on_input(b"ls");
        // Server echoes something else entirely (e.g. a completion redraw).
        p.on_server_output(b"x");
        assert_eq!(predicted_str(&p), "");
    }

    #[test]
    fn backspace_retracts_last_prediction() {
        let mut p = Predictor::new();
        p.on_input(b"cat");
        p.on_input(&[BACKSPACE]);
        assert_eq!(predicted_str(&p), "ca");
    }

    #[test]
    fn enter_ends_the_prediction_run() {
        let mut p = Predictor::new();
        p.on_input(b"ls");
        p.on_input(b"\r");
        assert_eq!(predicted_str(&p), "");
    }

    #[test]
    fn escape_sequences_in_output_are_skipped() {
        let mut p = Predictor::new();
        p.on_input(b"hi");
        // A cursor-position report interleaved before the real echo. Its printable
        // bytes ('[', '2', ';', '5', 'R') must not be mistaken for divergence.
        p.on_server_output(b"\x1b[2;5R");
        assert_eq!(predicted_str(&p), "hi");
        p.on_server_output(b"hi");
        assert_eq!(predicted_str(&p), "");
    }

    #[test]
    fn escape_split_across_chunks() {
        let mut p = Predictor::new();
        p.on_input(b"ab");
        // CSI split mid-sequence across two reads.
        p.on_server_output(b"\x1b[");
        p.on_server_output(b"0m");
        assert_eq!(predicted_str(&p), "ab"); // still intact
        p.on_server_output(b"ab");
        assert_eq!(predicted_str(&p), "");
    }
}
