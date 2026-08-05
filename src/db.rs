//! SQLite persistence. Workers, users, sessions, and invite structures live
//! here (path, credential) — the master key *contents* live in `.q-key`, not
//! here. A single connection is used behind a mutex; this tool is low-traffic
//! by design.

use std::path::Path;
use std::sync::Mutex;

use anyhow::Context;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

use crate::crypto::SecretKey;

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path, key: &SecretKey) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating parent of {}", path.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening database {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "foreign_keys", "ON").ok();
        conn.pragma_update(None, "busy_timeout", "5000").ok();
        let db = Db {
            conn: Mutex::new(conn),
        };
        db.migrate(&key)?;
        Ok(db)
    }

    fn migrate(&self, _key: &SecretKey) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS users (
                id            TEXT PRIMARY KEY,
                username      TEXT UNIQUE NOT NULL,
                password_hash TEXT NOT NULL,
                is_admin      INTEGER NOT NULL DEFAULT 0,
                created_at    TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS sessions (
                token_hash TEXT PRIMARY KEY,
                user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                csrf       TEXT NOT NULL,
                created_at TEXT NOT NULL,
                expires_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS invite_tokens (
                token_hash TEXT PRIMARY KEY,
                created_by TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                created_at TEXT NOT NULL,
                used_at    TEXT
            );
            CREATE TABLE IF NOT EXISTS workers (
                id           TEXT PRIMARY KEY,
                name         TEXT NOT NULL,
                kind         TEXT NOT NULL DEFAULT 'opencode',
                url          TEXT NOT NULL,
                username     TEXT NOT NULL DEFAULT 'opencode',
                password_enc TEXT NOT NULL,
                created_at   TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS kv (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;
        // First-run bootstrap: generate and store the data-encryption key for
        // worker passwords. This is separate from `.q-key` so we can rotate
        // independent of the master key if needed.
        let existing: Option<String> = conn
            .query_row("SELECT value FROM kv WHERE key='encryption_key'", [], |r| {
                r.get(0)
            })
            .optional()?;
        if existing.is_none() {
            let k = crate::crypto::new_token();
            conn.execute(
                "INSERT INTO kv (key, value) VALUES ('encryption_key', ?1)",
                [&k],
            )?;
        }
        // Migration: older databases lacked the workers.username column.
        let has_username: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('workers') WHERE name='username'",
            [],
            |r| r.get(0),
        )?;
        if has_username == 0 {
            conn.execute(
                "ALTER TABLE workers ADD COLUMN username TEXT NOT NULL DEFAULT 'opencode'",
                [],
            )?;
        }
        Ok(())
    }
}

// ---- Users ----

#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    pub is_admin: bool,
}

impl Db {
    pub fn create_user(
        &self,
        username: &str,
        password_hash: &str,
        is_admin: bool,
    ) -> anyhow::Result<User> {
        let id = crate::crypto::new_token();
        let now = Utc::now().to_rfc3339();
        let admin = if is_admin { 1 } else { 0 };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO users (id, username, password_hash, is_admin, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, username, password_hash, admin, now],
        )?;
        Ok(User {
            id,
            username: username.into(),
            password_hash: password_hash.into(),
            is_admin,
        })
    }

    pub fn user_by_username(&self, username: &str) -> anyhow::Result<Option<User>> {
        let conn = self.conn.lock().unwrap();
        let u = conn
            .query_row(
                "SELECT id, username, password_hash, is_admin FROM users WHERE username=?1",
                [username],
                |r| {
                    Ok(User {
                        id: r.get(0)?,
                        username: r.get(1)?,
                        password_hash: r.get(2)?,
                        is_admin: r.get::<_, i64>(3)? != 0,
                    })
                },
            )
            .optional()?;
        Ok(u)
    }

    pub fn user_by_id(&self, id: &str) -> anyhow::Result<Option<User>> {
        let conn = self.conn.lock().unwrap();
        let u = conn
            .query_row(
                "SELECT id, username, password_hash, is_admin FROM users WHERE id=?1",
                [id],
                |r| {
                    Ok(User {
                        id: r.get(0)?,
                        username: r.get(1)?,
                        password_hash: r.get(2)?,
                        is_admin: r.get::<_, i64>(3)? != 0,
                    })
                },
            )
            .optional()?;
        Ok(u)
    }

    pub fn count_users(&self) -> anyhow::Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
            .map_err(Into::into)
    }
}

// ---- Sessions (login) ----

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub user_id: String,
    pub csrf: String,
    pub expires_at: String,
}

impl Db {
    pub fn create_session(&self, user_id: &str) -> anyhow::Result<(String, String)> {
        let token = crate::crypto::new_token();
        let csrf = crate::crypto::new_token();
        let hash = crate::crypto::hash_secret(&token);
        let now = Utc::now().to_rfc3339();
        let expires = (Utc::now() + chrono::Duration::hours(24)).to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sessions (token_hash, user_id, csrf, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![hash, user_id, csrf, now, expires],
        )?;
        Ok((token, csrf))
    }

    pub fn session_by_token(&self, token: &str) -> anyhow::Result<Option<SessionRow>> {
        let hash = crate::crypto::hash_secret(token);
        let conn = self.conn.lock().unwrap();
        let s = conn
            .query_row(
                "SELECT user_id, csrf, expires_at FROM sessions WHERE token_hash=?1",
                [&hash],
                |r| {
                    Ok(SessionRow {
                        user_id: r.get(0)?,
                        csrf: r.get(1)?,
                        expires_at: r.get(2)?,
                    })
                },
            )
            .optional()?;
        Ok(s)
    }

    pub fn delete_session(&self, token: &str) -> anyhow::Result<()> {
        let hash = crate::crypto::hash_secret(token);
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM sessions WHERE token_hash=?1", [&hash])?;
        Ok(())
    }
}

// ---- Invite tokens ----

impl Db {
    pub fn create_invite(&self, created_by: &str) -> anyhow::Result<String> {
        let token = crate::crypto::new_token();
        let hash = crate::crypto::hash_secret(&token);
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO invite_tokens (token_hash, created_by, created_at) VALUES (?1, ?2, ?3)",
            params![hash, created_by, now],
        )?;
        Ok(token)
    }

    pub fn invite_valid(&self, token: &str) -> anyhow::Result<bool> {
        let hash = crate::crypto::hash_secret(token);
        let conn = self.conn.lock().unwrap();
        let used: Option<String> = conn
            .query_row(
                "SELECT used_at FROM invite_tokens WHERE token_hash=?1 AND used_at IS NULL",
                [&hash],
                |r| r.get(0),
            )
            .optional()?;
        Ok(used.is_some())
    }

    pub fn consume_invite(&self, token: &str) -> anyhow::Result<bool> {
        let hash = crate::crypto::hash_secret(token);
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let n = conn.execute(
            "UPDATE invite_tokens SET used_at=?2 WHERE token_hash=?1 AND used_at IS NULL",
            params![hash, now],
        )?;
        Ok(n == 1)
    }

    pub fn list_invites(&self) -> anyhow::Result<Vec<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT created_by, created_at FROM invite_tokens WHERE used_at IS NULL ORDER BY created_at DESC",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

// ---- Workers ----

#[derive(Debug, Clone)]
pub struct Worker {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub url: String,
    pub username: String,
    pub password_enc: String,
}

impl Db {
    /// Insert a new worker, encrypting the password at rest.
    pub fn create_worker(
        &self,
        name: &str,
        kind: &str,
        url: &str,
        username: &str,
        password: &str,
        key: &SecretKey,
    ) -> anyhow::Result<Worker> {
        let id = crate::crypto::new_token();
        let enc = key.encrypt(password)?;
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO workers (id, name, kind, url, username, password_enc, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, name, kind, url, username, enc, now],
        )?;
        Ok(Worker {
            id,
            name: name.into(),
            kind: kind.into(),
            url: url.into(),
            username: username.into(),
            password_enc: enc,
        })
    }

    pub fn list_workers(&self) -> anyhow::Result<Vec<Worker>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, name, kind, url, username, password_enc FROM workers ORDER BY name")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Worker {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    kind: r.get(2)?,
                    url: r.get(3)?,
                    username: r.get(4)?,
                    password_enc: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn worker_by_id(&self, id: &str) -> anyhow::Result<Option<Worker>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, name, kind, url, username, password_enc FROM workers WHERE id=?1",
            [id],
            |r| {
                Ok(Worker {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    kind: r.get(2)?,
                    url: r.get(3)?,
                    username: r.get(4)?,
                    password_enc: r.get(5)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    /// Update a worker's metadata. If `password` is empty, the existing
    /// encrypted password is kept; otherwise it is re-encrypted.
    pub fn update_worker(
        &self,
        id: &str,
        name: &str,
        kind: &str,
        url: &str,
        username: &str,
        password: &str,
        key: &SecretKey,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        if password.is_empty() {
            conn.execute(
                "UPDATE workers SET name=?2, kind=?3, url=?4, username=?5 WHERE id=?1",
                params![id, name, kind, url, username],
            )?;
        } else {
            let enc = key.encrypt(password)?;
            conn.execute(
                "UPDATE workers SET name=?2, kind=?3, url=?4, username=?5, password_enc=?6 WHERE id=?1",
                params![id, name, kind, url, username, enc],
            )?;
        }
        Ok(())
    }

    pub fn delete_worker(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM workers WHERE id=?1", [id])?;
        Ok(())
    }
}
