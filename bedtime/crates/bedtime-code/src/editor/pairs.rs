//! Brackets and quotes in pairs: typing an opening one closes it, typing a
//! closing one steps over the one already there, Backspace between an empty
//! pair takes both, and typing one with text selected wraps the text. Also
//! which bracket matches the one at the caret: siblings in the syntax tree
//! (so brackets in strings and comments don't count), else a bounded scan.
use super::carets::Planned;
use super::*;

/// How far to scan for a match without a syntax tree.
const MAX_SCAN: usize = 128 * 1024;

fn closer(open: char) -> Option<char> {
    match open {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' | '\'' | '`' => Some(open),
        _ => None,
    }
}

fn is_bracket(c: char) -> bool {
    matches!(c, '(' | '[' | '{' | ')' | ']' | '}')
}

/// The quotes a language closes (Rust's `'` starts lifetimes; plain text
/// pairs no quotes).
fn quotes(language: Option<&str>) -> &'static [char] {
    match language {
        None => &[],
        Some("rust") => &['"'],
        Some("markdown") => &['`'],
        Some("javascript" | "typescript" | "tsx" | "go" | "bash" | "kotlin" | "dart") => {
            &['"', '\'', '`']
        }
        Some(_) => &['"', '\''],
    }
}

/// The character starting at `byte` (none inside one).
fn char_at(text: &TextBuffer, byte: usize) -> Option<char> {
    if byte >= text.len() || !text.is_char_boundary(byte) {
        return None;
    }
    let (chunk, start) = text.chunk_at(byte);
    chunk[byte - start..].chars().next()
}

fn char_before(text: &TextBuffer, byte: usize) -> Option<char> {
    (byte > 0).then(|| char_at(text, text.previous_grapheme(byte)))?
}

impl EditorView {
    /// Whether `byte` is inside a string or a comment, by an analysis of
    /// this revision.
    fn in_string_or_comment(&self, revision: u64, byte: usize) -> bool {
        self.syntax
            .as_ref()
            .filter(|syntax| syntax.key.revision == revision)
            .is_some_and(|syntax| {
                syntax
                    .highlights_in(byte.saturating_sub(1)..byte + 1)
                    .iter()
                    .any(|span| {
                        span.range.start < byte
                            && byte < span.range.end
                            && (span.capture.starts_with("string")
                                || span.capture.starts_with("comment"))
                    })
            })
    }

    /// Whether `c` is one this text pairs, at `byte`.
    fn pairs(&self, revision: u64, byte: usize, c: char) -> bool {
        is_bracket(c) || {
            let language = match &self.syntax {
                Some(syntax) if syntax.key.revision == revision => Some(syntax.language_at(byte)),
                _ => self.language,
            };
            quotes(language).contains(&c)
        }
    }

    /// Typing a bracket or quote, and Backspace between a pair, at every
    /// caret. `None` leaves the event to ordinary typing.
    pub(super) fn pair_event(
        &mut self,
        id: egui::Id,
        session: &mut dyn TextSession,
        snapshot: &mut TextSnapshot,
        event: &egui::Event,
        now: f64,
    ) -> Result<Option<bool>, EditorError> {
        if self.settings.read_only || !self.settings.auto_close {
            return Ok(None);
        }
        match event {
            egui::Event::Text(typed) => {
                let mut chars = typed.chars();
                let (Some(c), None) = (chars.next(), chars.next()) else {
                    return Ok(None);
                };
                let carets = self.carets();
                let revision = snapshot.revision;
                let text = &snapshot.text;
                let mut special = false;
                let planned: Vec<Planned> = carets
                    .iter()
                    .enumerate()
                    .map(|(index, caret)| {
                        let range = caret.selection.range();
                        let (edit, placed) = self.type_at(revision, text, range.clone(), c);
                        special |=
                            edit.replacement().len() != c.len_utf8() || edit.range() != range;
                        let selection = match placed {
                            Placed::Caret(at) => TextSelection {
                                anchor: at,
                                head: at,
                            },
                            Placed::Inside(start, end)
                                if caret.selection.head < caret.selection.anchor =>
                            {
                                TextSelection {
                                    anchor: end,
                                    head: start,
                                }
                            }
                            Placed::Inside(start, end) => TextSelection {
                                anchor: start,
                                head: end,
                            },
                        };
                        Planned {
                            edit,
                            owners: vec![(index, selection)],
                        }
                    })
                    .collect();
                if !special {
                    return Ok(None);
                }
                session.end_undo_group();
                self.commit_carets(id, session, snapshot, now, carets, planned)?;
                Ok(Some(true))
            }
            egui::Event::Key {
                key: egui::Key::Backspace,
                pressed: true,
                modifiers,
                ..
            } if modifiers.is_none() => {
                let carets = self.carets();
                let text = &snapshot.text;
                let between = |head: usize| {
                    let (Some(open), Some(next)) = (char_before(text, head), char_at(text, head))
                    else {
                        return false;
                    };
                    closer(open) == Some(next)
                        && self.pairs(snapshot.revision, head, open)
                        && !(open == next && self.in_string_or_comment(snapshot.revision, head - 1))
                };
                if carets.iter().any(|c| !c.selection.range().is_empty())
                    || !carets.iter().any(|c| between(c.selection.head))
                {
                    return Ok(None);
                }
                let style = self.settings.indentation;
                let smart = self.settings.smart_indent;
                let planned = carets
                    .iter()
                    .enumerate()
                    .filter_map(|(index, caret)| {
                        let head = caret.selection.head;
                        let range = if between(head) {
                            text.previous_grapheme(head)..text.next_grapheme(head)
                        } else if head == 0 {
                            return None;
                        } else if smart {
                            whitespace::backspace(text, head, style)..head
                        } else {
                            text.previous_grapheme(head)..head
                        };
                        let at = range.start;
                        Some(Planned {
                            edit: BufferEdit::new(range, ""),
                            owners: vec![(
                                index,
                                TextSelection {
                                    anchor: at,
                                    head: at,
                                },
                            )],
                        })
                    })
                    .collect();
                self.commit_carets(id, session, snapshot, now, carets, planned)?;
                Ok(Some(true))
            }
            _ => Ok(None),
        }
    }

    /// What typing `c` does at one caret.
    fn type_at(
        &self,
        revision: u64,
        text: &TextBuffer,
        range: Range<usize>,
        c: char,
    ) -> (BufferEdit, Placed) {
        let insert = |s: String, at: usize| (BufferEdit::new(range.clone(), s), Placed::Caret(at));
        let typed = c.len_utf8();
        let pairs = self.pairs(revision, range.start, c);
        let close = closer(c).filter(|_| pairs);
        if !range.is_empty() {
            // Wrap the selection, which stays selected inside.
            if let Some(close) = close.filter(|_| !matches!(c, ')' | ']' | '}')) {
                let selected = text.slice(range.clone()).to_string();
                return (
                    BufferEdit::new(range.clone(), format!("{c}{selected}{close}")),
                    Placed::Inside(range.start + typed, range.end + typed),
                );
            }
            return insert(c.to_string(), range.start + typed);
        }
        let at = range.start;
        let next = char_at(text, at);
        // Step over the closing one already there.
        if pairs && next == Some(c) && (matches!(c, ')' | ']' | '}') || closer(c) == Some(c)) {
            return (
                BufferEdit::new(at..at + typed, c.to_string()),
                Placed::Caret(at + typed),
            );
        }
        let Some(close) = close.filter(|_| !matches!(c, ')' | ']' | '}')) else {
            return insert(c.to_string(), at + typed);
        };
        // Close it only before whitespace or what ends an expression.
        let before_ok = next.is_none_or(|n| n.is_whitespace() || ";:.,=}])>".contains(n));
        let quote = close == c;
        let quote_ok = !quote
            || (char_before(text, at).is_none_or(|p| !(p.is_alphanumeric() || p == '_' || p == c))
                && !self.in_string_or_comment(revision, at));
        if before_ok && quote_ok {
            insert(format!("{c}{close}"), at + typed)
        } else {
            insert(c.to_string(), at + typed)
        }
    }

    /// The bracket beside `byte` (after it first) and its match.
    pub(super) fn matching_bracket(
        &self,
        snapshot: &TextSnapshot,
        byte: usize,
    ) -> Option<(usize, usize)> {
        let text = &snapshot.text;
        let tree = self
            .syntax
            .as_ref()
            .filter(|syntax| syntax.key.revision == snapshot.revision)
            .map(|syntax| &syntax.tree);
        [Some(byte), byte.checked_sub(1)]
            .into_iter()
            .flatten()
            .find_map(|at| {
                let c = char_at(text, at).filter(|&c| is_bracket(c))?;
                let found = match tree {
                    Some(tree) => tree_match(tree, at, c),
                    None => scan_match(text, at, c),
                };
                found.map(|other| (at, other))
            })
    }

    /// The matching bracket pair at the primary caret, kept while the text
    /// and the caret stay put.
    pub(super) fn bracket_pair(&mut self, snapshot: &TextSnapshot) -> Option<(usize, usize)> {
        let key = (
            snapshot.revision,
            self.selection.head,
            self.syntax.is_some(),
        );
        if let Some((cached, pair)) = self.brackets
            && cached == key
        {
            return pair;
        }
        let pair = self.matching_bracket(snapshot, self.selection.head);
        self.brackets = Some((key, pair));
        pair
    }

    /// Cmd/Ctrl+Shift+\: each caret to the bracket matching the one beside it.
    pub(super) fn jump_to_bracket(&mut self, snapshot: &TextSnapshot) {
        let mut carets = self.carets();
        for caret in &mut carets {
            if let Some((_, other)) = self.matching_bracket(snapshot, caret.selection.head) {
                caret.selection = TextSelection {
                    anchor: other,
                    head: other,
                };
                caret.column = None;
                caret.upstream = false;
            }
        }
        self.set_carets(carets, snapshot.revision);
    }
}

/// Where a caret goes after typing.
enum Placed {
    Caret(usize),
    Inside(usize, usize),
}

fn partner(c: char) -> (char, bool) {
    match c {
        '(' => (')', true),
        '[' => (']', true),
        '{' => ('}', true),
        ')' => ('(', false),
        ']' => ('[', false),
        _ => ('{', false),
    }
}

/// The bracket token at `at`'s partner among its siblings.
fn tree_match(tree: &crate::syntax::tree_sitter::Tree, at: usize, c: char) -> Option<usize> {
    let node = tree.root_node().descendant_for_byte_range(at, at + 1)?;
    let mut own = [0u8; 4];
    let own: &str = c.encode_utf8(&mut own);
    if node.start_byte() != at || node.end_byte() != at + 1 || node.kind() != own {
        return None;
    }
    let (other, forward) = partner(c);
    let mut buffer = [0u8; 4];
    let other: &str = other.encode_utf8(&mut buffer);
    let mut depth = 0usize;
    let mut sibling = if forward {
        node.next_sibling()
    } else {
        node.prev_sibling()
    };
    while let Some(current) = sibling {
        if current.kind() == own {
            depth += 1;
        } else if current.kind() == other {
            if depth == 0 {
                return Some(current.start_byte());
            }
            depth -= 1;
        }
        sibling = if forward {
            current.next_sibling()
        } else {
            current.prev_sibling()
        };
    }
    None
}

/// Count brackets outward from `at`, up to `MAX_SCAN` bytes.
fn scan_match(text: &TextBuffer, at: usize, c: char) -> Option<usize> {
    let (other, forward) = partner(c);
    let (own, other) = (c as u8, other as u8);
    let mut depth = 0usize;
    let positions: Box<dyn Iterator<Item = usize>> = if forward {
        Box::new(at + 1..text.len().min(at + 1 + MAX_SCAN))
    } else {
        Box::new((at.saturating_sub(MAX_SCAN)..at).rev())
    };
    for byte in positions {
        match text.byte(byte) {
            Some(b) if b == own => depth += 1,
            Some(b) if b == other => {
                if depth == 0 {
                    return Some(byte);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}
