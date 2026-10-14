//! Read-only SQLite inspection, independent of Bevy, desktop UI, and documents.
//! Workers own short-lived connections and supersede only their own requests.
mod backend;
mod worker;

pub use backend::{
    Cell, ColumnInfo, DataSet, ForeignKeyInfo, IndexInfo, ObjectKind, PAGE_SIZE, QUERY_ROW_LIMIT,
    RESULT_BYTE_LIMIT, Request, Response, Schema, SchemaObject,
};
pub use worker::{Reply, Worker};
