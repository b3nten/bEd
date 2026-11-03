//! Searching a folder's files for a query, and the replacements for what it
//! finds. Plain Rust: the panel runs it on a worker.
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::{Regex, RegexBuilder};
use std::{
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

/// What to look for, and where.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "workbench", derive(bevy::prelude::Reflect))]
pub struct SearchQuery {
    pub text: String,
    pub match_case: bool,
    pub whole_word: bool,
    /// `text` is a regular expression (Rust's `regex` syntax); replacements
    /// can use its groups (`$1`, `${name}`).
    pub regex: bool,
    /// Only files matching these globs (comma separated), if any.
    pub include: String,
    /// Not files matching these globs (comma separated).
    pub exclude: String,
    /// Also files Git ignores.
    pub ignored: bool,
}

/// A query made ready to search with.
#[derive(Clone, Debug)]
pub struct Matcher {
    regex: Regex,
    literal: bool,
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
    ignored: bool,
}

impl SearchQuery {
    /// `None` for an empty query; an error for a pattern or glob that doesn't
    /// parse.
    pub fn matcher(&self) -> Option<Result<Matcher, String>> {
        if self.text.is_empty() {
            return None;
        }
        Some(self.build())
    }

    fn build(&self) -> Result<Matcher, String> {
        let pattern = if self.regex {
            self.text.clone()
        } else {
            regex::escape(&self.text)
        };
        let pattern = if self.whole_word {
            format!(r"\b(?:{pattern})\b")
        } else {
            pattern
        };
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(!self.match_case)
            .multi_line(true)
            .size_limit(16 * 1024 * 1024)
            .build()
            .map_err(|error| match error {
                regex::Error::Syntax(text) => text,
                other => other.to_string(),
            })?;
        Ok(Matcher {
            regex,
            literal: !self.regex,
            include: globs(&self.include)?,
            exclude: globs(&self.exclude)?,
            ignored: self.ignored,
        })
    }
}

/// Comma-separated globs; one without a `/` matches a name at any depth.
fn globs(text: &str) -> Result<Option<GlobSet>, String> {
    let mut builder = GlobSetBuilder::new();
    let mut any = false;
    for glob in text.split(',').map(str::trim).filter(|g| !g.is_empty()) {
        let glob = if glob.contains('/') {
            glob.trim_start_matches('/').to_owned()
        } else {
            format!("**/{glob}")
        };
        // A folder matches what's in it.
        for pattern in [glob.clone(), format!("{}/**", glob.trim_end_matches('/'))] {
            builder.add(Glob::new(&pattern).map_err(|e| e.to_string())?);
        }
        any = true;
    }
    if !any {
        return Ok(None);
    }
    builder.build().map(Some).map_err(|e| e.to_string())
}

/// One match in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// Bytes in the file's text.
    pub range: Range<usize>,
    /// From 0; columns count characters.
    pub start: (u32, u32),
    pub end: (u32, u32),
    /// The match's first line, shortened around it; `preview_range` is the
    /// match in it.
    pub preview: String,
    pub preview_range: Range<usize>,
}

/// A file's matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMatches {
    pub path: PathBuf,
    pub matches: Vec<Match>,
}

/// How a search went.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub files: usize,
    pub matches: usize,
    /// Stopped at `MAX_MATCHES`.
    pub limited: bool,
    pub cancelled: bool,
}

/// A search stops after this many matches.
pub const MAX_MATCHES: usize = 20_000;
/// Files larger than this aren't searched.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Longest preview line, in characters either side of the match.
const PREVIEW_CONTEXT: usize = 80;

impl Matcher {
    /// Whether the file at `relative` (to the folder searched) is searched.
    pub fn searches(&self, relative: &Path) -> bool {
        self.include
            .as_ref()
            .is_none_or(|set| set.is_match(relative))
            && !self
                .exclude
                .as_ref()
                .is_some_and(|set| set.is_match(relative))
    }

    /// The matches in `text`, at most `limit`.
    pub fn find(&self, text: &str, limit: usize) -> Vec<Match> {
        let mut lines = LineIndex::new(text);
        self.regex
            .find_iter(text)
            .filter(|found| !found.range().is_empty())
            .take(limit)
            .map(|found| {
                let range = found.range();
                let start = lines.position(range.start);
                let end = lines.position(range.end);
                let (preview, preview_range) = preview(text, range.clone());
                Match {
                    range,
                    start,
                    end,
                    preview,
                    preview_range,
                }
            })
            .collect()
    }

    /// What replaces the match at `range` in `text`: `replacement`, with a
    /// regular expression's groups filled in. `None` if it no longer matches
    /// there.
    pub fn replacement(
        &self,
        text: &str,
        range: Range<usize>,
        replacement: &str,
    ) -> Option<String> {
        let captures = self.regex.captures_at(text, range.start)?;
        let found = captures.get(0)?;
        if found.range() != range {
            return None;
        }
        if self.literal {
            return Some(replacement.to_owned());
        }
        let mut out = String::new();
        captures.expand(replacement, &mut out);
        Some(out)
    }

    /// Every match in `text` replaced, last first: `(range, replacement)`.
    pub fn replacements(&self, text: &str, replacement: &str) -> Vec<(Range<usize>, String)> {
        let mut edits: Vec<_> = self
            .find(text, usize::MAX)
            .into_iter()
            .filter_map(|m| {
                let with = self.replacement(text, m.range.clone(), replacement)?;
                Some((m.range, with))
            })
            .collect();
        edits.reverse();
        edits
    }

    /// Search the files under `root` (Git's ignore files apply unless the
    /// query asks for ignored files too; `.git` never). A file in `open` is
    /// searched as that text, not as on disk. `found` is called with each
    /// file that matches, in path order.
    pub fn search(
        &self,
        root: &Path,
        open: &HashMap<PathBuf, Arc<str>>,
        cancelled: &dyn Fn() -> bool,
        found: &mut dyn FnMut(FileMatches),
    ) -> Summary {
        let mut summary = Summary::default();
        let walk = ignore::WalkBuilder::new(root)
            .hidden(false)
            .git_ignore(!self.ignored)
            .git_exclude(!self.ignored)
            .git_global(!self.ignored)
            .ignore(!self.ignored)
            .parents(true)
            .require_git(true)
            .sort_by_file_name(|a, b| a.cmp(b))
            .filter_entry(|entry| entry.file_name() != ".git")
            .build();
        for entry in walk.filter_map(Result::ok) {
            if cancelled() {
                summary.cancelled = true;
                break;
            }
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.into_path();
            let relative = path.strip_prefix(root).unwrap_or(&path);
            if !self.searches(relative) {
                continue;
            }
            let text: Arc<str> = match open.get(&path) {
                Some(text) => text.clone(),
                None => match read_text(&path) {
                    Some(text) => text.into(),
                    None => continue,
                },
            };
            let room = MAX_MATCHES - summary.matches;
            let matches = self.find(&text, room);
            if matches.is_empty() {
                continue;
            }
            summary.files += 1;
            summary.matches += matches.len();
            found(FileMatches { path, matches });
            if summary.matches >= MAX_MATCHES {
                summary.limited = true;
                break;
            }
        }
        summary
    }
}

/// A file's text, if it's text: not too large, no NUL in its start, UTF-8.
pub fn read_text(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Turns byte offsets, asked in order, into lines and columns without
/// rescanning from the start each time.
struct LineIndex<'a> {
    text: &'a str,
    /// The last offset asked for, its line and that line's start.
    at: usize,
    line: u32,
    line_start: usize,
}

impl<'a> LineIndex<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            at: 0,
            line: 0,
            line_start: 0,
        }
    }

    /// The line and column (in characters) of `byte`.
    fn position(&mut self, byte: usize) -> (u32, u32) {
        if byte < self.at {
            *self = Self::new(self.text);
        }
        for (offset, _) in self.text[self.at..byte].match_indices('\n') {
            self.line += 1;
            self.line_start = self.at + offset + 1;
        }
        self.at = byte;
        let column = self.text[self.line_start..byte].chars().count() as u32;
        (self.line, column)
    }
}

/// The line `range` starts on, around it, and where the match is in that.
fn preview(text: &str, range: Range<usize>) -> (String, Range<usize>) {
    let line_start = text[..range.start].rfind('\n').map_or(0, |at| at + 1);
    let line_end = text[range.start..]
        .find('\n')
        .map_or(text.len(), |at| range.start + at);
    let end = range.end.min(line_end);
    // At most PREVIEW_CONTEXT characters either side.
    let before_start = text[line_start..range.start]
        .char_indices()
        .rev()
        .nth(PREVIEW_CONTEXT)
        .map_or(line_start, |(at, _)| line_start + at);
    let after_end = text[end..line_end]
        .char_indices()
        .nth(PREVIEW_CONTEXT)
        .map_or(line_end, |(at, _)| end + at);
    // Leading indentation isn't worth the room.
    let shown_start = before_start + text[before_start..range.start].len()
        - text[before_start..range.start].trim_start().len();
    let preview = text[shown_start..after_end]
        .trim_end_matches('\r')
        .to_owned();
    let preview_range = range.start - shown_start..(end - shown_start).min(preview.len());
    (preview, preview_range)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(text: &str) -> SearchQuery {
        SearchQuery {
            text: text.into(),
            ..Default::default()
        }
    }

    fn found(query: &SearchQuery, text: &str) -> Vec<String> {
        let matcher = query.matcher().unwrap().unwrap();
        matcher
            .find(text, usize::MAX)
            .into_iter()
            .map(|m| text[m.range].to_owned())
            .collect()
    }

    #[test]
    fn case_whole_words_and_regular_expressions() {
        let text = "Cat cat concat cat.dog c.t";
        assert_eq!(found(&query("cat"), text), ["Cat", "cat", "cat", "cat"]);
        let case = SearchQuery {
            match_case: true,
            ..query("Cat")
        };
        assert_eq!(found(&case, text), ["Cat"]);
        let word = SearchQuery {
            whole_word: true,
            ..query("cat")
        };
        assert_eq!(found(&word, text), ["Cat", "cat", "cat"]);
        // Literal unless asked: the dot is a dot.
        assert_eq!(found(&query("c.t"), text), ["c.t"]);
        let regex = SearchQuery {
            regex: true,
            match_case: true,
            ..query("c.t")
        };
        assert_eq!(found(&regex, text), ["cat", "cat", "cat", "c.t"]);
        assert!(query("").matcher().is_none());
        let bad = SearchQuery {
            regex: true,
            ..query("(")
        };
        assert!(bad.matcher().unwrap().is_err());
    }

    #[test]
    fn matches_know_their_lines_columns_and_preview() {
        let text = "one\n    let café = 1;\nlast café\n";
        let matcher = query("café").matcher().unwrap().unwrap();
        let matches = matcher.find(text, usize::MAX);
        assert_eq!(matches.len(), 2);
        assert_eq!((matches[0].start, matches[0].end), ((1, 8), (1, 12)));
        assert_eq!(matches[0].preview, "let café = 1;");
        assert_eq!(
            &matches[0].preview[matches[0].preview_range.clone()],
            "café"
        );
        assert_eq!((matches[1].start, matches[1].end), ((2, 5), (2, 9)));
        // A match over lines ends on the next.
        let multi = SearchQuery {
            regex: true,
            ..query(r"one\n")
        };
        let m = &multi.matcher().unwrap().unwrap().find(text, 1)[0];
        assert_eq!((m.start, m.end), ((0, 0), (1, 0)));
        assert_eq!(&m.preview[m.preview_range.clone()], "one");
    }

    #[test]
    fn replacements_fill_in_groups_only_for_regular_expressions() {
        let text = "fn alpha() {} fn beta() {}";
        let regex = SearchQuery {
            regex: true,
            ..query(r"fn (\w+)")
        };
        let matcher = regex.matcher().unwrap().unwrap();
        assert_eq!(
            matcher.replacements(text, "fn new_$1"),
            [
                (14..21, "fn new_beta".to_owned()),
                (0..8, "fn new_alpha".to_owned())
            ]
        );
        let literal = query("alpha").matcher().unwrap().unwrap();
        assert_eq!(literal.replacement(text, 3..8, "$1"), Some("$1".into()));
        // Not where it was: refused.
        assert_eq!(literal.replacement(text, 4..9, "x"), None);
    }

    #[test]
    fn folders_are_searched_in_order_with_filters_ignores_and_open_text() {
        let root = std::env::temp_dir().join(format!("bedtime-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(root.join("src/a.rs"), "needle\n").unwrap();
        std::fs::write(root.join("src/deep/b.rs"), "needle needle\n").unwrap();
        std::fs::write(root.join("src/c.txt"), "needle\n").unwrap();
        std::fs::write(root.join("target/out.rs"), "needle\n").unwrap();
        std::fs::write(root.join("binary.bin"), b"needle\0").unwrap();
        let search = |query: SearchQuery, open: HashMap<PathBuf, Arc<str>>| {
            let mut files = Vec::new();
            let summary =
                query
                    .matcher()
                    .unwrap()
                    .unwrap()
                    .search(&root, &open, &|| false, &mut |f| {
                        files.push((
                            f.path
                                .strip_prefix(&root)
                                .unwrap()
                                .to_string_lossy()
                                .into_owned(),
                            f.matches.len(),
                        ))
                    });
            (files, summary)
        };
        let (files, summary) = search(query("needle"), HashMap::new());
        assert_eq!(
            files,
            [
                ("src/a.rs".into(), 1),
                ("src/c.txt".into(), 1),
                ("src/deep/b.rs".into(), 2)
            ]
        );
        assert_eq!((summary.files, summary.matches), (3, 4));

        let rust = SearchQuery {
            include: "*.rs".into(),
            exclude: "deep".into(),
            ..query("needle")
        };
        assert_eq!(search(rust, HashMap::new()).0, [("src/a.rs".to_owned(), 1)]);

        let ignored = SearchQuery {
            ignored: true,
            ..query("needle")
        };
        assert!(
            search(ignored, HashMap::new())
                .0
                .contains(&("target/out.rs".to_owned(), 1))
        );

        // An open document's text, not the file's.
        let open = HashMap::from([(root.join("src/a.rs"), Arc::from("needle needle needle"))]);
        assert_eq!(
            search(query("needle"), open).0[0],
            ("src/a.rs".to_owned(), 3)
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
