//! The workspace's files, for `@` in a prompt: listed in the background,
//! and narrowed by what's typed after the `@`.
use crate::{composer::paint_row, panel::AcpPanel};
use bedtime_ui::{
    UiContext, UiWaker, egui,
    picker::{self, PickerRows},
};
use bedtime_workbench::{Cancellation, ManagedTask, ManagedWork};
use bevy::prelude::*;
use std::path::{Path, PathBuf};

/// The most files listed for `@` completion.
const MAX_WORKSPACE_FILES: usize = 50_000;
/// The most rows a query is narrowed to.
const MAX_ROWS: usize = 50;

/// A workspace file for `@` completion: its path from the workspace's
/// folder, and that as the text that's matched and shown.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorkspaceFile {
    pub path: PathBuf,
    label: String,
    /// Where the file's name starts in `label`, in characters.
    name_at: usize,
}

impl WorkspaceFile {
    pub fn new(path: PathBuf) -> Self {
        let label = path.to_string_lossy().into_owned();
        let name_at = label
            .rfind(std::path::MAIN_SEPARATOR)
            .map_or(0, |at| label[..=at].chars().count());
        Self {
            path,
            label,
            name_at,
        }
    }
}

/// Walk the workspace folder (respecting `.gitignore`) and return relative paths
/// for `@` file mention completion. Stops promptly if `cancel` is triggered.
pub fn list_workspace_files(cwd: &Path, cancel: &Cancellation) -> Vec<WorkspaceFile> {
    let mut files = Vec::new();
    let walker = ignore::WalkBuilder::new(cwd)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();

    for entry in walker.flatten() {
        if cancel.is_cancelled() {
            return Vec::new();
        }
        if entry.file_type().is_some_and(|ft| ft.is_file()) {
            let rel = entry
                .path()
                .strip_prefix(cwd)
                .unwrap_or(entry.path())
                .to_path_buf();
            files.push(WorkspaceFile::new(rel));
            if files.len() >= MAX_WORKSPACE_FILES {
                break;
            }
        }
    }
    files.sort();
    files
}

/// Extract an active `@query` token at the end of `input`, returning
/// `(byte_start_of_at, query_without_at)`.
pub fn active_at_mention(input: &str) -> Option<(usize, &str)> {
    let last_at = input.rfind('@')?;
    // Require `@` to be at the start of input or preceded by whitespace.
    if last_at > 0 {
        let prev = input[..last_at].chars().next_back()?;
        if !prev.is_whitespace() {
            return None;
        }
    }
    let query = &input[last_at + 1..];
    if query.chars().any(char::is_whitespace) {
        return None;
    }
    Some((last_at, query))
}

/// The files `query` fits, best first (at most `MAX_ROWS`), each with the
/// characters of its path that matched. A match in the file's name comes
/// before one in its folders.
pub fn rank_files(files: &[WorkspaceFile], query: &str) -> Vec<(usize, Vec<usize>)> {
    let mut scored: Vec<(i32, usize, Vec<usize>)> = files
        .iter()
        .enumerate()
        .filter_map(|(index, file)| {
            let (score, matched) = picker::fuzzy(&file.label, query)?;
            let in_name = matched.first().is_some_and(|&at| at >= file.name_at);
            Some((score + if in_name { 10 } else { 0 }, index, matched))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.truncate(MAX_ROWS);
    scored
        .into_iter()
        .map(|(_, index, matched)| (index, matched))
        .collect()
}

/// The files that fit what's typed after an `@`, as a picker's rows. The
/// rows found are kept by the panel (`shown`: path, text, characters
/// matched), so they outlast a new listing of the files.
pub(crate) struct FileRows<'a> {
    pub files: &'a [WorkspaceFile],
    pub shown: &'a mut Vec<(PathBuf, String, Vec<usize>)>,
}

impl FileRows<'_> {
    pub fn path(&self, row: usize) -> Option<PathBuf> {
        self.shown.get(row).map(|(path, ..)| path.clone())
    }
}

impl PickerRows for FileRows<'_> {
    fn filter(&mut self, query: &str) {
        *self.shown = rank_files(self.files, query)
            .into_iter()
            .map(|(index, matched)| {
                let file = &self.files[index];
                (file.path.clone(), file.label.clone(), matched)
            })
            .collect();
    }

    fn len(&self) -> usize {
        self.shown.len()
    }

    fn label(&self, row: usize) -> String {
        self.shown[row].1.clone()
    }

    fn paint(&self, ui: &mut egui::Ui, context: &UiContext<'_>, rect: egui::Rect, row: usize) {
        let (_, label, matched) = &self.shown[row];
        paint_row(ui, context, rect, label, matched, "");
    }
}

/// The workspace's files, for `@` mentions: listed in the background once a
/// chat panel is open, and again for another folder.
#[derive(Resource, Default)]
pub(crate) struct WorkspaceFiles {
    files: Option<(PathBuf, Vec<WorkspaceFile>)>,
    task: Option<(PathBuf, ManagedTask<Vec<WorkspaceFile>>)>,
}

impl WorkspaceFiles {
    pub(crate) fn cached(&self) -> &[WorkspaceFile] {
        self.files
            .as_ref()
            .map_or(&[], |(_, files)| files.as_slice())
    }
}

/// The file list is the old workspace's. Its connections, threads and chat
/// panels are `InWorkspace`, and go with it.
pub(crate) fn on_workspace_close(mut files: ResMut<WorkspaceFiles>) {
    *files = WorkspaceFiles::default();
}

pub(crate) fn poll_workspace_index(world: &mut World, cwd: &Path, waker: &UiWaker) {
    if cwd.as_os_str().is_empty() || world.query::<&AcpPanel>().iter(world).next().is_none() {
        return;
    }
    world.resource_scope(|world, mut index: Mut<WorkspaceFiles>| {
        // Poll an in-flight background file scan if present.
        if let Some((task_cwd, task)) = index.task.as_ref() {
            if task_cwd != cwd {
                task.cancel();
                index.task = None;
            } else if let Ok(files) = task.try_recv() {
                index.files = Some((task_cwd.clone(), files));
                index.task = None;
            }
        }

        let Some(mut work) = world.get_resource_mut::<ManagedWork>() else {
            return;
        };
        let needs_scan = !work.is_closing()
            && index.task.is_none()
            && index
                .files
                .as_ref()
                .is_none_or(|(cached_cwd, _)| cached_cwd != cwd);
        if needs_scan {
            let scan_cwd = cwd.to_path_buf();
            let wake = waker.clone();
            if let Ok(task) = work.spawn_task(None, "ACP: index workspace files", move |cancel| {
                let files = list_workspace_files(&scan_cwd, &cancel);
                wake.wake();
                files
            }) {
                index.task = Some((cwd.to_path_buf(), task));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_ranked_by_name_first() {
        let files: Vec<WorkspaceFile> = ["docs/main/notes.md", "src/Main.rs", "src/domain.rs"]
            .into_iter()
            .map(|path| WorkspaceFile::new(PathBuf::from(path)))
            .collect();
        let found: Vec<&str> = rank_files(&files, "main")
            .into_iter()
            .map(|(index, _)| files[index].label.as_str())
            .collect();
        assert_eq!(
            found,
            ["src/Main.rs", "src/domain.rs", "docs/main/notes.md"]
        );
        let (_, matched) = &rank_files(&files, "main")[0];
        assert_eq!(matched, &[4, 5, 6, 7]);
        assert_eq!(
            rank_files(&files, "").len(),
            3,
            "all of them, before anything is typed"
        );
    }

    #[test]
    fn at_mention_detection() {
        assert_eq!(
            active_at_mention("look at @src/main"),
            Some((8, "src/main"))
        );
        assert_eq!(active_at_mention("@"), Some((0, "")));
        assert_eq!(active_at_mention("email@example.com"), None);
        assert_eq!(active_at_mention("look at @src/main please"), None);
    }
}
