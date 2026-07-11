//! Interactive SSH sessions via `russh`.
//!
//! [`SshSession`] opens a remote shell over an SSH channel and presents the same
//! blocking surface as `core_pty::PtySession` — a [`Read`] stream for output plus
//! `write_all`/`resize` — so the app can drive it with the same reader-thread
//! model. russh is async, so a dedicated OS thread runs a current-thread tokio
//! runtime that drives the channel and bridges to the sync world over channels.

use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;

use russh::ChannelMsg;
use russh::client::{self, Handle};
use russh::keys::{PrivateKeyWithHashAlg, load_secret_key, ssh_key};
use thiserror::Error;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

#[derive(Debug, Error)]
pub enum SshError {
    #[error("failed to start tokio runtime")]
    Runtime(#[source] io::Error),
    #[error("failed to connect to {0}")]
    Connect(#[source] russh::Error),
    #[error("authentication failed")]
    AuthFailed,
    #[error("authentication error")]
    Auth(#[source] russh::Error),
    #[error("failed to load private key '{path}'")]
    Key {
        path: PathBuf,
        #[source]
        source: russh::keys::Error,
    },
    #[error("failed to set up remote shell channel")]
    Channel(#[source] russh::Error),
    #[error("the session driver thread stopped unexpectedly")]
    DriverGone,
    #[error("the session is closed")]
    Closed,
}

/// Terminal dimensions in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub rows: u16,
    pub cols: u16,
}

/// How to authenticate to the server.
#[derive(Debug, Clone)]
pub enum Auth {
    Password(String),
    /// OpenSSH private key file, with an optional passphrase to decrypt it.
    Key {
        path: PathBuf,
        passphrase: Option<String>,
    },
}

/// Where and how to connect.
#[derive(Debug, Clone)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: Auth,
    pub size: Size,
}

/// Commands sent from the sync facade into the async driver loop.
enum Command {
    Write(Vec<u8>),
    Resize(Size),
    Close,
}

/// A live remote shell over SSH.
///
/// Output is consumed through the single reader obtained from [`SshSession::reader`]
/// (unlike a PTY, an SSH channel has one stream, so the reader is taken once). The
/// reader hits EOF when the remote shell exits or the session is dropped.
pub struct SshSession {
    commands: UnboundedSender<Command>,
    output: Option<Receiver<Vec<u8>>>,
    driver: Option<JoinHandle<()>>,
}

impl SshSession {
    /// Connects, authenticates, and starts a remote shell. Blocks until the shell
    /// is ready or an error occurs.
    pub fn connect(config: SshConfig) -> Result<Self, SshError> {
        let (data_tx, data_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let (cmd_tx, cmd_rx) = unbounded_channel::<Command>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), SshError>>();

        let driver = std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = ready_tx.send(Err(SshError::Runtime(e)));
                    return;
                }
            };
            runtime.block_on(run(config, data_tx, cmd_rx, ready_tx));
        });

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                commands: cmd_tx,
                output: Some(data_rx),
                driver: Some(driver),
            }),
            Ok(Err(e)) => Err(e),
            // ready_tx dropped without sending => the driver thread panicked.
            Err(_) => Err(SshError::DriverGone),
        }
    }

    /// Takes the output reader. Returns `None` if already taken.
    pub fn reader(&mut self) -> Option<Box<dyn Read + Send>> {
        self.output
            .take()
            .map(|rx| Box::new(ChannelReader::new(rx)) as Box<dyn Read + Send>)
    }

    pub fn write_all(&self, data: &[u8]) -> Result<(), SshError> {
        self.commands
            .send(Command::Write(data.to_vec()))
            .map_err(|_| SshError::Closed)
    }

    pub fn resize(&self, size: Size) -> Result<(), SshError> {
        self.commands
            .send(Command::Resize(size))
            .map_err(|_| SshError::Closed)
    }

    /// Signals the driver to close the channel and disconnect.
    pub fn close(&self) {
        let _ = self.commands.send(Command::Close);
    }
}

impl Drop for SshSession {
    fn drop(&mut self) {
        self.close();
        if let Some(driver) = self.driver.take() {
            let _ = driver.join();
        }
    }
}

/// Accepts any server host key. `ponytail: prototype only — this is MITM-vulnerable.
/// Upgrade path: verify the key against ~/.ssh/known_hosts before returning true.`
struct AcceptAllHostKeys;

impl client::Handler for AcceptAllHostKeys {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// Connects and authenticates, then opens a shell channel with a PTY.
async fn open_channel(
    config: &SshConfig,
) -> Result<(Handle<AcceptAllHostKeys>, russh::Channel<client::Msg>), SshError> {
    let ssh_config = Arc::new(client::Config::default());
    let mut handle = client::connect(
        ssh_config,
        (config.host.as_str(), config.port),
        AcceptAllHostKeys,
    )
    .await
    .map_err(SshError::Connect)?;

    let result = match &config.auth {
        Auth::Password(password) => handle
            .authenticate_password(&config.user, password.clone())
            .await
            .map_err(SshError::Auth)?,
        Auth::Key { path, passphrase } => {
            let key =
                load_secret_key(path, passphrase.as_deref()).map_err(|source| SshError::Key {
                    path: path.clone(),
                    source,
                })?;
            handle
                .authenticate_publickey(
                    &config.user,
                    PrivateKeyWithHashAlg::new(Arc::new(key), None),
                )
                .await
                .map_err(SshError::Auth)?
        }
    };
    if !result.success() {
        return Err(SshError::AuthFailed);
    }

    let channel = handle
        .channel_open_session()
        .await
        .map_err(SshError::Channel)?;
    channel
        .request_pty(
            true,
            "xterm-256color",
            config.size.cols as u32,
            config.size.rows as u32,
            0,
            0,
            &[],
        )
        .await
        .map_err(SshError::Channel)?;
    channel
        .request_shell(true)
        .await
        .map_err(SshError::Channel)?;

    Ok((handle, channel))
}

/// Sets up the channel, reports readiness, then pumps data both ways until the
/// remote closes or a [`Command::Close`] arrives.
async fn run(
    config: SshConfig,
    data_tx: Sender<Vec<u8>>,
    mut commands: UnboundedReceiver<Command>,
    ready_tx: Sender<Result<(), SshError>>,
) {
    // `_handle` must stay alive for the whole session: dropping it disconnects.
    let (_handle, channel) = match open_channel(&config).await {
        Ok(pair) => pair,
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };
    let (mut reader, writer) = channel.split();
    let _ = ready_tx.send(Ok(()));

    loop {
        tokio::select! {
            msg = reader.wait() => match msg {
                Some(ChannelMsg::Data { data }) => {
                    // Reader dropped on the app side => nothing left to feed.
                    if data_tx.send(data.to_vec()).is_err() {
                        break;
                    }
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    let _ = data_tx.send(data.to_vec());
                }
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                _ => {}
            },
            cmd = commands.recv() => match cmd {
                Some(Command::Write(bytes)) => {
                    let _ = writer.data_bytes(bytes).await;
                }
                Some(Command::Resize(size)) => {
                    let _ = writer
                        .window_change(size.cols as u32, size.rows as u32, 0, 0)
                        .await;
                }
                Some(Command::Close) | None => break,
            },
        }
    }
    // Dropping `data_tx` here makes the app-side reader hit EOF.
}

/// Blocking [`Read`] adapter over the driver's byte stream. Buffers one chunk at a
/// time; a dropped sender (session closed) surfaces as EOF (`Ok(0)`).
struct ChannelReader {
    rx: Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    pos: usize,
}

impl ChannelReader {
    fn new(rx: Receiver<Vec<u8>>) -> Self {
        Self {
            rx,
            chunk: Vec::new(),
            pos: 0,
        }
    }
}

impl Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.pos >= self.chunk.len() {
            match self.rx.recv() {
                Ok(chunk) => {
                    self.chunk = chunk;
                    self.pos = 0;
                }
                Err(_) => return Ok(0), // sender dropped => EOF
            }
        }
        let n = (self.chunk.len() - self.pos).min(out.len());
        out[..n].copy_from_slice(&self.chunk[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_reader_streams_chunks_then_eof() {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        tx.send(b"hello ".to_vec()).unwrap();
        tx.send(b"world".to_vec()).unwrap();
        drop(tx);

        let mut reader = ChannelReader::new(rx);
        let mut out = String::new();
        reader.read_to_string(&mut out).unwrap();
        assert_eq!(out, "hello world");
    }

    #[test]
    fn channel_reader_respects_small_buffers() {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        tx.send(b"abcd".to_vec()).unwrap();
        drop(tx);

        let mut reader = ChannelReader::new(rx);
        let mut buf = [0u8; 3];
        assert_eq!(reader.read(&mut buf).unwrap(), 3);
        assert_eq!(&buf, b"abc");
        assert_eq!(reader.read(&mut buf).unwrap(), 1);
        assert_eq!(&buf[..1], b"d");
        assert_eq!(reader.read(&mut buf).unwrap(), 0); // EOF
    }
}
