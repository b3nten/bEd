//! Minimal libgit2 edge translated from ned editor/services/git/git_repo.{h,cpp}.
//! Source attribution and revision: LICENSE, NOTICE, UPSTREAM_REVISION.
use bed_core::editor_state::EditorState;
use git2::{Oid, Repository, Status, StatusOptions, StatusShow};
use std::{collections::BTreeSet, path::Path};

#[derive(Default)]
pub struct GitRepo {
    repo: Option<Repository>,
    head_tree: Option<Oid>,
    head_tree_valid: bool,
}

impl GitRepo {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn close(&mut self) {
        self.head_tree = None;
        self.head_tree_valid = false;
        self.repo = None;
    }
    pub fn open(&mut self, root: &str) -> bool {
        self.close();
        self.repo = Repository::open(root).ok();
        self.is_open()
    }
    pub fn is_open(&self) -> bool {
        self.repo.is_some()
    }
    fn ensure_head_tree(&mut self) -> bool {
        let Some(repo) = &self.repo else {
            return false;
        };
        if self.head_tree_valid {
            return self.head_tree.is_some();
        }
        // Cache failure too: an initially unborn HEAD stays without a baseline
        // until close/open, exactly as the original cached git_tree pointer.
        self.head_tree_valid = true;
        self.head_tree = repo
            .head()
            .ok()
            .and_then(|head| head.target())
            .and_then(|oid| repo.find_commit(oid).ok())
            .and_then(|commit| commit.tree().ok())
            .map(|tree| tree.id());
        self.head_tree.is_some()
    }
    pub fn head_lines(&mut self, relative_path: &str, out: &mut Vec<Vec<u8>>) -> bool {
        out.clear();
        if self.repo.is_none() || relative_path.is_empty() || !self.ensure_head_tree() {
            return true;
        }
        let repo = self.repo.as_ref().unwrap();
        let Ok(tree) = repo.find_tree(self.head_tree.unwrap()) else {
            return false;
        };
        let Ok(entry) = tree.get_path(Path::new(relative_path)) else {
            return true;
        };
        let Ok(blob) = repo.find_blob(entry.id()) else {
            return false;
        };
        // splitLines preserves bytes and trailing empty rows; BOM stripping
        // belongs to document loading and is deliberately not applied to HEAD.
        *out = EditorState::split_lines(blob.content()).0;
        true
    }
    pub fn modified_paths(&self) -> BTreeSet<Vec<u8>> {
        let mut result = BTreeSet::new();
        let Some(repo) = &self.repo else {
            return result;
        };
        let mut options = StatusOptions::new();
        options
            .show(StatusShow::IndexAndWorkdir)
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .exclude_submodules(true);
        let Ok(statuses) = repo.statuses(Some(&mut options)) else {
            return result;
        };
        for entry in &statuses {
            if entry.status() == Status::CURRENT || entry.status() == Status::IGNORED {
                continue;
            }
            let path = entry
                .index_to_workdir()
                .and_then(|delta| {
                    delta
                        .new_file()
                        .path_bytes()
                        .filter(|p| !p.is_empty())
                        .or_else(|| delta.old_file().path_bytes().filter(|p| !p.is_empty()))
                })
                .or_else(|| {
                    entry.head_to_index().and_then(|delta| {
                        delta
                            .new_file()
                            .path_bytes()
                            .filter(|p| !p.is_empty())
                            .or_else(|| delta.old_file().path_bytes().filter(|p| !p.is_empty()))
                    })
                });
            if let Some(path) = path {
                result.insert(path.to_vec());
            }
        }
        result
    }
}
