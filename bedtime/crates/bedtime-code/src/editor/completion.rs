//! Completions as edits: a main edit and others (an import, say), applied as
//! one change; a snippet's tab stops are parsed here. The menu that offers
//! them comes with hover and completions.
use crate::editor::*;
use std::collections::BTreeMap;
use std::ops::Range;

#[derive(Clone, Debug)]
pub struct CompletionEdit {
    pub primary: TextEdit,
    pub additional: Vec<TextEdit>,
    pub snippet: bool,
}
pub struct PreparedCompletion {
    pub edit: BufferEdit,
    pub stops: Vec<(u32, Range<usize>)>,
    pub caret: usize,
}
impl CompletionEdit {
    pub fn prepare(&self, source: &TextBuffer) -> Result<PreparedCompletion, String> {
        let (inserted, mut stops) = if self.snippet {
            parse_snippet(&self.primary.replacement)?
        } else {
            (self.primary.replacement.clone(), Vec::new())
        };
        let primary = TextEdit {
            range: self.primary.range.clone(),
            replacement: inserted.clone(),
        };
        let mut edits: Vec<_> = self.additional.iter().cloned().chain([primary]).collect();
        edits.sort_by_key(|e| (e.range.start, e.range.end));
        for edit in &edits {
            if edit.range.start > edit.range.end
                || !source.is_char_boundary(edit.range.start)
                || !source.is_char_boundary(edit.range.end)
            {
                return Err("Completion has an invalid UTF-8 range".into());
            }
        }
        for pair in edits.windows(2) {
            if pair[0].range.end > pair[1].range.start || pair[0].range.start == pair[1].range.start
            {
                return Err("Completion edits overlap".into());
            }
        }
        if edits.iter().map(|e| e.replacement.len()).sum::<usize>() > 8 * 1024 * 1024 {
            return Err("Completion replacement exceeds 8 MiB".into());
        }
        let start = edits.first().unwrap().range.start;
        let end = edits.last().unwrap().range.end;
        let mut result = source.slice(start..end);
        let mut new_end = end;
        let mut primary_start = self.primary.range.start;
        for edit in edits.iter().rev() {
            BufferEdit::new(
                edit.range.start - start..edit.range.end - start,
                edit.replacement.as_str(),
            )
            .apply(&mut result)?;
            new_end = new_end - edit.range.len() + edit.replacement.len();
        }
        for edit in &self.additional {
            if edit.range.end <= self.primary.range.start {
                primary_start = primary_start - edit.range.len() + edit.replacement.len();
            }
        }
        for (_, range) in &mut stops {
            range.start += primary_start;
            range.end += primary_start;
        }
        if !stops.iter().any(|(n, _)| *n == 0) {
            stops.push((
                0,
                primary_start + inserted.len()..primary_start + inserted.len(),
            ));
        }
        stops.sort_by_key(|(n, r)| (if *n == 0 { u32::MAX } else { *n }, r.start));
        let caret = stops
            .first()
            .map_or(primary_start + inserted.len(), |(_, r)| r.end);
        debug_assert_eq!(start + result.len(), new_end);
        Ok(PreparedCompletion {
            edit: BufferEdit::new(start..end, result),
            stops,
            caret,
        })
    }
}

/// LSP snippets: tabstops, nested defaults, choices, escapes and variable
/// defaults. Transform expressions are rejected instead of inserted literally.
pub fn parse_snippet(input: &str) -> Result<(String, Vec<(u32, Range<usize>)>), String> {
    struct Parser<'a> {
        input: &'a str,
        at: usize,
        out: String,
        stops: Vec<(u32, Range<usize>)>,
        values: BTreeMap<u32, String>,
    }
    impl Parser<'_> {
        fn part(&mut self, nested: bool, depth: usize) -> Result<(), String> {
            if depth > 32 {
                return Err("Snippet nesting exceeds 32".into());
            }
            while self.at < self.input.len() {
                let c = self.input[self.at..].chars().next().unwrap();
                self.at += c.len_utf8();
                if nested && c == '}' {
                    return Ok(());
                }
                if c == '\\' {
                    let next = self.input[self.at..]
                        .chars()
                        .next()
                        .ok_or("Unterminated snippet escape")?;
                    if matches!(next, '$' | '}' | '\\' | ',' | '|') {
                        self.at += next.len_utf8();
                        self.out.push(next);
                    } else {
                        self.out.push(c);
                    }
                    continue;
                }
                if c != '$' {
                    self.out.push(c);
                    continue;
                }
                let braced = self.input[self.at..].starts_with('{');
                if braced {
                    self.at += 1;
                }
                let begin = self.at;
                while self.at < self.input.len() && self.input.as_bytes()[self.at].is_ascii_digit()
                {
                    self.at += 1;
                }
                let number = if self.at > begin {
                    Some(
                        self.input[begin..self.at]
                            .parse::<u32>()
                            .map_err(|_| "Snippet tabstop is too large")?,
                    )
                } else {
                    None
                };
                if number.is_none() {
                    while self.at < self.input.len()
                        && (self.input.as_bytes()[self.at].is_ascii_alphanumeric()
                            || self.input.as_bytes()[self.at] == b'_')
                    {
                        self.at += 1;
                    }
                    if self.at == begin {
                        return Err("Invalid snippet variable".into());
                    }
                }
                let start = self.out.len();
                if braced {
                    let token = *self
                        .input
                        .as_bytes()
                        .get(self.at)
                        .ok_or("Unterminated snippet placeholder")?;
                    self.at += 1;
                    match token {
                        b'}' => {
                            if let Some(n) = number
                                && let Some(v) = self.values.get(&n)
                            {
                                self.out.push_str(v);
                            }
                        }
                        b':' => self.part(true, depth + 1)?,
                        b'|' if number.is_some() => {
                            let mut first = String::new();
                            let mut first_choice = true;
                            let mut ended = false;
                            while self.at < self.input.len() {
                                let ch = self.input[self.at..].chars().next().unwrap();
                                self.at += ch.len_utf8();
                                if ch == '|' && self.input[self.at..].starts_with('}') {
                                    self.at += 1;
                                    ended = true;
                                    break;
                                }
                                if ch == ',' {
                                    first_choice = false;
                                    continue;
                                }
                                if ch == '\\' {
                                    let ch = self.input[self.at..]
                                        .chars()
                                        .next()
                                        .ok_or("Invalid snippet choice")?;
                                    self.at += ch.len_utf8();
                                    if first_choice {
                                        first.push(ch);
                                    }
                                } else if first_choice {
                                    first.push(ch);
                                }
                            }
                            if !ended {
                                return Err("Unterminated snippet choice".into());
                            }
                            self.out.push_str(&first);
                        }
                        _ => {
                            return Err(
                                "Unsupported snippet transform or invalid placeholder".into()
                            );
                        }
                    }
                } else if let Some(n) = number
                    && let Some(v) = self.values.get(&n)
                {
                    self.out.push_str(v);
                }
                if let Some(n) = number {
                    if self.stops.len() >= 256 {
                        return Err("Too many snippet placeholders".into());
                    }
                    self.values
                        .entry(n)
                        .or_insert_with(|| self.out[start..].to_owned());
                    self.stops.push((n, start..self.out.len()));
                }
            }
            if nested {
                Err("Unterminated snippet placeholder".into())
            } else {
                Ok(())
            }
        }
    }
    let mut parser = Parser {
        input,
        at: 0,
        out: String::new(),
        stops: vec![],
        values: BTreeMap::new(),
    };
    parser.part(false, 0)?;
    Ok((parser.out, parser.stops))
}
#[derive(Clone, Debug)]
pub struct CompletionChoice {
    /// Short host-supplied category, such as `fn` or `type`.
    pub kind: std::sync::Arc<str>,
    pub label: std::sync::Arc<str>,
    pub detail: std::sync::Arc<str>,
}
