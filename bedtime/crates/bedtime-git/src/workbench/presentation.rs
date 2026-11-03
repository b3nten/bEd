//! Immutable rows over a status snapshot. Preparation happens on a worker;
//! folders reference ranges in one shared file index rather than copying paths.
use super::{Staging, staging};
use crate::Status;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::Range,
    sync::Arc,
};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, bevy::prelude::Reflect,
)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    List,
    #[default]
    Tree,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, bevy::prelude::Reflect,
)]
#[serde(rename_all = "snake_case")]
pub enum GroupBy {
    None,
    #[default]
    TrackedUntracked,
    StagedUnstaged,
}
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, bevy::prelude::Reflect,
)]
#[serde(default, deny_unknown_fields)]
pub struct GitPanelOptions {
    pub view: ViewMode,
    pub group_by: GroupBy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Group {
    Conflicts,
    Tracked,
    Untracked,
    Staged,
    Unstaged,
}
impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Self::Conflicts => "Conflicts",
            Self::Tracked => "Tracked",
            Self::Untracked => "Untracked",
            Self::Staged => "Staged",
            Self::Unstaged => "Unstaged",
        }
    }
    pub fn contains(self, entry: &crate::StatusEntry) -> bool {
        if self == Self::Conflicts {
            return entry.conflicted;
        }
        if entry.conflicted {
            return false;
        }
        match self {
            Self::Tracked => !matches!(entry.index_status, '?' | 'A'),
            Self::Untracked => matches!(entry.index_status, '?' | 'A'),
            Self::Staged => !matches!(entry.index_status, ' ' | '.' | '?'),
            Self::Unstaged => {
                !matches!(entry.worktree_status, ' ' | '.') || entry.index_status == '?'
            }
            Self::Conflicts => unreachable!(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct DirectoryKey {
    pub group: Option<Group>,
    pub path: String,
}
#[derive(Clone)]
pub(super) struct ChangeSet {
    indices: Arc<[usize]>,
    range: Range<usize>,
}
impl ChangeSet {
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.indices[self.range.clone()].iter().copied()
    }
    pub fn len(&self) -> usize {
        self.range.len()
    }
    fn staging(&self, status: &Status, group: Option<Group>) -> Staging {
        if group == Some(Group::Staged) {
            return Staging::Staged;
        }
        if group == Some(Group::Unstaged) {
            return Staging::Unstaged;
        }
        let mut any = false;
        let mut all = true;
        for index in self.iter() {
            let state = staging(&status.entries[index]);
            any |= state != Staging::Unstaged;
            all &= state == Staging::Staged;
        }
        if all {
            Staging::Staged
        } else if any {
            Staging::Partial
        } else {
            Staging::Unstaged
        }
    }
}
#[derive(Clone)]
pub(super) enum Row {
    Header(Group, ChangeSet, Staging),
    Directory {
        key: DirectoryKey,
        depth: usize,
        changes: ChangeSet,
        staged: Staging,
        expanded: bool,
        conflicted: bool,
    },
    File {
        index: usize,
        group: Option<Group>,
        depth: usize,
    },
}
impl Row {
    pub fn key(&self, status: &Status) -> String {
        match self {
            Self::Header(group, ..) => format!("group:{}", group.label()),
            Self::Directory { key, .. } => format!("directory:{:?}:{}", key.group, key.path),
            Self::File { index, group, .. } => {
                format!("file:{group:?}:{}", status.entries[*index].path)
            }
        }
    }
}
pub(super) struct RepositoryView {
    pub status: Arc<Status>,
    pub options: GitPanelOptions,
    pub collapsed: BTreeSet<DirectoryKey>,
    pub rows: Vec<Row>,
    pub row_index: HashMap<String, usize>,
    pub staged_count: usize,
    /// A message summing up the staged files, when there are one to three.
    pub suggested_message: Option<String>,
    pub has_conflicts: bool,
}
impl RepositoryView {
    #[cfg(test)]
    pub fn new(status: Status) -> Self {
        Self::prepare(
            Arc::new(status),
            GitPanelOptions::default(),
            BTreeSet::new(),
        )
    }
    pub fn prepare(
        status: Arc<Status>,
        options: GitPanelOptions,
        collapsed: BTreeSet<DirectoryKey>,
    ) -> Self {
        let groups: &[Option<Group>] = match options.group_by {
            GroupBy::None => &[None],
            GroupBy::TrackedUntracked => &[
                Some(Group::Conflicts),
                Some(Group::Tracked),
                Some(Group::Untracked),
            ],
            GroupBy::StagedUnstaged => &[
                Some(Group::Conflicts),
                Some(Group::Staged),
                Some(Group::Unstaged),
            ],
        };
        let mut rows = Vec::new();
        for &group in groups {
            let mut indices: Vec<_> = status
                .entries
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    group.is_none_or(|g| g.contains(entry)).then_some(index)
                })
                .collect();
            if indices.is_empty() {
                continue;
            }
            indices.sort_by(|&a, &b| status.entries[a].path.cmp(&status.entries[b].path));
            let mut root = Folder::default();
            if options.view == ViewMode::Tree {
                for index in indices {
                    let mut node = &mut root;
                    let mut parts = status.entries[index].path.split('/').peekable();
                    while let Some(part) = parts.next() {
                        if parts.peek().is_some() {
                            node = node.directories.entry(part.to_owned()).or_default();
                        }
                    }
                    node.files.push(index);
                }
                indices = Vec::new();
                root.index(&mut indices);
            }
            let all = ChangeSet {
                range: 0..indices.len(),
                indices: Arc::from(indices),
            };
            if let Some(group) = group {
                rows.push(Row::Header(
                    group,
                    all.clone(),
                    all.staging(&status, Some(group)),
                ));
            }
            if options.view == ViewMode::Tree {
                root.rows(&status, group, "", 0, &all.indices, &collapsed, &mut rows);
            } else {
                rows.extend(all.iter().map(|index| Row::File {
                    index,
                    group,
                    depth: 0,
                }));
            }
        }
        let staged_count = status
            .entries
            .iter()
            .filter(|entry| staging(entry) != Staging::Unstaged)
            .count();
        let suggested_message = suggest_message(&status);
        let has_conflicts = status.entries.iter().any(|entry| entry.conflicted);
        let row_index = if options.view == ViewMode::Tree {
            rows.iter()
                .enumerate()
                .map(|(i, row)| (row.key(&status), i))
                .collect()
        } else {
            HashMap::new()
        };
        Self {
            status,
            options,
            collapsed,
            rows,
            row_index,
            staged_count,
            suggested_message,
            has_conflicts,
        }
    }
}
/// "create foo.md, update bar.rs" for up to three staged files.
fn suggest_message(status: &Status) -> Option<String> {
    let staged: Vec<_> = status
        .entries
        .iter()
        .filter(|entry| staging(entry) != Staging::Unstaged)
        .collect();
    if staged.is_empty() || staged.len() > 3 {
        return None;
    }
    let name = |path: &str| path.rsplit('/').next().unwrap().to_owned();
    let parts: Vec<_> = staged
        .iter()
        .map(|entry| match entry.index_status {
            'A' => format!("create {}", name(&entry.path)),
            'D' => format!("delete {}", name(&entry.path)),
            'R' => match &entry.old_path {
                Some(old) => format!("rename {} to {}", name(old), name(&entry.path)),
                None => format!("rename {}", name(&entry.path)),
            },
            _ => format!("update {}", name(&entry.path)),
        })
        .collect();
    Some(parts.join(", "))
}
#[derive(Default)]
struct Folder {
    directories: BTreeMap<String, Folder>,
    files: Vec<usize>,
    range: Range<usize>,
}
impl Folder {
    fn index(&mut self, indices: &mut Vec<usize>) {
        self.range.start = indices.len();
        for child in self.directories.values_mut() {
            child.index(indices);
        }
        indices.extend_from_slice(&self.files);
        self.range.end = indices.len();
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "One traversal keeps source, group, tree location and output explicit"
    )]
    fn rows(
        &self,
        status: &Status,
        group: Option<Group>,
        parent: &str,
        depth: usize,
        indices: &Arc<[usize]>,
        collapsed: &BTreeSet<DirectoryKey>,
        rows: &mut Vec<Row>,
    ) {
        for (name, child) in &self.directories {
            let path = if parent.is_empty() {
                name.clone()
            } else {
                format!("{parent}/{name}")
            };
            let key = DirectoryKey { group, path };
            let expanded = !collapsed.contains(&key);
            let changes = ChangeSet {
                indices: indices.clone(),
                range: child.range.clone(),
            };
            let staged = changes.staging(status, group);
            let conflicted = changes.iter().any(|index| status.entries[index].conflicted);
            rows.push(Row::Directory {
                key: key.clone(),
                depth,
                changes,
                staged,
                expanded,
                conflicted,
            });
            if expanded {
                child.rows(
                    status,
                    group,
                    &key.path,
                    depth + 1,
                    indices,
                    collapsed,
                    rows,
                );
            }
        }
        rows.extend(self.files.iter().map(|&index| Row::File {
            index,
            group,
            depth,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StatusEntry;
    fn fixture() -> Arc<Status> {
        Arc::new(Status {
            entries: [
                ("src/lib.rs", 'M', 'M', false),
                ("src/deep/new.rs", '?', '?', false),
                ("readme.md", 'M', ' ', false),
                ("old.txt", ' ', 'D', false),
                ("merge/conflict.rs", 'U', 'U', true),
                ("brand/new.txt", 'A', ' ', false),
            ]
            .into_iter()
            .map(
                |(path, index_status, worktree_status, conflicted)| StatusEntry {
                    path: path.into(),
                    old_path: None,
                    index_status,
                    worktree_status,
                    conflicted,
                },
            )
            .collect(),
            ..Default::default()
        })
    }
    #[test]
    fn up_to_three_staged_files_suggest_a_commit_message() {
        let entry = |path: &str, index_status, old_path: Option<&str>| StatusEntry {
            path: path.into(),
            old_path: old_path.map(Into::into),
            index_status,
            worktree_status: ' ',
            conflicted: false,
        };
        let status = |entries: Vec<StatusEntry>| Status {
            entries,
            ..Default::default()
        };
        // The fixture stages readme.md, brand/new.txt and part of src/lib.rs.
        assert_eq!(
            RepositoryView::prepare(fixture(), GitPanelOptions::default(), BTreeSet::new())
                .suggested_message
                .as_deref(),
            Some("update lib.rs, update readme.md, create new.txt")
        );
        assert_eq!(
            RepositoryView::new(status(vec![
                entry("docs/foo.md", 'A', None),
                entry("gone.rs", 'D', None),
                entry("src/b.rs", 'R', Some("src/a.rs")),
                entry("untracked.rs", '?', None),
            ]))
            .suggested_message
            .as_deref(),
            Some("create foo.md, delete gone.rs, rename a.rs to b.rs")
        );
        let four = (0..4).map(|i| entry(&format!("{i}.rs"), 'M', None)).collect();
        assert_eq!(RepositoryView::new(status(four)).suggested_message, None);
        assert_eq!(RepositoryView::new(status(vec![])).suggested_message, None);
    }
    #[test]
    fn missing_preferences_use_tree_and_tracked_groups_but_saved_choices_survive() {
        let defaults = ron::from_str::<GitPanelOptions>("()").unwrap();
        assert_eq!(defaults.view, ViewMode::Tree);
        assert_eq!(defaults.group_by, GroupBy::TrackedUntracked);
        let saved =
            ron::from_str::<GitPanelOptions>("(view:list,group_by:staged_unstaged)").unwrap();
        assert_eq!(saved.view, ViewMode::List);
        assert_eq!(saved.group_by, GroupBy::StagedUnstaged);
        assert_eq!(
            ron::from_str::<GitPanelOptions>(&ron::to_string(&saved).unwrap()).unwrap(),
            saved
        );
    }
    #[test]
    fn every_view_and_grouping_keeps_all_changes_and_counts_staged_files_once() {
        for view in [ViewMode::List, ViewMode::Tree] {
            for group_by in [
                GroupBy::None,
                GroupBy::TrackedUntracked,
                GroupBy::StagedUnstaged,
            ] {
                let result = RepositoryView::prepare(
                    fixture(),
                    GitPanelOptions { view, group_by },
                    BTreeSet::new(),
                );
                let files: Vec<_> = result
                    .rows
                    .iter()
                    .filter_map(|row| {
                        if let Row::File { index, group, .. } = row {
                            Some((*index, *group))
                        } else {
                            None
                        }
                    })
                    .collect();
                assert_eq!(
                    files.len(),
                    if group_by == GroupBy::StagedUnstaged {
                        7
                    } else {
                        6
                    }
                );
                for index in 0..6 {
                    assert!(files.iter().any(|(i, _)| *i == index));
                }
                assert_eq!(result.staged_count, 3);
                assert!(result.has_conflicts);
                if group_by == GroupBy::None {
                    assert!(!result.rows.iter().any(|r| matches!(r, Row::Header(..))));
                }
                if view == ViewMode::List {
                    assert!(
                        !result
                            .rows
                            .iter()
                            .any(|r| matches!(r, Row::Directory { .. }))
                    );
                }
            }
        }
    }
    #[test]
    fn partial_files_appear_on_both_sides_with_distinct_widget_identity() {
        let result = RepositoryView::prepare(
            fixture(),
            GitPanelOptions {
                view: ViewMode::List,
                group_by: GroupBy::StagedUnstaged,
            },
            BTreeSet::new(),
        );
        let copies: Vec<_> = result
            .rows
            .iter()
            .filter(|row| matches!(row, Row::File { index: 0, .. }))
            .collect();
        assert_eq!(copies.len(), 2);
        assert_ne!(copies[0].key(&result.status), copies[1].key(&result.status));
        assert!(
            result
                .rows
                .iter()
                .any(|row| matches!(row, Row::Header(Group::Staged, _, Staging::Staged)))
        );
        assert!(
            result
                .rows
                .iter()
                .any(|row| matches!(row, Row::Header(Group::Unstaged, _, Staging::Unstaged)))
        );
        assert_eq!(
            result
                .rows
                .iter()
                .filter(|row| matches!(
                    row,
                    Row::File {
                        index: 4,
                        group: Some(Group::Conflicts),
                        ..
                    }
                ))
                .count(),
            1
        );
    }
    #[test]
    fn collapsed_folders_keep_exact_descendant_targets_for_each_side() {
        let key = DirectoryKey {
            group: Some(Group::Unstaged),
            path: "src".into(),
        };
        let result = RepositoryView::prepare(
            fixture(),
            GitPanelOptions {
                view: ViewMode::Tree,
                group_by: GroupBy::StagedUnstaged,
            },
            BTreeSet::from([key.clone()]),
        );
        let (targets, expanded) = result
            .rows
            .iter()
            .find_map(|row| match row {
                Row::Directory {
                    key: actual,
                    changes,
                    expanded,
                    ..
                } if *actual == key => Some((changes.iter().collect::<Vec<_>>(), expanded)),
                _ => None,
            })
            .unwrap();
        assert!(!expanded);
        assert_eq!(targets, vec![1, 0]);
        assert!(!result.rows.iter().any(|row| matches!(
            row,
            Row::File {
                index: 0 | 1,
                group: Some(Group::Unstaged),
                ..
            }
        )));
        assert!(result.rows.iter().any(|row| matches!(
            row,
            Row::File {
                index: 0,
                group: Some(Group::Staged),
                ..
            }
        )));
        assert!(!result.rows.iter().any(|row| matches!(
            row,
            Row::File {
                index: 1,
                group: Some(Group::Staged),
                ..
            }
        )));
    }
    #[test]
    fn ungrouped_tree_marks_folders_with_conflicts_and_preserves_added_file_grouping() {
        let result = RepositoryView::prepare(
            fixture(),
            GitPanelOptions {
                view: ViewMode::Tree,
                group_by: GroupBy::None,
            },
            BTreeSet::new(),
        );
        assert!(
            result.rows.iter().any(
                |row| matches!(row,Row::Directory {key,conflicted:true,..} if key.path=="merge")
            )
        );
        let result =
            RepositoryView::prepare(fixture(), GitPanelOptions::default(), BTreeSet::new());
        assert!(result.rows.iter().any(|row| matches!(
            row,
            Row::File {
                index: 5,
                group: Some(Group::Untracked),
                ..
            }
        )));
    }
}
