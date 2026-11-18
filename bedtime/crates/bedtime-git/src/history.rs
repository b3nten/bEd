//! Reading history: the commits reachable from HEAD, what one changed, and a
//! file as a commit has it. GUI-free and read-only: nothing here takes a
//! lock. Call from a worker; `canceled` stops the Git process.
use crate::{Error, ErrorKind, Result, process};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// HEAD itself, when it's on no branch.
    Head,
    /// The branch HEAD is on.
    Current,
    Branch,
    Remote,
    Tag,
}

/// A name pointing at a commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    pub kind: RefKind,
    /// As it's said: `main`, `origin/main`, `v1.0`.
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    /// When it was authored: seconds since the Unix epoch.
    pub time: i64,
    /// The same, as text in this machine's time zone.
    pub date: String,
    pub subject: String,
    pub refs: Vec<Ref>,
}

impl Commit {
    pub fn short_hash(&self) -> &str {
        short(&self.hash)
    }
}

/// The first seven characters of a hash.
pub fn short(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

/// One file a commit changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    /// Where it was, when renamed or copied.
    pub old_path: Option<String>,
    /// Git's letter: A, M, D, R, C, T.
    pub status: char,
    /// Lines added and removed; `None` for a binary file.
    pub lines: Option<(usize, usize)>,
}

/// What a commit says and changed, against its first parent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Detail {
    /// The message after its subject.
    pub body: String,
    pub files: Vec<FileChange>,
}

const FIELD: char = '\x1f';

/// Up to `limit` commits reachable from HEAD, newest first, after the first
/// `skip`. A repository without commits has none.
pub fn log(
    root: &Path,
    skip: usize,
    limit: usize,
    canceled: &Arc<AtomicBool>,
) -> Result<Vec<Commit>> {
    let skip = format!("--skip={skip}");
    let limit = format!("--max-count={limit}");
    let run = process::git(
        root,
        &[
            "log",
            "-z",
            "--no-show-signature",
            "--decorate=full",
            "--date=format-local:%e %b %Y %H:%M",
            "--format=%H%x1f%P%x1f%an%x1f%at%x1f%ad%x1f%D%x1f%s",
            &skip,
            &limit,
            "HEAD",
            "--",
        ],
        None,
        canceled,
    )?;
    if !run.status.success() {
        let stderr = String::from_utf8_lossy(&run.stderr);
        if stderr.contains("not a git repository") {
            return Err(Error::new(
                ErrorKind::NotRepository,
                "Folder is not a Git repository",
            ));
        }
        // No commits yet: HEAD names a branch that doesn't exist.
        if stderr.contains("unknown revision") || stderr.contains("bad revision") {
            return Ok(Vec::new());
        }
        return run.checked().map(|_| Vec::new());
    }
    Ok(parse_log(&String::from_utf8_lossy(&run.stdout)))
}

fn parse_log(text: &str) -> Vec<Commit> {
    text.split('\0')
        .filter_map(|record| {
            let mut fields = record.splitn(7, FIELD);
            let hash = fields.next().filter(|hash| !hash.is_empty())?;
            let parents = fields.next()?;
            let author = fields.next()?;
            let time = fields.next()?;
            let date = fields.next()?;
            let refs = fields.next()?;
            let subject = fields.next()?;
            Some(Commit {
                hash: hash.to_owned(),
                parents: parents.split_whitespace().map(str::to_owned).collect(),
                author: author.to_owned(),
                time: time.parse().unwrap_or(0),
                date: date.split_whitespace().collect::<Vec<_>>().join(" "),
                subject: subject.to_owned(),
                refs: parse_refs(refs),
            })
        })
        .collect()
}

/// `%D` with full names: `HEAD -> refs/heads/main, tag: refs/tags/v1, …`.
fn parse_refs(text: &str) -> Vec<Ref> {
    let mut refs = Vec::new();
    for name in text.split(", ") {
        let (current, name) = match name.strip_prefix("HEAD -> ") {
            Some(name) => (true, name),
            None => (false, name),
        };
        let name = name.strip_prefix("tag: ").unwrap_or(name);
        let (kind, name) = if name == "HEAD" {
            (RefKind::Head, name)
        } else if let Some(name) = name.strip_prefix("refs/heads/") {
            let kind = if current {
                RefKind::Current
            } else {
                RefKind::Branch
            };
            (kind, name)
        } else if let Some(name) = name.strip_prefix("refs/remotes/") {
            // A remote's own HEAD says nothing its branch doesn't.
            if name.ends_with("/HEAD") {
                continue;
            }
            (RefKind::Remote, name)
        } else if let Some(name) = name.strip_prefix("refs/tags/") {
            (RefKind::Tag, name)
        } else {
            // The stash, notes, and whatever else: not part of the story.
            continue;
        };
        refs.push(Ref {
            kind,
            name: name.to_owned(),
        });
    }
    // What you're on first, then branches, remotes and tags.
    refs.sort_by_key(|r| match r.kind {
        RefKind::Head | RefKind::Current => 0,
        RefKind::Branch => 1,
        RefKind::Remote => 2,
        RefKind::Tag => 3,
    });
    refs
}

/// A full or abbreviated object name, and nothing Git could read as an option.
fn object(hash: &str) -> Result<&str> {
    if hash.len() >= 4 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(hash)
    } else {
        Err(Error::new(ErrorKind::InvalidInput, "Not a commit hash"))
    }
}

/// A commit's message body and the files it changed, compared with `parent`
/// (its first; `None` for a commit without one).
pub fn detail(
    root: &Path,
    commit: &str,
    parent: Option<&str>,
    canceled: &Arc<AtomicBool>,
) -> Result<Detail> {
    let commit = object(commit)?;
    let body = process::git(
        root,
        &[
            "show",
            "-s",
            "--no-show-signature",
            "--format=%b",
            commit,
            "--",
        ],
        None,
        canceled,
    )?
    .checked()?;
    let mut args = vec![
        "diff-tree",
        "-r",
        "-M",
        "-z",
        "--raw",
        "--numstat",
        "--no-commit-id",
    ];
    match parent {
        Some(parent) => args.extend([object(parent)?, commit]),
        None => args.extend(["--root", commit]),
    }
    args.push("--");
    let changes = process::git(root, &args, None, canceled)?.checked()?;
    Ok(Detail {
        body: String::from_utf8_lossy(&body.stdout).trim().to_owned(),
        files: parse_changes(&String::from_utf8_lossy(&changes.stdout)),
    })
}

/// `diff-tree -z --raw --numstat`: the raw records (`:modes hashes status`,
/// then one path, or two for a rename), then the counts
/// (`added<TAB>removed<TAB>path`, or with an empty path and then two).
fn parse_changes(text: &str) -> Vec<FileChange> {
    let mut files: Vec<FileChange> = Vec::new();
    let mut tokens = text.split('\0');
    while let Some(token) = tokens.next() {
        if let Some(raw) = token.strip_prefix(':') {
            let status = raw
                .split(' ')
                .nth(4)
                .and_then(|status| status.chars().next())
                .unwrap_or('M');
            let Some(first) = tokens.next() else { break };
            let (path, old_path) = if matches!(status, 'R' | 'C') {
                let Some(path) = tokens.next() else { break };
                (path, Some(first.to_owned()))
            } else {
                (first, None)
            };
            files.push(FileChange {
                path: path.to_owned(),
                old_path,
                status,
                lines: None,
            });
        } else if !token.is_empty() {
            let mut fields = token.splitn(3, '\t');
            let (Some(added), Some(removed), Some(path)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let path = if path.is_empty() {
                // A rename: the old path, then the new.
                tokens.next();
                let Some(path) = tokens.next() else { break };
                path
            } else {
                path
            };
            if let (Ok(added), Ok(removed)) = (added.parse(), removed.parse())
                && let Some(file) = files.iter_mut().find(|file| file.path == path)
            {
                file.lines = Some((added, removed));
            }
        }
    }
    files
}

/// A file's bytes as `commit` has it.
pub fn file_at(
    root: &Path,
    commit: &str,
    path: &str,
    canceled: &Arc<AtomicBool>,
) -> Result<Vec<u8>> {
    let object = format!("{}:{path}", object(commit)?);
    Ok(
        process::git(root, &["cat-file", "blob", &object], None, canceled)?
            .checked()?
            .stdout,
    )
}

/// Where a commit sits among the lines of history, and the lines through
/// its row. Lanes count from the left.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphRow {
    pub lane: usize,
    /// Lanes that only pass by, top to bottom.
    pub through: Vec<usize>,
    /// Lanes that end here, coming down from a child.
    pub above: Vec<usize>,
    /// Lanes that leave from here, down to a parent.
    pub below: Vec<usize>,
}

/// Lays commits (newest first) out in lanes: a commit takes the lane that
/// was waiting for it, hands it on to its first parent, and opens a lane for
/// each further parent nothing is waiting for yet. Returns the rows and how
/// many lanes the widest needs.
pub fn graph(commits: &[Commit]) -> (Vec<GraphRow>, usize) {
    // Each lane holds the commit it's waiting for.
    let mut lanes: Vec<Option<&str>> = Vec::new();
    let mut width = 0;
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        let waiting = |lane: &Option<&str>| *lane == Some(commit.hash.as_str());
        let above: Vec<usize> = (0..lanes.len()).filter(|&i| waiting(&lanes[i])).collect();
        let through: Vec<usize> = (0..lanes.len())
            .filter(|&i| lanes[i].is_some() && !waiting(&lanes[i]))
            .collect();
        for &lane in &above {
            lanes[lane] = None;
        }
        // The lane it came down, else a new line starts in the first free.
        let lane = above.first().copied().unwrap_or_else(|| free(&mut lanes));
        let mut below = Vec::new();
        for (index, parent) in commit.parents.iter().enumerate() {
            let target = match lanes.iter().position(|lane| *lane == Some(parent.as_str())) {
                Some(waiting) => waiting,
                None => {
                    let target = if index == 0 { lane } else { free(&mut lanes) };
                    lanes[target] = Some(parent);
                    target
                }
            };
            if !below.contains(&target) {
                below.push(target);
            }
        }
        // The lane stays taken through this row even when nothing follows.
        width = width.max(lanes.len()).max(lane + 1);
        while lanes.last() == Some(&None) {
            lanes.pop();
        }
        rows.push(GraphRow {
            lane,
            through,
            above,
            below,
        });
    }
    (rows, width)
}

fn free(lanes: &mut Vec<Option<&str>>) -> usize {
    match lanes.iter().position(Option::is_none) {
        Some(lane) => lane,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str, parents: &[&str]) -> Commit {
        Commit {
            hash: hash.into(),
            parents: parents.iter().map(|p| (*p).to_owned()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_line_of_commits_stays_in_one_lane() {
        let (rows, width) = graph(&[commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])]);
        assert_eq!(width, 1);
        assert!(
            rows.iter()
                .all(|row| row.lane == 0 && row.through.is_empty())
        );
        assert_eq!(rows[0].above, Vec::<usize>::new());
        assert_eq!(rows[0].below, vec![0]);
        assert_eq!(rows[1].above, vec![0]);
        assert_eq!(rows[2].below, Vec::<usize>::new());
    }

    #[test]
    fn a_merge_opens_a_lane_that_closes_at_the_fork() {
        // m merges side (s) into main (b); both come from a.
        let (rows, width) = graph(&[
            commit("m", &["b", "s"]),
            commit("b", &["a"]),
            commit("s", &["a"]),
            commit("a", &[]),
        ]);
        assert_eq!(width, 2);
        assert_eq!(rows[0].below, vec![0, 1]);
        assert_eq!((rows[1].lane, rows[1].through.clone()), (0, vec![1]));
        // s joins the lane already waiting for a, and frees its own.
        assert_eq!((rows[2].lane, rows[2].below.clone()), (1, vec![0]));
        assert_eq!(rows[2].through, vec![0]);
        assert_eq!((rows[3].lane, rows[3].above.clone()), (0, vec![0]));
        assert!(rows[3].through.is_empty());
    }

    #[test]
    fn reads_log_records() {
        let text = "abc1234\x1fp1 p2\x1fAda\x1f1700000000\x1f 5 Nov 2026 14:03\x1fHEAD -> refs/heads/main, tag: refs/tags/v1, refs/remotes/origin/main, refs/remotes/origin/HEAD, refs/stash\x1fFix \x1f it\0def\x1f\x1fBob\x1f5\x1fd\x1f\x1froot";
        let commits = parse_log(text);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].parents, ["p1", "p2"]);
        assert_eq!(commits[0].date, "5 Nov 2026 14:03");
        assert_eq!(commits[0].subject, "Fix \x1f it");
        let refs: Vec<_> = commits[0]
            .refs
            .iter()
            .map(|r| (r.kind, r.name.as_str()))
            .collect();
        assert_eq!(
            refs,
            [
                (RefKind::Current, "main"),
                (RefKind::Remote, "origin/main"),
                (RefKind::Tag, "v1"),
            ]
        );
        assert!(commits[1].parents.is_empty() && commits[1].refs.is_empty());
    }

    #[test]
    fn reads_changes_with_renames_and_binaries() {
        let text = ":100644 100644 a b M\0src/a.rs\0:100644 100644 a b R090\0old.rs\0new.rs\0:000000 100644 a b A\0img.png\0\
                    3\t1\tsrc/a.rs\x000\t2\t\0old.rs\0new.rs\0-\t-\timg.png\0";
        let files = parse_changes(text);
        assert_eq!(files.len(), 3);
        assert_eq!((files[0].status, files[0].lines), ('M', Some((3, 1))));
        assert_eq!(files[1].old_path.as_deref(), Some("old.rs"));
        assert_eq!(
            (files[1].path.as_str(), files[1].lines),
            ("new.rs", Some((0, 2)))
        );
        assert_eq!((files[2].status, files[2].lines), ('A', None));
    }

    #[test]
    fn refuses_what_is_not_a_hash() {
        assert!(object("--output=x").is_err());
        assert!(object("abc1234").is_ok());
    }
}
