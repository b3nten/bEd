//! Source-preserving CSV indexing and edits. All offsets refer to one immutable
//! text-document revision; callers apply the returned edits as one transaction.

use bed_session::editor_session::ByteEdit;
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    ops::{Range, RangeInclusive},
    sync::Arc,
};

pub const MAX_COLUMNS: usize = 510;
pub const MAX_INDEX_BYTES: usize = 128 * 1024 * 1024;
const MAX_DOCUMENT_BYTES: usize = 128 * 1024 * 1024;
const DELIMITERS: [u8; 4] = *b",\t;|";

#[derive(Clone, Debug)]
struct Field {
    bytes: Range<usize>,
}

#[derive(Clone, Debug)]
pub struct Record {
    fields: Vec<Field>,
    start: usize,
    end: usize,
    terminator_end: usize,
}

#[derive(Clone, Debug)]
pub struct Table {
    pub bytes: Arc<[u8]>,
    pub records: Vec<Record>,
    pub columns: usize,
    pub delimiter: u8,
    pub line_ending: Vec<u8>,
    pub detected_header: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortMode {
    #[default]
    Auto,
    Text,
    Number,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sort {
    pub column: usize,
    pub descending: bool,
    pub mode: SortMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterOp {
    #[default]
    Contains,
    Equals,
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnFilter {
    pub column: usize,
    pub value: String,
    pub op: FilterOp,
}

pub fn parse(
    bytes: Arc<[u8]>,
    delimiter: Option<u8>,
    path: &str,
    cancel: impl Fn() -> bool,
) -> Result<Table, String> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err("CSV exceeds the 128 MiB document limit.".into());
    }
    std::str::from_utf8(&bytes).map_err(|_| {
        "CSV editing requires UTF-8 text. Open this file in the text editor.".to_owned()
    })?;
    if bytes.contains(&0) {
        return Err("CSV contains NUL bytes and cannot be edited as text. Open this file in the text editor.".into());
    }
    if cancel() {
        return Err("Cancelled".into());
    }
    let delimiter = match delimiter {
        Some(value) if DELIMITERS.contains(&value) => value,
        Some(_) => return Err("Choose comma, tab, semicolon, or pipe as the delimiter.".into()),
        None => detect_delimiter(&bytes, path, &cancel)?,
    };
    let mut records = Vec::new();
    let mut position = bom_len(&bytes);
    let mut columns = 1;
    let mut line_ending = None;
    let mut field_index_bytes = 0;
    while position < bytes.len() {
        if cancel() {
            return Err("Cancelled".into());
        }
        let start = position;
        let mut fields = Vec::new();
        let end;
        let terminator_end;
        loop {
            let field_start = position;
            if bytes.get(position) == Some(&b'"') {
                position += 1;
                loop {
                    if position & 4095 <= 1 && cancel() {
                        return Err("Cancelled".into());
                    }
                    match bytes.get(position) {
                        None => {
                            return Err(format!(
                                "Unclosed quoted field in record {}.",
                                records.len() + 1
                            ));
                        }
                        Some(b'"') if bytes.get(position + 1) == Some(&b'"') => {
                            position += 2;
                        }
                        Some(b'"') => {
                            position += 1;
                            break;
                        }
                        Some(_) => position += 1,
                    }
                }
                if bytes
                    .get(position)
                    .is_some_and(|&value| value != delimiter && value != b'\r' && value != b'\n')
                {
                    return Err(format!(
                        "Unexpected text after a quoted field in record {}.",
                        records.len() + 1
                    ));
                }
            } else {
                while let Some(&value) = bytes.get(position) {
                    if value == delimiter || value == b'\r' || value == b'\n' {
                        break;
                    }
                    if value == b'"' {
                        return Err(format!(
                            "Unexpected quote in an unquoted field in record {}.",
                            records.len() + 1
                        ));
                    }
                    position += 1;
                    if position & 4095 == 0 && cancel() {
                        return Err("Cancelled".into());
                    }
                }
            }
            fields.push(Field {
                bytes: field_start..position,
            });
            if fields.len() > MAX_COLUMNS {
                return Err(format!("CSV supports at most {MAX_COLUMNS} columns."));
            }
            check_index_size(
                field_index_bytes + fields.capacity() * std::mem::size_of::<Field>(),
                records.capacity(),
            )?;
            match bytes.get(position) {
                Some(&value) if value == delimiter => position += 1,
                Some(b'\r' | b'\n') => {
                    end = position;
                    let length =
                        if bytes[position] == b'\r' && bytes.get(position + 1) == Some(&b'\n') {
                            2
                        } else {
                            1
                        };
                    if line_ending.is_none() {
                        line_ending = Some(bytes[position..position + length].to_vec());
                    }
                    position += length;
                    terminator_end = position;
                    break;
                }
                None => {
                    end = position;
                    terminator_end = position;
                    break;
                }
                Some(_) => unreachable!("field parser stops only at a separator"),
            }
        }
        columns = columns.max(fields.len());
        field_index_bytes += fields.capacity() * std::mem::size_of::<Field>();
        records.push(Record {
            fields,
            start,
            end,
            terminator_end,
        });
        check_index_size(field_index_bytes, records.capacity())?;
    }
    let mut table = Table {
        bytes,
        records,
        columns,
        delimiter,
        line_ending: line_ending.unwrap_or_else(|| vec![b'\n']),
        detected_header: false,
    };
    table.detected_header = table.detect_header();
    Ok(table)
}

fn check_index_size(fields: usize, records_capacity: usize) -> Result<(), String> {
    if fields + records_capacity * std::mem::size_of::<Record>() > MAX_INDEX_BYTES {
        Err("CSV index exceeds the 128 MiB memory limit. Open this file in the text editor.".into())
    } else {
        Ok(())
    }
}

fn bom_len(bytes: &[u8]) -> usize {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        3
    } else {
        0
    }
}

/// Count separators outside quoted fields in a bounded sample. Comparing the
/// modal width avoids letting an occasional ragged record choose the format.
fn detect_delimiter(bytes: &[u8], path: &str, cancel: &impl Fn() -> bool) -> Result<u8, String> {
    let extension_default = if path
        .rsplit('.')
        .next()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("tsv"))
    {
        b'\t'
    } else {
        b','
    };
    let mut samples: [Vec<usize>; 4] = std::array::from_fn(|_| Vec::new());
    let mut counts = [0; 4];
    let mut position = bom_len(bytes);
    let mut quoted = false;
    let mut field_start = true;
    let mut record_started = false;
    while position < bytes.len() && position < 256 * 1024 && samples[0].len() < 64 {
        if position & 4095 <= 1 && cancel() {
            return Err("Cancelled".into());
        }
        let value = bytes[position];
        if value == b'"' {
            if quoted && bytes.get(position + 1) == Some(&b'"') {
                position += 2;
                continue;
            }
            if quoted || field_start {
                quoted = !quoted;
            }
            field_start = false;
        } else if !quoted {
            if let Some(index) = DELIMITERS.iter().position(|&candidate| candidate == value) {
                counts[index] += 1;
                field_start = true;
            } else if value == b'\r' || value == b'\n' {
                for (sample, count) in samples.iter_mut().zip(counts) {
                    sample.push(count);
                }
                counts = [0; 4];
                if value == b'\r' && bytes.get(position + 1) == Some(&b'\n') {
                    position += 1;
                }
                field_start = true;
                record_started = false;
                position += 1;
                continue;
            } else {
                field_start = false;
            }
        }
        record_started = true;
        position += 1;
    }
    if position == bytes.len() && record_started {
        for (sample, count) in samples.iter_mut().zip(counts) {
            sample.push(count);
        }
    }
    let score = |index: usize| -> (usize, usize) {
        let mut frequencies = BTreeMap::<usize, usize>::new();
        for &count in &samples[index] {
            if count > 0 {
                *frequencies.entry(count).or_default() += 1;
            }
        }
        let (_, frequency) = frequencies
            .into_iter()
            .max_by_key(|&(width, frequency)| (frequency, width))
            .unwrap_or((0, 0));
        // First prefer consistent nonempty widths, then total sample support.
        let consistency = if samples[index].is_empty() {
            0
        } else {
            frequency * 1000 / samples[index].len()
        };
        (consistency, frequency)
    };
    let default_index = DELIMITERS
        .iter()
        .position(|&value| value == extension_default)
        .unwrap();
    let mut best = default_index;
    for index in 0..DELIMITERS.len() {
        if score(index) > score(best) {
            best = index;
        }
    }
    Ok(DELIMITERS[best])
}

impl Table {
    pub fn record_columns(&self, record: usize) -> usize {
        self.records
            .get(record)
            .map_or(0, |record| record.fields.len())
    }

    /// Missing fields in ragged rows are displayed as empty without modifying
    /// the source. Quoted fields allocate only when doubled quotes need decoding.
    pub fn cell(&self, record: usize, column: usize) -> Cow<'_, str> {
        let Some(field) = self
            .records
            .get(record)
            .and_then(|record| record.fields.get(column))
        else {
            return Cow::Borrowed("");
        };
        let raw = std::str::from_utf8(&self.bytes[field.bytes.clone()])
            .expect("parse validates the entire UTF-8 document");
        if raw.starts_with('"') {
            let value = &raw[1..raw.len() - 1];
            if value.contains("\"\"") {
                Cow::Owned(value.replace("\"\"", "\""))
            } else {
                Cow::Borrowed(value)
            }
        } else {
            Cow::Borrowed(raw)
        }
    }

    fn detect_header(&self) -> bool {
        if self.records.len() < 2 || self.record_columns(0) != self.columns {
            return false;
        }
        let mut labels = BTreeSet::new();
        for column in 0..self.columns {
            let value = self.cell(0, column);
            let label = value.trim();
            if label.is_empty()
                || value.contains(['\r', '\n'])
                || value_type(label) != ValueType::Text
                || !labels.insert(label.to_lowercase())
            {
                return false;
            }
        }
        // Text-only exports are intentionally ambiguous. At least one column
        // must contrast a label with consistent numeric, boolean, or date data.
        (0..self.columns).any(|column| {
            let mut types = Vec::new();
            for record in 1..self.records.len().min(33) {
                let value = self.cell(record, column);
                if !value.trim().is_empty() {
                    types.push(value_type(value.trim()));
                }
            }
            !types.is_empty()
                && [ValueType::Number, ValueType::Boolean, ValueType::Date]
                    .into_iter()
                    .any(|kind| {
                        types.iter().filter(|&&value| value == kind).count() * 4 >= types.len() * 3
                    })
        })
    }

    fn raw_fields(&self, record: usize) -> Vec<Vec<u8>> {
        self.records[record]
            .fields
            .iter()
            .map(|field| self.bytes[field.bytes.clone()].to_vec())
            .collect()
    }

    fn record_edit(
        &self,
        record: usize,
        fields: &[Vec<u8>],
        followed_by_append: bool,
    ) -> Option<ByteEdit> {
        let source = &self.records[record];
        let mut bytes = join_fields(fields, self.delimiter);
        // An empty final unterminated record has no representation. Give it a
        // terminator so clearing its last cell does not delete the row.
        if bytes.is_empty() && source.end == source.terminator_end && !followed_by_append {
            bytes.extend_from_slice(&self.line_ending);
        }
        if bytes == self.bytes[source.start..source.end] {
            None
        } else {
            Some(ByteEdit {
                range: source.start..source.end,
                bytes,
            })
        }
    }

    /// Large rectangular or structural operations must not turn into hundreds
    /// of thousands of rope mutations. Keep ordinary edits precise, and group
    /// bulk edits into bounded blocks while copying every untouched byte inside
    /// each block verbatim (including ragged fields and mixed terminators).
    fn coalesce_bulk_edits(&self, edits: Vec<ByteEdit>) -> Vec<ByteEdit> {
        if edits.len() <= 128 {
            return edits;
        }
        let mut output = Vec::new();
        let mut edits = edits.into_iter().peekable();
        while let Some(first) = edits.next() {
            let start = first.range.start;
            let mut end = first.range.end;
            let mut bytes = first.bytes;
            let mut count = 1;
            while count < 1024
                && edits
                    .peek()
                    .is_some_and(|next| next.range.end - start <= 1024 * 1024)
            {
                let next = edits.next().unwrap();
                bytes.extend_from_slice(&self.bytes[end..next.range.start]);
                bytes.extend_from_slice(&next.bytes);
                end = next.range.end;
                count += 1;
            }
            output.push(ByteEdit {
                range: start..end,
                bytes,
            });
        }
        output
    }

    pub fn replace_cells(
        &self,
        changes: &[(usize, usize, String)],
    ) -> Result<Vec<ByteEdit>, String> {
        if changes.is_empty() {
            return Ok(Vec::new());
        }
        let mut rows = BTreeMap::<usize, BTreeMap<usize, &str>>::new();
        let mut new_columns = BTreeSet::new();
        let mut columns = self.columns;
        for (row, column, value) in changes {
            if *column >= MAX_COLUMNS {
                return Err(format!("CSV supports at most {MAX_COLUMNS} columns."));
            }
            rows.entry(*row).or_default().insert(*column, value);
            if *column >= self.columns {
                new_columns.insert(*column);
            }
            columns = columns.max(column + 1);
        }
        rows.retain(|row, updates| {
            if *row < self.records.len() {
                updates.retain(|column, value| {
                    *column >= self.columns || self.cell(*row, *column) != *value
                });
            }
            !updates.is_empty()
        });
        if !(self.columns..columns).all(|column| new_columns.contains(&column)) {
            return Err("New columns must be contiguous with the table.".into());
        }
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let last_row = *rows.last_key_value().unwrap().0;
        if last_row >= self.records.len()
            && !(self.records.len()..=last_row).all(|row| rows.contains_key(&row))
        {
            return Err("New rows must be contiguous with the table.".into());
        }
        let mut edits = Vec::new();
        let mut appended = Vec::new();
        for (row, updates) in rows {
            let existing = row < self.records.len();
            let width = if existing {
                self.record_columns(row)
                    .max(updates.last_key_value().unwrap().0 + 1)
            } else {
                columns
            };
            let mut fields = if existing {
                self.raw_fields(row)
            } else {
                Vec::new()
            };
            fields.resize_with(width, Vec::new);
            for (column, value) in updates {
                if !existing || self.cell(row, column) != value {
                    fields[column] = encode_field(value, self.delimiter);
                }
            }
            if existing {
                if let Some(edit) = self.record_edit(
                    row,
                    &fields,
                    row + 1 == self.records.len() && last_row >= self.records.len(),
                ) {
                    edits.push(edit);
                }
            } else {
                appended.push(join_fields(&fields, self.delimiter));
            }
        }
        if !appended.is_empty() {
            let mut bytes = Vec::new();
            let terminated = self
                .records
                .last()
                .is_some_and(|row| row.end < row.terminator_end);
            if !self.records.is_empty() && !terminated {
                bytes.extend_from_slice(&self.line_ending);
            }
            for (index, row) in appended.iter().enumerate() {
                bytes.extend_from_slice(row);
                if index + 1 < appended.len() || terminated || row.is_empty() {
                    bytes.extend_from_slice(&self.line_ending);
                }
            }
            edits.push(ByteEdit {
                range: self.bytes.len()..self.bytes.len(),
                bytes,
            });
        }
        Ok(self.coalesce_bulk_edits(edits))
    }

    pub fn insert_row(&self, record: usize) -> Result<Vec<ByteEdit>, String> {
        if record > self.records.len() {
            return Err("Row is outside the table.".into());
        }
        let mut bytes = vec![self.delimiter; self.columns.saturating_sub(1)];
        bytes.extend_from_slice(&self.line_ending);
        let position = if record < self.records.len() {
            self.records[record].start
        } else {
            if self
                .records
                .last()
                .is_some_and(|row| row.end == row.terminator_end)
            {
                let mut prefix = self.line_ending.clone();
                prefix.extend_from_slice(&bytes);
                bytes = prefix;
            }
            self.bytes.len()
        };
        Ok(vec![ByteEdit {
            range: position..position,
            bytes,
        }])
    }

    pub fn delete_rows(&self, rows: &[usize]) -> Result<Vec<ByteEdit>, String> {
        let rows = rows.iter().copied().collect::<BTreeSet<_>>();
        if rows.iter().any(|&row| row >= self.records.len()) {
            return Err("Row is outside the table.".into());
        }
        let mut groups = Vec::<Range<usize>>::new();
        for row in rows {
            if let Some(last) = groups.last_mut()
                && last.end == row
            {
                last.end += 1;
                continue;
            }
            groups.push(row..row + 1);
        }
        Ok(self.coalesce_bulk_edits(
            groups
                .into_iter()
                .map(|group| {
                    let last = &self.records[group.end - 1];
                    let start = if group.end == self.records.len()
                        && last.end == last.terminator_end
                        && group.start > 0
                    {
                        self.records[group.start - 1].end
                    } else {
                        self.records[group.start].start
                    };
                    ByteEdit {
                        range: start..last.terminator_end,
                        bytes: Vec::new(),
                    }
                })
                .collect(),
        ))
    }

    pub fn insert_column(&self, column: usize, header: bool) -> Result<Vec<ByteEdit>, String> {
        if column > self.columns {
            return Err("Column is outside the table.".into());
        }
        if self.columns >= MAX_COLUMNS {
            return Err(format!("CSV supports at most {MAX_COLUMNS} columns."));
        }
        if self.records.is_empty() {
            return Ok(vec![ByteEdit {
                range: self.bytes.len()..self.bytes.len(),
                bytes: vec![self.delimiter; self.columns],
            }]);
        }
        let mut edits = Vec::new();
        for record in 0..self.records.len() {
            let mut fields = self.raw_fields(record);
            fields.resize_with(fields.len().max(column), Vec::new);
            fields.insert(
                column,
                if record == 0 && header {
                    encode_field(&format!("Column {}", column + 1), self.delimiter)
                } else {
                    Vec::new()
                },
            );
            if let Some(edit) = self.record_edit(record, &fields, false) {
                edits.push(edit);
            }
        }
        Ok(self.coalesce_bulk_edits(edits))
    }

    pub fn delete_columns(&self, columns: &[usize]) -> Result<Vec<ByteEdit>, String> {
        let columns = columns.iter().copied().collect::<BTreeSet<_>>();
        if columns.iter().any(|&column| column >= self.columns) {
            return Err("Column is outside the table.".into());
        }
        if columns.len() >= self.columns {
            return Err("Keep at least one column in the table.".into());
        }
        if columns.is_empty() {
            return Ok(Vec::new());
        }
        let mut edits = Vec::new();
        for record in 0..self.records.len() {
            let fields = self
                .raw_fields(record)
                .into_iter()
                .enumerate()
                .filter_map(|(column, bytes)| (!columns.contains(&column)).then_some(bytes))
                .collect::<Vec<_>>();
            if let Some(edit) = self.record_edit(record, &fields, false) {
                edits.push(edit);
            }
        }
        Ok(self.coalesce_bulk_edits(edits))
    }

    pub fn copy_tsv(&self, rows: &[usize], columns: RangeInclusive<usize>) -> String {
        let mut output = Vec::new();
        for (row_index, &row) in rows.iter().enumerate() {
            if row_index > 0 {
                output.push(b'\n');
            }
            for (column_index, column) in columns.clone().enumerate() {
                if column_index > 0 {
                    output.push(b'\t');
                }
                let value = self.cell(row, column);
                if rows.len() > 1
                    && row_index + 1 == rows.len()
                    && columns.start() == columns.end()
                    && value.is_empty()
                {
                    // A final empty one-field record otherwise looks exactly
                    // like a trailing record separator to clipboard parsers.
                    output.extend_from_slice(b"\"\"");
                } else {
                    output.extend(encode_field(&value, b'\t'));
                }
            }
        }
        String::from_utf8(output).expect("cells contain validated UTF-8")
    }

    pub fn visible_rows(
        &self,
        header: bool,
        global_filter: &str,
        filters: &[ColumnFilter],
        sort: Option<&Sort>,
        cancel: impl Fn() -> bool,
    ) -> Result<Vec<usize>, String> {
        let start = usize::from(header).min(self.records.len());
        let global_filter = global_filter.to_lowercase();
        let filters = filters
            .iter()
            .map(|filter| (filter, filter.value.to_lowercase()))
            .collect::<Vec<_>>();
        let mut rows = Vec::new();
        for record in start..self.records.len() {
            if record & 255 == 0 && cancel() {
                return Err("Cancelled".into());
            }
            if !global_filter.is_empty()
                && !(0..self.record_columns(record)).any(|column| {
                    self.cell(record, column)
                        .to_lowercase()
                        .contains(&global_filter)
                })
            {
                continue;
            }
            if !filters.iter().all(|(filter, needle)| {
                let value = self.cell(record, filter.column);
                match filter.op {
                    FilterOp::Contains => value.to_lowercase().contains(needle),
                    FilterOp::Equals => value.to_lowercase() == *needle,
                    FilterOp::Empty => value.is_empty(),
                }
            }) {
                continue;
            }
            rows.push(record);
        }
        if let Some(sort) = sort {
            if sort.column >= self.columns {
                return Err("Sort column is outside the table.".into());
            }
            let mut keys = Vec::with_capacity(self.records.len());
            let mut all_numeric = true;
            for record in 0..self.records.len() {
                if record & 255 == 0 && cancel() {
                    return Err("Cancelled".into());
                }
                let value = self.cell(record, sort.column);
                let number = finite_number(&value);
                let empty = value.is_empty();
                if record >= start && !empty && number.is_none() {
                    all_numeric = false;
                }
                keys.push(SortKey {
                    text: value.to_lowercase(),
                    number,
                    empty,
                });
            }
            let numeric =
                sort.mode == SortMode::Number || (sort.mode == SortMode::Auto && all_numeric);
            cancellable_sort(
                &mut rows,
                |left, right| compare_keys(&keys[left], &keys[right], numeric, sort.descending),
                &cancel,
            )?;
        }
        if cancel() {
            return Err("Cancelled".into());
        }
        Ok(rows)
    }
}

pub fn parse_clipboard(text: &str) -> Result<Vec<Vec<String>>, String> {
    if text.is_empty() {
        return Ok(vec![vec![String::new()]]);
    }
    let table = parse(
        Arc::from(text.as_bytes()),
        Some(b'\t'),
        "clipboard.tsv",
        || false,
    )?;
    if table.records.is_empty() {
        return Ok(vec![vec![String::new()]]);
    }
    Ok((0..table.records.len())
        .map(|record| {
            (0..table.record_columns(record))
                .map(|column| table.cell(record, column).into_owned())
                .collect()
        })
        .collect())
}

fn join_fields(fields: &[Vec<u8>], delimiter: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (index, value) in fields.iter().enumerate() {
        if index > 0 {
            bytes.push(delimiter);
        }
        bytes.extend_from_slice(value);
    }
    bytes
}

fn encode_field(value: &str, delimiter: u8) -> Vec<u8> {
    if value
        .as_bytes()
        .iter()
        .any(|&byte| byte == delimiter || byte == b'"' || byte == b'\r' || byte == b'\n')
    {
        let mut output = Vec::with_capacity(value.len() + 2);
        output.push(b'"');
        for byte in value.bytes() {
            output.push(byte);
            if byte == b'"' {
                output.push(byte);
            }
        }
        output.push(b'"');
        output
    } else {
        value.as_bytes().to_vec()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueType {
    Number,
    Boolean,
    Date,
    Text,
}

fn value_type(value: &str) -> ValueType {
    if finite_number(value).is_some() {
        ValueType::Number
    } else if value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false") {
        ValueType::Boolean
    } else if value.len() >= 10
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value.as_bytes()[..4].iter().all(u8::is_ascii_digit)
        && value.as_bytes()[5..7].iter().all(u8::is_ascii_digit)
        && value.as_bytes()[8..10].iter().all(u8::is_ascii_digit)
    {
        ValueType::Date
    } else {
        ValueType::Text
    }
}

fn finite_number(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

struct SortKey {
    text: String,
    number: Option<f64>,
    empty: bool,
}

fn compare_keys(left: &SortKey, right: &SortKey, numeric: bool, descending: bool) -> Ordering {
    if left.empty != right.empty {
        return left.empty.cmp(&right.empty);
    }
    if left.empty {
        return Ordering::Equal;
    }
    let order = if numeric {
        match (left.number, right.number) {
            (Some(left), Some(right)) => left.partial_cmp(&right).unwrap(),
            (Some(_), None) => return Ordering::Less,
            (None, Some(_)) => return Ordering::Greater,
            (None, None) => left.text.cmp(&right.text),
        }
    } else {
        left.text.cmp(&right.text)
    };
    if descending { order.reverse() } else { order }
}

/// Bottom-up stable merge sort lets a new document revision interrupt expensive
/// work without changing comparator behavior midway through a standard sort.
fn cancellable_sort(
    rows: &mut Vec<usize>,
    compare: impl Fn(usize, usize) -> Ordering,
    cancel: &impl Fn() -> bool,
) -> Result<(), String> {
    let mut buffer = vec![0; rows.len()];
    let mut width = 1;
    while width < rows.len() {
        if cancel() {
            return Err("Cancelled".into());
        }
        let mut start = 0;
        while start < rows.len() {
            let middle = (start + width).min(rows.len());
            let end = (middle + width).min(rows.len());
            let mut left = start;
            let mut right = middle;
            for (offset, destination) in buffer[start..end].iter_mut().enumerate() {
                if offset & 4095 == 0 && cancel() {
                    return Err("Cancelled".into());
                }
                if right == end
                    || (left < middle && compare(rows[left], rows[right]) != Ordering::Greater)
                {
                    *destination = rows[left];
                    left += 1;
                } else {
                    *destination = rows[right];
                    right += 1;
                }
            }
            start = end;
        }
        std::mem::swap(rows, &mut buffer);
        width *= 2;
    }
    Ok(())
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
