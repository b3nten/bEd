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

pub fn parse(text: &str, delimiter: u8) -> (Vec<Vec<String>>, Format) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let crlf = text.contains("\r\n");
    let final_newline = text.ends_with('\n');
    let delimiter = delimiter as char;
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
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
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
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
    }
    (
        rows,
        Format {
            delimiter: delimiter as u8,
            crlf,
            final_newline,
        },
    )
}

pub fn write(
    rows: &[Vec<String>],
    format: Format,
    out: &mut dyn std::io::Write,
) -> std::io::Result<()> {
    let delimiter = format.delimiter as char;
    let newline = if format.crlf { "\r\n" } else { "\n" };
    for (index, row) in rows.iter().enumerate() {
        let mut line = String::new();
        for (column, field) in row.iter().enumerate() {
            if column > 0 {
                line.push(delimiter);
            }
            let quote = field.contains(delimiter)
                || field.contains('"')
                || field.contains('\n')
                || field.contains('\r')
                || field.starts_with(' ')
                || field.ends_with(' ');
            if quote {
                line.push('"');
                line.push_str(&field.replace('"', "\"\""));
                line.push('"');
            } else {
                line.push_str(field);
            }
        }
        out.write_all(line.as_bytes())?;
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
        let (rows, format) = parse(text, sniff(text));
        let mut out = Vec::new();
        write(&rows, format, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn quotes_delimiters_and_line_breaks() {
        let text = "name,note\n\"Smith, J\",\"said \"\"hi\"\"\nthen left\"\nplain,\n";
        let (rows, format) = parse(text, b',');
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1], ["Smith, J", "said \"hi\"\nthen left"]);
        assert_eq!(rows[2], ["plain", ""]);
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
        let (rows, _) = parse("a,b,c\n1\n", b',');
        assert_eq!(rows, [vec!["a", "b", "c"], vec!["1"]]);
        assert!(parse("", b',').0.is_empty());
        assert_eq!(parse("\u{feff}x\n", b',').0, [vec!["x"]]);
    }
}
