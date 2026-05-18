//! Adapter-owned SQLite context/cursor store.
//!
//! Persists WeChat context tokens and per-account polling cursors in private
//! tables inside lucarned state SQLite.

use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection, OptionalExtension};
use wechat_ilink::WechatContext;

/// A stored WeChat context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WechatStoredContext {
    /// The context token and metadata.
    pub context: WechatContext,
    /// When `true` the context is excluded from sends.
    pub disabled: bool,
}

/// A stored polling cursor for one WeChat account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WechatStoredCursor {
    pub account_key: String,
    pub cursor: String,
    pub updated_at_unix_ms: i64,
}

/// Thread-safe, SQLite-backed WeChat context store.
#[derive(Debug, Clone)]
pub struct WechatContextStore {
    conn: Arc<Mutex<Connection>>,
}

impl WechatContextStore {
    /// Open the store using lucarned's state database connection.
    pub fn open(conn: Arc<Mutex<Connection>>) -> std::io::Result<Self> {
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// Insert or update a stored context.
    pub async fn upsert_context(&self, context: WechatContext) -> std::io::Result<()> {
        let conn = self.conn.lock().expect("wechat context sqlite lock");
        conn.execute(
            "INSERT INTO lucarne_wechat_contexts (
                account_key, user_id, context_token, observed_at_unix_ms, source_message_id, disabled
             ) VALUES (?1, ?2, ?3, ?4, ?5, 0)
             ON CONFLICT(account_key, user_id) DO UPDATE SET
                context_token = excluded.context_token,
                observed_at_unix_ms = excluded.observed_at_unix_ms,
                source_message_id = excluded.source_message_id,
                disabled = 0",
            params![
                context.account_key,
                context.user_id,
                context.context_token,
                context.observed_at_unix_ms,
                context.source_message_id,
            ],
        )
        .map_err(sqlite_io_error)?;
        Ok(())
    }

    /// Look up a stored context by account and user id.
    pub async fn context(
        &self,
        account_key: &str,
        user_id: &str,
    ) -> std::io::Result<Option<WechatStoredContext>> {
        let conn = self.conn.lock().expect("wechat context sqlite lock");
        conn.query_row(
            "SELECT account_key, user_id, context_token, observed_at_unix_ms, source_message_id, disabled
             FROM lucarne_wechat_contexts
             WHERE account_key = ?1 AND user_id = ?2",
            params![account_key, user_id],
            stored_context_from_row,
        )
        .optional()
        .map_err(sqlite_io_error)
    }

    /// Return every stored context (enabled and disabled).
    pub async fn all_contexts(&self) -> std::io::Result<Vec<WechatStoredContext>> {
        let conn = self.conn.lock().expect("wechat context sqlite lock");
        let mut stmt = conn
            .prepare(
                "SELECT account_key, user_id, context_token, observed_at_unix_ms, source_message_id, disabled
                 FROM lucarne_wechat_contexts
                 ORDER BY account_key, user_id",
            )
            .map_err(sqlite_io_error)?;
        let rows = stmt
            .query_map([], stored_context_from_row)
            .map_err(sqlite_io_error)?;
        let mut contexts = Vec::new();
        for row in rows {
            contexts.push(row.map_err(sqlite_io_error)?);
        }
        Ok(contexts)
    }

    /// Persist a polling cursor for an account.
    pub async fn save_cursor(
        &self,
        account_key: &str,
        cursor: String,
        updated_at_unix_ms: i64,
    ) -> std::io::Result<()> {
        let conn = self.conn.lock().expect("wechat context sqlite lock");
        conn.execute(
            "INSERT INTO lucarne_wechat_cursors (account_key, cursor, updated_at_unix_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(account_key) DO UPDATE SET
                cursor = excluded.cursor,
                updated_at_unix_ms = excluded.updated_at_unix_ms",
            params![account_key, cursor, updated_at_unix_ms],
        )
        .map_err(sqlite_io_error)?;
        Ok(())
    }

    /// Return the last-saved polling cursor for an account, if any.
    pub async fn cursor(&self, account_key: &str) -> std::io::Result<Option<String>> {
        let conn = self.conn.lock().expect("wechat context sqlite lock");
        conn.query_row(
            "SELECT cursor FROM lucarne_wechat_cursors WHERE account_key = ?1",
            params![account_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(sqlite_io_error)
    }

    /// Return the stored cursor record for an account, if any.
    pub async fn cursor_record(
        &self,
        account_key: &str,
    ) -> std::io::Result<Option<WechatStoredCursor>> {
        let conn = self.conn.lock().expect("wechat context sqlite lock");
        conn.query_row(
            "SELECT account_key, cursor, updated_at_unix_ms
             FROM lucarne_wechat_cursors
             WHERE account_key = ?1",
            params![account_key],
            |row| {
                Ok(WechatStoredCursor {
                    account_key: row.get(0)?,
                    cursor: row.get(1)?,
                    updated_at_unix_ms: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(sqlite_io_error)
    }

    /// Set the `disabled` flag on a stored context.
    pub async fn set_disabled(
        &self,
        account_key: &str,
        user_id: &str,
        disabled: bool,
    ) -> std::io::Result<()> {
        let conn = self.conn.lock().expect("wechat context sqlite lock");
        conn.execute(
            "UPDATE lucarne_wechat_contexts
             SET disabled = ?3
             WHERE account_key = ?1 AND user_id = ?2",
            params![account_key, user_id, disabled],
        )
        .map_err(sqlite_io_error)?;
        Ok(())
    }

    /// Disable all stored contexts for the given account.
    pub async fn disable_account(&self, account_key: &str) -> std::io::Result<()> {
        let conn = self.conn.lock().expect("wechat context sqlite lock");
        conn.execute(
            "UPDATE lucarne_wechat_contexts SET disabled = 1 WHERE account_key = ?1",
            params![account_key],
        )
        .map_err(sqlite_io_error)?;
        Ok(())
    }

    /// Return the WechatContext for a user if it exists and is enabled.
    pub async fn enabled_context_for_user(
        &self,
        account_key: &str,
        user_id: &str,
    ) -> std::io::Result<Option<wechat_ilink::WechatContext>> {
        let context = self.context(account_key, user_id).await?;
        Ok(context
            .filter(|entry| !entry.disabled)
            .map(|entry| entry.context))
    }

    /// Find an enabled context by user_id across all accounts.
    pub async fn context_by_user(
        &self,
        user_id: &str,
    ) -> std::io::Result<Option<wechat_ilink::WechatContext>> {
        let conn = self.conn.lock().expect("wechat context sqlite lock");
        conn.query_row(
            "SELECT account_key, user_id, context_token, observed_at_unix_ms, source_message_id, disabled
             FROM lucarne_wechat_contexts
             WHERE user_id = ?1 AND disabled = 0
             ORDER BY observed_at_unix_ms DESC
             LIMIT 1",
            params![user_id],
            stored_context_from_row,
        )
        .optional()
        .map(|entry| entry.map(|entry| entry.context))
        .map_err(sqlite_io_error)
    }

    fn init_schema(&self) -> std::io::Result<()> {
        let conn = self.conn.lock().expect("wechat context sqlite lock");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS lucarne_wechat_contexts (
                account_key TEXT NOT NULL,
                user_id TEXT NOT NULL,
                context_token TEXT NOT NULL,
                observed_at_unix_ms INTEGER NOT NULL,
                source_message_id TEXT,
                disabled INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(account_key, user_id)
            );
            CREATE INDEX IF NOT EXISTS idx_lucarne_wechat_contexts_user_enabled
                ON lucarne_wechat_contexts(user_id, disabled, observed_at_unix_ms DESC);
            CREATE TABLE IF NOT EXISTS lucarne_wechat_cursors (
                account_key TEXT NOT NULL PRIMARY KEY,
                cursor TEXT NOT NULL,
                updated_at_unix_ms INTEGER NOT NULL
            );",
        )
        .map_err(sqlite_io_error)
    }
}

fn stored_context_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WechatStoredContext> {
    Ok(WechatStoredContext {
        context: WechatContext {
            account_key: row.get(0)?,
            user_id: row.get(1)?,
            context_token: row.get(2)?,
            observed_at_unix_ms: row.get(3)?,
            source_message_id: row.get(4)?,
        },
        disabled: row.get(5)?,
    })
}

fn sqlite_io_error(err: rusqlite::Error) -> std::io::Error {
    std::io::Error::other(err)
}
