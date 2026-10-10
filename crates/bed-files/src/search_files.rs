//! Cancellable one-shot project-search discovery; live discovery is incremental.
use crate::file_finder::FileEntry;
use std::{fs, io, path::Path};

/// Git workspaces use libgit2's actual ignore rules, including parent worktrees,
/// nested .gitignore files, negation, and configured excludes. Non-Git folders
/// include all regular files. Git administrative paths are never searched.
pub(super) fn discover(
    root: &Path,
    include_ignored: bool,
    mut canceled: impl FnMut() -> bool,
    mut progress: impl FnMut(usize, usize) -> bool,
) -> io::Result<Option<Vec<FileEntry>>> {
    let root = fs::canonicalize(root)?;
    let repository = if include_ignored {
        None
    } else {
        match git2::Repository::discover(&root) {
            Ok(repo) => Some(repo),
            Err(error) if error.code() == git2::ErrorCode::NotFound => None,
            Err(error) => return Err(io::Error::other(error)),
        }
    };
    let workdir = repository
        .as_ref()
        .and_then(|repo| repo.workdir())
        .map(fs::canonicalize)
        .transpose()?;
    let mut stack = vec![fs::read_dir(&root)?];
    let mut files = Vec::new();
    let mut ignored = 0;
    while let Some(entries) = stack.last_mut() {
        if canceled() {
            return Ok(None);
        }
        let Some(entry) = entries.next() else {
            stack.pop();
            continue;
        };
        let entry = entry?;
        let path = entry.path();
        let private_temporary = entry.file_name().to_str().is_some_and(|name| {
            name.starts_with(".bed-transfer-") || name.starts_with(".bed-save-")
        });
        let excluded = entry.file_name() == ".git"
            || private_temporary
            || match (&repository, &workdir) {
                (Some(repository), Some(workdir)) => {
                    let relative = path.strip_prefix(workdir).map_err(io::Error::other)?;
                    repository
                        .is_path_ignored(relative)
                        .map_err(io::Error::other)?
                }
                _ => false,
            };
        if excluded {
            ignored += 1;
        } else if let Ok(kind) = entry.file_type() {
            if kind.is_dir() {
                stack.push(fs::read_dir(&path)?);
            } else if (kind.is_file() || (kind.is_symlink() && path.is_file()))
                && let Ok(file) = FileEntry::from_path(&path, &root)
            {
                files.push(file);
            }
        }
        if !progress(files.len(), ignored) {
            return Ok(None);
        }
    }
    // Preserve the original path-byte-length rank and traversal-order ties.
    files.sort_by_key(|file| file.relative_path.len());
    Ok(Some(files))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::cell::Cell;

    fn paths(root: &Path, include_ignored: bool) -> Vec<String> {
        let mut paths: Vec<_> = discover(root, include_ignored, || false, |_, _| true)
            .unwrap()
            .unwrap()
            .into_iter()
            .map(|file| file.relative_path)
            .collect();
        paths.sort();
        paths
    }
    #[test]
    fn git_rules_negation_nested_rules_and_parent_worktree_are_respected() {
        let temp = TempDir::new();
        git2::Repository::init(temp.root()).unwrap();
        temp.write(
            ".gitignore",
            b"/build/\nignored/*\n!ignored/keep.rs\n/src/parent-skip\n",
        );
        temp.write("build/artifact", b"needle");
        temp.write("ignored/skipped.rs", b"needle");
        temp.write("ignored/keep.rs", b"needle");
        temp.write("src/.gitignore", b"*.log\n!keep.log\n");
        for name in [
            "src/skip.log",
            "src/keep.log",
            "src/parent-skip",
            ".hidden",
            "target/user.rs",
        ] {
            temp.write(name, b"needle");
        }
        assert_eq!(
            paths(temp.root(), false),
            vec![
                ".gitignore",
                ".hidden",
                "ignored/keep.rs",
                "src/.gitignore",
                "src/keep.log",
                "target/user.rs"
            ]
        );
        assert_eq!(
            paths(&temp.path("src"), false),
            vec![".gitignore", "keep.log"]
        );
        let included = paths(temp.root(), true);
        assert!(included.contains(&"build/artifact".into()));
        assert!(included.contains(&"ignored/skipped.rs".into()));
        assert!(included.contains(&"src/parent-skip".into()));
        assert!(included.iter().all(|path| !path.starts_with(".git/")));
    }
    #[test]
    fn non_git_folders_keep_hidden_files_and_all_user_directory_names() {
        let temp = TempDir::new();
        for name in [
            ".gitignore",
            ".hidden",
            "target/user.rs",
            "vendor/user.rs",
            "reference/user.rs",
        ] {
            temp.write(name, b"target/\nneedle");
        }
        temp.write(".git/metadata", b"needle");
        // A plain .git directory is administrative, even without a valid repository.
        // Discover this fixture before creating .git to avoid a corrupt-repo error.
        std::fs::remove_dir_all(temp.path(".git")).unwrap();
        assert_eq!(paths(temp.root(), false).len(), 5);
        temp.write(".git/metadata", b"needle");
        assert_eq!(paths(temp.root(), true).len(), 5);
    }
    #[test]
    fn discovery_checks_cancellation_between_entries() {
        let temp = TempDir::new();
        for name in ["a", "b", "c"] {
            temp.write(name, b"text");
        }
        let canceled = Cell::new(false);
        let visits = Cell::new(0);
        let result = discover(
            temp.root(),
            false,
            || canceled.get(),
            |_, _| {
                visits.set(visits.get() + 1);
                canceled.set(true);
                true
            },
        )
        .unwrap();
        assert!(result.is_none());
        assert_eq!(visits.get(), 1);
    }
    #[cfg(unix)]
    #[test]
    fn directory_links_are_not_walked_and_file_links_keep_source_relative_paths() {
        use std::os::unix::fs::symlink;
        let temp = TempDir::new();
        let outside = TempDir::new();
        outside.write("external", b"needle");
        temp.write("plain", b"needle");
        symlink(outside.root(), temp.path("directory-link")).unwrap();
        symlink(temp.path("plain"), temp.path("alias")).unwrap();
        assert_eq!(paths(temp.root(), false), vec!["alias", "plain"]);
    }
}
