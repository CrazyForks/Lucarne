use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::SystemTime,
};

use rusqlite::{
    params,
    types::{Type, ValueRef},
    Connection, OptionalExtension,
};
use serde::Deserialize;
use smol_str::SmolStr;

use super::{
    message_session_binding_id, ControlPlanePersistenceEntity, ControlPlaneState,
    MessageSessionBinding, TimelineItem, TimelineItemKind, TimelineSeq, TurnId, WorkspaceId,
};
use tracing::{debug, info};

#[derive(Debug, thiserror::Error)]
pub enum ControlPlaneStoreError {
    #[error("control-plane sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("control-plane json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("control-plane io: {0}")]
    Io(#[from] std::io::Error),
}

struct SerializedEntity {
    kind: String,
    entity_id: String,
    workspace_id: Option<String>,
    state_json: String,
}

#[derive(Debug, Deserialize)]
struct TimelineIndexState {
    workspace_id: WorkspaceId,
    turn_id: TurnId,
    seq: TimelineSeq,
    epoch: u64,
    provider_item_id: Option<SmolStr>,
    kind: TimelineItemKind,
    created_at: SystemTime,
}

#[derive(Clone)]
pub struct ControlPlaneSqliteStore {
    conn: Arc<Mutex<Connection>>,
}

impl ControlPlaneSqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ControlPlaneStoreError> {
        if let Some(parent) = path.as_ref().parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        info!(
            target: "lucarne::control_plane::store",
            path = ?path.as_ref(),
            "control-plane sqlite opened"
        );
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    pub fn open_in_memory() -> Result<Self, ControlPlaneStoreError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, ControlPlaneStoreError> {
        conn.execute_batch("PRAGMA mmap_size = 268435456; PRAGMA cache_size = -100;")?;
        conn.execute_batch(
            "DROP TABLE IF EXISTS work_sessions;
            CREATE TABLE IF NOT EXISTS control_plane_entities (
                kind TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                workspace_id TEXT,
                state_json TEXT NOT NULL,
                PRIMARY KEY(kind, entity_id)
            );
            CREATE INDEX IF NOT EXISTS idx_control_plane_entities_workspace
                ON control_plane_entities(workspace_id, kind);
            CREATE INDEX IF NOT EXISTS idx_control_plane_entities_kind
                ON control_plane_entities(kind);",
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn clone_connection(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.conn)
    }

    pub fn load_control_plane(&self) -> Result<Option<ControlPlaneState>, ControlPlaneStoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT kind, entity_id, workspace_id, state_json
             FROM control_plane_entities
             WHERE kind NOT IN ('timeline', 'message_session_binding')
             ORDER BY kind, workspace_id, entity_id",
        )?;
        let rows = stmt.query_map([], |row| {
            let kind: String = row.get(0)?;
            let state = decode_entity_state(kind.as_str(), row.get_ref(3)?)?;
            Ok(ControlPlanePersistenceEntity {
                kind,
                entity_id: row.get(1)?,
                workspace_id: row.get(2)?,
                state,
            })
        })?;
        let mut entities = Vec::new();
        for row in rows {
            entities.push(row?);
        }
        if entities.is_empty() {
            debug!(
                target: "lucarne::control_plane::store",
                "control-plane store empty"
            );
            return Ok(None);
        }
        debug!(
            target: "lucarne::control_plane::store",
            entity_count = entities.len(),
            "control-plane entities loaded (timeline and message bindings deferred)"
        );
        let mut state = ControlPlaneState::from_persistence_entities(entities)?;
        state.set_timeline_store(Arc::clone(&self.conn));
        Ok(Some(state))
    }

    pub fn message_session_binding(
        &self,
        channel: &str,
        chat_id: &str,
        message_id: &str,
    ) -> Result<Option<MessageSessionBinding>, ControlPlaneStoreError> {
        let conn = self.conn.lock().unwrap();
        let binding_id = message_session_binding_id(channel, chat_id, message_id);
        let state_json = conn
            .query_row(
                "SELECT state_json
                 FROM control_plane_entities
                 WHERE kind = 'message_session_binding' AND entity_id = ?1",
                params![binding_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        state_json
            .map(|state_json| serde_json::from_str(&state_json).map_err(Into::into))
            .transpose()
    }

    pub fn timeline_item(
        &self,
        workspace_id: &WorkspaceId,
        seq: TimelineSeq,
    ) -> Result<Option<TimelineItem>, ControlPlaneStoreError> {
        let conn = self.conn.lock().unwrap();
        let entity_id = format!("{}:{}", workspace_id.as_str(), seq.get());
        let state_json = conn
            .query_row(
                "SELECT state_json
                 FROM control_plane_entities
                 WHERE kind = 'timeline' AND entity_id = ?1",
                params![entity_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        state_json
            .map(|state_json| serde_json::from_str(&state_json).map_err(Into::into))
            .transpose()
    }

    pub fn upsert_entities(
        &self,
        entities: Vec<ControlPlanePersistenceEntity>,
    ) -> Result<(), ControlPlaneStoreError> {
        let entities = serialize_entities(entities)?;
        let entity_count = entities.len();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO control_plane_entities
                    (kind, entity_id, workspace_id, state_json)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(kind, entity_id) DO UPDATE SET
                    workspace_id = excluded.workspace_id,
                    state_json = excluded.state_json",
            )?;
            for entity in entities {
                stmt.execute(params![
                    entity.kind,
                    entity.entity_id,
                    entity.workspace_id,
                    entity.state_json
                ])?;
            }
        }
        tx.commit()?;
        debug!(
            target: "lucarne::control_plane::store",
            entity_count,
            "control-plane entities upserted"
        );
        Ok(())
    }

    /// Replace snapshot-owned entities in one transaction while preserving
    /// rows whose serialized state did not change. Timeline and message-session
    /// binding rows are lazy/on-demand records and are not owned by snapshots.
    pub fn replace_non_timeline_entities(
        &self,
        entities: Vec<ControlPlanePersistenceEntity>,
    ) -> Result<(), ControlPlaneStoreError> {
        let entities = serialize_entities(entities)?;
        let entity_count = entities.len();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            tx.execute(
                "CREATE TEMP TABLE IF NOT EXISTS control_plane_replace_keys (
                    kind TEXT NOT NULL,
                    entity_id TEXT NOT NULL,
                    PRIMARY KEY(kind, entity_id)
                 ) WITHOUT ROWID",
                [],
            )?;
            tx.execute("DELETE FROM control_plane_replace_keys", [])?;
            {
                let mut key_stmt = tx.prepare(
                    "INSERT INTO control_plane_replace_keys (kind, entity_id)
                     VALUES (?1, ?2)",
                )?;
                for entity in &entities {
                    debug_assert_ne!(entity.kind, "timeline");
                    key_stmt.execute(params![entity.kind, entity.entity_id])?;
                }
            }
            tx.execute(
                "DELETE FROM control_plane_entities
                 WHERE kind NOT IN ('timeline', 'message_session_binding')
                   AND NOT EXISTS (
                       SELECT 1
                       FROM control_plane_replace_keys keys
                       WHERE keys.kind = control_plane_entities.kind
                         AND keys.entity_id = control_plane_entities.entity_id
                   )",
                [],
            )?;
            let mut stmt = tx.prepare(
                "INSERT INTO control_plane_entities
                    (kind, entity_id, workspace_id, state_json)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(kind, entity_id) DO UPDATE SET
                    workspace_id = excluded.workspace_id,
                    state_json = excluded.state_json
                 WHERE control_plane_entities.workspace_id IS NOT excluded.workspace_id
                    OR control_plane_entities.state_json IS NOT excluded.state_json",
            )?;
            for entity in &entities {
                stmt.execute(params![
                    entity.kind,
                    entity.entity_id,
                    entity.workspace_id,
                    entity.state_json
                ])?;
            }
            tx.execute("DELETE FROM control_plane_replace_keys", [])?;
        }
        tx.commit()?;
        debug!(
            target: "lucarne::control_plane::store",
            entity_count,
            "control-plane non-timeline entities replaced"
        );
        Ok(())
    }

    pub fn delete_workspace_entities(
        &self,
        workspace_id: &str,
    ) -> Result<(), ControlPlaneStoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM control_plane_entities WHERE workspace_id = ?1",
            params![workspace_id],
        )?;
        debug!(
            target: "lucarne::control_plane::store",
            workspace_id,
            "control-plane workspace entities deleted"
        );
        Ok(())
    }

    pub fn replace_entities(
        &self,
        entities: Vec<ControlPlanePersistenceEntity>,
    ) -> Result<(), ControlPlaneStoreError> {
        let entities = serialize_entities(entities)?;
        let entity_count = entities.len();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM control_plane_entities", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO control_plane_entities
                    (kind, entity_id, workspace_id, state_json)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for entity in entities {
                stmt.execute(params![
                    entity.kind,
                    entity.entity_id,
                    entity.workspace_id,
                    entity.state_json
                ])?;
            }
        }
        tx.commit()?;
        debug!(
            target: "lucarne::control_plane::store",
            entity_count,
            "control-plane entities replaced"
        );
        Ok(())
    }
}

fn decode_entity_state(
    kind: &str,
    state_json: ValueRef<'_>,
) -> Result<serde_json::Value, rusqlite::Error> {
    let bytes = match state_json {
        ValueRef::Text(bytes) => bytes,
        _ => {
            return Err(rusqlite::Error::InvalidColumnType(
                3,
                "state_json".into(),
                state_json.data_type(),
            ));
        }
    };
    let state = if kind == "timeline" {
        let index = serde_json::from_slice::<TimelineIndexState>(bytes).map_err(json_error)?;
        serde_json::to_value(TimelineItem {
            workspace_id: index.workspace_id,
            turn_id: index.turn_id,
            seq: index.seq,
            epoch: index.epoch,
            provider_item_id: index.provider_item_id,
            kind: index.kind,
            payload: serde_json::Value::Null,
            created_at: index.created_at,
        })
        .map_err(json_error)?
    } else {
        serde_json::from_slice(bytes).map_err(json_error)?
    };
    Ok(state)
}

fn json_error(err: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(3, Type::Text, Box::new(err))
}

fn serialize_entities(
    entities: Vec<ControlPlanePersistenceEntity>,
) -> Result<Vec<SerializedEntity>, ControlPlaneStoreError> {
    entities
        .into_iter()
        .map(|entity| {
            Ok(SerializedEntity {
                kind: entity.kind,
                entity_id: entity.entity_id,
                workspace_id: entity.workspace_id,
                state_json: serde_json::to_string(&entity.state)?,
            })
        })
        .collect()
}
