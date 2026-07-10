//! Local PTY sessions: spawn a shell, read output, write input.

use std::io::{Read, Write};
use std::path::PathBuf;

use portable_pty::{Child, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system};
use thiserror::Error;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("failed to open pty")]
    Open(#[source] BoxError),
    #[error("failed to spawn '{command}' in pty")]
    Spawn {
        command: String,
        #[source]
        source: BoxError,
    },
    #[error("failed to clone pty reader")]
    Reader(#[source] BoxError),
    #[error("failed to take pty writer")]
    Writer(#[source] BoxError),
    #[error("failed to resize pty")]
    Resize(#[source] BoxError),
    #[error("pty i/o failed")]
    Io(#[from] std::io::Error),
}

/// Terminal dimensions in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub rows: u16,
    pub cols: u16,
}

/// What to launch inside the PTY.
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub size: Size,
}

impl SpawnConfig {
    /// Config for the user's login shell (`$SHELL`, falling back to `/bin/zsh`).
    pub fn user_shell(size: Size) -> Self {
        let program = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned());
        Self {
            program,
            args: Vec::new(),
            cwd: None,
            size,
        }
    }
}

/// A live PTY with a child process attached.
///
/// Reads are blocking: obtain a reader via [`PtySession::reader`] and consume it
/// from a dedicated thread. The reader hits EOF once the child exits.
pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl PtySession {
    pub fn spawn(config: &SpawnConfig) -> Result<Self, PtyError> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: config.size.rows,
                cols: config.size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Open(e.into()))?;

        let mut cmd = CommandBuilder::new(&config.program);
        cmd.args(&config.args);
        if let Some(cwd) = &config.cwd {
            cmd.cwd(cwd);
        }
        cmd.env("TERM", "xterm-256color");

        let child = pair.slave.spawn_command(cmd).map_err(|e| PtyError::Spawn {
            command: config.program.clone(),
            source: e.into(),
        })?;
        // Drop our copy of the slave end so the master reader sees EOF when the
        // child exits instead of blocking forever.
        drop(pair.slave);

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| PtyError::Writer(e.into()))?;

        Ok(Self {
            master: pair.master,
            writer,
            child,
        })
    }

    /// Clones a blocking reader for the terminal output stream.
    pub fn reader(&self) -> Result<Box<dyn Read + Send>, PtyError> {
        self.master
            .try_clone_reader()
            .map_err(|e| PtyError::Reader(e.into()))
    }

    /// Writes keyboard input (already encoded as terminal bytes) to the child.
    pub fn write_all(&mut self, data: &[u8]) -> Result<(), PtyError> {
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, size: Size) -> Result<(), PtyError> {
        self.master
            .resize(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Resize(e.into()))
    }

    /// Blocks until the child exits.
    pub fn wait(&mut self) -> Result<ExitStatus, PtyError> {
        Ok(self.child.wait()?)
    }

    /// Terminates the child process (used when a pane is closed).
    pub fn kill(&mut self) -> Result<(), PtyError> {
        Ok(self.child.kill()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    const SIZE: Size = Size { rows: 24, cols: 80 };

    /// Reads the whole output stream on a background thread so a broken test
    /// fails by timeout instead of hanging forever on a blocking read.
    fn collect_output(mut reader: Box<dyn Read + Send>) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut out = String::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                out.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
            let _ = tx.send(out);
        });
        rx
    }

    #[test]
    fn echo_roundtrip() {
        let config = SpawnConfig {
            program: "/bin/sh".to_owned(),
            args: vec!["-c".to_owned(), "echo hello".to_owned()],
            cwd: None,
            size: SIZE,
        };
        let mut session = PtySession::spawn(&config).expect("spawn");
        let rx = collect_output(session.reader().expect("reader"));

        let out = rx.recv_timeout(Duration::from_secs(5)).expect("output");
        assert!(out.contains("hello"), "unexpected output: {out:?}");

        let status = session.wait().expect("wait");
        assert!(status.success());
    }

    #[test]
    fn write_reaches_child() {
        let config = SpawnConfig {
            program: "/bin/sh".to_owned(),
            args: vec!["-c".to_owned(), "read line; echo got:$line".to_owned()],
            cwd: None,
            size: SIZE,
        };
        let mut session = PtySession::spawn(&config).expect("spawn");
        let rx = collect_output(session.reader().expect("reader"));

        session.write_all(b"ping\n").expect("write");

        let out = rx.recv_timeout(Duration::from_secs(5)).expect("output");
        assert!(out.contains("got:ping"), "unexpected output: {out:?}");
    }

    #[test]
    fn resize_succeeds() {
        let config = SpawnConfig::user_shell(SIZE);
        let mut session = PtySession::spawn(&config).expect("spawn");
        session
            .resize(Size {
                rows: 50,
                cols: 120,
            })
            .expect("resize");
        session.kill().expect("kill");
    }
}
