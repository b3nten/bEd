use rusqlite::{
    Connection, OpenFlags, OptionalExtension,
    fallible_iterator::FallibleIterator,
    hooks::{AuthAction, AuthContext, Authorization},
    limits::Limit,
    types::ValueRef,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

pub const PAGE_SIZE: usize = 200;
pub const QUERY_ROW_LIMIT: usize = 1_000;
pub const RESULT_BYTE_LIMIT: usize = 8 * 1024 * 1024;
const TEXT_PREVIEW_BYTES: usize = 4_096;
const BLOB_PREVIEW_BYTES: usize = 64;
const QUERY_DEADLINE: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub enum Request {
    Schema,
    Browse { table: String, offset: usize },
    Count { table: String },
    Query { sql: String },
}

#[derive(Debug)]
pub enum Response {
    Schema(Schema),
    Data(DataSet),
    Count(usize),
}

#[derive(Debug)]
pub struct Schema {
    pub objects: Vec<SchemaObject>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    Table,
    View,
}

#[derive(Debug)]
pub struct SchemaObject {
    pub name: String,
    pub kind: ObjectKind,
    pub create_sql: Option<String>,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
}

#[derive(Debug)]
pub struct ColumnInfo {
    pub name: String,
    pub declared_type: String,
    pub not_null: bool,
    pub default_value: Option<String>,
    pub primary_key_position: i64,
    pub hidden: i64,
}

#[derive(Debug)]
pub struct IndexInfo {
    pub name: String,
    pub unique: bool,
    pub origin: String,
    pub partial: bool,
    pub columns: Vec<Option<String>>,
    pub create_sql: Option<String>,
}

#[derive(Debug)]
pub struct ForeignKeyInfo {
    pub id: i64,
    pub sequence: i64,
    pub table: String,
    pub from: String,
    pub to: Option<String>,
    pub on_update: String,
    pub on_delete: String,
}

#[derive(Debug)]
pub struct DataSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
    pub has_more: bool,
    pub truncated: bool,
    pub elapsed: Duration,
}

#[derive(Debug, PartialEq)]
pub enum Cell {
    Null,
    Integer(i64),
    Real(f64),
    Text {
        preview: String,
        byte_len: usize,
        truncated: bool,
    },
    Blob {
        preview: Vec<u8>,
        byte_len: usize,
    },
}

impl Cell {
    pub fn display_text(&self) -> String {
        match self {
            Self::Null => "NULL".into(),
            Self::Integer(value) => value.to_string(),
            Self::Real(value) => value.to_string(),
            Self::Text { preview, .. } => preview.clone(),
            Self::Blob { preview, byte_len } => {
                use std::fmt::Write;
                let mut text = String::with_capacity(preview.len() * 2 + 24);
                text.push_str("X'");
                for byte in preview {
                    write!(text, "{byte:02X}").expect("write to String");
                }
                text.push('\'');
                if preview.len() < *byte_len {
                    write!(text, " … ({byte_len} bytes)").expect("write to String");
                }
                text
            }
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Null => "NULL",
            Self::Integer(_) => "INTEGER",
            Self::Real(_) => "REAL",
            Self::Text { .. } => "TEXT",
            Self::Blob { .. } => "BLOB",
        }
    }

    pub fn is_truncated(&self) -> bool {
        match self {
            Self::Text { truncated, .. } => *truncated,
            Self::Blob { preview, byte_len } => preview.len() < *byte_len,
            _ => false,
        }
    }
}

// The worker is the sole owner of SQLite execution. Each request has a fresh
// connection, so no transaction or read lock survives a response or a refresh.
pub(crate) fn execute(
    path: &Path,
    request: Request,
    generation: Arc<AtomicU64>,
    id: u64,
) -> Result<Response, String> {
    execute_with_deadline(path, request, generation, id, QUERY_DEADLINE)
}

fn execute_with_deadline(
    path: &Path,
    request: Request,
    generation: Arc<AtomicU64>,
    id: u64,
    deadline: Duration,
) -> Result<Response, String> {
    let started = Instant::now();
    let stopped = || generation.load(Ordering::Relaxed) != id;
    if stopped() {
        return Err("Query cancelled.".into());
    }
    let run = || -> rusqlite::Result<Response> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_millis(250))?;
        connection.pragma_update(None, "query_only", true)?;
        connection.pragma_update(None, "trusted_schema", false)?;
        // Bound SQLite's own allocations as well as the copied result. A file
        // can be many GB; an individual value exceeding this limit fails clearly.
        connection.set_limit(Limit::SQLITE_LIMIT_LENGTH, 32 * 1024 * 1024)?;
        connection.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, 1024 * 1024)?;
        connection.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)?;
        let active = Arc::clone(&generation);
        connection.progress_handler(
            1_000,
            Some(move || active.load(Ordering::Relaxed) != id || started.elapsed() >= deadline),
        )?;

        match request {
            Request::Schema => read_schema(&connection).map(Response::Schema),
            Request::Browse { table, offset } => {
                connection.authorizer(Some(read_only_authorizer))?;
                let remaining = PAGE_SIZE - offset % PAGE_SIZE;
                // Identifiers are quoted, never interpreted as SQL. No rowid
                // assumption is made, including for views and WITHOUT ROWID.
                // A byte-capped continuation finishes the current 200-row page
                // before advancing to the next page's fixed starting offset.
                let sql = format!(
                    "SELECT * FROM \"{}\" LIMIT {} OFFSET {}",
                    table.replace('"', "\"\""),
                    remaining + 1,
                    offset
                );
                read_data(&connection, &sql, remaining, started).map(Response::Data)
            }
            Request::Count { table } => {
                connection.authorizer(Some(read_only_authorizer))?;
                // A WHERE clause keeps SQLite from using its single OP_Count
                // b-tree traversal, which does not invoke the progress hook.
                // The row scan remains cancellable and observes our deadline.
                let sql = format!(
                    "SELECT count(*) FROM \"{}\" WHERE 1",
                    table.replace('"', "\"\"")
                );
                let count: i64 = connection.query_row(&sql, [], |row| row.get(0))?;
                let count = usize::try_from(count)
                    .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, count))?;
                Ok(Response::Count(count))
            }
            Request::Query { sql } => {
                connection.authorizer(Some(read_only_authorizer))?;
                read_data(&connection, &sql, QUERY_ROW_LIMIT, started).map(Response::Data)
            }
        }
    };
    let result = run();
    if stopped() {
        return Err("Query cancelled.".into());
    }
    if started.elapsed() >= deadline {
        return Err(
            "Query exceeded the 10-second deadline. Narrow the query and try again.".into(),
        );
    }
    result.map_err(|error| format!("SQLite: {error}"))
}

fn read_only_authorizer(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Select | AuthAction::Read { .. } | AuthAction::Recursive => {
            Authorization::Allow
        }
        AuthAction::Function { function_name }
            if !function_name.eq_ignore_ascii_case("load_extension") =>
        {
            Authorization::Allow
        }
        // Deny by default: writes, PRAGMAs (including table-valued PRAGMAs),
        // ATTACH/DETACH, transaction control, and unknown future operations.
        _ => Authorization::Deny,
    }
}

fn read_data(
    connection: &Connection,
    sql: &str,
    row_limit: usize,
    started: Instant,
) -> rusqlite::Result<DataSet> {
    let mut batch = rusqlite::Batch::new(connection, sql);
    let mut statement = batch.next()?.ok_or(rusqlite::Error::InvalidQuery)?;
    if batch.next()?.is_some() {
        return Err(rusqlite::Error::MultipleStatement);
    }
    if !statement.readonly() || statement.column_count() == 0 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let columns = statement
        .column_names()
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    let mut bytes = columns.iter().map(String::len).sum::<usize>();
    let mut rows = Vec::new();
    let mut has_more = false;
    let mut cursor = statement.query([])?;
    while let Some(row) = cursor.next()? {
        if rows.len() == row_limit {
            has_more = true;
            break;
        }
        let mut cells = Vec::with_capacity(columns.len());
        let mut row_bytes = columns.len() * std::mem::size_of::<Cell>();
        for column in 0..columns.len() {
            let cell = match row.get_ref(column)? {
                ValueRef::Null => Cell::Null,
                ValueRef::Integer(value) => Cell::Integer(value),
                ValueRef::Real(value) => Cell::Real(value),
                ValueRef::Text(value) => {
                    let mut end = value.len().min(TEXT_PREVIEW_BYTES);
                    // SQLite permits invalid UTF-8 and embedded NUL in TEXT.
                    // Use a bounded, loss-tolerant preview and make NUL visible
                    // before it reaches ImGui's C-string boundary.
                    while end < value.len() && end > 0 && value[end] & 0xc0 == 0x80 {
                        end -= 1;
                    }
                    let preview = String::from_utf8_lossy(&value[..end]).replace('\0', "␀");
                    row_bytes += preview.len();
                    Cell::Text {
                        preview,
                        byte_len: value.len(),
                        truncated: end < value.len(),
                    }
                }
                ValueRef::Blob(value) => {
                    let preview = value[..value.len().min(BLOB_PREVIEW_BYTES)].to_vec();
                    row_bytes += preview.len();
                    Cell::Blob {
                        preview,
                        byte_len: value.len(),
                    }
                }
            };
            cells.push(cell);
        }
        if bytes + row_bytes > RESULT_BYTE_LIMIT {
            if rows.is_empty() {
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_TOOBIG),
                    Some("A result row exceeds the viewer's 8 MiB preview limit; select fewer columns".into()),
                ));
            }
            has_more = true;
            break;
        }
        bytes += row_bytes;
        rows.push(cells);
    }
    Ok(DataSet {
        columns,
        rows,
        has_more,
        truncated: has_more,
        elapsed: started.elapsed(),
    })
}

fn read_schema(connection: &Connection) -> rusqlite::Result<Schema> {
    let mut statement = connection.prepare(
        "SELECT name, type, sql FROM sqlite_schema \
         WHERE type IN ('table', 'view') AND name NOT GLOB 'sqlite_*' ORDER BY name COLLATE NOCASE",
    )?;
    let mut cursor = statement.query([])?;
    let mut objects = Vec::new();
    let mut bytes = 0;
    while let Some(row) = cursor.next()? {
        if objects.len() == 10_000 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_TOOBIG),
                Some("Schema exceeds the viewer's 10,000-object limit".into()),
            ));
        }
        let name: String = row.get(0)?;
        let kind: String = row.get(1)?;
        let create_sql: Option<String> = row.get(2)?;
        reserve_schema_bytes(
            &mut bytes,
            std::mem::size_of::<SchemaObject>()
                + name.len()
                + create_sql.as_ref().map_or(0, String::len),
        )?;
        let columns = connection
            .prepare(
                "SELECT name, type, \"notnull\", dflt_value, pk, hidden FROM pragma_table_xinfo(?)",
            )?
            .query_map([&name], |row| {
                let column = ColumnInfo {
                    name: row.get(0)?,
                    declared_type: row.get(1)?,
                    not_null: row.get(2)?,
                    default_value: row.get(3)?,
                    primary_key_position: row.get(4)?,
                    hidden: row.get(5)?,
                };
                reserve_schema_bytes(
                    &mut bytes,
                    std::mem::size_of::<ColumnInfo>()
                        + column.name.len()
                        + column.declared_type.len()
                        + column.default_value.as_ref().map_or(0, String::len),
                )?;
                Ok(column)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut indexes = connection
            .prepare("SELECT name, \"unique\", origin, partial FROM pragma_index_list(?)")?
            .query_map([&name], |row| {
                let index = IndexInfo {
                    name: row.get(0)?,
                    unique: row.get(1)?,
                    origin: row.get(2)?,
                    partial: row.get(3)?,
                    columns: Vec::new(),
                    create_sql: None,
                };
                reserve_schema_bytes(
                    &mut bytes,
                    std::mem::size_of::<IndexInfo>() + index.name.len() + index.origin.len(),
                )?;
                Ok(index)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for index in &mut indexes {
            index.columns = connection
                .prepare("SELECT name FROM pragma_index_info(?) ORDER BY seqno")?
                .query_map([&index.name], |row| {
                    let name: Option<String> = row.get(0)?;
                    reserve_schema_bytes(
                        &mut bytes,
                        std::mem::size_of::<Option<String>>()
                            + name.as_ref().map_or(0, String::len),
                    )?;
                    Ok(name)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            index.create_sql = connection
                .query_row(
                    "SELECT sql FROM sqlite_schema WHERE type = 'index' AND name = ?",
                    [&index.name],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            reserve_schema_bytes(&mut bytes, index.create_sql.as_ref().map_or(0, String::len))?;
        }
        let foreign_keys = connection
            .prepare("SELECT id, seq, \"table\", \"from\", \"to\", on_update, on_delete FROM pragma_foreign_key_list(?)")?
            .query_map([&name], |row| {
                let key = ForeignKeyInfo {
                    id: row.get(0)?,
                    sequence: row.get(1)?,
                    table: row.get(2)?,
                    from: row.get(3)?,
                    to: row.get(4)?,
                    on_update: row.get(5)?,
                    on_delete: row.get(6)?,
                };
                reserve_schema_bytes(&mut bytes, std::mem::size_of::<ForeignKeyInfo>() + key.table.len() + key.from.len() + key.to.as_ref().map_or(0, String::len) + key.on_update.len() + key.on_delete.len())?;
                Ok(key)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        objects.push(SchemaObject {
            name,
            kind: if kind == "view" {
                ObjectKind::View
            } else {
                ObjectKind::Table
            },
            create_sql,
            columns,
            indexes,
            foreign_keys,
        });
    }
    Ok(Schema { objects })
}

fn reserve_schema_bytes(bytes: &mut usize, amount: usize) -> rusqlite::Result<()> {
    *bytes += amount;
    if *bytes > RESULT_BYTE_LIMIT {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_TOOBIG),
            Some("Schema exceeds the viewer's 8 MiB limit".into()),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, thread};

    struct Database(PathBuf);

    impl Database {
        fn new(sql: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "bed-sqlite-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&directory).unwrap();
            let database = Self(directory.join("test.sqlite"));
            database.writer().execute_batch(sql).unwrap();
            database
        }

        fn writer(&self) -> Connection {
            Connection::open(&self.0).unwrap()
        }

        fn run(&self, request: Request) -> Result<Response, String> {
            execute(&self.0, request, Arc::new(AtomicU64::new(1)), 1)
        }

        fn query(&self, sql: &str) -> DataSet {
            let Response::Data(data) = self.run(Request::Query { sql: sql.into() }).unwrap() else {
                panic!("expected data")
            };
            data
        }
    }

    impl Drop for Database {
        fn drop(&mut self) {
            fs::remove_dir_all(self.0.parent().unwrap()).unwrap();
        }
    }

    #[test]
    fn schema_and_browse_support_quoted_names_views_and_without_rowid() {
        let db = Database::new(
            r#"CREATE TABLE parent (id INTEGER PRIMARY KEY);
               INSERT INTO parent VALUES(7);
               CREATE TABLE sqliteData (value TEXT);
               CREATE TABLE "odd""table" (
                   a TEXT, b INTEGER, n, payload BLOB, real_value REAL,
                   PRIMARY KEY(a, b), FOREIGN KEY(b) REFERENCES parent(id)
               ) WITHOUT ROWID;
               CREATE INDEX "quoted index" ON "odd""table"(b) WHERE b > 0;
               INSERT INTO "odd""table" VALUES ('Zoë', 7, NULL, X'00FF', 1.5);
               CREATE VIEW sample_view AS SELECT a, b FROM "odd""table";"#,
        );
        let Response::Schema(schema) = db.run(Request::Schema).unwrap() else {
            panic!("expected schema")
        };
        assert!(
            schema
                .objects
                .iter()
                .any(|table| table.name == "sqliteData")
        );
        let table = schema
            .objects
            .iter()
            .find(|table| table.name == "odd\"table")
            .unwrap();
        assert_eq!(table.kind, ObjectKind::Table);
        assert!(table.create_sql.as_ref().unwrap().contains("WITHOUT ROWID"));
        assert_eq!(table.columns[0].primary_key_position, 1);
        assert_eq!(table.columns[1].primary_key_position, 2);
        assert_eq!(table.foreign_keys[0].table, "parent");
        assert!(
            table
                .indexes
                .iter()
                .any(|index| index.name == "quoted index" && index.partial)
        );
        assert!(table.indexes.iter().any(|index| index.origin == "pk"));
        assert_eq!(
            schema
                .objects
                .iter()
                .find(|table| table.name == "sample_view")
                .unwrap()
                .kind,
            ObjectKind::View
        );
        let Response::Data(data) = db
            .run(Request::Browse {
                table: "odd\"table".into(),
                offset: 0,
            })
            .unwrap()
        else {
            panic!("expected data")
        };
        assert_eq!(data.columns, ["a", "b", "n", "payload", "real_value"]);
        assert_eq!(data.rows[0][0].display_text(), "Zoë");
        assert_eq!(data.rows[0][1], Cell::Integer(7));
        assert_eq!(data.rows[0][2], Cell::Null);
        assert_eq!(data.rows[0][3].display_text(), "X'00FF'");
        assert_eq!(data.rows[0][4], Cell::Real(1.5));
        assert!(!data.has_more);
        assert_eq!(db.query("SELECT * FROM sample_view").rows.len(), 1);
        assert!(
            db.run(Request::Browse {
                table: "odd\"table\"; DROP TABLE parent; --".into(),
                offset: 0
            })
            .is_err()
        );
        assert_eq!(
            db.query("SELECT count(*) FROM parent").rows[0][0],
            Cell::Integer(1)
        );
    }

    #[test]
    fn browsing_is_paged_and_queries_are_bounded_by_rows_and_bytes() {
        let db = Database::new(
            "CREATE TABLE numbers(n INTEGER); \
             WITH RECURSIVE nums(n) AS (VALUES(0) UNION ALL SELECT n+1 FROM nums WHERE n < 400) \
             INSERT INTO numbers SELECT n FROM nums;",
        );
        let Response::Data(first) = db
            .run(Request::Browse {
                table: "numbers".into(),
                offset: 0,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(first.rows.len(), PAGE_SIZE);
        assert!(first.has_more);
        let Response::Data(last) = db
            .run(Request::Browse {
                table: "numbers".into(),
                offset: 400,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(last.rows.len(), 1);
        assert!(!last.has_more);
        let data = db.query("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1001) SELECT x FROM n");
        assert_eq!(data.rows.len(), QUERY_ROW_LIMIT);
        assert!(data.truncated);
        let data = db.query("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000) SELECT printf('%10000s','x'), printf('%10000s','x'), printf('%10000s','x'), printf('%10000s','x') FROM n");
        assert!(!data.rows.is_empty());
        assert!(data.rows.len() < QUERY_ROW_LIMIT);
        assert!(data.truncated);
        assert!(data.rows[0][0].is_truncated());
    }

    #[test]
    fn counts_support_quoted_tables_views_and_empty_results() {
        let db = Database::new(
            r#"CREATE TABLE "quoted""table" (n INTEGER);
               INSERT INTO "quoted""table" VALUES (1), (2), (3);
               CREATE VIEW filtered AS SELECT n FROM "quoted""table" WHERE n > 1;
               CREATE TABLE empty (n INTEGER);"#,
        );
        for (table, expected) in [("quoted\"table", 3), ("filtered", 2), ("empty", 0)] {
            let Response::Count(count) = db
                .run(Request::Count {
                    table: table.into(),
                })
                .unwrap()
            else {
                panic!("expected count")
            };
            assert_eq!(count, expected);
        }
        assert!(
            db.run(Request::Count {
                table: "quoted\"table\"; DROP TABLE empty; --".into()
            })
            .is_err()
        );
        assert!(matches!(
            db.run(Request::Count {
                table: "empty".into()
            })
            .unwrap(),
            Response::Count(0)
        ));
        db.writer().execute_batch("CREATE VIEW slow AS WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT x FROM n;").unwrap();
        assert!(
            execute_with_deadline(
                &db.0,
                Request::Count {
                    table: "slow".into()
                },
                Arc::new(AtomicU64::new(1)),
                1,
                Duration::from_millis(5),
            )
            .unwrap_err()
            .contains("deadline")
        );
    }

    #[test]
    fn byte_capped_browse_continuations_preserve_every_row_and_page_boundary() {
        let columns = (0..16)
            .map(|index| format!("printf('%4096s', 'x') AS c{index}"))
            .collect::<Vec<_>>()
            .join(",");
        let db = Database::new(&format!(
            "CREATE VIEW wide AS WITH RECURSIVE nums(n) AS (VALUES(0) UNION ALL SELECT n+1 FROM nums WHERE n<400) SELECT n, {columns} FROM nums;"
        ));
        let mut offset = 0;
        let mut seen = Vec::new();
        let mut offsets = Vec::new();
        loop {
            offsets.push(offset);
            let Response::Data(data) = db
                .run(Request::Browse {
                    table: "wide".into(),
                    offset,
                })
                .unwrap()
            else {
                panic!("expected data")
            };
            assert!(!data.rows.is_empty());
            assert!(data.rows.len() <= PAGE_SIZE - offset % PAGE_SIZE);
            seen.extend(data.rows.iter().map(|row| {
                let Cell::Integer(n) = row[0] else {
                    panic!("expected row number")
                };
                n
            }));
            offset += data.rows.len();
            if !data.has_more {
                break;
            }
        }
        assert!(offsets.iter().any(|offset| offset % PAGE_SIZE != 0));
        assert!(offsets.contains(&200));
        assert!(offsets.contains(&400));
        assert_eq!(seen, (0..=400).collect::<Vec<_>>());
    }

    #[test]
    fn only_a_single_read_only_statement_can_run() {
        let db = Database::new("CREATE TABLE t(n); INSERT INTO t VALUES(1);");
        for sql in [
            "INSERT INTO t VALUES(2)",
            "UPDATE t SET n=2",
            "DELETE FROM t",
            "CREATE TABLE other(n)",
            "DROP TABLE t",
            "ALTER TABLE t ADD COLUMN x",
            "ATTACH ':memory:' AS other",
            "DETACH main",
            "BEGIN",
            "COMMIT",
            "ROLLBACK",
            "SAVEPOINT s",
            "PRAGMA query_only=OFF",
            "PRAGMA schema_version",
            "SELECT * FROM pragma_table_info('t')",
            "SELECT load_extension('anything')",
            "SELECT 1; SELECT 2",
            "SELECT 1; DROP TABLE t",
            "",
            "-- only a comment",
        ] {
            assert!(
                db.run(Request::Query { sql: sql.into() }).is_err(),
                "accepted {sql}"
            );
        }
        assert_eq!(
            db.query("SELECT n FROM t; -- trailing comment").rows[0][0],
            Cell::Integer(1)
        );
        assert_eq!(
            db.query("SELECT 'a;b' /* ; */;").rows[0][0].display_text(),
            "a;b"
        );
        assert_eq!(
            db.query("WITH x AS (SELECT n FROM t) SELECT * FROM x").rows[0][0],
            Cell::Integer(1)
        );
        assert_eq!(
            db.writer()
                .query_row("SELECT count(*) FROM t", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn previews_bound_large_cells_and_handle_unicode_invalid_utf8_and_nul() {
        let db = Database::new("");
        let data =
            db.query("SELECT zeroblob(16000000), CAST(X'FF0061' AS TEXT), printf('%5000s', '界')");
        let Cell::Blob { preview, byte_len } = &data.rows[0][0] else {
            panic!()
        };
        assert_eq!(preview.len(), BLOB_PREVIEW_BYTES);
        assert_eq!(*byte_len, 16_000_000);
        assert_eq!(data.rows[0][1].display_text(), "�␀a");
        assert!(!data.rows[0][1].is_truncated());
        let text = data.rows[0][2].display_text();
        assert!(text.len() <= TEXT_PREVIEW_BYTES);
        assert!(data.rows[0][2].is_truncated());
        assert!(
            db.run(Request::Query {
                sql: "SELECT zeroblob(40000000)".into()
            })
            .is_err()
        );
    }

    #[test]
    fn refresh_reads_committed_wal_data_and_releases_read_locks() {
        let db = Database::new("CREATE TABLE t(n);");
        let writer = db.writer();
        writer.pragma_update(None, "journal_mode", "wal").unwrap();
        writer.execute("INSERT INTO t VALUES(1)", []).unwrap();
        assert_eq!(db.query("SELECT * FROM t").rows.len(), 1);
        writer
            .execute_batch("BEGIN; INSERT INTO t VALUES(2);")
            .unwrap();
        assert_eq!(db.query("SELECT * FROM t").rows.len(), 1);
        writer.execute_batch("COMMIT;").unwrap();
        assert_eq!(db.query("SELECT * FROM t").rows.len(), 2);
        let busy: i64 = writer
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
            .unwrap();
        assert_eq!(busy, 0);
    }

    #[test]
    fn exclusive_database_locks_return_a_clear_error() {
        let db = Database::new("CREATE TABLE t(n);");
        let writer = db.writer();
        writer.execute_batch("BEGIN EXCLUSIVE;").unwrap();
        let error = db.run(Request::Schema).unwrap_err();
        assert!(error.contains("locked"), "{error}");
        writer.execute_batch("ROLLBACK;").unwrap();
        assert!(db.run(Request::Schema).is_ok());
    }

    #[test]
    fn missing_malformed_and_large_database_files_are_handled_without_copying() {
        let db = Database::new("CREATE TABLE t(n); INSERT INTO t VALUES(42);");
        fs::OpenOptions::new()
            .write(true)
            .open(&db.0)
            .unwrap()
            .set_len(129 * 1024 * 1024)
            .unwrap();
        assert!(fs::metadata(&db.0).unwrap().len() > 128 * 1024 * 1024);
        assert_eq!(db.query("SELECT * FROM t").rows[0][0], Cell::Integer(42));
        let malformed = db.0.with_file_name("malformed.db");
        fs::write(&malformed, b"not a SQLite database").unwrap();
        let request = || execute(&malformed, Request::Schema, Arc::new(AtomicU64::new(1)), 1);
        assert!(request().unwrap_err().contains("not a database"));
        let missing = db.0.with_file_name("missing.db");
        assert!(execute(&missing, Request::Schema, Arc::new(AtomicU64::new(1)), 1).is_err());
        assert!(!missing.exists());
    }

    #[test]
    fn expensive_queries_stop_on_cancellation_and_deadline() {
        let db = Database::new("");
        let query = || {
            Request::Query { sql: "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n".into() }
        };
        let generation = Arc::new(AtomicU64::new(1));
        let active = Arc::clone(&generation);
        let path = db.0.clone();
        let request = query();
        let started = Instant::now();
        let running = thread::spawn(move || execute(&path, request, active, 1));
        thread::sleep(Duration::from_millis(20));
        generation.store(2, Ordering::Relaxed);
        assert!(running.join().unwrap().unwrap_err().contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            execute_with_deadline(
                &db.0,
                query(),
                Arc::new(AtomicU64::new(1)),
                1,
                Duration::from_millis(5)
            )
            .unwrap_err()
            .contains("deadline")
        );
    }

    #[test]
    fn wide_tables_and_empty_results_keep_all_column_names() {
        let definition = (0..600)
            .map(|index| format!("c{index} TEXT"))
            .collect::<Vec<_>>()
            .join(",");
        let db = Database::new(&format!("CREATE TABLE wide({definition});"));
        let Response::Data(data) = db
            .run(Request::Browse {
                table: "wide".into(),
                offset: 0,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(data.columns.len(), 600);
        assert!(data.rows.is_empty());
        assert!(!data.has_more);
    }
}
