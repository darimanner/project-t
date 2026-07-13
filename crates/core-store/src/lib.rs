//! SQLite storage (hosts, history) plus OS-keychain secrets.
//!
//! Sketch for phase 0: the DB holds connection metadata and command history but
//! never secrets — passwords live in the OS keychain (via [`secrets`]) and the
//! DB only records the auth *kind*. Enough to persist a host list and recall it.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error")]
    Db(#[from] rusqlite::Error),
    #[error("unknown auth kind '{0}' in database")]
    BadAuthKind(String),
    #[error("invalid hosts JSON")]
    Json(#[from] serde_json::Error),
}

/// How a host authenticates. The secret itself lives in the keychain, not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthKind {
    Password,
    Key,
}

impl AuthKind {
    fn as_str(self) -> &'static str {
        match self {
            AuthKind::Password => "password",
            AuthKind::Key => "key",
        }
    }

    fn parse(s: &str) -> Result<Self, StoreError> {
        match s {
            "password" => Ok(AuthKind::Password),
            "key" => Ok(AuthKind::Key),
            other => Err(StoreError::BadAuthKind(other.to_owned())),
        }
    }
}

/// A saved SSH host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub id: i64,
    pub label: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthKind,
    /// Private-key path, when `auth == Key`.
    pub key_path: Option<String>,
}

/// Fields for creating a host (no id yet). Also the JSON schema for host
/// export/import — deliberately secret-free.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewHost {
    pub label: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthKind,
    pub key_path: Option<String>,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (creating if needed) a database file and applies the schema.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::from_conn(Connection::open(path)?)
    }

    /// In-memory database, for tests.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> Result<Self, StoreError> {
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS hosts (
                 id         INTEGER PRIMARY KEY,
                 label      TEXT    NOT NULL,
                 host       TEXT    NOT NULL,
                 port       INTEGER NOT NULL DEFAULT 22,
                 username   TEXT    NOT NULL,
                 auth       TEXT    NOT NULL,
                 key_path   TEXT,
                 created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS history (
                 id      INTEGER PRIMARY KEY,
                 host_id INTEGER NOT NULL REFERENCES hosts(id) ON DELETE CASCADE,
                 line    TEXT    NOT NULL,
                 ts      INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_history_host ON history(host_id, ts);",
        )?;
        Ok(Self { conn })
    }

    pub fn add_host(&self, host: &NewHost) -> Result<i64, StoreError> {
        self.conn.execute(
            "INSERT INTO hosts (label, host, port, username, auth, key_path, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                host.label,
                host.host,
                host.port,
                host.username,
                host.auth.as_str(),
                host.key_path,
                now(),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_hosts(&self) -> Result<Vec<Host>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, label, host, port, username, auth, key_path
             FROM hosts ORDER BY label",
        )?;
        let rows = stmt.query_map([], row_to_host)?;
        // Each row yields Result<Host, StoreError>; flatten the two error layers.
        rows.collect::<Result<Vec<_>, _>>()?.into_iter().collect()
    }

    pub fn get_host(&self, id: i64) -> Result<Option<Host>, StoreError> {
        self.conn
            .query_row(
                "SELECT id, label, host, port, username, auth, key_path
                 FROM hosts WHERE id = ?1",
                params![id],
                row_to_host,
            )
            .optional()?
            .transpose()
    }

    pub fn delete_host(&self, id: i64) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM hosts WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn record_history(&self, host_id: i64, line: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO history (host_id, line, ts) VALUES (?1, ?2, ?3)",
            params![host_id, line, now()],
        )?;
        Ok(())
    }

    /// All hosts as pretty JSON (the [`NewHost`] schema — ids and secrets are
    /// never exported).
    pub fn export_hosts_json(&self) -> Result<String, StoreError> {
        let hosts: Vec<NewHost> = self
            .list_hosts()?
            .into_iter()
            .map(|h| NewHost {
                label: h.label,
                host: h.host,
                port: h.port,
                username: h.username,
                auth: h.auth,
                key_path: h.key_path,
            })
            .collect();
        Ok(serde_json::to_string_pretty(&hosts)?)
    }

    /// Adds every host from a JSON export. Returns how many were added.
    /// Passwords are not part of the format and must be re-entered.
    pub fn import_hosts_json(&self, json: &str) -> Result<usize, StoreError> {
        let hosts: Vec<NewHost> = serde_json::from_str(json)?;
        for host in &hosts {
            self.add_host(host)?;
        }
        Ok(hosts.len())
    }

    /// Most recent history lines for a host, newest first.
    pub fn recent_history(&self, host_id: i64, limit: u32) -> Result<Vec<String>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT line FROM history WHERE host_id = ?1 ORDER BY ts DESC, id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![host_id, limit], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

/// Maps a row to a `Host`. An invalid `auth` value surfaces as the inner
/// `Result` so it flows through `query_map` without conflating with SQL errors.
fn row_to_host(row: &rusqlite::Row) -> rusqlite::Result<Result<Host, StoreError>> {
    let auth_str: String = row.get(5)?;
    let port: i64 = row.get(3)?;
    let id: i64 = row.get(0)?;
    let label: String = row.get(1)?;
    let host: String = row.get(2)?;
    let username: String = row.get(4)?;
    let key_path: Option<String> = row.get(6)?;
    Ok(AuthKind::parse(&auth_str).map(|auth| Host {
        id,
        label,
        host,
        port: port as u16,
        username,
        auth,
        key_path,
    }))
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// OS-keychain secrets, keyed by host id. The database never stores these.
pub mod secrets {
    use keyring::Entry;

    const SERVICE: &str = "project-t-ssh";

    fn entry(host_id: i64) -> keyring::Result<Entry> {
        Entry::new(SERVICE, &host_id.to_string())
    }

    pub fn set_password(host_id: i64, password: &str) -> keyring::Result<()> {
        entry(host_id)?.set_password(password)
    }

    /// Returns `None` when no secret is stored for this host.
    pub fn get_password(host_id: i64) -> keyring::Result<Option<String>> {
        match entry(host_id)?.get_password() {
            Ok(pw) => Ok(Some(pw)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn delete_password(host_id: i64) -> keyring::Result<()> {
        match entry(host_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> NewHost {
        NewHost {
            label: "prod".to_owned(),
            host: "10.0.0.1".to_owned(),
            port: 22,
            username: "root".to_owned(),
            auth: AuthKind::Key,
            key_path: Some("~/.ssh/id_ed25519".to_owned()),
        }
    }

    #[test]
    fn add_and_get_host_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        let id = store.add_host(&sample()).unwrap();

        let got = store.get_host(id).unwrap().expect("host exists");
        assert_eq!(got.label, "prod");
        assert_eq!(got.port, 22);
        assert_eq!(got.auth, AuthKind::Key);
        assert_eq!(got.key_path.as_deref(), Some("~/.ssh/id_ed25519"));
    }

    #[test]
    fn list_is_sorted_and_delete_works() {
        let store = Store::open_in_memory().unwrap();
        let mut b = sample();
        b.label = "zeta".to_owned();
        let mut a = sample();
        a.label = "alpha".to_owned();
        store.add_host(&b).unwrap();
        let a_id = store.add_host(&a).unwrap();

        let labels: Vec<_> = store
            .list_hosts()
            .unwrap()
            .into_iter()
            .map(|h| h.label)
            .collect();
        assert_eq!(labels, vec!["alpha", "zeta"]);

        store.delete_host(a_id).unwrap();
        assert!(store.get_host(a_id).unwrap().is_none());
    }

    #[test]
    fn history_is_scoped_and_ordered() {
        let store = Store::open_in_memory().unwrap();
        let h1 = store.add_host(&sample()).unwrap();
        let h2 = store.add_host(&sample()).unwrap();

        store.record_history(h1, "ls").unwrap();
        store.record_history(h1, "cd /tmp").unwrap();
        store.record_history(h2, "whoami").unwrap();

        let recent = store.recent_history(h1, 10).unwrap();
        assert_eq!(recent, vec!["cd /tmp", "ls"]); // newest first
        assert_eq!(store.recent_history(h2, 10).unwrap(), vec!["whoami"]);
    }

    #[test]
    fn export_import_roundtrip() {
        let src = Store::open_in_memory().unwrap();
        src.add_host(&sample()).unwrap();
        let json = src.export_hosts_json().unwrap();
        assert!(!json.contains("password\":")); // schema holds kind, not secrets

        let dst = Store::open_in_memory().unwrap();
        assert_eq!(dst.import_hosts_json(&json).unwrap(), 1);
        let hosts = dst.list_hosts().unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].label, "prod");
        assert_eq!(hosts[0].auth, AuthKind::Key);
    }

    #[test]
    fn deleting_host_cascades_history() {
        let store = Store::open_in_memory().unwrap();
        let id = store.add_host(&sample()).unwrap();
        store.record_history(id, "ls").unwrap();
        store.delete_host(id).unwrap();
        assert!(store.recent_history(id, 10).unwrap().is_empty());
    }
}
