use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use crate::channel::TelegramBotCommand;

const MENU_SCOPE: &str = "default_menu";

pub(crate) struct TelegramCommandSyncCache {
    conn: Arc<Mutex<Connection>>,
}

impl TelegramCommandSyncCache {
    pub(crate) fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    pub(crate) fn sync_needed(
        &self,
        cache_key: &str,
        command_hash: &str,
    ) -> Result<bool, rusqlite::Error> {
        let conn = self.conn.lock().expect("telegram command sync cache lock");
        ensure_table(&conn)?;
        let stored = conn
            .query_row(
                "SELECT command_hash
                 FROM telegram_command_sync_state
                 WHERE cache_key = ?1",
                params![cache_key],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(stored.as_deref() != Some(command_hash))
    }

    pub(crate) fn record_synced(
        &self,
        cache_key: &str,
        command_hash: &str,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().expect("telegram command sync cache lock");
        ensure_table(&conn)?;
        conn.execute(
            "INSERT INTO telegram_command_sync_state (cache_key, command_hash, updated_unix_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(cache_key) DO UPDATE SET
                command_hash = excluded.command_hash,
                updated_unix_ms = excluded.updated_unix_ms",
            params![cache_key, command_hash, unix_ms_now()],
        )?;
        Ok(())
    }
}

fn ensure_table(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS telegram_command_sync_state (
            cache_key TEXT PRIMARY KEY,
            command_hash TEXT NOT NULL,
            updated_unix_ms INTEGER NOT NULL
         ) WITHOUT ROWID",
        [],
    )?;
    Ok(())
}

pub(crate) fn telegram_command_sync_cache_key(token: &str) -> Option<String> {
    let (bot_id, _) = token.split_once(':')?;
    let bot_id = bot_id.trim();
    if bot_id.is_empty() || !bot_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(format!("bot:{bot_id}:{MENU_SCOPE}"))
}

pub(crate) fn telegram_command_hash(commands: &[TelegramBotCommand]) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;

    let mut hash = FNV_OFFSET;
    feed_hash(&mut hash, "telegram-command-menu-v1");
    for command in commands {
        feed_hash(&mut hash, command.command);
        feed_hash(&mut hash, command.description);
    }
    format!("v1:{hash:016x}")
}

fn feed_hash(hash: &mut u64, value: &str) {
    const FNV_PRIME: u64 = 0x100000001b3;
    for byte in value.bytes().chain(std::iter::once(0)) {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

fn unix_ms_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> TelegramCommandSyncCache {
        TelegramCommandSyncCache::new(Arc::new(Mutex::new(
            Connection::open_in_memory().expect("open in-memory sqlite"),
        )))
    }

    #[test]
    fn command_hash_changes_when_menu_changes() {
        let baseline = telegram_command_hash(&[TelegramBotCommand {
            command: "start",
            description: "Open the panel",
        }]);
        let changed = telegram_command_hash(&[TelegramBotCommand {
            command: "start",
            description: "Refresh the panel",
        }]);

        assert_ne!(baseline, changed);
    }

    #[test]
    fn cache_skips_matching_command_hash_after_successful_record() {
        let cache = cache();
        let hash = telegram_command_hash(&[TelegramBotCommand {
            command: "start",
            description: "Open the panel",
        }]);

        assert!(cache
            .sync_needed("bot:1:default_menu", &hash)
            .expect("read cache"));
        cache
            .record_synced("bot:1:default_menu", &hash)
            .expect("write cache");

        assert!(!cache
            .sync_needed("bot:1:default_menu", &hash)
            .expect("read cache"));
    }

    #[test]
    fn cache_is_scoped_by_bot_id() {
        let cache = cache();
        let hash = telegram_command_hash(&[TelegramBotCommand {
            command: "start",
            description: "Open the panel",
        }]);
        cache
            .record_synced("bot:1:default_menu", &hash)
            .expect("write cache");

        assert!(cache
            .sync_needed("bot:2:default_menu", &hash)
            .expect("read cache"));
    }

    #[test]
    fn cache_key_uses_bot_id_without_storing_token_secret() {
        assert_eq!(
            telegram_command_sync_cache_key("123456:secret-token"),
            Some("bot:123456:default_menu".to_string())
        );
        assert_eq!(telegram_command_sync_cache_key("not-a-token"), None);
    }
}
