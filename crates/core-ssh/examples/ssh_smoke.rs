//! Manual smoke test for the SSH prototype against a real server.
//!
//! Usage:
//!   SSH_PASSWORD=... cargo run -p core-ssh --example ssh_smoke -- user@host [port]
//!   SSH_KEY=~/.ssh/id_ed25519 cargo run -p core-ssh --example ssh_smoke -- user@host
//!
//! Opens a remote shell, prints its output, runs `echo` + `exit`, and stops.

use std::io::Read;
use std::thread;
use std::time::Duration;

use core_ssh::{Auth, SshConfig, SshSession, Size};

fn main() {
    let target = std::env::args().nth(1).expect("usage: ssh_smoke user@host [port]");
    let port: u16 = std::env::args()
        .nth(2)
        .and_then(|p| p.parse().ok())
        .unwrap_or(22);
    let (user, host) = target.split_once('@').expect("expected user@host");

    let auth = if let Ok(path) = std::env::var("SSH_KEY") {
        Auth::Key {
            path: path.into(),
            passphrase: std::env::var("SSH_KEY_PASSPHRASE").ok(),
        }
    } else {
        Auth::Password(std::env::var("SSH_PASSWORD").expect("set SSH_PASSWORD or SSH_KEY"))
    };

    let mut session = SshSession::connect(SshConfig {
        host: host.to_owned(),
        port,
        user: user.to_owned(),
        auth,
        size: Size { rows: 24, cols: 80 },
    })
    .expect("connect");
    println!("-- connected, remote shell open --");

    let mut reader = session.reader().expect("reader");
    let printer = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            print!("{}", String::from_utf8_lossy(&buf[..n]));
        }
    });

    session.write_all(b"echo hello from ssh\r\n").expect("write");
    thread::sleep(Duration::from_millis(500));
    session.write_all(b"exit\r\n").expect("write");

    printer.join().ok();
    println!("\n-- session closed --");
}
