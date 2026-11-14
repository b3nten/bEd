//! An SQLite database file, without Bevy or egui: opening it and reading
//! what's in it. Everything here blocks, so panels call it from a task.
use rusqlite::{Connection, OpenFlags};
use std::{path::Path, time::Duration};

/// The first bytes of every SQLite 3 database file.
pub const MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// How long to wait for another program's lock before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

/// An open database.
pub struct Database {
    connection: Connection,
    read_only: bool,
}

impl Database {
    /// Open the database at `path`, for writing if the file allows it. The
    /// file is never created, and nothing is read until the first question,
    /// so a file that isn't a database fails then.
    pub fn open(path: &Path) -> Result<Self, String> {
        let flags = OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI;
        let (connection, read_only) =
            match Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_READ_WRITE) {
                Ok(connection) => {
                    let read_only = connection.is_readonly("main").unwrap_or(false);
                    (connection, read_only)
                }
                Err(_) => (
                    Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_READ_ONLY)
                        .map_err(message)?,
                    true,
                ),
            };
        Self::prepared(connection, read_only)
    }

    /// A database in memory, for tests.
    pub fn in_memory() -> Result<Self, String> {
        Self::prepared(Connection::open_in_memory().map_err(message)?, false)
    }

    fn prepared(connection: Connection, read_only: bool) -> Result<Self, String> {
        connection.busy_timeout(BUSY_TIMEOUT).map_err(message)?;
        // The file may be anyone's: its views and triggers don't get to call
        // functions with side effects.
        connection
            .pragma_update(None, "trusted_schema", false)
            .map_err(message)?;
        Ok(Self {
            connection,
            read_only,
        })
    }

    /// Whether the file can't be written (its permissions, or its volume's).
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// A number that changes whenever another connection (this program's or
    /// any other's) changes the database. This connection's own changes
    /// don't count.
    pub fn data_version(&self) -> Result<i64, String> {
        self.connection
            .query_row("PRAGMA data_version", [], |row| row.get(0))
            .map_err(message)
    }

    /// Run statements that return nothing.
    pub fn execute_batch(&self, sql: &str) -> Result<(), String> {
        self.connection.execute_batch(sql).map_err(message)
    }

    /// The tables and views, each kind by name. SQLite's own tables
    /// (`sqlite_sequence`, `sqlite_stat1`) are left out.
    pub fn schema(&self) -> Result<Schema, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT type, name, sql FROM sqlite_schema \
                 WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
                 ORDER BY type, name COLLATE NOCASE",
            )
            .map_err(message)?;
        let listed = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(message)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(message)?;
        let mut objects = Vec::with_capacity(listed.len());
        for (kind, name, sql) in listed {
            let kind = if kind == "view" {
                Kind::View
            } else {
                Kind::Table
            };
            // A view over a table that's gone can't say what its columns are;
            // it's still listed, with its statement.
            let columns = self.columns(&name).unwrap_or_default();
            objects.push(Object {
                columns,
                indexes: self.indexes(&name)?,
                foreign_keys: self.foreign_keys(&name)?,
                triggers: self.triggers(&name)?,
                sql: sql.unwrap_or_default(),
                kind,
                name,
            });
        }
        Ok(Schema { objects })
    }

    /// One page of a table's or view's rows, filtered and sorted by SQLite,
    /// with how many rows there are in all. A page past the end gives the
    /// last one.
    pub fn page(&self, browse: &Browse) -> Result<Page, String> {
        let table = quote(&browse.table);
        // On lines of its own, so a filter ending in a comment still closes.
        let filter = match browse.filter.trim() {
            "" => String::new(),
            filter => format!(" WHERE (\n{filter}\n)"),
        };
        let total = self
            .reading(&format!("SELECT count(*) FROM {table}{filter}"))?
            .query_row([], |row| row.get::<_, i64>(0))
            .map_err(message)?
            .max(0) as u64;
        let page = browse.page.min(total.saturating_sub(1) / PAGE_ROWS);
        let order = match &browse.sort {
            Some(sort) => format!(
                " ORDER BY {} {}",
                quote(&sort.column),
                if sort.descending { "DESC" } else { "ASC" }
            ),
            None => String::new(),
        };
        let keying = self.keying(&browse.table)?;
        // A table's rowid comes first, ahead of its columns.
        let rowid = match &keying {
            Some(Keying::Rowid(name)) => format!("{name}, "),
            _ => String::new(),
        };
        let mut statement = self.reading(&format!(
            "SELECT {rowid}* FROM {table}{filter}{order} LIMIT {PAGE_ROWS} OFFSET {}",
            page * PAGE_ROWS
        ))?;
        let lead = usize::from(!rowid.is_empty());
        let columns: Vec<String> = statement
            .column_names()
            .skip(lead)
            .map(str::to_owned)
            .collect();
        let mut keys = keying.map(|keying| match keying {
            Keying::Rowid(name) => (RowKeys::by(vec![name.to_owned()]), Vec::new()),
            Keying::Columns(names) => {
                // Where the key's columns are among those shown.
                let places = names
                    .iter()
                    .filter_map(|name| columns.iter().position(|column| column == name))
                    .collect();
                (
                    RowKeys::by(names.iter().map(|name| quote(name)).collect()),
                    places,
                )
            }
        });
        let mut rows = Vec::new();
        let mut found = statement.query([]).map_err(message)?;
        while let Some(row) = found.next().map_err(message)? {
            let mut values = Vec::with_capacity(columns.len());
            for column in 0..columns.len() {
                values.push(Value::from(row.get_ref(column + lead).map_err(message)?));
            }
            if let Some((keys, places)) = &mut keys {
                keys.rows.push(if lead == 1 {
                    vec![Value::from(row.get_ref(0).map_err(message)?)]
                } else {
                    places.iter().map(|&place| values[place].clone()).collect()
                });
            }
            rows.push(values);
        }
        Ok(Page {
            columns,
            rows,
            total,
            page,
            keys: keys.map(|(keys, _)| keys),
        })
    }

    /// What tells a table's rows apart: its rowid, by a name no column has
    /// taken, or its primary key if it's WITHOUT ROWID. Nothing for a view or
    /// a virtual table, whose rows aren't changed here.
    fn keying(&self, table: &str) -> Result<Option<Keying>, String> {
        let listed = self
            .connection
            .query_row(
                "SELECT type, wr FROM pragma_table_list WHERE schema = 'main' AND name = ?1",
                [table],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)),
            )
            .map_err(message)?;
        let (kind, without_rowid) = listed;
        if kind != "table" {
            return Ok(None);
        }
        let columns = self.columns(table)?;
        if without_rowid {
            let mut key: Vec<&Column> = columns.iter().filter(|c| c.primary_key > 0).collect();
            key.sort_by_key(|column| column.primary_key);
            return Ok(Some(Keying::Columns(
                key.into_iter().map(|column| column.name.clone()).collect(),
            )));
        }
        Ok(["rowid", "_rowid_", "oid"]
            .into_iter()
            .find(|name| !columns.iter().any(|c| c.name.eq_ignore_ascii_case(name)))
            .map(Keying::Rowid))
    }

    /// Make one change to a table's rows, kept at once.
    pub fn apply(&self, edit: &Edit) -> Result<(), String> {
        use rusqlite::types::Value as Sql;
        let changed = match edit {
            Edit::Set {
                table,
                row,
                column,
                declared,
                input,
            } => {
                let (condition, mut values) = row.condition(2)?;
                values.insert(0, input.value(declared));
                self.connection
                    .execute(
                        &format!(
                            "UPDATE {} SET {} = ?1 WHERE {condition}",
                            quote(table),
                            quote(column)
                        ),
                        rusqlite::params_from_iter(values),
                    )
                    .map_err(message)?
            }
            Edit::Delete { table, row } => {
                let (condition, values) = row.condition(1)?;
                self.connection
                    .execute(
                        &format!("DELETE FROM {} WHERE {condition}", quote(table)),
                        rusqlite::params_from_iter(values),
                    )
                    .map_err(message)?
            }
            Edit::Insert { table, values } if values.is_empty() => self
                .connection
                .execute(&format!("INSERT INTO {} DEFAULT VALUES", quote(table)), [])
                .map_err(message)?,
            Edit::Insert { table, values } => {
                let names: Vec<String> = values.iter().map(|(name, _, _)| quote(name)).collect();
                let places: Vec<String> = (1..=values.len()).map(|n| format!("?{n}")).collect();
                let given: Vec<Sql> = values
                    .iter()
                    .map(|(_, declared, input)| input.value(declared))
                    .collect();
                self.connection
                    .execute(
                        &format!(
                            "INSERT INTO {} ({}) VALUES ({})",
                            quote(table),
                            names.join(", "),
                            places.join(", ")
                        ),
                        rusqlite::params_from_iter(given),
                    )
                    .map_err(message)?
            }
        };
        // Another program got to the row first.
        if changed == 0 {
            return Err("That row isn't there any more".into());
        }
        Ok(())
    }

    /// Run what's typed: any number of statements, one after another, each
    /// kept as it finishes (unless they say BEGIN themselves). The rows of the
    /// last one that gives any come back, up to `RESULT_ROWS` of them. One
    /// that fails stops the rest; those before it stay done.
    pub fn run(&self, sql: &str) -> Result<Outcome, String> {
        use rusqlite::fallible_iterator::FallibleIterator;
        let started = std::time::Instant::now();
        let changes = self.connection.total_changes();
        let mut outcome = Outcome::default();
        let mut batch = rusqlite::Batch::new(&self.connection, sql);
        let numbered = |outcome: &Outcome, error: rusqlite::Error| {
            let error = message(error);
            // Which one, if there's been more than one.
            match outcome.statements {
                0 => error,
                done => format!("Statement {}: {error}", done + 1),
            }
        };
        loop {
            let mut statement = match batch.next() {
                Ok(Some(statement)) => statement,
                Ok(None) => break,
                Err(error) => return Err(numbered(&outcome, error)),
            };
            outcome.wrote |= !statement.readonly();
            if statement.column_count() == 0 {
                if let Err(error) = statement.execute([]) {
                    return Err(numbered(&outcome, error));
                }
            } else {
                let columns: Vec<String> = statement.column_names().map(str::to_owned).collect();
                let mut rows = Vec::new();
                let mut truncated = false;
                let read = (|| {
                    let mut found = statement.query([])?;
                    while let Some(row) = found.next()? {
                        if rows.len() == RESULT_ROWS {
                            truncated = true;
                            break;
                        }
                        let mut values = Vec::with_capacity(columns.len());
                        for column in 0..columns.len() {
                            values.push(Value::from(row.get_ref(column)?));
                        }
                        rows.push(values);
                    }
                    Ok(())
                })();
                if let Err(error) = read {
                    return Err(numbered(&outcome, error));
                }
                outcome.columns = columns;
                outcome.rows = rows;
                outcome.truncated = truncated;
            }
            outcome.statements += 1;
        }
        outcome.changed = self.connection.total_changes() - changes;
        outcome.seconds = started.elapsed().as_secs_f64();
        Ok(outcome)
    }

    /// Stops whatever this connection is running, from any thread: the
    /// statement fails, saying it was interrupted.
    pub fn interrupter(&self) -> Interrupter {
        Interrupter(std::sync::Arc::new(self.connection.get_interrupt_handle()))
    }

    /// A statement that only reads. What's typed into a filter goes into
    /// one, so it can't be made to change anything.
    fn reading(&self, sql: &str) -> Result<rusqlite::Statement<'_>, String> {
        let statement = self.connection.prepare(sql).map_err(message)?;
        if statement.readonly() {
            Ok(statement)
        } else {
            Err("A filter can't change the database".into())
        }
    }

    fn columns(&self, table: &str) -> Result<Vec<Column>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT name, type, \"notnull\", dflt_value, pk, hidden \
                 FROM pragma_table_xinfo(?1) ORDER BY cid",
            )
            .map_err(message)?;
        let columns = statement
            .query_map([table], |row| {
                Ok((
                    Column {
                        name: row.get(0)?,
                        declared: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                        not_null: row.get(2)?,
                        default: row.get(3)?,
                        primary_key: row.get(4)?,
                        generated: false,
                    },
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(message)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(message)?;
        Ok(columns
            .into_iter()
            // 1: a virtual table's hidden column. 2 and 3: generated ones.
            .filter(|(_, hidden)| *hidden != 1)
            .map(|(column, hidden)| Column {
                generated: hidden >= 2,
                ..column
            })
            .collect())
    }

    fn indexes(&self, table: &str) -> Result<Vec<Index>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT name, \"unique\", origin FROM pragma_index_list(?1) ORDER BY seq")
            .map_err(message)?;
        let listed = statement
            .query_map([table], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(message)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(message)?;
        let mut columns = self
            .connection
            .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
            .map_err(message)?;
        let mut indexes = Vec::with_capacity(listed.len());
        for (name, unique, origin) in listed {
            let columns = columns
                .query_map([&name], |row| row.get::<_, Option<String>>(0))
                .map_err(message)?
                .map(|column| column.map(|name| name.unwrap_or_else(|| "(expression)".into())))
                .collect::<Result<Vec<_>, _>>()
                .map_err(message)?;
            indexes.push(Index {
                name,
                unique,
                // "c": made by CREATE INDEX. Otherwise it comes with a
                // UNIQUE or PRIMARY KEY constraint.
                automatic: origin != "c",
                columns,
            });
        }
        Ok(indexes)
    }

    fn foreign_keys(&self, table: &str) -> Result<Vec<ForeignKey>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT \"from\", \"table\", \"to\" FROM pragma_foreign_key_list(?1) \
                 ORDER BY id, seq",
            )
            .map_err(message)?;
        statement
            .query_map([table], |row| {
                Ok(ForeignKey {
                    column: row.get(0)?,
                    table: row.get(1)?,
                    to: row.get(2)?,
                })
            })
            .map_err(message)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(message)
    }

    fn triggers(&self, table: &str) -> Result<Vec<String>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT name FROM sqlite_schema WHERE type = 'trigger' AND tbl_name = ?1 \
                 ORDER BY name COLLATE NOCASE",
            )
            .map_err(message)?;
        statement
            .query_map([table], |row| row.get(0))
            .map_err(message)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(message)
    }
}

/// How many rows a page holds.
pub const PAGE_ROWS: u64 = 1000;

/// Which rows of a table or view to show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Browse {
    pub table: String,
    /// An SQL condition, as after WHERE; empty for every row.
    pub filter: String,
    pub sort: Option<Sort>,
    /// From 0.
    pub page: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sort {
    pub column: String,
    pub descending: bool,
}

/// A page of rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// How many rows pass the filter, on every page.
    pub total: u64,
    /// The page this is, from 0.
    pub page: u64,
    /// What tells each row apart, if they can be changed: a table's can, a
    /// view's can't.
    pub keys: Option<RowKeys>,
}

/// What tells a page's rows apart.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowKeys {
    /// As SQL: the rowid's name, or the primary key's columns.
    pub by: Vec<String>,
    /// Their values, for each row.
    pub rows: Vec<Vec<Value>>,
}

impl RowKeys {
    fn by(by: Vec<String>) -> Self {
        Self {
            by,
            rows: Vec::new(),
        }
    }

    /// The key of a row of the page.
    pub fn row(&self, row: usize) -> Option<RowKey> {
        Some(RowKey {
            by: self.by.clone(),
            values: self.rows.get(row)?.clone(),
        })
    }
}

/// What tells one row from the others.
#[derive(Clone, Debug, PartialEq)]
pub struct RowKey {
    pub by: Vec<String>,
    pub values: Vec<Value>,
}

impl RowKey {
    /// The condition only this row passes, with its values as parameters
    /// numbered from `first`.
    fn condition(&self, first: usize) -> Result<(String, Vec<rusqlite::types::Value>), String> {
        use rusqlite::types::Value as Sql;
        let mut parts = Vec::new();
        let mut values = Vec::new();
        for (index, (by, value)) in self.by.iter().zip(&self.values).enumerate() {
            parts.push(format!("{by} = ?{}", first + index));
            values.push(match value {
                Value::Integer(integer) => Sql::Integer(*integer),
                Value::Real(real) => Sql::Real(*real),
                Value::Text(text) => Sql::Text(text.clone()),
                // Bytes aren't kept, and NULL equals nothing.
                Value::Null | Value::Blob(_) => {
                    return Err("This row's key can't be used to find it again".into());
                }
            });
        }
        if parts.is_empty() {
            return Err("This table has nothing to tell its rows apart".into());
        }
        Ok((parts.join(" AND "), values))
    }
}

enum Keying {
    Rowid(&'static str),
    Columns(Vec<String>),
}

/// What's typed into a cell.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Null,
    Text(String),
}

impl Input {
    /// As a value for a column declared as `declared`. A column that holds
    /// text gets it as typed (`007` stays `007`); in any other, what reads
    /// as a number is one, as SQLite itself would make it where the column
    /// says INTEGER or REAL.
    fn value(&self, declared: &str) -> rusqlite::types::Value {
        use rusqlite::types::Value as Sql;
        let Input::Text(text) = self else {
            return Sql::Null;
        };
        let declared = declared.to_uppercase();
        let holds_text = !declared.contains("INT")
            && ["CHAR", "CLOB", "TEXT"]
                .iter()
                .any(|name| declared.contains(name));
        let number = text.trim();
        if holds_text || !number.starts_with(|c: char| c.is_ascii_digit() || "+-.".contains(c)) {
            return Sql::Text(text.clone());
        }
        if let Ok(integer) = number.parse::<i64>() {
            Sql::Integer(integer)
        } else if let Some(real) = number.parse::<f64>().ok().filter(|real| real.is_finite()) {
            Sql::Real(real)
        } else {
            Sql::Text(text.clone())
        }
    }
}

/// A change to a table's rows.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// Put a value in a row's column, declared as `declared`.
    Set {
        table: String,
        row: RowKey,
        column: String,
        declared: String,
        input: Input,
    },
    /// A new row: the columns given (a name, how it's declared, a value);
    /// the rest get their defaults.
    Insert {
        table: String,
        values: Vec<(String, String, Input)>,
    },
    Delete {
        table: String,
        row: RowKey,
    },
}

impl Page {
    /// How many pages there are (at least one).
    pub fn pages(&self) -> u64 {
        self.total.saturating_sub(1) / PAGE_ROWS + 1
    }

    /// The number of this page's first row, from 0.
    pub fn first_row(&self) -> u64 {
        self.page * PAGE_ROWS
    }
}

/// As many rows as a query's result keeps.
pub const RESULT_ROWS: usize = 10_000;

/// What running some statements came to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// The rows of the last statement that gave any; no columns if none did.
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// There were more rows than `RESULT_ROWS`.
    pub truncated: bool,
    /// How many statements ran.
    pub statements: usize,
    /// How many rows they inserted, changed and deleted.
    pub changed: u64,
    /// Some statement wasn't one that only reads.
    pub wrote: bool,
    pub seconds: f64,
}

impl Outcome {
    /// In words: "2 statements · 5 rows · 3 rows changed".
    pub fn summary(&self) -> String {
        let count = |count: u64, one: &str, many: &str| match count {
            1 => format!("1 {one}"),
            count => format!("{} {many}", grouped(count)),
        };
        let mut parts = Vec::new();
        if self.statements == 0 {
            return "Nothing to run".into();
        }
        if self.statements > 1 {
            parts.push(count(self.statements as u64, "statement", "statements"));
        }
        if !self.columns.is_empty() {
            let rows = count(self.rows.len() as u64, "row", "rows");
            parts.push(if self.truncated {
                format!("The first {rows}")
            } else {
                rows
            });
        }
        if self.wrote {
            parts.push(format!("{} changed", count(self.changed, "row", "rows")));
        }
        if parts.is_empty() {
            parts.push("Done".into());
        }
        parts.join(" · ")
    }
}

/// Stops what a connection is running; see `Database::interrupter`.
#[derive(Clone)]
pub struct Interrupter(std::sync::Arc<rusqlite::InterruptHandle>);

impl Interrupter {
    pub fn interrupt(&self) {
        self.0.interrupt();
    }
}

/// What a row holds in a column.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    /// Bytes: only how many there are is kept.
    Blob(usize),
}

impl Value {
    /// As shown in a cell, and as copied.
    pub fn text(&self) -> String {
        match self {
            Value::Null => "NULL".into(),
            Value::Integer(integer) => integer.to_string(),
            Value::Real(real) => real.to_string(),
            Value::Text(text) => text.clone(),
            Value::Blob(1) => "BLOB (1 byte)".into(),
            Value::Blob(bytes) => format!("BLOB ({} bytes)", grouped(*bytes as u64)),
        }
    }

    /// The order SQLite sorts values in: NULL, numbers, text, bytes.
    pub fn order(&self, other: &Value) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let class = |value: &Value| match value {
            Value::Null => 0,
            Value::Integer(_) | Value::Real(_) => 1,
            Value::Text(_) => 2,
            Value::Blob(_) => 3,
        };
        let number = |value: &Value| match value {
            Value::Integer(integer) => *integer as f64,
            Value::Real(real) => *real,
            _ => 0.0,
        };
        match (self, other) {
            (Value::Integer(a), Value::Integer(b)) => a.cmp(b),
            (Value::Text(a), Value::Text(b)) => a.cmp(b),
            (Value::Blob(a), Value::Blob(b)) => a.cmp(b),
            (a, b) if class(a) == 1 && class(b) == 1 => {
                number(a).partial_cmp(&number(b)).unwrap_or(Ordering::Equal)
            }
            (a, b) => class(a).cmp(&class(b)),
        }
    }

    pub fn is_number(&self) -> bool {
        matches!(self, Value::Integer(_) | Value::Real(_))
    }

    /// Not a value of its own to read: NULL, or bytes.
    pub fn is_placeholder(&self) -> bool {
        matches!(self, Value::Null | Value::Blob(_))
    }
}

impl From<rusqlite::types::ValueRef<'_>> for Value {
    fn from(value: rusqlite::types::ValueRef<'_>) -> Self {
        use rusqlite::types::ValueRef;
        match value {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(integer) => Value::Integer(integer),
            ValueRef::Real(real) => Value::Real(real),
            ValueRef::Text(text) => Value::Text(String::from_utf8_lossy(text).into_owned()),
            ValueRef::Blob(bytes) => Value::Blob(bytes.len()),
        }
    }
}

/// A count with its thousands apart: `12,345`.
pub fn grouped(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// What a database holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Schema {
    /// Tables, then views; each by name.
    pub objects: Vec<Object>,
}

impl Schema {
    pub fn object(&self, name: &str) -> Option<&Object> {
        self.objects.iter().find(|object| object.name == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Table,
    View,
}

/// A table or a view.
#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    pub name: String,
    pub kind: Kind,
    /// The statement that made it, as written.
    pub sql: String,
    pub columns: Vec<Column>,
    pub indexes: Vec<Index>,
    pub foreign_keys: Vec<ForeignKey>,
    /// The names of the triggers on it.
    pub triggers: Vec<String>,
}

impl Object {
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|column| column.name == name)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub name: String,
    /// The type as declared (`VARCHAR(20)`); empty if none was.
    pub declared: String,
    pub not_null: bool,
    /// The default, as an SQL expression.
    pub default: Option<String>,
    /// Its place in the primary key, from 1; 0 if it isn't part of it.
    pub primary_key: u32,
    /// Worked out from other columns, so it can't be set.
    pub generated: bool,
}

impl Column {
    /// What constrains it, in words: "primary key, not null, default 0".
    pub fn constraints(&self, foreign_keys: &[ForeignKey]) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.primary_key > 0 {
            parts.push("primary key".into());
        }
        if self.not_null {
            parts.push("not null".into());
        }
        if let Some(default) = &self.default {
            parts.push(format!("default {default}"));
        }
        if self.generated {
            parts.push("generated".into());
        }
        for key in foreign_keys.iter().filter(|key| key.column == self.name) {
            parts.push(match &key.to {
                Some(to) => format!("references {}({to})", key.table),
                None => format!("references {}", key.table),
            });
        }
        parts.join(", ")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Index {
    pub name: String,
    pub unique: bool,
    /// Made by a UNIQUE or PRIMARY KEY constraint, not by CREATE INDEX.
    pub automatic: bool,
    pub columns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ForeignKey {
    pub column: String,
    /// The table referred to, and its column (its primary key if `None`).
    pub table: String,
    pub to: Option<String>,
}

/// A name as SQL: in double quotes, with any inside doubled.
pub fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Whether a file starts as SQLite databases do.
pub fn is_database(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 16];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut head))
        .is_ok_and(|()| &head == MAGIC)
}

/// SQLite's own words, without rusqlite's wrapping.
fn message(error: rusqlite::Error) -> String {
    match error {
        rusqlite::Error::SqliteFailure(_, Some(message)) => capitalized(&message),
        // Without the statement: it's ours, around what was typed.
        rusqlite::Error::SqlInputError { msg, .. } => capitalized(&msg),
        other => capitalized(&other.to_string()),
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Database {
        let database = Database::in_memory().unwrap();
        database
            .execute_batch(
                "CREATE TABLE artist (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
                 CREATE TABLE album (
                     id INTEGER PRIMARY KEY,
                     title VARCHAR(80) NOT NULL DEFAULT 'Untitled',
                     artist_id INTEGER REFERENCES artist(id),
                     year,
                     shout TEXT GENERATED ALWAYS AS (upper(title)) VIRTUAL
                 );
                 CREATE INDEX album_by_year ON album (year, title);
                 CREATE TRIGGER album_named AFTER INSERT ON album BEGIN SELECT 1; END;
                 CREATE VIEW Recent AS SELECT title FROM album WHERE year > 2000;
                 INSERT INTO artist (name) VALUES ('Low');
                 INSERT INTO album (title, artist_id, year) VALUES ('Things We Lost', 1, 2001);",
            )
            .unwrap();
        database
    }

    #[test]
    fn lists_tables_then_views_without_sqlites_own() {
        let database = sample();
        database
            .execute_batch(
                "CREATE TABLE counted (id INTEGER PRIMARY KEY AUTOINCREMENT);
                 INSERT INTO counted DEFAULT VALUES;",
            )
            .unwrap();
        let schema = database.schema().unwrap();
        let names: Vec<(&str, Kind)> = schema
            .objects
            .iter()
            .map(|object| (object.name.as_str(), object.kind))
            .collect();
        assert_eq!(
            names,
            [
                ("album", Kind::Table),
                ("artist", Kind::Table),
                ("counted", Kind::Table),
                ("Recent", Kind::View),
            ]
        );
    }

    #[test]
    fn reads_columns_and_what_constrains_them() {
        let schema = sample().schema().unwrap();
        let album = schema.object("album").unwrap();
        let described: Vec<(&str, &str, String)> = album
            .columns
            .iter()
            .map(|column| {
                (
                    column.name.as_str(),
                    column.declared.as_str(),
                    column.constraints(&album.foreign_keys),
                )
            })
            .collect();
        assert_eq!(
            described,
            [
                ("id", "INTEGER", "primary key".to_owned()),
                (
                    "title",
                    "VARCHAR(80)",
                    "not null, default 'Untitled'".to_owned()
                ),
                ("artist_id", "INTEGER", "references artist(id)".to_owned()),
                ("year", "", String::new()),
                ("shout", "TEXT", "generated".to_owned()),
            ]
        );
        assert!(album.sql.starts_with("CREATE TABLE album"));
        assert_eq!(album.triggers, ["album_named"]);
    }

    #[test]
    fn reads_indexes_and_views() {
        let schema = sample().schema().unwrap();
        let album = schema.object("album").unwrap();
        assert_eq!(
            album.indexes,
            [Index {
                name: "album_by_year".into(),
                unique: false,
                automatic: false,
                columns: vec!["year".into(), "title".into()],
            }]
        );
        let artist = schema.object("artist").unwrap();
        assert!(artist.indexes[0].unique && artist.indexes[0].automatic);
        let recent = schema.object("Recent").unwrap();
        assert_eq!(recent.kind, Kind::View);
        assert_eq!(recent.columns.len(), 1);
        assert_eq!(recent.columns[0].name, "title");
    }

    #[test]
    fn a_view_of_a_missing_table_is_still_listed() {
        let database = sample();
        database
            .execute_batch(
                "CREATE TABLE gone (name);
                 CREATE VIEW lost AS SELECT name FROM gone;
                 DROP TABLE gone;",
            )
            .unwrap();
        let schema = database.schema().unwrap();
        let lost = schema.object("lost").unwrap();
        assert!(lost.columns.is_empty());
        assert!(lost.sql.contains("SELECT name FROM gone"));
    }

    #[test]
    fn files_are_told_by_their_first_bytes() {
        let folder = std::env::temp_dir().join(format!("bedtime-sqlite-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("music.anything");
        let database = Database::open(&file);
        // Opening never creates the file.
        assert!(database.is_err() || !file.exists());
        Connection::open(&file)
            .unwrap()
            .execute_batch("CREATE TABLE t (a);")
            .unwrap();
        assert!(is_database(&file));
        let database = Database::open(&file).unwrap();
        assert!(!database.read_only());
        assert_eq!(database.schema().unwrap().objects.len(), 1);

        let text = folder.join("notes.db");
        std::fs::write(&text, "not a database at all, just some text").unwrap();
        assert!(!is_database(&text));
        let error = Database::open(&text).unwrap().schema().unwrap_err();
        assert_eq!(error, "File is not a database");
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn others_changes_are_noticed_and_ours_are_not() {
        let folder =
            std::env::temp_dir().join(format!("bedtime-sqlite-version-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("shared.db");
        let other = Connection::open(&file).unwrap();
        other.execute_batch("CREATE TABLE t (a);").unwrap();
        let database = Database::open(&file).unwrap();
        let before = database.data_version().unwrap();
        database.execute_batch("INSERT INTO t VALUES (1);").unwrap();
        assert_eq!(database.data_version().unwrap(), before);
        other.execute_batch("INSERT INTO t VALUES (2);").unwrap();
        assert_ne!(database.data_version().unwrap(), before);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    fn numbers() -> Database {
        let database = Database::in_memory().unwrap();
        database
            .execute_batch(
                "CREATE TABLE n (i INTEGER, label TEXT, half REAL, bytes BLOB);
                 WITH RECURSIVE count(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM count LIMIT 2500)
                 INSERT INTO n SELECT i, 'row ' || i, i / 2.0, NULL FROM count;
                 UPDATE n SET label = NULL, bytes = x'010203' WHERE i = 2;",
            )
            .unwrap();
        database
    }

    fn browse(page: u64) -> Browse {
        Browse {
            table: "n".into(),
            page,
            ..Default::default()
        }
    }

    #[test]
    fn rows_come_a_page_at_a_time() {
        let database = numbers();
        let first = database.page(&browse(0)).unwrap();
        assert_eq!(first.columns, ["i", "label", "half", "bytes"]);
        assert_eq!(
            (first.total, first.pages(), first.rows.len()),
            (2500, 3, 1000)
        );
        assert_eq!(
            first.rows[0],
            [
                Value::Integer(1),
                Value::Text("row 1".into()),
                Value::Real(0.5),
                Value::Null
            ]
        );
        assert_eq!(first.rows[1][1], Value::Null);
        assert_eq!(first.rows[1][3], Value::Blob(3));
        let last = database.page(&browse(2)).unwrap();
        assert_eq!((last.rows.len(), last.first_row()), (500, 2000));
        assert_eq!(last.rows[0][0], Value::Integer(2001));
        // Past the end: the last page.
        let past = database.page(&browse(9)).unwrap();
        assert_eq!((past.page, past.rows.len()), (2, 500));
    }

    #[test]
    fn sqlite_filters_and_sorts() {
        let database = numbers();
        let page = database
            .page(&Browse {
                filter: "i % 500 = 0 -- every five hundredth".into(),
                sort: Some(Sort {
                    column: "i".into(),
                    descending: true,
                }),
                ..browse(0)
            })
            .unwrap();
        let found: Vec<&Value> = page.rows.iter().map(|row| &row[0]).collect();
        assert_eq!(
            found,
            [2500, 2000, 1500, 1000, 500]
                .map(Value::Integer)
                .iter()
                .collect::<Vec<_>>()
        );
        assert_eq!((page.total, page.pages()), (5, 1));
        // Nothing passes: one empty page, with its columns.
        let none = database
            .page(&Browse {
                filter: "i < 0".into(),
                ..browse(3)
            })
            .unwrap();
        assert_eq!(
            (none.total, none.pages(), none.page, none.rows.len()),
            (0, 1, 0, 0)
        );
        assert_eq!(none.columns.len(), 4);
    }

    #[test]
    fn a_filter_cant_change_anything() {
        let database = numbers();
        let wrong = database.page(&Browse {
            filter: "missing = 1".into(),
            ..browse(0)
        });
        assert_eq!(wrong.unwrap_err(), "No such column: missing");
        let sneaky = database.page(&Browse {
            filter: "1); DELETE FROM n; --".into(),
            ..browse(0)
        });
        let broken = database.page(&Browse {
            filter: "i >".into(),
            ..browse(0)
        });
        assert_eq!(broken.unwrap_err(), "Near \")\": syntax error");
        // Only the first statement is ever made, and it only reads.
        assert!(sneaky.is_err() || sneaky.unwrap().total == 2500);
        assert_eq!(database.page(&browse(0)).unwrap().total, 2500);
    }

    #[test]
    fn views_and_odd_names_are_browsed_too() {
        let database = numbers();
        database
            .execute_batch(
                "CREATE VIEW \"odd \"\"name\"\"\" AS SELECT i AS \"the number\" FROM n WHERE i <= 3;",
            )
            .unwrap();
        let page = database
            .page(&Browse {
                table: "odd \"name\"".into(),
                sort: Some(Sort {
                    column: "the number".into(),
                    descending: true,
                }),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.columns, ["the number"]);
        assert_eq!(page.rows[0], [Value::Integer(3)]);
    }

    #[test]
    fn statements_run_one_after_another() {
        let database = numbers();
        let outcome = database
            .run(
                "CREATE TABLE made (a, b);
                 INSERT INTO made VALUES (1, 'one'), (2, 'two'), (3, NULL);
                 UPDATE made SET b = 'both' WHERE a < 3; -- two of them
                 SELECT a, b FROM made ORDER BY a DESC;",
            )
            .unwrap();
        assert_eq!(outcome.columns, ["a", "b"]);
        assert_eq!(outcome.rows[0], [Value::Integer(3), Value::Null]);
        assert_eq!(
            (outcome.statements, outcome.changed, outcome.wrote),
            (4, 5, true)
        );
        assert_eq!(outcome.summary(), "4 statements · 3 rows · 5 rows changed");

        let read = database.run("SELECT count(*) AS n FROM n").unwrap();
        assert_eq!(
            (read.rows.clone(), read.wrote),
            (vec![vec![Value::Integer(2500)]], false)
        );
        assert_eq!(read.summary(), "1 row");
        assert_eq!(
            database
                .run("DELETE FROM made WHERE a = 1")
                .unwrap()
                .summary(),
            "1 row changed"
        );
        assert_eq!(
            database.run("  -- nothing\n").unwrap().summary(),
            "Nothing to run"
        );
        assert_eq!(
            database.run("PRAGMA foreign_keys = ON").unwrap().summary(),
            "Done"
        );
    }

    #[test]
    fn a_failing_statement_stops_the_rest_and_says_which() {
        let database = numbers();
        let error = database
            .run("CREATE TABLE kept (a); INSERT INTO nowhere VALUES (1); CREATE TABLE never (a);")
            .unwrap_err();
        assert_eq!(error, "Statement 2: No such table: nowhere");
        let schema = database.schema().unwrap();
        assert!(schema.object("kept").is_some() && schema.object("never").is_none());
        assert_eq!(
            database.run("SELEC 1").unwrap_err(),
            "Near \"SELEC\": syntax error"
        );
    }

    #[test]
    fn long_results_are_cut_short() {
        let database = Database::in_memory().unwrap();
        let outcome = database
            .run(
                "WITH RECURSIVE count(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM count LIMIT 10500)
                 SELECT i FROM count",
            )
            .unwrap();
        assert_eq!((outcome.rows.len(), outcome.truncated), (RESULT_ROWS, true));
        assert_eq!(outcome.summary(), "The first 10,000 rows");
    }

    #[test]
    fn a_running_statement_can_be_interrupted() {
        let database = Database::in_memory().unwrap();
        let interrupter = database.interrupter();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            interrupter.interrupt();
        });
        // Counts forever, unless stopped.
        let error = database
            .run("WITH RECURSIVE count(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM count) SELECT max(i) FROM count")
            .unwrap_err();
        stopper.join().unwrap();
        assert_eq!(error, "Interrupted");
    }

    #[test]
    fn values_sort_as_sqlite_sorts_them() {
        let mut values = vec![
            Value::Text("b".into()),
            Value::Blob(2),
            Value::Real(1.5),
            Value::Null,
            Value::Integer(2),
            Value::Text("a".into()),
            Value::Integer(1),
        ];
        values.sort_by(Value::order);
        assert_eq!(
            values,
            [
                Value::Null,
                Value::Integer(1),
                Value::Real(1.5),
                Value::Integer(2),
                Value::Text("a".into()),
                Value::Text("b".into()),
                Value::Blob(2),
            ]
        );
    }

    fn editable() -> Database {
        let database = Database::in_memory().unwrap();
        database
            .execute_batch(
                "CREATE TABLE person (
                     name TEXT NOT NULL, age INTEGER, code VARCHAR(8), anything,
                     initial TEXT GENERATED ALWAYS AS (substr(name, 1, 1)) VIRTUAL
                 );
                 INSERT INTO person (name, age, code, anything) VALUES
                     ('Ada', 36, '001', 1), ('Grace', 85, '002', 'two'), ('Alan', 41, '003', NULL);
                 CREATE TABLE setting (key TEXT, scope TEXT, value, PRIMARY KEY (scope, key)) WITHOUT ROWID;
                 INSERT INTO setting VALUES ('theme', 'user', 'dark'), ('theme', 'app', 'light');
                 CREATE TABLE shadowed (rowid TEXT, _rowid_ TEXT, note TEXT);
                 INSERT INTO shadowed VALUES ('a', 'b', 'first');
                 CREATE VIEW adults AS SELECT name FROM person WHERE age >= 18;",
            )
            .unwrap();
        database
    }

    fn table(name: &str) -> Browse {
        Browse {
            table: name.into(),
            ..Default::default()
        }
    }

    fn set(
        database: &Database,
        name: &str,
        row: usize,
        column: &str,
        declared: &str,
        input: Input,
    ) -> Result<(), String> {
        let page = database.page(&table(name)).unwrap();
        database.apply(&Edit::Set {
            table: name.into(),
            row: page.keys.unwrap().row(row).unwrap(),
            column: column.into(),
            declared: declared.into(),
            input,
        })
    }

    #[test]
    fn rows_are_told_apart() {
        let database = editable();
        let person = database.page(&table("person")).unwrap();
        // The rowid isn't one of the columns shown.
        assert_eq!(
            person.columns,
            ["name", "age", "code", "anything", "initial"]
        );
        let keys = person.keys.unwrap();
        assert_eq!(keys.by, ["rowid"]);
        assert_eq!(keys.rows[2], [Value::Integer(3)]);
        // WITHOUT ROWID: the primary key, in its order.
        let setting = database.page(&table("setting")).unwrap().keys.unwrap();
        assert_eq!(setting.by, ["\"scope\"", "\"key\""]);
        assert_eq!(
            setting.rows[0],
            [Value::Text("app".into()), Value::Text("theme".into())]
        );
        // Columns that took the rowid's names leave another.
        let shadowed = database.page(&table("shadowed")).unwrap();
        assert_eq!(shadowed.keys.unwrap().by, ["oid"]);
        assert_eq!(shadowed.rows[0][0], Value::Text("a".into()));
        // A view's rows can't be changed.
        assert_eq!(database.page(&table("adults")).unwrap().keys, None);
    }

    #[test]
    fn cells_are_set() {
        let database = editable();
        let text = |text: &str| Input::Text(text.into());
        set(&database, "person", 0, "name", "TEXT", text("Ada Lovelace")).unwrap();
        set(&database, "person", 0, "age", "INTEGER", text("37")).unwrap();
        // Text stays as typed where the column holds text; a number is one elsewhere.
        set(&database, "person", 0, "code", "VARCHAR(8)", text("007")).unwrap();
        set(&database, "person", 1, "anything", "", text("2.5")).unwrap();
        set(&database, "person", 2, "anything", "", text("-")).unwrap();
        set(&database, "person", 1, "age", "INTEGER", Input::Null).unwrap();
        let rows = database.page(&table("person")).unwrap().rows;
        assert_eq!(
            rows[0][..4],
            [
                Value::Text("Ada Lovelace".into()),
                Value::Integer(37),
                Value::Text("007".into()),
                Value::Integer(1)
            ]
        );
        assert_eq!(rows[0][4], Value::Text("A".into()));
        assert_eq!(
            (&rows[1][1], &rows[1][3]),
            (&Value::Null, &Value::Real(2.5))
        );
        assert_eq!(rows[2][3], Value::Text("-".into()));
        // What SQLite refuses comes back in its words, and nothing changes.
        let refused = set(&database, "person", 2, "name", "TEXT", Input::Null);
        assert_eq!(
            refused.unwrap_err(),
            "NOT NULL constraint failed: person.name"
        );
        let generated = set(&database, "person", 2, "initial", "TEXT", text("Z"));
        assert!(generated.is_err());
        // WITHOUT ROWID rows are found by their key.
        set(&database, "setting", 1, "value", "", text("solarized")).unwrap();
        let setting = database.page(&table("setting")).unwrap().rows;
        assert_eq!(setting[1][2], Value::Text("solarized".into()));
        assert_eq!(setting[0][2], Value::Text("light".into()));
    }

    #[test]
    fn rows_are_inserted_and_deleted() {
        let database = editable();
        database
            .apply(&Edit::Insert {
                table: "person".into(),
                values: vec![
                    ("name".into(), "TEXT".into(), Input::Text("Barbara".into())),
                    ("age".into(), "INTEGER".into(), Input::Text("69".into())),
                ],
            })
            .unwrap();
        let page = database.page(&table("person")).unwrap();
        assert_eq!(page.total, 4);
        assert_eq!(
            page.rows[3][..3],
            [
                Value::Text("Barbara".into()),
                Value::Integer(69),
                Value::Null
            ]
        );
        // Nothing given: every default, which this table doesn't allow.
        let empty = database.apply(&Edit::Insert {
            table: "person".into(),
            values: vec![],
        });
        assert_eq!(
            empty.unwrap_err(),
            "NOT NULL constraint failed: person.name"
        );
        database
            .apply(&Edit::Insert {
                table: "shadowed".into(),
                values: vec![],
            })
            .unwrap();
        assert_eq!(database.page(&table("shadowed")).unwrap().total, 2);

        let row = page.keys.unwrap().row(1).unwrap();
        let delete = Edit::Delete {
            table: "person".into(),
            row,
        };
        database.apply(&delete).unwrap();
        let names: Vec<Value> = database
            .page(&table("person"))
            .unwrap()
            .rows
            .into_iter()
            .map(|mut row| row.swap_remove(0))
            .collect();
        assert_eq!(
            names,
            ["Ada", "Alan", "Barbara"].map(|name| Value::Text(name.into()))
        );
        // It's gone: doing it again says so.
        assert_eq!(
            database.apply(&delete).unwrap_err(),
            "That row isn't there any more"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_file_that_cant_be_written_opens_read_only() {
        use std::os::unix::fs::PermissionsExt;
        let folder =
            std::env::temp_dir().join(format!("bedtime-sqlite-locked-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("locked.db");
        Connection::open(&file)
            .unwrap()
            .execute_batch("CREATE TABLE t (a); INSERT INTO t VALUES (1);")
            .unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
        let database = Database::open(&file).unwrap();
        assert!(database.read_only());
        let page = database.page(&table("t")).unwrap();
        assert_eq!(page.rows, [[Value::Integer(1)]]);
        let refused = database.apply(&Edit::Insert {
            table: "t".into(),
            values: vec![],
        });
        assert_eq!(refused.unwrap_err(), "Attempt to write a readonly database");
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn values_as_text() {
        assert_eq!(Value::Null.text(), "NULL");
        assert_eq!(Value::Real(2.5).text(), "2.5");
        assert_eq!(Value::Blob(1).text(), "BLOB (1 byte)");
        assert_eq!(Value::Blob(1234567).text(), "BLOB (1,234,567 bytes)");
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
    }

    #[test]
    fn names_are_quoted() {
        assert_eq!(quote("album"), "\"album\"");
        assert_eq!(quote("odd\"name"), "\"odd\"\"name\"");
    }
}
