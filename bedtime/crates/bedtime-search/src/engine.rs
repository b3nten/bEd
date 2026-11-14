//! Searching a folder's files for a query, and the replacements for what it
//! finds. Plain Rust: the panel runs it on a worker.
#[cfg(feature = "workbench")]
use bevy::prelude::ReflectDefault;
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::{Regex, RegexBuilder};
use std::{
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// What to look for, and where.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(
    feature = "workbench",
    derive(bevy::prelude::Reflect),
    reflect(Default)
)]
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
    /// Only the files that are open, wherever they are.
    pub open_only: bool,
}

/// The files open in the app, each with its unsaved text if it has any: that
/// is searched, not what's on disk.
pub type OpenFiles = HashMap<PathBuf, Option<Arc<str>>>;

/// A query made ready to search with.
#[derive(Clone, Debug)]
pub struct Matcher {
    regex: Regex,
    literal: bool,
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
    ignored: bool,
    open_only: bool,
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
            open_only: self.open_only,
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
    /// Files that weren't searched: over `MAX_FILE_BYTES`, or not UTF-8.
    /// (Binary files aren't counted.)
    pub skipped: usize,
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

    /// The matches found earlier at `ranges` (in order) replaced, last first.
    /// `None` if any of them no longer matches there: `text` has changed.
    pub fn replacements_at(
        &self,
        text: &str,
        ranges: impl IntoIterator<Item = Range<usize>>,
        replacement: &str,
    ) -> Option<Vec<(Range<usize>, String)>> {
        let mut edits = ranges
            .into_iter()
            .map(|range| {
                let with = self.replacement(text, range.clone(), replacement)?;
                Some((range, with))
            })
            .collect::<Option<Vec<_>>>()?;
        edits.reverse();
        Some(edits)
    }

    /// What `found` would be replaced with, worked out from its preview: for
    /// showing, not for replacing. `None` when the preview isn't enough to
    /// tell (a match over several lines, or cut short).
    pub fn preview_replacement(&self, found: &Match, replacement: &str) -> Option<String> {
        if self.literal {
            return Some(replacement.to_owned());
        }
        if found.start.0 != found.end.0 {
            return None;
        }
        let captures = self
            .regex
            .captures_at(&found.preview, found.preview_range.start)?;
        if captures.get(0)?.range() != found.preview_range {
            return None;
        }
        let mut out = String::new();
        captures.expand(replacement, &mut out);
        Some(out)
    }

    /// Search the files under `root` (Git's ignore files apply unless the
    /// query asks for ignored files too; `.git` never), or only the `open`
    /// files if the query asks for that. An open file with unsaved text is
    /// searched as that text, not as on disk. `found` is called with each
    /// file that matches, from several threads and in no particular order.
    pub fn search(
        &self,
        root: &Path,
        open: &OpenFiles,
        cancelled: &(dyn Fn() -> bool + Sync),
        found: &(dyn Fn(FileMatches) + Sync),
    ) -> Summary {
        let summary = Mutex::new(Summary::default());
        // Search one file. Returns whether to go on.
        let visit = |path: PathBuf| -> bool {
            if cancelled() {
                summary.lock().unwrap().cancelled = true;
                return false;
            }
            if summary.lock().unwrap().limited {
                return false;
            }
            if !self.searches(path.strip_prefix(root).unwrap_or(&path)) {
                return true;
            }
            let text: Arc<str> = match open.get(&path) {
                Some(Some(text)) => text.clone(),
                _ => match read(&path) {
                    Ok(text) => text.into(),
                    Err(Unread::Skipped) => {
                        summary.lock().unwrap().skipped += 1;
                        return true;
                    }
                    Err(Unread::Binary | Unread::Missing) => return true,
                },
            };
            let mut matches = self.find(&text, MAX_MATCHES);
            if matches.is_empty() {
                return true;
            }
            let mut summary = summary.lock().unwrap();
            let room = MAX_MATCHES - summary.matches;
            if matches.len() >= room {
                matches.truncate(room);
                summary.limited = true;
            }
            if !matches.is_empty() {
                summary.files += 1;
                summary.matches += matches.len();
                // Under the lock, so nothing is handed over past the limit.
                found(FileMatches { path, matches });
            }
            !summary.limited
        };
        if self.open_only {
            let mut paths: Vec<&PathBuf> = open.keys().collect();
            paths.sort();
            for path in paths {
                if !visit(path.clone()) {
                    break;
                }
            }
        } else {
            ignore::WalkBuilder::new(root)
                .hidden(false)
                .git_ignore(!self.ignored)
                .git_exclude(!self.ignored)
                .git_global(!self.ignored)
                .ignore(!self.ignored)
                .parents(true)
                .require_git(true)
                .filter_entry(|entry| entry.file_name() != ".git")
                .build_parallel()
                .run(|| {
                    Box::new(|entry| {
                        let Ok(entry) = entry else {
                            return ignore::WalkState::Continue;
                        };
                        if !entry.file_type().is_some_and(|t| t.is_file()) {
                            // Still asked between folders, so an empty tree stops too.
                            return if cancelled() {
                                summary.lock().unwrap().cancelled = true;
                                ignore::WalkState::Quit
                            } else {
                                ignore::WalkState::Continue
                            };
                        }
                        if visit(entry.into_path()) {
                            ignore::WalkState::Continue
                        } else {
                            ignore::WalkState::Quit
                        }
                    })
                });
        }
        summary.into_inner().unwrap()
    }
}

/// Why a file wasn't read as text.
enum Unread {
    Missing,
    /// A NUL in its start.
    Binary,
    /// Too large, or not UTF-8.
    Skipped,
}

fn read(path: &Path) -> Result<String, Unread> {
    let metadata = std::fs::metadata(path).map_err(|_| Unread::Missing)?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(Unread::Skipped);
    }
    let bytes = std::fs::read(path).map_err(|_| Unread::Missing)?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Err(Unread::Binary);
    }
    String::from_utf8(bytes).map_err(|_| Unread::Skipped)
}

/// A file's text, if it's text: not too large, no NUL in its start, UTF-8.
pub fn read_text(path: &Path) -> Option<String> {
    read(path).ok()
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
    fn listed_matches_are_replaced_only_while_they_still_match() {
        let text = "alpha beta alpha";
        let matcher = query("alpha").matcher().unwrap().unwrap();
        // Only the second: the first was dismissed.
        assert_eq!(
            matcher.replacements_at(text, std::iter::once(11..16), "x"),
            Some(vec![(11..16, "x".to_owned())])
        );
        assert_eq!(
            matcher.replacements_at(text, [0..5, 11..16], "x"),
            Some(vec![(11..16, "x".to_owned()), (0..5, "x".to_owned())])
        );
        // The text changed under one of them: none are replaced.
        assert_eq!(
            matcher.replacements_at("alpha beta gamma", [0..5, 11..16], "x"),
            None
        );
    }

    #[test]
    fn a_replacement_is_previewed_from_the_match_line() {
        let text = "    fn alpha() {}\nfn\nbeta() {}\n";
        let regex = SearchQuery {
            regex: true,
            ..query(r"fn\s+(\w+)")
        };
        let matcher = regex.matcher().unwrap().unwrap();
        let matches = matcher.find(text, usize::MAX);
        assert_eq!(
            matcher.preview_replacement(&matches[0], "fn new_$1"),
            Some("fn new_alpha".into())
        );
        // Over two lines: the preview has only the first.
        assert_eq!(matcher.preview_replacement(&matches[1], "fn new_$1"), None);
        let literal = query("alpha").matcher().unwrap().unwrap();
        let found = &literal.find(text, 1)[0];
        assert_eq!(literal.preview_replacement(found, "$1"), Some("$1".into()));
    }

    #[test]
    fn folders_are_searched_with_filters_ignores_and_open_text() {
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
        std::fs::write(root.join("latin1.txt"), b"needle \xe9\n").unwrap();
        let search = |query: SearchQuery, open: OpenFiles| {
            let files = Mutex::new(Vec::new());
            let summary = query
                .matcher()
                .unwrap()
                .unwrap()
                .search(&root, &open, &|| false, &|f| {
                    files.lock().unwrap().push((
                        f.path
                            .strip_prefix(&root)
                            .unwrap()
                            .to_string_lossy()
                            .into_owned(),
                        f.matches.len(),
                    ))
                });
            // Found in any order.
            let mut files = files.into_inner().unwrap();
            files.sort();
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
        // Not UTF-8 is counted as skipped; binary isn't.
        assert_eq!(summary.skipped, 1);

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

        // An open document's unsaved text, not the file's; one without any is
        // read from disk.
        let open = HashMap::from([
            (
                root.join("src/a.rs"),
                Some(Arc::from("needle needle needle")),
            ),
            (root.join("src/c.txt"), None),
        ]);
        assert_eq!(
            search(query("needle"), open.clone()).0,
            [
                ("src/a.rs".to_owned(), 3),
                ("src/c.txt".to_owned(), 1),
                ("src/deep/b.rs".to_owned(), 2)
            ]
        );
        // Only the open files, filters still applying.
        let only = SearchQuery {
            open_only: true,
            ..query("needle")
        };
        assert_eq!(
            search(only.clone(), open.clone()).0,
            [("src/a.rs".to_owned(), 3), ("src/c.txt".to_owned(), 1)]
        );
        let only_rust = SearchQuery {
            include: "*.rs".into(),
            ..only
        };
        assert_eq!(search(only_rust, open).0, [("src/a.rs".to_owned(), 3)]);

        // A search that's cancelled says so.
        let cancelled = query("needle").matcher().unwrap().unwrap().search(
            &root,
            &HashMap::new(),
            &|| true,
            &|_| {},
        );
        assert!(cancelled.cancelled);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_search_stops_at_the_limit() {
        let root =
            std::env::temp_dir().join(format!("bedtime-search-limit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let line = "x ".repeat(MAX_MATCHES / 2 + 10);
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(root.join(name), &line).unwrap();
        }
        let handed = Mutex::new(0);
        let summary = query("x").matcher().unwrap().unwrap().search(
            &root,
            &HashMap::new(),
            &|| false,
            &|f| *handed.lock().unwrap() += f.matches.len(),
        );
        assert!(summary.limited);
        assert_eq!(summary.matches, MAX_MATCHES);
        assert_eq!(*handed.lock().unwrap(), MAX_MATCHES);
        let _ = std::fs::remove_dir_all(&root);
    }
}
