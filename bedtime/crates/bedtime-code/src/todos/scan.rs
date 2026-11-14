//! Finding TODO comments: in one text (`find`), and in a folder's files
//! (`scan`). Plain Rust: the panel runs it on a worker.
use regex::Regex;
use std::{
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

/// The tags looked for, in the order Sort by Tag puts them.
pub const TAGS: [&str; 5] = ["FIXME", "BUG", "TODO", "HACK", "XXX"];

/// A scan stops after this many items.
pub const MAX_ITEMS: usize = 10_000;
/// Files larger than this aren't read.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Where comments start: one of these before a tag on its line makes it a
/// comment. A line starting with `*` is inside a block comment.
const OPENERS: [&str; 7] = ["//", "/*", "#", "--", ";", "<!--", "%"];

static TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(FIXME|BUG|TODO|HACK|XXX)\b(?:\(([^)\n]*)\))?:?").expect("the tag pattern")
});

/// A tag found in a text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// From 0; columns count characters.
    pub line: u32,
    pub column: u32,
    pub tag: &'static str,
    /// `TODO(name)`: the name.
    pub author: Option<String>,
    /// What follows the tag, without comment closers.
    pub text: String,
}

/// A tag found in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TodoItem {
    pub path: PathBuf,
    /// `path` under the scanned folder, with `/` between names.
    pub relative: String,
    pub found: Found,
}

/// What a scan found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    pub root: PathBuf,
    /// By `relative`, then line.
    pub items: Vec<TodoItem>,
    /// Stopped at `MAX_ITEMS`.
    pub limited: bool,
    pub cancelled: bool,
}

/// The tags in `text`'s comments: at most one per line, the first.
pub fn find(text: &str) -> Vec<Found> {
    let mut found = Vec::new();
    for (number, line) in text.lines().enumerate() {
        // Cheap test first: most lines have no tag.
        if !TAGS.iter().any(|tag| line.contains(tag)) {
            continue;
        }
        for captures in TAG.captures_iter(line) {
            let whole = captures.get(0).unwrap();
            if !in_comment(&line[..whole.start()]) {
                continue;
            }
            let tag = TAGS
                .into_iter()
                .find(|tag| *tag == &captures[1])
                .unwrap_or("TODO");
            let author = captures
                .get(2)
                .map(|m| m.as_str().trim().to_owned())
                .filter(|a| !a.is_empty());
            found.push(Found {
                line: number as u32,
                column: line[..whole.start()].chars().count() as u32,
                tag,
                author,
                text: tidy(&line[whole.end()..]),
            });
            break;
        }
    }
    found
}

fn in_comment(before: &str) -> bool {
    before.trim_start().starts_with('*') || OPENERS.iter().any(|opener| before.contains(opener))
}

/// The text after a tag: no leading punctuation, no comment closer.
fn tidy(rest: &str) -> String {
    let mut text = rest.trim_start_matches([':', '-', ' ', '\t']).trim_end();
    for closer in ["*/", "-->"] {
        text = text.strip_suffix(closer).unwrap_or(text).trim_end();
    }
    text.to_owned()
}

/// The tags in the files under `root`, as Search reads them: Git's ignore
/// files apply, `.git` is skipped, binary and very large files aren't read.
pub fn scan(root: &Path, cancelled: &(dyn Fn() -> bool + Sync)) -> Scan {
    let items = Mutex::new(Vec::new());
    let stop = Mutex::new((false, false)); // (limited, cancelled)
    ignore::WalkBuilder::new(root)
        .hidden(false)
        .parents(true)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build_parallel()
        .run(|| {
            Box::new(|entry| {
                if cancelled() {
                    stop.lock().unwrap().1 = true;
                    return ignore::WalkState::Quit;
                }
                let Ok(entry) = entry else {
                    return ignore::WalkState::Continue;
                };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return ignore::WalkState::Continue;
                }
                let path = entry.into_path();
                let Some(text) = read_text(&path) else {
                    return ignore::WalkState::Continue;
                };
                let found = find(&text);
                if found.is_empty() {
                    return ignore::WalkState::Continue;
                }
                let relative = relative(root, &path);
                let mut items = items.lock().unwrap();
                for found in found {
                    if items.len() >= MAX_ITEMS {
                        stop.lock().unwrap().0 = true;
                        return ignore::WalkState::Quit;
                    }
                    items.push(TodoItem {
                        path: path.clone(),
                        relative: relative.clone(),
                        found,
                    });
                }
                ignore::WalkState::Continue
            })
        });
    let mut items = items.into_inner().unwrap();
    items.sort_by(|a, b| (&a.relative, a.found.line).cmp(&(&b.relative, b.found.line)));
    let (limited, cancelled) = stop.into_inner().unwrap();
    Scan {
        root: root.to_owned(),
        items,
        limited,
        cancelled,
    }
}

fn relative(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// A file's text, if it's text: not too large, no NUL in its start, UTF-8.
fn read_text(path: &Path) -> Option<String> {
    if std::fs::metadata(path).ok()?.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}
