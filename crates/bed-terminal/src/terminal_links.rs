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
    File { host: Option<String>, path: PathBuf },
}

/// Only schemes with a defined opening action cross the host boundary. In
/// particular an OSC 8 label cannot launch an arbitrary executable URI scheme.
pub fn classify_link(target: &str) -> Option<LinkTarget> {
    if target.chars().any(char::is_control) {
        return None;
    }
    let (scheme, remainder) = target.split_once(':')?;
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
        return None;
    }
    let path_and_host = remainder.strip_prefix("//")?;
    let (host, path) = path_and_host.split_once('/')?;
    let path = format!("/{path}");
    // A fragment identifies a location within a file; it is not part of its path.
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
        let rest = &text[at..];
        let scheme = ["https://", "http://", "mailto:", "file://"]
            .into_iter()
            .find(|scheme| {
                rest.get(..scheme.len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
            });
        let Some(scheme) = scheme else {
            at += rest.chars().next().unwrap().len_utf8();
            continue;
        };
        // A scheme embedded in an identifier is not a separate URL.
        if text[..at]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_alphanumeric() || character == '_')
        {
            at += scheme.len();
            continue;
        }
        let mut end = at + scheme.len();
        for character in text[end..].chars() {
            if character.is_whitespace()
                || character.is_control()
                || matches!(character, '<' | '>' | '"' | '\'' | '`')
            {
                break;
            }
            end += character.len_utf8();
        }
        end = trim_punctuation(text, at, end);
        let candidate = &text[at..end];
        let cells = mapping
            .iter()
            .filter(|cell| cell.start < end && at < cell.end)
            .collect::<Vec<_>>();
        if end > at + scheme.len()
            && classify_link(candidate).is_some()
            && !cells.iter().any(|cell| cell.explicit)
        {
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
        at = end.max(at + scheme.len());
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
    }

    #[test]
    fn files_decode_paths_and_preserve_remote_host_for_workbench_routing() {
        assert_eq!(
            classify_link("file:///tmp/a%20b.rs#L10"),
            Some(LinkTarget::File {
                host: None,
                path: "/tmp/a b.rs".into(),
            })
        );
        assert_eq!(
            classify_link("file://devbox/home/user/main.rs"),
            Some(LinkTarget::File {
                host: Some("devbox".into()),
                path: "/home/user/main.rs".into(),
            })
        );
        assert!(classify_link("file:///tmp/%00").is_none());
        assert!(classify_link("javascript:alert(1)").is_none());
        assert!(classify_link("https:///path").is_none());
        assert!(classify_link("file:///tmp/%XX").is_none());
    }
}
