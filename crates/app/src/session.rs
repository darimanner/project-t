//! Uniform interface over terminal byte sources (local PTY, SSH), so the
//! terminal view/renderer stay blind to where bytes come from. The future
//! predictive-echo layer becomes just another implementation.

use std::io::Read;

use core_pty::{PtySession, SpawnConfig};
use core_ssh::{SshConfig, SshError, SshSession};

/// A live terminal byte source. Adapter errors are logged, not propagated —
/// a dead session surfaces to the app as reader EOF.
pub trait Session: Send {
    /// Output stream; may only be taken once.
    fn take_reader(&mut self) -> Option<Box<dyn Read + Send>>;
    fn write_all(&mut self, data: &[u8]);
    fn resize(&mut self, rows: u16, cols: u16);
    fn kill(&mut self);
}

pub struct LocalSession(PtySession);

impl LocalSession {
    pub fn spawn(rows: u16, cols: u16) -> Result<Self, core_pty::PtyError> {
        let config = SpawnConfig::user_shell(core_pty::Size { rows, cols });
        Ok(Self(PtySession::spawn(&config)?))
    }
}

impl Session for LocalSession {
    fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
        self.0.reader().ok()
    }

    fn write_all(&mut self, data: &[u8]) {
        if let Err(err) = self.0.write_all(data) {
            eprintln!("pty write failed: {err}");
        }
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        if let Err(err) = self.0.resize(core_pty::Size { rows, cols }) {
            eprintln!("pty resize failed: {err}");
        }
    }

    fn kill(&mut self) {
        let _ = self.0.kill();
    }
}

pub struct RemoteSession(SshSession);

impl RemoteSession {
    /// Blocking connect + auth + shell; call from a background thread.
    pub fn connect(config: SshConfig) -> Result<Self, SshError> {
        Ok(Self(SshSession::connect(config)?))
    }
}

impl Session for RemoteSession {
    fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
        self.0.reader()
    }

    fn write_all(&mut self, data: &[u8]) {
        if let Err(err) = self.0.write_all(data) {
            eprintln!("ssh write failed: {err}");
        }
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        if let Err(err) = self.0.resize(core_ssh::Size { rows, cols }) {
            eprintln!("ssh resize failed: {err}");
        }
    }

    fn kill(&mut self) {
        self.0.close();
    }
}
