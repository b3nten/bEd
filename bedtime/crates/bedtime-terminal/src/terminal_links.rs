//! Link recognition uses terminal cells, so hit testing follows grapheme widths
//! and soft-wrapped rows rather than treating the viewport as unrelated strings.

use std::path::PathBuf;

pub struct LinkCell<'a> {
    pub text: &'a str,
    /// Continuation cells have width zero and are skipped.
    pub width: u8,
    pub explicit: Option<&'a str>,
}

pub struct LinkRow<'a> {
    pub cells: Vec<LinkCell<'a>>,
    /// This row continues onto the next row without a newline.
    pub wrapped: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkSpan {
    pub row: usize,
    pub col: usize,
    pub end_col: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalLink {
    pub target: String,
    pub spans: Vec<LinkSpan>,
    pub explicit: bool,
}

impl TerminalLink {
    pub fn contains(&self, row: usize, col: usize) -> bool {
        self.spans
            .iter()
            .any(|span| span.row == row && span.col <= col && col < span.end_col)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    External(String),
    File {
        host: Option<String>,
        path: PathBuf,
        /// Zero-based line and byte column, matching editor coordinates.
        position: Option<(i32, i32)>,
    },
}

/// Only schemes with a defined opening action cross the host boundary. In
/// particular an OSC 8 label cannot launch an arbitrary executable URI scheme.
pub fn classify_link(target: &str) -> Option<LinkTarget> {
    if target.chars().any(char::is_control) {
        return None;
    }
    let (scheme, remainder) = target.split_once(':').unwrap_or(("", ""));
    if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
        let authority = remainder
            .strip_prefix("//")?
            .split(['/', '?', '#'])
            .next()?;
        return (!authority.is_empty()).then(|| LinkTarget::External(target.into()));
    }
    if scheme.eq_ignore_ascii_case("mailto") {
        return (!remainder.is_empty()).then(|| LinkTarget::External(target.into()));
    }
    if !scheme.eq_ignore_ascii_case("file") {
        return classify_path(target);
    }
    let path_and_host = remainder.strip_prefix("//")?;
    let (host, path) = path_and_host.split_once('/')?;
    let path = format!("/{path}");
    // A fragment identifies a location within a file; it is not part of its path.
    let position = path
        .split_once('#')
        .and_then(|(_, fragment)| parse_position(fragment.strip_prefix('L').unwrap_or(fragment)));
    let path = decode_percent(path.split(['?', '#']).next()?)?;
    if path.as_bytes().contains(&0) {
        return None;
    }
    Some(LinkTarget::File {
        host: if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
            None
        } else {
            Some(host.into())
        },
        path: PathBuf::from(path),
        position,
    })
}

fn parse_position(text: &str) -> Option<(i32, i32)> {
    let (line, column) = text.split_once([':', 'C']).unwrap_or((text, "1"));
    let coordinate = |value: &str| {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        value
            .parse::<i32>()
            .ok()?
            .checked_sub(1)
            .filter(|value| *value >= 0)
    };
    Some((coordinate(line)?, coordinate(column)?))
}

fn classify_path(target: &str) -> Option<LinkTarget> {
    // A colon is allowed only in a numeric location suffix. This also keeps
    // unsupported URI schemes from being interpreted as local paths.
    let (path, position) = if let Some((path, location)) = target.split_once(':') {
        (path, Some(parse_position(location)?))
    } else {
        (target, None)
    };
    if path.is_empty()
        || path.ends_with('/')
        || path
            .chars()
            .any(|character| matches!(character, '?' | '#' | '*' | '=' | '|' | '\\'))
        || path
            .split('/')
            .all(|part| part.is_empty() || matches!(part, "." | ".." | "~"))
    {
        return None;
    }
    let name = path.rsplit('/').next()?;
    if !path.contains('/')
        && (!name
            .chars()
            .any(|character| character.is_alphabetic() || character == '_')
            || (!name.contains('.')
                && !matches!(
                    name,
                    "Makefile"
                        | "Dockerfile"
                        | "Justfile"
                        | "Gemfile"
                        | "Rakefile"
                        | "README"
                        | "LICENSE"
                        | "NOTICE"
                )))
    {
        return None;
    }
    Some(LinkTarget::File {
        host: None,
        path: PathBuf::from(path),
        position,
    })
}

fn decode_percent(text: &str) -> Option<String> {
    let source = text.as_bytes();
    let mut bytes = Vec::with_capacity(source.len());
    let mut at = 0;
    while at < source.len() {
        if source[at] == b'%' {
            let high = char::from(*source.get(at + 1)?).to_digit(16)?;
            let low = char::from(*source.get(at + 2)?).to_digit(16)?;
            bytes.push((high * 16 + low) as u8);
            at += 3;
        } else {
            bytes.push(source[at]);
            at += 1;
        }
    }
    String::from_utf8(bytes).ok()
}

struct CellBytes {
    start: usize,
    end: usize,
    span: LinkSpan,
    explicit: bool,
}

pub fn collect_links(rows: &[LinkRow<'_>]) -> Vec<TerminalLink> {
    let mut links: Vec<TerminalLink> = Vec::new();
    let mut text = String::new();
    let mut mapping: Vec<CellBytes> = Vec::new();
    let mut explicit: Option<TerminalLink> = None;
    for (row_index, row) in rows.iter().enumerate() {
        for (col, cell) in row.cells.iter().enumerate() {
            if cell.width == 0 {
                continue;
            }
            let span = LinkSpan {
                row: row_index,
                col,
                end_col: col + usize::from(cell.width),
            };
            match (cell.explicit, explicit.as_mut()) {
                (Some(target), Some(link)) if link.target == target => {
                    append_span(&mut link.spans, span)
                }
                (Some(target), _) => {
                    if let Some(link) = explicit.take() {
                        links.push(link);
                    }
                    explicit = Some(TerminalLink {
                        target: target.into(),
                        spans: vec![span],
                        explicit: true,
                    });
                }
                (None, _) => {
                    if let Some(link) = explicit.take() {
                        links.push(link);
                    }
                }
            }
            let start = text.len();
            text.push_str(cell.text);
            mapping.push(CellBytes {
                start,
                end: text.len(),
                span,
                explicit: cell.explicit.is_some(),
            });
        }
        if !row.wrapped || row_index + 1 == rows.len() {
            recognize_plain(&text, &mapping, &mut links);
            text.clear();
            mapping.clear();
            if let Some(link) = explicit.take() {
                links.push(link);
            }
        }
    }
    links
}

fn append_span(spans: &mut Vec<LinkSpan>, span: LinkSpan) {
    if let Some(previous) = spans.last_mut()
        && previous.row == span.row
        && previous.end_col == span.col
    {
        previous.end_col = span.end_col;
    } else {
        spans.push(span);
    }
}

fn recognize_plain(text: &str, mapping: &[CellBytes], links: &mut Vec<TerminalLink>) {
    let mut at = 0;
    while at < text.len() {
        let character = text[at..].chars().next().unwrap();
        if character.is_whitespace()
            || character.is_control()
            || matches!(character, '<' | '>' | '`' | '(' | '[' | '{' | '=')
        {
            at += character.len_utf8();
            continue;
        }
        let quote = matches!(character, '"' | '\'').then_some(character);
        if quote.is_some() {
            at += character.len_utf8();
        }
        let uri = ["https://", "http://", "mailto:", "file://"]
            .iter()
            .any(|scheme| {
                text[at..]
                    .get(..scheme.len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
            });
        let mut end = at;
        for character in text[end..].chars() {
            let delimiter = if let Some(quote) = quote {
                character == quote || character.is_control()
            } else {
                character.is_whitespace()
                    || character.is_control()
                    || matches!(character, '<' | '>' | '"' | '\'' | '`')
                    || (!uri && character == '=')
            };
            if delimiter {
                break;
            }
            end += character.len_utf8();
        }
        let scanned_end = end;
        let end = if quote.is_some() {
            end
        } else {
            trim_punctuation(text, at, end)
        };
        let candidate = &text[at..end];
        let cells = mapping
            .iter()
            .filter(|cell| cell.start < end && at < cell.end)
            .collect::<Vec<_>>();
        if classify_link(candidate).is_some() && !cells.iter().any(|cell| cell.explicit) {
            let mut spans = Vec::new();
            for cell in cells {
                append_span(&mut spans, cell.span);
            }
            links.push(TerminalLink {
                target: candidate.into(),
                spans,
                explicit: false,
            });
        }
        // Advance beyond the whole scanned token, including trailing punctuation.
        at = scanned_end;
        if at < text.len() {
            at += text[at..].chars().next().unwrap().len_utf8();
        }
    }
}

fn trim_punctuation(text: &str, start: usize, mut end: usize) -> usize {
    while end > start {
        let candidate = &text[start..end];
        let character = candidate.chars().next_back().unwrap();
        let unmatched = match character {
            ')' => candidate.matches(')').count() > candidate.matches('(').count(),
            ']' => candidate.matches(']').count() > candidate.matches('[').count(),
            '}' => candidate.matches('}').count() > candidate.matches('{').count(),
            _ => false,
        };
        if unmatched || matches!(character, '.' | ',' | ';' | ':' | '!' | '?') {
            end -= character.len_utf8();
        } else {
            break;
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str, wrapped: bool) -> LinkRow<'_> {
        LinkRow {
            cells: text
                .char_indices()
                .map(|(offset, character)| LinkCell {
                    text: &text[offset..offset + character.len_utf8()],
                    width: 1,
                    explicit: None,
                })
                .collect(),
            wrapped,
        }
    }

    #[test]
    fn links_continue_over_soft_wraps_but_not_hard_newlines() {
        let rows = [
            row("see https://example.", true),
            row("com/path end", false),
        ];
        let links = collect_links(&rows);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "https://example.com/path");
        assert!(links[0].contains(0, 4));
        assert!(links[0].contains(1, 7));
        assert!(!links[0].contains(1, 8));
        let rows = [row("https://example.com", false), row("/unrelated", false)];
        assert_eq!(collect_links(&rows)[0].target, "https://example.com");
    }

    #[test]
    fn explicit_labels_take_priority_and_cover_wide_graphemes() {
        let mut rows = vec![row("https://example.com", false)];
        for cell in &mut rows[0].cells {
            cell.explicit = Some("https://different.example");
        }
        let links = collect_links(&rows);
        assert_eq!(links.len(), 1);
        assert!(links[0].explicit);
        assert_eq!(links[0].target, "https://different.example");
        let rows = [LinkRow {
            cells: vec![
                LinkCell {
                    text: "👩‍💻",
                    width: 2,
                    explicit: Some("https://example.com"),
                },
                LinkCell {
                    text: "",
                    width: 0,
                    explicit: None,
                },
                LinkCell {
                    text: "!",
                    width: 1,
                    explicit: None,
                },
            ],
            wrapped: false,
        }];
        let links = collect_links(&rows);
        assert!(links[0].contains(0, 1));
        assert!(!links[0].contains(0, 2));
    }

    #[test]
    fn prose_punctuation_and_balanced_url_parentheses_are_distinguished() {
        let rows = [row(
            "(https://example.com/a_(b)). https://other.example/!",
            false,
        )];
        let links = collect_links(&rows);
        assert_eq!(
            links
                .iter()
                .map(|link| link.target.as_str())
                .collect::<Vec<_>>(),
            ["https://example.com/a_(b)", "https://other.example/",]
        );
        let links = collect_links(&[row("URL=https://example.com/?a=1 FILE=src/main.rs", false)]);
        assert_eq!(
            links
                .iter()
                .map(|link| link.target.as_str())
                .collect::<Vec<_>>(),
            ["https://example.com/?a=1", "src/main.rs"]
        );
    }

    #[test]
    fn files_decode_paths_and_preserve_remote_host_for_workbench_routing() {
        assert_eq!(
            classify_link("file:///tmp/a%20b.rs#L10"),
            Some(LinkTarget::File {
                host: None,
                path: "/tmp/a b.rs".into(),
                position: Some((9, 0)),
            })
        );
        assert_eq!(
            classify_link("file://devbox/home/user/main.rs"),
            Some(LinkTarget::File {
                host: Some("devbox".into()),
                path: "/home/user/main.rs".into(),
                position: None,
            })
        );
        assert!(classify_link("file:///tmp/%00").is_none());
        assert!(classify_link("javascript:alert(1)").is_none());
        assert!(classify_link("https:///path").is_none());
        assert!(classify_link("file:///tmp/%XX").is_none());
    }

    #[test]
    fn bare_paths_and_locations_are_detected_in_diagnostic_output() {
        let rows = [row(
            "--> src/main.rs:42:7: error; (/tmp/log.txt:3), ./readme.md ../lib.rs ~/notes.txt main.rs Makefile:2 1.23 ordinary words",
            false,
        )];
        let links = collect_links(&rows);
        assert_eq!(
            links
                .iter()
                .map(|link| link.target.as_str())
                .collect::<Vec<_>>(),
            [
                "src/main.rs:42:7",
                "/tmp/log.txt:3",
                "./readme.md",
                "../lib.rs",
                "~/notes.txt",
                "main.rs",
                "Makefile:2",
            ]
        );
        assert_eq!(
            classify_link(&links[0].target),
            Some(LinkTarget::File {
                host: None,
                path: "src/main.rs".into(),
                position: Some((41, 6)),
            })
        );
        for invalid in [
            "main.rs:0",
            "main.rs:2:0",
            "main.rs:2147483648",
            "main.rs:2:3:4",
            "main.rs:no",
            "javascript:1",
            "data:text/plain,x",
            ".",
            "..",
            "/",
            "~/",
            "1.23",
        ] {
            assert!(classify_link(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn paths_wrap_and_quoted_paths_keep_spaces_and_literal_percent_signs() {
        let rows = [
            row("src/long/", true),
            row("main.rs:12:4 \"my files/100%.rs:2\"", false),
        ];
        let links = collect_links(&rows);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "src/long/main.rs:12:4");
        assert!(links[0].contains(1, 0));
        assert_eq!(
            classify_link(&links[1].target),
            Some(LinkTarget::File {
                host: None,
                path: "my files/100%.rs".into(),
                position: Some((1, 0)),
            })
        );
        assert_eq!(
            collect_links(&[row("src/long/", false), row("main.rs:12", false)]).len(),
            1
        );
    }

    #[test]
    fn file_url_fragments_and_explicit_labels_retain_locations() {
        for (fragment, position) in [
            ("L10", Some((9, 0))),
            ("L10C3", Some((9, 2))),
            ("10:3", Some((9, 2))),
            ("L0", None),
            ("section", None),
        ] {
            assert_eq!(
                classify_link(&format!("file:///tmp/a.rs#{fragment}")),
                Some(LinkTarget::File {
                    host: None,
                    path: "/tmp/a.rs".into(),
                    position,
                })
            );
        }
        let mut label = row("source", false);
        for cell in &mut label.cells {
            cell.explicit = Some("file:///tmp/a.rs#L10C3");
        }
        let links = collect_links(&[label]);
        assert_eq!(links.len(), 1);
        assert!(links[0].explicit);
        assert_eq!(
            classify_link(&links[0].target),
            Some(LinkTarget::File {
                host: None,
                path: "/tmp/a.rs".into(),
                position: Some((9, 2)),
            })
        );
    }
}
