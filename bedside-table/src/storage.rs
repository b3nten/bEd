//! Bedside owns its metadata schema; inspected databases are never written.
use bedtime_core::DocumentWriter;
use bedtime_storage::{StateScope, StateStore};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConnectionId(pub i64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QueryId(pub i64);

#[derive(Clone, Debug)]
pub struct ConnectionRecord {
    pub id: ConnectionId,
    pub path: PathBuf,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct QueryRecord {
    pub id: QueryId,
    pub connection: ConnectionId,
    pub title: String,
    pub sql: String,
}

/// Clones share one serialized metadata writer. Core's document worker uses
/// this same service as the application's scoped StateStore adapter.
#[derive(Clone, bevy::prelude::Resource)]
pub struct MetadataStore(Arc<Mutex<Connection>>);

impl MetadataStore {
    pub fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut connection = Connection::open(path).map_err(io::Error::other)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(2))
            .map_err(io::Error::other)?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(io::Error::other)?;
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(io::Error::other)?;
        if version < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unsupported Bedside metadata schema version: {version}"),
            ));
        }
        if version > 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Bedside metadata was created by a newer application",
            ));
        }
        if version == 0 {
            let transaction = connection.transaction().map_err(io::Error::other)?;
            transaction
                .execute_batch(
                    "CREATE TABLE connections (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    path TEXT NOT NULL UNIQUE,
                    label TEXT NOT NULL
                );
                CREATE TABLE queries (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    connection_id INTEGER NOT NULL REFERENCES connections(id),
                    title TEXT NOT NULL,
                    sql TEXT NOT NULL
                );
                CREATE TABLE state (scope TEXT PRIMARY KEY, payload TEXT NOT NULL);
                PRAGMA user_version = 1;",
                )
                .map_err(io::Error::other)?;
            transaction.commit().map_err(io::Error::other)?;
        }
        Ok(Self(Arc::new(Mutex::new(connection))))
    }

    pub fn connections(&self) -> io::Result<Vec<ConnectionRecord>> {
        let connection = self.0.lock().expect("metadata connection");
        let mut statement = connection
            .prepare("SELECT id, path, label FROM connections ORDER BY id")
            .map_err(io::Error::other)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(io::Error::other)?;
        rows.map(|row| {
            let (id, path, label) = row.map_err(io::Error::other)?;
            let path = serde_json::from_str(&path).map_err(io::Error::other)?;
            Ok(ConnectionRecord {
                id: ConnectionId(id),
                path,
                label,
            })
        })
        .collect()
    }

    pub fn add_connection(&self, path: &Path) -> io::Result<ConnectionRecord> {
        let path = path.canonicalize()?;
        if !path.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Open an existing SQLite database file",
            ));
        }
        let encoded = serde_json::to_string(&path).map_err(io::Error::other)?;
        let label = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let mut connection = self.0.lock().expect("metadata connection");
        let transaction = connection.transaction().map_err(io::Error::other)?;
        transaction
            .execute(
                "INSERT INTO connections(path, label) VALUES (?1, ?2) ON CONFLICT(path) DO NOTHING",
                params![encoded, label],
            )
            .map_err(io::Error::other)?;
        let id = transaction
            .query_row(
                "SELECT id FROM connections WHERE path = ?1",
                [&encoded],
                |row| row.get(0),
            )
            .map_err(io::Error::other)?;
        transaction.commit().map_err(io::Error::other)?;
        Ok(ConnectionRecord {
            id: ConnectionId(id),
            path,
            label,
        })
    }

    pub fn queries(&self) -> io::Result<Vec<QueryRecord>> {
        let connection = self.0.lock().expect("metadata connection");
        let mut statement = connection
            .prepare("SELECT id, connection_id, title, sql FROM queries ORDER BY id")
            .map_err(io::Error::other)?;
        let rows = statement
            .query_map([], |row| {
                Ok(QueryRecord {
                    id: QueryId(row.get(0)?),
                    connection: ConnectionId(row.get(1)?),
                    title: row.get(2)?,
                    sql: row.get(3)?,
                })
            })
            .map_err(io::Error::other)?;
        rows.map(|row| row.map_err(io::Error::other)).collect()
    }

    pub fn new_query(&self, connection_id: ConnectionId) -> io::Result<QueryRecord> {
        let mut connection = self.0.lock().expect("metadata connection");
        let transaction = connection.transaction().map_err(io::Error::other)?;
        transaction
            .execute(
                "INSERT INTO queries(connection_id, title, sql) VALUES (?1, 'Query', '')",
                [connection_id.0],
            )
            .map_err(io::Error::other)?;
        let id = QueryId(transaction.last_insert_rowid());
        let title = format!("Query {}", id.0);
        transaction
            .execute(
                "UPDATE queries SET title = ?1 WHERE id = ?2",
                params![title, id.0],
            )
            .map_err(io::Error::other)?;
        transaction.commit().map_err(io::Error::other)?;
        Ok(QueryRecord {
            id,
            connection: connection_id,
            title,
            sql: String::new(),
        })
    }
}

impl DocumentWriter for MetadataStore {
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > 1024 * 1024 {
            return Err("SQL text exceeds the 1 MiB query limit".into());
        }
        let id: i64 = key.parse().map_err(|_| "Invalid query persistence key")?;
        let sql = std::str::from_utf8(bytes).map_err(|_| "Query is not UTF-8")?;
        let mut connection = self.0.lock().expect("metadata connection");
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let written = transaction
            .execute(
                "UPDATE queries SET sql = ?1 WHERE id = ?2",
                params![sql, id],
            )
            .map_err(|error| error.to_string())?;
        if written != 1 {
            return Err("The saved query is no longer available".into());
        }
        transaction.commit().map_err(|error| error.to_string())
    }
}

impl StateStore for MetadataStore {
    fn load(&self, scope: &StateScope) -> io::Result<Option<Value>> {
        let connection = self.0.lock().expect("metadata connection");
        let payload: Option<String> = connection
            .query_row(
                "SELECT payload FROM state WHERE scope = ?1",
                [&scope.0],
                |row| row.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        payload
            .map(|value| serde_json::from_str(&value).map_err(io::Error::other))
            .transpose()
    }
    fn save(&mut self, scope: &StateScope, snapshot: &Value) -> io::Result<()> {
        bedtime_storage::validate_snapshot(snapshot)?;
        let payload = serde_json::to_string(snapshot).map_err(io::Error::other)?;
        let mut connection = self.0.lock().expect("metadata connection");
        let transaction = connection.transaction().map_err(io::Error::other)?;
        transaction.execute("INSERT INTO state(scope, payload) VALUES (?1, ?2) ON CONFLICT(scope) DO UPDATE SET payload = excluded.payload", params![scope.0, payload]).map_err(io::Error::other)?;
        transaction.commit().map_err(io::Error::other)
    }
}
