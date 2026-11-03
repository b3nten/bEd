//! CSV and TSV (RFC 4180): fields split by a delimiter, quoted when they
//! hold one, a quote or a line break, with quotes doubled inside. Rows may
//! have different lengths. Writing keeps the file's delimiter and line
//! endings, and quotes only fields that need it.

/// How a file was written, to write it back the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub delimiter: u8,
    pub crlf: bool,
    /// The last row ends with a line break.
    pub final_newline: bool,
}

impl Default for Format {
    fn default() -> Self {
        Self {
            delimiter: b',',
            crlf: false,
            final_newline: true,
        }
    }
}

/// The delimiter of `text`: whichever of comma, tab, semicolon and pipe
/// splits its first lines most evenly (outside quotes).
pub fn sniff(text: &str) -> u8 {
    let lines: Vec<&str> = text.lines().take(20).collect();
    let mut best = (b',', 0usize);
    for delimiter in *b",\t;|" {
        let counts: Vec<usize> = lines
            .iter()
            .map(|line| {
                let mut quoted = false;
                line.bytes()
                    .filter(|&b| {
                        if b == b'"' {
                            quoted = !quoted;
                        }
                        !quoted && b == delimiter
                    })
                    .count()
            })
            .collect();
        let Some(&first) = counts.first() else {
            continue;
        };
        // Every line the same, and as many as possible.
        let consistent = counts.iter().filter(|&&count| count == first).count();
        let score = if first == 0 {
            0
        } else {
            consistent * 1000 + first
        };
        if score > best.1 {
            best = (delimiter, score);
        }
    }
    best.0
}

/// The rows of `text`, how it's written, and where each row starts (byte
/// offsets into `text`).
pub fn parse(text: &str, delimiter: u8) -> (Vec<Vec<String>>, Format, Vec<usize>) {
    let bom = if text.starts_with('\u{feff}') { 3 } else { 0 };
    let crlf = text.contains("\r\n");
    let final_newline = text.ends_with('\n');
    let delimiter = delimiter as char;
    let mut rows = Vec::new();
    let mut starts = Vec::new();
    let mut start = bom;
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut chars = text[bom..].char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek().is_some_and(|&(_, next)| next == '"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => {
                quoted = true;
                started = true;
            }
            c if c == delimiter => {
                row.push(std::mem::take(&mut field));
                started = true;
            }
            '\r' if chars.peek().is_some_and(|&(_, next)| next == '\n') => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                starts.push(start);
                start = bom + at + 1;
                started = false;
            }
            c => {
                field.push(c);
                started = true;
            }
        }
    }
    if started || !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
        starts.push(start);
    }
    (
        rows,
        Format {
            delimiter: delimiter as u8,
            crlf,
            final_newline,
        },
        starts,
    )
}

/// Where each field of one row's text is (quotes included).
pub fn field_spans(row: &str, delimiter: u8) -> Vec<std::ops::Range<usize>> {
    let mut spans = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    for (at, byte) in row.bytes().enumerate() {
        if byte == b'"' {
            quoted = !quoted;
        } else if byte == delimiter && !quoted {
            spans.push(start..at);
            start = at + 1;
        }
    }
    spans.push(start..row.len());
    spans
}

/// A field as written: quoted when it holds the delimiter, a quote, a line
/// break, or space at either end.
pub fn quote(field: &str, delimiter: u8) -> std::borrow::Cow<'_, str> {
    let quote = field.contains(delimiter as char)
        || field.contains('"')
        || field.contains('\n')
        || field.contains('\r')
        || field.starts_with(' ')
        || field.ends_with(' ');
    if quote {
        format!("\"{}\"", field.replace('"', "\"\"")).into()
    } else {
        field.into()
    }
}

/// One row as written, without its line break.
pub fn row_text(row: &[String], delimiter: u8) -> String {
    let mut line = String::new();
    for (column, field) in row.iter().enumerate() {
        if column > 0 {
            line.push(delimiter as char);
        }
        line.push_str(&quote(field, delimiter));
    }
    line
}

pub fn write(
    rows: &[Vec<String>],
    format: Format,
    out: &mut dyn std::io::Write,
) -> std::io::Result<()> {
    let newline = if format.crlf { "\r\n" } else { "\n" };
    for (index, row) in rows.iter().enumerate() {
        out.write_all(row_text(row, format.delimiter).as_bytes())?;
        if index + 1 < rows.len() || format.final_newline {
            out.write_all(newline.as_bytes())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(text: &str) -> String {
        let (rows, format, _) = parse(text, sniff(text));
        let mut out = Vec::new();
        write(&rows, format, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn quotes_delimiters_and_line_breaks() {
        let text = "name,note\n\"Smith, J\",\"said \"\"hi\"\"\nthen left\"\nplain,\n";
        let (rows, format, _) = parse(text, b',');
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1], ["Smith, J", "said \"hi\"\nthen left"]);
        assert_eq!(rows[2], ["plain", ""]);
        let (_, _, starts) = parse(text, b',');
        assert_eq!(starts, [0, 10, 45]);
        assert_eq!(field_spans("\"Smith, J\",x", b','), [0..10, 11..12]);
        assert!(format.final_newline && !format.crlf);
        assert_eq!(round_trip(text), text);
    }

    #[test]
    fn delimiters_are_sniffed_and_kept() {
        assert_eq!(sniff("a\tb\tc\n1\t2\t3\n"), b'\t');
        assert_eq!(sniff("a;b\n1;2\n"), b';');
        assert_eq!(sniff("a,b\n\"1;2\",3\n"), b',');
        let crlf = "a;b\r\n1;2";
        assert_eq!(round_trip(crlf), crlf);
    }

    #[test]
    fn ragged_rows_and_empty_files() {
        let (rows, _, starts) = parse("a,b,c\n1\n", b',');
        assert_eq!(starts, [0, 6]);
        assert_eq!(rows, [vec!["a", "b", "c"], vec!["1"]]);
        assert!(parse("", b',').0.is_empty());
        assert_eq!(parse("\u{feff}x\n", b',').0, [vec!["x"]]);
        assert_eq!(parse("\u{feff}x\ny", b',').2, [3, 5]);
    }
}
