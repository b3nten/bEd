//! Direct context-menu intentions. Drawing only reads captured paths and the
//! worker's visible-row index; Update owns clipboard/filesystem I/O and edits.
use crate::{
    Action, FileTreeActivation, FileTreeDirectoryState, FileTreeEntryKind, FileTreePanel,
    FileTreeRow, MenuTarget,
};
use bedtime_ui::egui;
use std::path::Path;

#[derive(Clone, Copy, Debug)]
enum Item {
    Open,
    NewFile,
    NewFolder,
    Rename,
    Copy,
    Cut,
    Paste,
    Duplicate,
    Trash,
    Hide,
    Unhide,
    ToggleHiddenFiles,
    ToggleRevealHidden,
    ClearHidden,
    Refresh,
}

/// Facts about the capture, never today's cursor or selection. The lookup uses
/// the worker-built index in production; an explicit lookup also lets tests
/// exercise stale observations without filesystem work or rebuilding that index.
struct TargetFacts {
    open: bool,
    selection: bool,
    count: usize,
}

fn target_facts<'a>(
    target: &MenuTarget,
    generation: u64,
    root: &Path,
    row: impl Fn(&Path) -> Option<&'a FileTreeRow>,
) -> Option<TargetFacts> {
    if target.generation != generation || !target.path.starts_with(root) {
        return None;
    }
    let captured_row = row(&target.path)?;
    let directory = if captured_row.kind == FileTreeEntryKind::Directory {
        target.path.as_path()
    } else {
        target.path.parent()?
    };
    // Missing/filtered rows and changed destinations cannot silently turn into
    // actions on the root, another directory, or a later keyboard selection.
    if directory != target.directory
        || !row(directory).is_some_and(|row| row.kind == FileTreeEntryKind::Directory)
    {
        return None;
    }
    Some(TargetFacts {
        open: captured_row.kind != FileTreeEntryKind::Directory,
        selection: !target.paths.is_empty()
            && target
                .paths
                .iter()
                .all(|path| path != root && path.starts_with(root) && row(path).is_some()),
        count: target.paths.len(),
    })
}

struct Availability {
    target: Option<TargetFacts>,
    ready: bool,
    writable_idle: bool,
    configured: bool,
    fresh_background: bool,
    has_hidden: bool,
}

impl Availability {
    fn new(state: &FileTreePanel, target: Option<TargetFacts>, read_only: bool) -> Self {
        Self {
            target,
            ready: state.root_input.is_some()
                && !state.snapshot.root.as_os_str().is_empty()
                && state.snapshot.generation == state.generation
                && !state.snapshot.loading
                && state.snapshot.rows.first().is_some_and(|row| {
                    row.path == state.snapshot.root
                        && row.kind == FileTreeEntryKind::Directory
                        && row.directory_state == FileTreeDirectoryState::Ready
                }),
            writable_idle: !read_only && !state.is_busy() && state.editing.is_none(),
            configured: state.root_input.is_some(),
            fresh_background: state.menu_target.as_ref().is_some_and(|target| {
                target.generation == state.generation
                    && target.path == state.snapshot.root
                    && target.directory == state.snapshot.root
                    && target.paths.is_empty()
            }),
            has_hidden: !state.visibility.hidden_paths.is_empty(),
        }
    }

    fn enabled(&self, item: Item) -> bool {
        if matches!(item, Item::Refresh) {
            // Removing the toolbar must not remove recovery for a failed root
            // whose snapshot has no row yet. Refresh has no per-file effect.
            return self.configured && (self.target.is_some() || self.fresh_background);
        }
        let Some(target) = &self.target else {
            return false;
        };
        if !self.ready {
            return false;
        }
        match item {
            Item::Open => target.open,
            // Paste deliberately does not probe the native clipboard in draw.
            // Admission in Update reports empty/unsupported clipboard errors.
            Item::NewFile | Item::NewFolder | Item::Paste => self.writable_idle,
            Item::Rename => self.writable_idle && target.selection && target.count == 1,
            Item::Cut | Item::Duplicate | Item::Trash => self.writable_idle && target.selection,
            Item::Copy | Item::Hide => target.selection,
            // Unhide is idempotent; mixed selections remain valid.
            Item::Unhide => target.selection && self.has_hidden,
            Item::ToggleHiddenFiles | Item::ToggleRevealHidden => true,
            Item::ClearHidden => self.has_hidden,
            Item::Refresh => unreachable!(),
        }
    }
}

impl Item {
    /// Called only for an enabled item. Clone the explicit selection only when
    /// clicked, not once per menu item or once per frame of an open popup.
    fn action(self, target: &MenuTarget) -> Action {
        match self {
            Self::Open => Action::Open {
                path: target.path.clone(),
                activation: FileTreeActivation::ContextMenu,
            },
            Self::NewFile => Action::NewFile {
                directory: target.directory.clone(),
            },
            Self::NewFolder => Action::NewFolder {
                directory: target.directory.clone(),
            },
            Self::Rename => Action::Rename {
                path: target.paths[0].clone(),
            },
            Self::Copy => Action::Copy {
                paths: target.paths.clone(),
            },
            Self::Cut => Action::Cut {
                paths: target.paths.clone(),
            },
            Self::Paste => Action::Paste {
                directory: target.directory.clone(),
            },
            Self::Duplicate => Action::Duplicate {
                paths: target.paths.clone(),
            },
            Self::Trash => Action::Trash {
                paths: target.paths.clone(),
            },
            Self::Hide => Action::Hide {
                paths: target.paths.clone(),
            },
            Self::Unhide => Action::Unhide {
                paths: target.paths.clone(),
            },
            Self::ToggleHiddenFiles => Action::ToggleHiddenFiles,
            Self::ToggleRevealHidden => Action::ToggleRevealHidden,
            Self::ClearHidden => Action::ClearHidden,
            Self::Refresh => Action::Refresh,
        }
    }
}

/// Draw inside the row/background Response::context_menu closure. The caller
/// captures MenuTarget when opening the menu and supplies its read-only policy.
/// No World, menu entities, shortcut registration or native I/O is involved.
pub(crate) fn draw(
    ui: &mut egui::Ui,
    state: &mut FileTreePanel,
    read_only: bool,
    open_with: Option<&crate::FileTreeOpenWith>,
) {
    let target = state.menu_target.as_ref().and_then(|target| {
        target_facts(target, state.generation, &state.snapshot.root, |path| {
            state
                .snapshot
                .row_index(path)
                .map(|index| &state.snapshot.rows[index])
        })
    });
    let availability = Availability::new(state, target, read_only);
    if availability.enabled(Item::Open)
        && let Some(provider) = open_with
        && let Some(target) = &state.menu_target
    {
        let choices = (provider.0)(&target.path);
        if !choices.is_empty() {
            let path = target.path.clone();
            let mut choice = None;
            ui.menu_button("Open With", |ui| {
                for item in choices {
                    if ui.button(item.label).clicked() {
                        choice = Some(item.id);
                        ui.close();
                    }
                }
            });
            if let Some(choice) = choice {
                state.queue(crate::Action::OpenWith { path, choice });
            }
        }
    }
    let mut clicked = None;
    let mut entry = |ui: &mut egui::Ui, item, label: &str| {
        if ui
            .add_enabled(availability.enabled(item), egui::Button::new(label))
            .clicked()
        {
            clicked = Some(item);
        }
    };
    entry(ui, Item::Open, "Open");
    ui.separator();
    entry(ui, Item::NewFile, "New File…");
    entry(ui, Item::NewFolder, "New Folder…");
    entry(ui, Item::Rename, "Rename…");
    ui.separator();
    entry(ui, Item::Copy, "Copy");
    entry(ui, Item::Cut, "Cut");
    entry(ui, Item::Paste, "Paste");
    entry(ui, Item::Duplicate, "Duplicate");
    entry(ui, Item::Trash, "Move to Trash");
    ui.separator();
    entry(ui, Item::Hide, "Hide Selected");
    entry(ui, Item::Unhide, "Unhide Selected");
    entry(
        ui,
        Item::ToggleHiddenFiles,
        if state.visibility.hide_hidden {
            "Show Hidden Files"
        } else {
            "Hide Hidden Files"
        },
    );
    entry(
        ui,
        Item::ToggleRevealHidden,
        if state.show_hidden {
            "Stop Revealing Hidden Entries"
        } else {
            "Reveal Hidden Entries"
        },
    );
    entry(ui, Item::ClearHidden, "Clear Explicitly Hidden Entries");
    ui.separator();
    entry(ui, Item::Refresh, "Refresh");
    if let Some(item) = clicked
        && let Some(target) = &state.menu_target
    {
        let action = item.action(target);
        state.queue(action);
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileTreeSnapshot;
    use std::{path::PathBuf, sync::Arc};

    fn fixture() -> FileTreePanel {
        let rows = [
            ("root", FileTreeEntryKind::Directory),
            ("root/folder", FileTreeEntryKind::Directory),
            ("root/a", FileTreeEntryKind::File),
            ("root/b", FileTreeEntryKind::Symlink),
        ]
        .map(|(path, kind)| FileTreeRow {
            path: path.into(),
            name: Path::new(path).file_name().unwrap().into(),
            label: Arc::from(path),
            kind,
            depth: usize::from(path != "root"),
            hidden: false,
            directory_state: FileTreeDirectoryState::Ready,
        });
        // The snapshot's index is worker-owned. These pure tests supply a lookup
        // over four rows to the same capture validator used by drawing.
        let mut snapshot = FileTreeSnapshot::default();
        snapshot.root = "root".into();
        snapshot.generation = 7;
        snapshot.rows = rows.into();
        let mut state = FileTreePanel::default();
        state.snapshot = Arc::new(snapshot);
        state.root_input = Some("root".into());
        state.generation = 7;
        state.menu_target = Some(capture("root/a", "root", &["root/a"]));
        state.visibility.hidden_paths.insert("hidden".into());
        state
    }

    fn capture(path: &str, directory: &str, paths: &[&str]) -> MenuTarget {
        MenuTarget {
            path: path.into(),
            directory: directory.into(),
            paths: paths.iter().map(|path| PathBuf::from(*path)).collect(),
            generation: 7,
        }
    }

    fn availability(state: &FileTreePanel, read_only: bool) -> Availability {
        let facts = state.menu_target.as_ref().and_then(|target| {
            target_facts(target, state.generation, &state.snapshot.root, |path| {
                state.snapshot.rows.iter().find(|row| row.path == path)
            })
        });
        Availability::new(state, facts, read_only)
    }

    #[test]
    fn failed_root_background_retains_refresh_without_authorizing_file_actions() {
        let mut state = fixture();
        Arc::make_mut(&mut state.snapshot).rows = Arc::default();
        state.menu_target = Some(capture("root", "root", &[]));
        let available = availability(&state, false);
        assert!(available.enabled(Item::Refresh));
        assert!(!available.enabled(Item::Paste));
        assert!(!available.enabled(Item::NewFile));
        state.menu_target.as_mut().unwrap().generation -= 1;
        assert!(!availability(&state, false).enabled(Item::Refresh));
    }

    #[test]
    fn captured_selection_and_destination_do_not_follow_the_cursor() {
        let mut state = fixture();
        state.menu_target = Some(capture("root/a", "root", &["root/a", "root/b"]));
        state.cursor = Some("root/folder".into());
        state.selection = [PathBuf::from("root/folder")].into();
        assert!(availability(&state, false).enabled(Item::Copy));
        assert!(!availability(&state, false).enabled(Item::Rename));
        let target = state.menu_target.as_ref().unwrap();
        assert!(matches!(Item::Copy.action(target), Action::Copy { paths }
            if paths == [PathBuf::from("root/a"), PathBuf::from("root/b")]));
        assert!(
            matches!(Item::Open.action(target), Action::Open { path, activation }
            if path == Path::new("root/a") && activation == FileTreeActivation::ContextMenu)
        );
        assert!(
            matches!(Item::Paste.action(target), Action::Paste { directory }
            if directory == Path::new("root"))
        );
    }

    #[test]
    fn stale_generation_missing_row_and_changed_destination_disable_the_capture() {
        let mut state = fixture();
        let invalid = [
            MenuTarget {
                generation: 6,
                ..capture("root/a", "root", &["root/a"])
            },
            capture("root/gone", "root", &["root/gone"]),
            capture("root/a", "root/folder", &["root/a"]),
            capture("elsewhere/a", "elsewhere", &["elsewhere/a"]),
        ];
        for target in invalid {
            state.menu_target = Some(target);
            let available = availability(&state, false);
            for item in [
                Item::Open,
                Item::Copy,
                Item::NewFile,
                Item::Paste,
                Item::Refresh,
            ] {
                assert!(!available.enabled(item), "stale capture enabled {item:?}");
            }
        }
    }

    #[test]
    fn missing_or_protected_captured_sources_never_fall_back_to_current_selection() {
        let mut state = fixture();
        state.selection = [PathBuf::from("root/b")].into();
        for paths in [
            &["root/a", "root/gone"][..],
            &["root"][..],
            &["elsewhere/a"][..],
        ] {
            state.menu_target = Some(capture("root/a", "root", paths));
            let available = availability(&state, false);
            for item in [
                Item::Rename,
                Item::Copy,
                Item::Cut,
                Item::Duplicate,
                Item::Trash,
                Item::Hide,
                Item::Unhide,
            ] {
                assert!(!available.enabled(item), "invalid sources enabled {item:?}");
            }
            assert!(available.enabled(Item::Open));
        }
    }

    #[test]
    fn read_only_disables_filesystem_mutations_but_keeps_copy_and_browsing() {
        let state = fixture();
        let writable = availability(&state, false);
        let read_only = availability(&state, true);
        for item in [
            Item::NewFile,
            Item::NewFolder,
            Item::Rename,
            Item::Cut,
            Item::Paste,
            Item::Duplicate,
            Item::Trash,
        ] {
            assert!(writable.enabled(item), "writable capture disabled {item:?}");
            assert!(
                !read_only.enabled(item),
                "read-only capture enabled {item:?}"
            );
        }
        for item in [
            Item::Copy,
            Item::Open,
            Item::Hide,
            Item::Unhide,
            Item::ToggleHiddenFiles,
            Item::ToggleRevealHidden,
            Item::ClearHidden,
            Item::Refresh,
        ] {
            assert!(
                read_only.enabled(item),
                "read-only browsing disabled {item:?}"
            );
        }
    }

    #[test]
    fn root_background_supports_creation_and_paste_without_a_selection_or_clipboard() {
        let mut state = fixture();
        state.menu_target = Some(capture("root", "root", &[]));
        let available = availability(&state, false);
        for item in [Item::NewFile, Item::NewFolder, Item::Paste, Item::Refresh] {
            assert!(available.enabled(item));
        }
        for item in [
            Item::Open,
            Item::Rename,
            Item::Copy,
            Item::Cut,
            Item::Duplicate,
            Item::Trash,
            Item::Hide,
            Item::Unhide,
        ] {
            assert!(!available.enabled(item), "root background enabled {item:?}");
        }
        assert!(!availability(&state, true).enabled(Item::Paste));
    }

    #[test]
    fn loading_busy_and_editing_gate_mutations_without_disabling_refresh() {
        let mut state = fixture();
        Arc::make_mut(&mut state.snapshot).loading = true;
        assert!(!availability(&state, false).enabled(Item::Paste));
        assert!(availability(&state, false).enabled(Item::Refresh));
        Arc::make_mut(&mut state.snapshot).loading = false;
        state.pending.insert(1);
        assert!(!availability(&state, false).enabled(Item::NewFile));
        assert!(!availability(&state, false).enabled(Item::Cut));
        assert!(availability(&state, false).enabled(Item::Copy));
        assert!(availability(&state, false).enabled(Item::Open));
        state.pending.clear();
        state.editing = Some(crate::InlineEdit {
            kind: crate::EditKind::Rename,
            path: "root/a".into(),
            text: "a".into(),
            original: "a".into(),
            focus: false,
            pending: None,
            error: None,
        });
        assert!(!availability(&state, false).enabled(Item::Rename));
        assert!(!availability(&state, false).enabled(Item::Paste));
        assert!(availability(&state, false).enabled(Item::Copy));
        assert!(availability(&state, false).enabled(Item::Refresh));
    }
}
