//! Git-ignore classification for lazy file-tree listings, without GUI state.
use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
};

pub struct TreeIgnore {
    workdir: PathBuf,
    root: PathBuf,
    repository_prefix: PathBuf,
    tracked: HashSet<PathBuf>,
}

impl TreeIgnore {
    pub fn discover(root: &Path) -> io::Result<Option<Self>> {
        let root = std::path::absolute(root)?;
        let canonical_root = std::fs::canonicalize(&root)?;
        let repository = match git2::Repository::discover(&canonical_root) {
            Ok(repository) => repository,
            Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(None),
            Err(error) => return Err(io::Error::other(error)),
        };
        let Some(workdir) = repository.workdir() else {
            return Ok(None);
        };
        let workdir = std::fs::canonicalize(workdir)?;
        let repository_prefix = canonical_root
            .strip_prefix(&workdir)
            .map_err(io::Error::other)?
            .to_owned();
        let mut tracked = HashSet::new();
        for entry in repository.index().map_err(io::Error::other)?.iter() {
            let path = std::str::from_utf8(&entry.path).map_err(io::Error::other)?;
            let path = Path::new(path);
            // Keep directories containing tracked files accessible too.
            for ancestor in path.ancestors().filter(|path| !path.as_os_str().is_empty()) {
                tracked.insert(ancestor.to_owned());
            }
        }
        Ok(Some(Self {
            workdir,
            root,
            repository_prefix,
            tracked,
        }))
    }

    pub fn classify(&self, paths: &[PathBuf]) -> io::Result<Vec<bool>> {
        // Map the root once, preserving lexical child paths and symlink names.
        let relative: Vec<_> = paths
            .iter()
            .map(|path| {
                let path = std::path::absolute(path)?;
                Ok(self
                    .repository_prefix
                    .join(path.strip_prefix(&self.root).map_err(io::Error::other)?))
            })
            .collect::<io::Result<_>>()?;
        let names: Vec<_> = relative
            .iter()
            .map(|path| {
                path.components()
                    .map(|part| part.as_os_str().to_str())
                    .collect::<Option<Vec<_>>>()
                    .map(|parts| parts.join("/"))
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "Git path is not UTF-8")
                    })
            })
            .collect::<io::Result<_>>()?;
        // libgit2 1.9 drops non-wildcard child negations of parent ignore rules.
        // Git supplies exact semantics; the index keeps tracked directories accessible.
        let ignored = bed_remote::git_ignored_paths(&self.workdir, &names)?;
        Ok(names
            .iter()
            .zip(&relative)
            .map(|(name, path)| ignored.contains(name) && !self.tracked.contains(path))
            .collect())
    }
    #[cfg(test)]
    fn is_ignored(&self, path: &Path) -> io::Result<bool> {
        self.classify(&[path.to_owned()]).map(|ignored| ignored[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn nested_rules_tracked_files_parent_worktree_and_excludes() {
        let temp = TempDir::new();
        let repo = git2::Repository::init(temp.root()).unwrap();
        temp.write("src/tracked.log", b"tracked");
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("src/tracked.log")).unwrap();
        index.write().unwrap();
        temp.write(".gitignore", b"*.log\nignored/\n");
        temp.write("src/.gitignore", b"!keep.log\n");
        temp.write(".git/info/exclude", b"excluded\n");
        for name in ["src/drop.log", "src/keep.log", "ignored/file", "excluded"] {
            temp.write(name, b"");
        }
        let rules = TreeIgnore::discover(&temp.path("src")).unwrap().unwrap();
        assert!(rules.is_ignored(&temp.path("src/drop.log")).unwrap());
        assert!(!rules.is_ignored(&temp.path("src/keep.log")).unwrap());
        let rules = TreeIgnore::discover(temp.root()).unwrap().unwrap();
        for name in ["src/drop.log", "ignored", "excluded"] {
            assert!(rules.is_ignored(&temp.path(name)).unwrap(), "{name}");
        }
        for name in ["src", "src/tracked.log", "src/keep.log"] {
            assert!(!rules.is_ignored(&temp.path(name)).unwrap(), "{name}");
        }
        // Ignoring a directory must not conceal its tracked descendants.
        temp.write(".gitignore", b"src/\n");
        let rules = TreeIgnore::discover(temp.root()).unwrap().unwrap();
        assert!(!rules.is_ignored(&temp.path("src")).unwrap());
        assert!(!rules.is_ignored(&temp.path("src/tracked.log")).unwrap());
    }

    #[test]
    fn non_git_folder_has_no_rules() {
        let temp = TempDir::new();
        temp.write(".gitignore", b"*\n");
        assert!(TreeIgnore::discover(temp.root()).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_project_root_keeps_repository_relative_ignore_paths() {
        let temp = TempDir::new();
        let aliases = TempDir::new();
        git2::Repository::init(temp.root()).unwrap();
        temp.write(".gitignore", b"*.log\n");
        temp.write("src/drop.log", b"");
        let alias = aliases.path("project");
        std::os::unix::fs::symlink(temp.path("src"), &alias).unwrap();
        let rules = TreeIgnore::discover(&alias).unwrap().unwrap();
        assert!(rules.is_ignored(&alias.join("drop.log")).unwrap());
    }
}
