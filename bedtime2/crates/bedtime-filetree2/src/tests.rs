//! Headless integration coverage: real workspace workers, files and buffered messages.
use super::*;
use bedtime_workbench2::{InArea, Workbench, WorkbenchCorePlugin};
use bedtime_workbench2::{shutdown_work, work_finished};
use bevy::ecs::message::Messages;
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

pub(super) struct Fixture(pub(super) PathBuf);
impl Fixture {
    pub(super) fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "bedtime-filetree-integration-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        // macOS's /var is a symlink; compare worker identities, not temp spelling.
        Self(fs::canonicalize(path).unwrap())
    }
    pub(super) fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
    pub(super) fn directory(&self, relative: &str) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(&path).unwrap();
        path
    }
    pub(super) fn file(&self, relative: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn host() -> (App, Entity) {
    let mut app = App::new();
    // Core Workbench does not require a window, renderer or native clipboard.
    app.insert_resource(Clipboard::memory());
    app.add_plugins((FileTreePlugin, WorkbenchCorePlugin));
    let workspace = crate::test_layout(app.world_mut());
    (app, workspace)
}

pub(super) fn panel(app: &mut App, workspace: Entity, root: Option<&Path>) -> Entity {
    let workbench = Workbench::new(app.world_mut());
    let area = workbench.destination_area(workspace).unwrap();
    let panel = app
        .world_mut()
        .spawn((FileTreePanel::default(), InArea(area)))
        .id();
    if let Some(root) = root {
        app.world_mut()
            .entity_mut(panel)
            .insert(FileTreeRoot(root.to_owned()));
    }
    panel
}

pub(super) fn tree(app: &App, panel: Entity) -> &FileTreePanel {
    app.world().get::<FileTreePanel>(panel).unwrap()
}
fn visible(app: &App, panel: Entity, path: &Path) -> bool {
    tree(app, panel).snapshot().row_index(path).is_some()
}
fn queue(app: &mut App, panel: Entity, action: Action) {
    app.world_mut()
        .get_mut::<FileTreePanel>(panel)
        .unwrap()
        .queue(action);
}
fn select(app: &mut App, panel: Entity, path: &Path) {
    app.world_mut()
        .get_mut::<FileTreePanel>(panel)
        .unwrap()
        .select(path, false, false);
}
fn expand(app: &mut App, panel: Entity, path: &Path) {
    queue(
        app,
        panel,
        Action::Open {
            path: path.to_owned(),
            activation: FileTreeActivation::Keyboard,
        },
    );
}
pub(super) fn eventually(app: &mut App, mut condition: impl FnMut(&mut App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition(app) {
        assert!(Instant::now() < deadline, "File tree operation timed out");
        app.update();
        std::thread::sleep(Duration::from_millis(2));
    }
}
pub(super) fn ready(app: &mut App, panel: Entity, root: &Path) {
    eventually(app, |app| {
        let snapshot = tree(app, panel).snapshot();
        snapshot.root == root
            && !snapshot.loading
            && snapshot.error.is_none()
            && snapshot
                .rows
                .first()
                .is_some_and(|row| row.directory_state == FileTreeDirectoryState::Ready)
    });
}
fn drain<M: Message>(app: &mut App) -> Vec<M> {
    app.world_mut()
        .resource_mut::<Messages<M>>()
        .drain()
        .collect()
}

#[test]
fn source_open_callback_preserves_identity_and_rejects_a_changed_root() {
    use bedtime_workbench2::{DocumentId,TextDocument,document_source_id,DocumentLoadState,inspect_document};
    #[derive(Resource,Default)]
    struct Opened(Vec<(bedtime_filesystem2::FsDocumentSource,DocumentId)>);
    fn open(world:&mut World,request:FileTreeSourceOpen)->Result<(),String> {
        let handle=Workbench::new(world).open_document::<TextDocument>(request.source.clone()).map_err(|e|e.to_string())?;
        world.resource_mut::<Opened>().0.push((request.source,handle.id()));Ok(())
    }
    let fixture=Fixture::new();let path=fixture.file("file.txt","shared");let other=fixture.directory("other");
    let (mut app,layout)=host();app.init_resource::<Opened>().insert_resource(FileTreeOnOpen(open));let p=panel(&mut app,layout,Some(&fixture.0));ready(&mut app,p,&fixture.0);
    for count in 1..=2 {
        queue(&mut app,p,Action::Open {path:path.clone(),activation:FileTreeActivation::Keyboard});
        eventually(&mut app,|app|app.world().resource::<Opened>().0.len()==count);
    }
    let opened=&app.world().resource::<Opened>().0;let source=opened[0].0.id();let id=opened[0].1;
    assert_eq!(opened[1].0.id(),source);assert_eq!(opened[1].1,id);assert_eq!(document_source_id(app.world(),id),Some(source));
    eventually(&mut app,|app|inspect_document(app.world(),id).unwrap().load_state==DocumentLoadState::Ready);
    let state=app.world_mut().entity_mut(p).take::<FileTreePanel>().unwrap();
    assert!(opening::queue(app.world_mut(),p,&state,path,FileTreeActivation::Keyboard,None).unwrap());
    app.world_mut().entity_mut(p).insert((state,FileTreeRoot(other.clone())));ready(&mut app,p,&other);
    assert_eq!(app.world().resource::<Opened>().0.len(),2);
    shutdown_work(app.world_mut()).unwrap();eventually(&mut app,|app|work_finished(app.world()));
}
fn completion(app: &mut App, panel: Entity) -> FileTreeOperationFinished {
    let mut found = None;
    eventually(app, |app| {
        found = drain::<FileTreeOperationFinished>(app)
            .into_iter()
            .find(|event| event.panel == panel);
        found.is_some()
    });
    found.unwrap()
}
fn operate(app: &mut App, panel: Entity, action: Action) -> FileTreeOperationFinished {
    assert!(
        drain::<FileTreeOperationFinished>(app).is_empty(),
        "Unobserved previous operation"
    );
    queue(app, panel, action);
    completion(app, panel)
}
fn edit_name(app: &mut App, panel: Entity, text: &str) {
    app.world_mut()
        .get_mut::<FileTreePanel>(panel)
        .unwrap()
        .editing
        .as_mut()
        .expect("Inline editor should be open")
        .text = text.into();
}
fn rename(app: &mut App, panel: Entity, path: &Path, name: &str) -> FileTreeOperationFinished {
    queue(
        app,
        panel,
        Action::Rename {
            path: path.to_owned(),
        },
    );
    app.update();
    edit_name(app, panel, name);
    operate(app, panel, Action::SubmitEdit)
}
pub(super) fn finish(app: &mut App) {
    shutdown_work(app.world_mut()).unwrap();
    eventually(app, |app| work_finished(app.world_mut()));
    // Observe reserved operation outcomes and let Workbench reap finished work.
    app.update();
}

#[test]
fn supplied_roots_survive_initialization_and_missing_roots_never_browse_cwd() {
    let fixture = Fixture::new();
    let file = fixture.file("supplied.txt", b"supplied");
    let (mut app, workspace) = host();
    let supplied = panel(&mut app, workspace, Some(&fixture.0));
    let unconfigured = panel(&mut app, workspace, None);
    assert!(tree(&app, supplied).snapshot().rows.is_empty());
    ready(&mut app, supplied, &fixture.0);
    assert_eq!(
        app.world().get::<FileTreeRoot>(supplied).unwrap().0,
        fixture.0
    );
    assert!(visible(&app, supplied, &file));
    assert!(app.world().get::<FileTreeRoot>(unconfigured).is_none());
    assert!(
        tree(&app, unconfigured)
            .snapshot()
            .root
            .as_os_str()
            .is_empty()
    );
    assert!(tree(&app, unconfigured).snapshot().rows.is_empty());
    assert!(tree(&app, unconfigured).error().is_none());
    assert!(drain::<FileTreePreferencesChanged>(&mut app).is_empty());
    finish(&mut app);
}

#[test]
fn loading_is_dirs_first_and_only_expanded_directories_are_enumerated() {
    let fixture = Fixture::new();
    let branch = fixture.directory("A-dir");
    fixture.directory("z-dir");
    let child = fixture.file("A-dir/nested/child.txt", b"child");
    fixture.file("a.txt", b"a");
    fixture.file("Z.txt", b"z");
    fixture.file(".hidden", b"hidden");
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, panel, &fixture.0);
    let snapshot = tree(&app, panel).snapshot().clone();
    let names: Vec<_> = snapshot
        .rows
        .iter()
        .skip(1)
        .map(|row| row.name.to_str().unwrap())
        .collect();
    assert_eq!(names, ["A-dir", "z-dir", "a.txt", "Z.txt"]);
    assert_eq!(snapshot.cached_directories, 1);
    assert_eq!(
        snapshot.rows[1].directory_state,
        FileTreeDirectoryState::Unloaded
    );
    assert!(!visible(&app, panel, &child));
    expand(&mut app, panel, &branch);
    let nested = fixture.path("A-dir/nested");
    eventually(&mut app, |app| visible(app, panel, &nested));
    assert!(!visible(&app, panel, &child));
    expand(&mut app, panel, &nested);
    eventually(&mut app, |app| visible(app, panel, &child));
    let row =
        &tree(&app, panel).snapshot().rows[tree(&app, panel).snapshot().row_index(&child).unwrap()];
    assert_eq!(row.depth, 3);
    select(&mut app, panel, &child);
    expand(&mut app, panel, &branch); // collapse, retaining no invisible operation target
    eventually(&mut app, |app| !visible(app, panel, &child));
    assert!(tree(&app, panel).selection().is_empty());
    assert_eq!(
        snapshot.rows.len(),
        5,
        "Previously published snapshots remain immutable"
    );
    finish(&mut app);
}

#[test]
fn panels_keep_independent_roots_navigation_and_selection_but_share_one_workspace_worker() {
    let first = Fixture::new();
    let second = Fixture::new();
    let branch = first.directory("branch");
    let child = first.file("branch/child.txt", b"first");
    let other = second.file("other.txt", b"second");
    let (mut app, workspace) = host();
    let a = panel(&mut app, workspace, Some(&first.0));
    let b = panel(&mut app, workspace, Some(&second.0));
    let same_root = panel(&mut app, workspace, Some(&first.0));
    for (panel, root) in [(a, &first.0), (b, &second.0), (same_root, &first.0)] {
        ready(&mut app, panel, root);
    }
    expand(&mut app, a, &branch);
    eventually(&mut app, |app| visible(app, a, &child));
    select(&mut app, a, &child);
    select(&mut app, b, &other);
    assert_eq!(tree(&app, a).selection(), &BTreeSet::from([child.clone()]));
    assert_eq!(tree(&app, b).selection(), &BTreeSet::from([other]));
    assert!(tree(&app, same_root).selection().is_empty());
    assert!(!visible(&app, same_root, &child));
    assert!(!visible(&app, b, &branch));
    let work = app.world().get_resource::<ManagedWork>().unwrap().list();
    assert_eq!(work.len(), 1);
    assert!(
        work[0].owner.is_none(),
        "Closing one panel must not cancel shared filesystem work"
    );
    assert_eq!(work[0].status, WorkStatus::Running);
    finish(&mut app);
}

#[test]
fn changing_root_retires_old_observations_and_old_selection() {
    let first = Fixture::new();
    let second = Fixture::new();
    let old = first.file("old/child.txt", b"old");
    let new = second.file("new.txt", b"new");
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, Some(&first.0));
    ready(&mut app, panel, &first.0);
    select(&mut app, panel, &first.path("old"));
    let original = tree(&app, panel).snapshot().clone();
    expand(&mut app, panel, &first.path("old"));
    app.update(); // an old-root projection can now be in flight
    app.world_mut()
        .entity_mut(panel)
        .insert(FileTreeRoot(second.0.clone()));
    app.update();
    assert!(tree(&app, panel).selection().is_empty());
    assert!(!visible(&app, panel, &old));
    eventually(&mut app, |app| {
        let snapshot = tree(app, panel).snapshot();
        assert!(
            snapshot
                .rows
                .iter()
                .all(|row| row.path.starts_with(&second.0))
        );
        snapshot.root == second.0 && !snapshot.loading && visible(app, panel, &new)
    });
    assert!(tree(&app, panel).snapshot().generation > original.generation);
    tree(&app, panel).snapshot().rows.iter().for_each(|row| {
        assert!(!row.path.starts_with(&first.0));
    });
    assert_eq!(original.root, first.0);
    finish(&mut app);
}

#[test]
fn filesystem_observations_continue_in_an_inactive_workspace_without_background_ui_updates() {
    let fixture = Fixture::new();
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, panel, &fixture.0);
    let other = crate::test_layout(app.world_mut());
    Workbench::new(app.world_mut())
        .activate_layout(other)
        .unwrap();
    // No refresh or drawing: native notifications or the worker fallback discover it.
    let added = fixture.file("background.txt", b"observed");
    eventually(&mut app, |app| visible(app, panel, &added));
    fs::remove_file(&added).unwrap();
    eventually(&mut app, |app| !visible(app, panel, &added));
    finish(&mut app);
}

#[test]
fn refresh_and_watcher_reload_preserve_selection_cursor_and_unsubmitted_rename_until_disappearance()
{
    let fixture = Fixture::new();
    let branch = fixture.directory("branch");
    let selected = fixture.file("branch/selected.txt", b"original contents");
    for index in 0..256 {
        fixture.file(&format!("branch/entry-{index:04}.txt"), b"entry");
    }
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, panel, &fixture.0);
    expand(&mut app, panel, &branch);
    eventually(&mut app, |app| {
        visible(app, panel, &selected) && !tree(app, panel).snapshot().loading
    });
    select(&mut app, panel, &selected);
    queue(
        &mut app,
        panel,
        Action::Rename {
            path: selected.clone(),
        },
    );
    app.update();
    edit_name(&mut app, panel, "unsubmitted name.txt");
    let assert_draft = |app: &App| {
        let state = tree(app, panel);
        assert_eq!(state.selection(), &BTreeSet::from([selected.clone()]));
        assert_eq!(state.cursor.as_deref(), Some(selected.as_path()));
        let edit = state
            .editing
            .as_ref()
            .expect("Reloading a directory is not deletion of the edited row");
        assert_eq!(edit.path, selected);
        assert_eq!(edit.text, "unsubmitted name.txt");
        assert!(edit.pending.is_none());
    };
    let before = tree(&app, panel).snapshot().version;
    app.world_mut()
        .get_mut::<FileTreePanel>(panel)
        .unwrap()
        .refresh();
    // Assert every snapshot the application actually admits, including Loading
    // if observed. A fast worker may coalesce Loading; catching it is not required.
    eventually(&mut app, |app| {
        assert_draft(app);
        let snapshot = tree(app, panel).snapshot();
        snapshot.version > before && !snapshot.loading
    });
    let added = fixture.file("branch/unrelated-new.txt", b"external addition");
    eventually(&mut app, |app| {
        assert_draft(app);
        visible(app, panel, &added) && !tree(app, panel).snapshot().loading
    });
    assert_eq!(fs::read(&selected).unwrap(), b"original contents");
    assert!(!fixture.path("branch/unsubmitted name.txt").exists());

    // A completed observation of real disappearance, unlike invalidation, does
    // retire both the draft and its now-invisible destructive-operation target.
    fs::remove_file(&selected).unwrap();
    eventually(&mut app, |app| {
        !visible(app, panel, &selected) && !tree(app, panel).snapshot().loading
    });
    assert!(tree(&app, panel).selection().is_empty());
    assert!(tree(&app, panel).editing.is_none());
    assert_ne!(
        tree(&app, panel).cursor.as_deref(),
        Some(selected.as_path())
    );
    select(&mut app, panel, &added);
    queue(
        &mut app,
        panel,
        Action::Rename {
            path: added.clone(),
        },
    );
    app.update();
    edit_name(&mut app, panel, "another draft.txt");
    expand(&mut app, panel, &branch); // collapse makes the selected child invisible
    eventually(&mut app, |app| !visible(app, panel, &added));
    assert!(tree(&app, panel).selection().is_empty());
    assert!(tree(&app, panel).editing.is_none());
    assert_eq!(fs::read(added).unwrap(), b"external addition");
    finish(&mut app);
}

#[test]
fn activations_are_buffered_keep_both_clicks_and_capture_the_originating_workspace() {
    let fixture = Fixture::new();
    let file = fixture.file("document.txt", b"document");
    let directory = fixture.directory("folder");
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, panel, &fixture.0);
    expand(&mut app, panel, &fixture.0);
    expand(&mut app, panel, &directory);
    app.update();
    assert!(
        drain::<FileTreeOpenRequested>(&mut app).is_empty(),
        "Directories, including the root, are never open-file requests"
    );
    for activation in [
        FileTreeActivation::SingleClick,
        FileTreeActivation::DoubleClick,
    ] {
        queue(
            &mut app,
            panel,
            Action::Open {
                path: file.clone(),
                activation,
            },
        );
    }
    assert!(
        drain::<FileTreeOpenRequested>(&mut app).is_empty(),
        "Capturing an intention must not invoke the host during drawing"
    );
    let other = crate::test_layout(app.world_mut());
    Workbench::new(app.world_mut())
        .activate_layout(other)
        .unwrap();
    app.update();
    let events = drain::<FileTreeOpenRequested>(&mut app);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].activation, FileTreeActivation::SingleClick);
    assert_eq!(events[1].activation, FileTreeActivation::DoubleClick);
    for event in events {
        assert_eq!(event.panel, panel);
        assert_eq!(event.path, file);
    }
    assert_eq!(tree(&app, panel).snapshot().rows[0].path, fixture.0);
    finish(&mut app);
}

#[test]
fn hide_preferences_are_root_relative_shared_persistable_and_recoverable_without_persisting_reveal()
{
    let fixture = Fixture::new();
    let unrelated = Fixture::new();
    let folder = fixture.directory("hidden-folder");
    let same_name = unrelated.directory("hidden-folder");
    let (mut app, workspace) = host();
    let a = panel(&mut app, workspace, Some(&fixture.0));
    let b = panel(&mut app, workspace, Some(&fixture.0));
    let other = panel(&mut app, workspace, Some(&unrelated.0));
    for (panel, root) in [(a, &fixture.0), (b, &fixture.0), (other, &unrelated.0)] {
        ready(&mut app, panel, root);
    }
    assert!(drain::<FileTreePreferencesChanged>(&mut app).is_empty());
    queue(
        &mut app,
        a,
        Action::Hide {
            paths: vec![folder.clone()],
        },
    );
    app.update();
    let events = drain::<FileTreePreferencesChanged>(&mut app);
    assert_eq!(events.len(), 1);

    eventually(&mut app, |app| {
        !visible(app, a, &folder) && !visible(app, b, &folder)
    });
    assert!(visible(&app, other, &same_name));
    let saved = app
        .world()
        .get_resource::<FileTreePreferences>()
        .unwrap()
        .clone();
    assert_eq!(
        saved.roots[&fixture.0].hidden_paths,
        BTreeSet::from([PathBuf::from("hidden-folder")])
    );
    assert_eq!(
        FileTreePreferences::decode(&saved.encode().unwrap()).unwrap(),
        saved
    );
    queue(&mut app, a, Action::ToggleRevealHidden);
    eventually(&mut app, |app| visible(app, a, &folder));
    assert!(
        !visible(&app, b, &folder),
        "Recovery reveal belongs to this panel, not persisted policy"
    );
    let snapshot = tree(&app, a).snapshot();
    assert!(snapshot.rows[snapshot.row_index(&folder).unwrap()].hidden);
    assert!(!snapshot.rows[0].hidden);
    assert_eq!(
        app.world().get_resource::<FileTreePreferences>().unwrap(),
        &saved
    );
    assert!(drain::<FileTreePreferencesChanged>(&mut app).is_empty());
    queue(
        &mut app,
        a,
        Action::Unhide {
            paths: vec![folder.clone()],
        },
    );
    app.update();
    assert_eq!(drain::<FileTreePreferencesChanged>(&mut app).len(), 1);
    eventually(&mut app, |app| visible(app, b, &folder));
    assert!(
        app.world()
            .get_resource::<FileTreePreferences>()
            .unwrap()
            .roots[&fixture.0]
            .hidden_paths
            .is_empty()
    );
    finish(&mut app);
}

#[test]
fn show_hidden_files_updates_both_same_root_panels_and_can_be_restored_from_the_record() {
    let fixture = Fixture::new();
    let hidden = fixture.file(".settings", b"settings");
    let (mut app, workspace) = host();
    let a = panel(&mut app, workspace, Some(&fixture.0));
    let b = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, a, &fixture.0);
    ready(&mut app, b, &fixture.0);
    assert!(!visible(&app, a, &hidden));
    queue(&mut app, a, Action::ToggleHiddenFiles);
    app.update();
    assert_eq!(drain::<FileTreePreferencesChanged>(&mut app).len(), 1);
    eventually(&mut app, |app| {
        visible(app, a, &hidden) && visible(app, b, &hidden)
    });
    let saved = app
        .world()
        .get_resource::<FileTreePreferences>()
        .unwrap()
        .encode()
        .unwrap();
    assert!(!FileTreePreferences::decode(&saved).unwrap().roots[&fixture.0].hide_hidden);
    // Applications restore their own preferences independently of panel layout.
    let restored_workspace = crate::test_layout(app.world_mut());
    app.world_mut()
        .insert_resource(FileTreePreferences::decode(&saved).unwrap());
    let restored = panel(&mut app, restored_workspace, Some(&fixture.0));
    ready(&mut app, restored, &fixture.0);
    eventually(&mut app, |app| visible(app, restored, &hidden));
    assert!(
        drain::<FileTreePreferencesChanged>(&mut app).is_empty(),
        "Applying persisted policy is not a new user edit"
    );
    finish(&mut app);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn successful_directory_rename_migrates_hidden_descendants_and_other_panels_expansion() {
    let fixture = Fixture::new();
    let folder = fixture.directory("before");
    let child = fixture.file("before/secret.txt", b"secret");
    let (mut app, workspace) = host();
    let a = panel(&mut app, workspace, Some(&fixture.0));
    let b = panel(&mut app, workspace, Some(&fixture.0));
    for panel in [a, b] {
        ready(&mut app, panel, &fixture.0);
        expand(&mut app, panel, &folder);
        eventually(&mut app, |app| visible(app, panel, &child));
    }
    queue(
        &mut app,
        a,
        Action::Hide {
            paths: vec![child.clone()],
        },
    );
    app.update();
    drain::<FileTreePreferencesChanged>(&mut app);
    eventually(&mut app, |app| {
        !visible(app, a, &child) && !visible(app, b, &child)
    });
    let event = rename(&mut app, a, &folder, "after");
    assert!(event.result.errors.is_empty());
    assert_eq!(event.result.completed.len(), 1);
    assert_eq!(event.root, fixture.0);
    assert!(!folder.exists());
    assert_eq!(
        fs::read(fixture.path("after/secret.txt")).unwrap(),
        b"secret"
    );
    let preferences = app.world().get_resource::<FileTreePreferences>().unwrap();
    assert_eq!(
        preferences.roots[&fixture.0].hidden_paths,
        BTreeSet::from([PathBuf::from("after/secret.txt")])
    );
    assert_eq!(drain::<FileTreePreferencesChanged>(&mut app).len(), 1);
    queue(&mut app, b, Action::ToggleRevealHidden);
    let renamed_child = fixture.path("after/secret.txt");
    eventually(&mut app, |app| visible(app, b, &renamed_child));
    assert_eq!(
        tree(&app, b).snapshot().rows[tree(&app, b).snapshot().row_index(&renamed_child).unwrap()]
            .depth,
        2
    );
    assert!(!visible(&app, b, &folder));
    finish(&mut app);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn failed_inline_rename_preserves_the_editor_preferences_and_both_files_then_allows_retry() {
    let fixture = Fixture::new();
    let source = fixture.file("source.txt", b"source contents");
    let occupied = fixture.file("occupied.txt", b"do not replace");
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, panel, &fixture.0);
    select(&mut app, panel, &source);
    let before = app
        .world()
        .get_resource::<FileTreePreferences>()
        .unwrap()
        .clone();
    let failed = rename(&mut app, panel, &source, "occupied.txt");
    assert!(failed.result.completed.is_empty());
    assert_eq!(failed.result.errors.len(), 1);
    let edit = tree(&app, panel)
        .editing
        .as_ref()
        .expect("A failed rename must remain editable");
    assert_eq!(edit.text, "occupied.txt");
    assert!(edit.error.is_some());
    assert!(edit.pending.is_none());
    assert!(!tree(&app, panel).is_busy());
    assert!(tree(&app, panel).error().is_some());
    assert_eq!(fs::read(&source).unwrap(), b"source contents");
    assert_eq!(fs::read(&occupied).unwrap(), b"do not replace");
    assert_eq!(
        app.world().get_resource::<FileTreePreferences>().unwrap(),
        &before
    );
    assert!(drain::<FileTreePreferencesChanged>(&mut app).is_empty());
    edit_name(&mut app, panel, "renamed.txt");
    let retried = operate(&mut app, panel, Action::SubmitEdit);
    assert!(retried.result.errors.is_empty());
    assert!(tree(&app, panel).editing.is_none());
    assert!(!source.exists());
    let renamed = fixture.path("renamed.txt");
    eventually(&mut app, |app| visible(app, panel, &renamed));
    assert_eq!(
        tree(&app, panel).selection(),
        &BTreeSet::from([renamed.clone()])
    );
    assert_eq!(fs::read(renamed).unwrap(), b"source contents");
    assert_eq!(fs::read(occupied).unwrap(), b"do not replace");
    finish(&mut app);
}

#[test]
fn inline_creation_rejects_escaping_names_without_writes_and_supports_file_and_folder_creation() {
    let fixture = Fixture::new();
    // Even a broken boundary check must only write inside this test's fixture.
    let root = fixture.directory("root");
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, Some(&root));
    ready(&mut app, panel, &root);
    queue(
        &mut app,
        panel,
        Action::NewFile {
            directory: root.clone(),
        },
    );
    app.update();
    edit_name(&mut app, panel, "../escape");
    let failed = operate(&mut app, panel, Action::SubmitEdit);
    assert!(failed.result.completed.is_empty());
    assert!(!failed.result.errors.is_empty());
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    assert!(!fixture.path("escape").exists());
    assert_eq!(
        tree(&app, panel).editing.as_ref().unwrap().text,
        "../escape"
    );
    edit_name(&mut app, panel, "new.txt");
    assert!(
        operate(&mut app, panel, Action::SubmitEdit)
            .result
            .errors
            .is_empty()
    );
    assert_eq!(fs::read(root.join("new.txt")).unwrap(), b"");
    queue(
        &mut app,
        panel,
        Action::NewFolder {
            directory: root.clone(),
        },
    );
    app.update();
    edit_name(&mut app, panel, "new-folder");
    assert!(
        operate(&mut app, panel, Action::SubmitEdit)
            .result
            .errors
            .is_empty()
    );
    assert!(root.join("new-folder").is_dir());
    finish(&mut app);
}

// Copies/renames use an atomic no-replace commit that is deliberately unsupported
// on other platforms; do not weaken those safety semantics to make a test pass.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn duplicate_keeps_both_and_clipboard_copy_eliminates_nested_duplicate_sources() {
    let fixture = Fixture::new();
    let source = fixture.file("note.txt", b"original");
    let occupied = fixture.file("note copy.txt", b"existing copy");
    let folder = fixture.directory("folder");
    let child = fixture.file("folder/child.txt", b"nested");
    let destination = fixture.directory("destination");
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, panel, &fixture.0);
    let duplicate = operate(
        &mut app,
        panel,
        Action::Duplicate {
            paths: vec![source.clone()],
        },
    );
    assert!(duplicate.result.errors.is_empty());
    let [
        FileTreeChange::Copied {
            source: copied_source,
            destination: duplicate_path,
        },
    ] = duplicate.result.completed.as_slice()
    else {
        panic!("Expected one committed duplicate: {:?}", duplicate.result);
    };
    assert_eq!(copied_source, &source);
    assert_ne!(duplicate_path, &source);
    assert_ne!(duplicate_path, &occupied);
    assert_eq!(fs::read(duplicate_path).unwrap(), b"original");
    assert_eq!(fs::read(&occupied).unwrap(), b"existing copy");
    queue(
        &mut app,
        panel,
        Action::Copy {
            paths: vec![folder.clone(), child],
        },
    );
    app.update();
    let copied = operate(
        &mut app,
        panel,
        Action::Paste {
            directory: destination.clone(),
        },
    );
    assert!(copied.result.errors.is_empty());
    assert_eq!(copied.result.completed.len(), 1);
    assert_eq!(
        fs::read(destination.join("folder/child.txt")).unwrap(),
        b"nested"
    );
    assert!(folder.is_dir());
    assert_eq!(fs::read(source).unwrap(), b"original");
    finish(&mut app);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn replacing_the_shared_clipboard_even_with_the_same_payload_revokes_destructive_cut() {
    let fixture = Fixture::new();
    let external = Fixture::new();
    let source = fixture.file("source.txt", b"keep source");
    let outside = external.file("external.txt", b"external source");
    let destination = fixture.directory("destination");
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, panel, &fixture.0);
    queue(
        &mut app,
        panel,
        Action::Cut {
            paths: vec![source.clone()],
        },
    );
    app.update();
    // Another consumer shares the memory clipboard. This is deterministic and
    // never reads/writes the user's native pasteboard contents.
    let mut other_consumer = app.world().resource::<Clipboard>().clone();
    other_consumer
        .write_files(std::slice::from_ref(&source))
        .unwrap();
    let replaced = operate(
        &mut app,
        panel,
        Action::Paste {
            directory: destination.clone(),
        },
    );
    assert!(replaced.result.errors.is_empty());
    assert!(matches!(
        replaced.result.completed.as_slice(),
        [FileTreeChange::Copied { .. }]
    ));
    assert_eq!(fs::read(&source).unwrap(), b"keep source");
    assert_eq!(
        fs::read(destination.join("source.txt")).unwrap(),
        b"keep source"
    );
    other_consumer
        .write_files(std::slice::from_ref(&outside))
        .unwrap();
    let imported = operate(
        &mut app,
        panel,
        Action::Paste {
            directory: destination.clone(),
        },
    );
    assert!(imported.result.errors.is_empty());
    assert!(matches!(
        imported.result.completed.as_slice(),
        [FileTreeChange::Copied { .. }]
    ));
    assert_eq!(
        fs::read(destination.join("external.txt")).unwrap(),
        b"external source"
    );
    assert_eq!(fs::read(outside).unwrap(), b"external source");
    finish(&mut app);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn an_old_move_completion_cannot_consume_another_panels_newer_cut_of_the_same_path() {
    let fixture = Fixture::new();
    let source = fixture.file("source.txt", b"first generation");
    let old_destination = fixture.directory("old-destination");
    let new_destination = fixture.directory("new-destination");
    let (mut app, workspace) = host();
    let a = panel(&mut app, workspace, Some(&fixture.0));
    let b = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, a, &fixture.0);
    ready(&mut app, b, &fixture.0);
    queue(
        &mut app,
        a,
        Action::Cut {
            paths: vec![source.clone()],
        },
    );
    app.update();
    let old_revision = app.world().resource::<Clipboard>().revision();
    let origin = OperationOrigin {
        panel: a,
        error_title: "Couldn't move the items",
        root: fixture.0.clone(),
        generation: tree(&app, a).snapshot().generation,
        cut_snapshot: Some(
            app.world()
                .resource::<FileTreeClipboard>()
                .0
                .as_ref()
                .unwrap()
                .clipboard_snapshot
                .clone(),
        ),
    };
    // Execute real filesystem effects, then deliberately defer only reconciliation.
    // This isolates the completion/new-intention ordering without guessed copy
    // durations, a synthetic success result, or reliance on worker scheduling.
    let old_result = operations::execute(
        FileTreeOperationRequest {
            id: 10_000,
            root: fixture.0.clone(),
            operation: FileTreeOperation::Move {
                sources: vec![source.clone()],
                directory: old_destination.clone(),
            },
        },
        || false,
        |_| {},
    );
    assert!(old_result.errors.is_empty());
    assert!(matches!(
        old_result.completed.as_slice(),
        [FileTreeChange::Moved { .. }]
    ));
    assert_eq!(
        fs::read(old_destination.join("source.txt")).unwrap(),
        b"first generation"
    );
    fixture.file("source.txt", b"second generation");
    // Replacing the shared service can reuse the same numerical local revision.
    // An operation's identity must not be only that counter.
    app.insert_resource(Clipboard::memory());
    queue(
        &mut app,
        b,
        Action::Cut {
            paths: vec![source.clone()],
        },
    );
    app.update();
    let mut clipboard = app.world().resource::<Clipboard>().clone();
    let new_revision = clipboard.revision();
    assert_eq!(
        new_revision, old_revision,
        "This regression deliberately reuses the numerical revision"
    );
    reconcile_operation(app.world_mut(), &origin, &old_result);
    assert_eq!(
        clipboard.revision(),
        new_revision,
        "An old result must not rewrite a newer owned clipboard"
    );
    assert_eq!(clipboard.read_files().unwrap(), [source.clone()]);
    let newer = operate(
        &mut app,
        b,
        Action::Paste {
            directory: new_destination.clone(),
        },
    );
    assert!(newer.result.errors.is_empty());
    assert!(
        matches!(
            newer.result.completed.as_slice(),
            [FileTreeChange::Moved { .. }]
        ),
        "The newer Cut must remain a move, not degrade into Copy or an empty paste"
    );
    assert!(!source.exists());
    assert_eq!(
        fs::read(new_destination.join("source.txt")).unwrap(),
        b"second generation"
    );
    assert_eq!(
        fs::read(old_destination.join("source.txt")).unwrap(),
        b"first generation"
    );
    assert_eq!(
        clipboard.read_files().unwrap_err().kind(),
        std::io::ErrorKind::NotFound,
        "Successful cut completion clears its own file clipboard"
    );
    finish(&mut app);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn partial_cut_moves_preserve_failed_intent_for_retry_and_migrate_only_completed_hidden_paths() {
    let fixture = Fixture::new();
    let good = fixture.file("good.txt", b"good");
    let blocked = fixture.file("blocked.txt", b"blocked source");
    let destination = fixture.directory("destination");
    let collision = fixture.file("destination/blocked.txt", b"existing destination");
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, panel, &fixture.0);
    queue(
        &mut app,
        panel,
        Action::Hide {
            paths: vec![good.clone(), blocked.clone()],
        },
    );
    queue(&mut app, panel, Action::ToggleRevealHidden);
    app.update();
    drain::<FileTreePreferencesChanged>(&mut app);
    queue(
        &mut app,
        panel,
        Action::Cut {
            paths: vec![good.clone(), blocked.clone()],
        },
    );
    app.update();
    let partial = operate(
        &mut app,
        panel,
        Action::Paste {
            directory: destination.clone(),
        },
    );
    assert_eq!(partial.result.completed.len(), 1);
    assert_eq!(partial.result.errors.len(), 1);
    assert!(
        matches!(&partial.result.completed[0], FileTreeChange::Moved { source, .. } if source == &good)
    );
    assert!(!good.exists());
    assert_eq!(fs::read(destination.join("good.txt")).unwrap(), b"good");
    assert_eq!(fs::read(&blocked).unwrap(), b"blocked source");
    assert_eq!(fs::read(&collision).unwrap(), b"existing destination");
    let mut clipboard = app.world().resource::<Clipboard>().clone();
    assert_eq!(clipboard.read_files().unwrap(), [blocked.clone()]);
    assert_eq!(
        app.world()
            .get_resource::<FileTreePreferences>()
            .unwrap()
            .roots[&fixture.0]
            .hidden_paths,
        BTreeSet::from([
            PathBuf::from("destination/good.txt"),
            PathBuf::from("blocked.txt")
        ])
    );
    assert_eq!(drain::<FileTreePreferencesChanged>(&mut app).len(), 1);
    fs::remove_file(&collision).unwrap();
    let retry = operate(
        &mut app,
        panel,
        Action::Paste {
            directory: destination.clone(),
        },
    );
    assert!(retry.result.errors.is_empty());
    assert!(
        matches!(
            retry.result.completed.as_slice(),
            [FileTreeChange::Moved { .. }]
        ),
        "Failed cut entries must remain moves, not silently become copies"
    );
    assert!(!blocked.exists());
    assert_eq!(fs::read(collision).unwrap(), b"blocked source");
    assert_eq!(
        app.world()
            .get_resource::<FileTreePreferences>()
            .unwrap()
            .roots[&fixture.0]
            .hidden_paths,
        BTreeSet::from([
            PathBuf::from("destination/good.txt"),
            PathBuf::from("destination/blocked.txt")
        ])
    );
    assert_eq!(
        clipboard.read_files().unwrap_err().kind(),
        std::io::ErrorKind::NotFound,
        "A fully completed cut clears its owned file clipboard"
    );
    finish(&mut app);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn closing_an_originating_panel_does_not_cancel_admitted_work_or_the_shared_worker() {
    let fixture = Fixture::new();
    // A nontrivial copy exercises admission/closure without a mock worker or a shell.
    let source = fixture.file("large.bin", vec![0x5a; 16 * 1024 * 1024]);
    let (mut app, workspace) = host();
    let origin = panel(&mut app, workspace, Some(&fixture.0));
    let survivor = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, origin, &fixture.0);
    ready(&mut app, survivor, &fixture.0);
    let worker_id = app.world().get_resource::<ManagedWork>().unwrap().list()[0].id;
    assert_eq!(filetree_operation_count(app.world()), 0);
    queue(
        &mut app,
        origin,
        Action::Duplicate {
            paths: vec![source.clone()],
        },
    );
    assert_eq!(
        filetree_operation_count(app.world()),
        0,
        "A queued UI intention is not yet admitted work"
    );
    app.update(); // admit the operation before closing its UI owner
    assert!(
        tree(&app, origin).is_busy(),
        "The close must precede observation of this copy's completion"
    );
    assert_eq!(filetree_operation_count(app.world()), 1);
    Workbench::new(app.world_mut()).close_panel(origin).unwrap();
    assert!(app.world().get_entity(origin).is_err());
    assert_eq!(
        filetree_operation_count(app.world()),
        1,
        "Quit preflight must still see work from closed panels"
    );
    // Leave the World unpumped until the real worker commits its destination.
    // A completed disk effect is still admitted work until Update reconciles it.
    let deadline = Instant::now() + Duration::from_secs(10);
    let committed = loop {
        assert_eq!(filetree_operation_count(app.world()), 1);
        let copied = fs::read_dir(&fixture.0)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path != &source && path.is_file());
        if let Some(path) = copied {
            break path;
        }
        assert!(
            Instant::now() < deadline,
            "Closed panel's admitted copy did not commit"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(fs::metadata(&committed).unwrap().len(), 16 * 1024 * 1024);
    assert_eq!(filetree_operation_count(app.world()), 1);
    let result = completion(&mut app, origin);
    assert_eq!(
        filetree_operation_count(app.world()),
        0,
        "Only reconciliation retires admitted operations"
    );

    assert_eq!(result.root, fixture.0);
    assert!(!result.result.cancelled);
    assert!(result.result.errors.is_empty());
    let [FileTreeChange::Copied { destination, .. }] = result.result.completed.as_slice() else {
        panic!("Closed panel's operation must still report its committed result");
    };
    assert_eq!(destination, &committed);
    assert_eq!(fs::metadata(destination).unwrap().len(), 16 * 1024 * 1024);
    eventually(&mut app, |app| visible(app, survivor, destination));
    Workbench::new(app.world_mut())
        .close_panel(survivor)
        .unwrap();
    app.update();
    let work = app.world().get_resource::<ManagedWork>().unwrap().list();
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].id, worker_id);
    assert_eq!(work[0].status, WorkStatus::Running);
    let reopened = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, reopened, &fixture.0);
    assert!(visible(&app, reopened, destination));
    assert_eq!(
        app.world().get_resource::<ManagedWork>().unwrap().list()[0].id,
        worker_id
    );
    finish(&mut app);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn workspace_shutdown_drains_every_admitted_outcome_and_leaves_only_reported_commits() {
    let fixture = Fixture::new();
    let source = fixture.file("source.bin", vec![0x33; 8 * 1024 * 1024]);
    let (mut app, workspace) = host();
    let a = panel(&mut app, workspace, Some(&fixture.0));
    let b = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, a, &fixture.0);
    ready(&mut app, b, &fixture.0);
    for panel in [a, b] {
        queue(
            &mut app,
            panel,
            Action::Duplicate {
                paths: vec![source.clone()],
            },
        );
    }
    app.update();
    shutdown_work(app.world_mut()).unwrap();
    let mut outcomes = Vec::new();
    eventually(&mut app, |app| {
        outcomes.extend(drain::<FileTreeOperationFinished>(app));
        outcomes.len() == 2 && work_finished(app.world_mut())
    });
    assert_eq!(
        outcomes
            .iter()
            .map(|event| event.panel)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([a, b])
    );
    let mut expected_entries = BTreeSet::from([source.clone()]);
    for event in outcomes {
        assert!(
            event.result.errors.is_empty(),
            "Cancellation is not an invented I/O failure"
        );
        assert!(event.result.cancelled || !event.result.completed.is_empty());
        for change in &event.result.completed {
            let FileTreeChange::Copied { destination, .. } = change else {
                panic!("Expected a copy result");
            };
            assert_eq!(fs::read(destination).unwrap(), fs::read(&source).unwrap());
            expected_entries.insert(destination.clone());
        }
    }
    let actual_entries: BTreeSet<_> = fs::read_dir(&fixture.0)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(
        actual_entries, expected_entries,
        "No unreported destination or abandoned copy staging may survive shutdown"
    );
    app.update();
    assert!(
        app.world()
            .get_resource::<ManagedWork>()
            .unwrap()
            .list()
            .is_empty()
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn replacing_the_clipboard_service_cannot_reuse_a_pending_cut_revision() {
    let fixture = Fixture::new();
    let source = fixture.file("source.txt", b"preserve the source");
    let destination = fixture.directory("destination");
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, panel, &fixture.0);
    queue(
        &mut app,
        panel,
        Action::Cut {
            paths: vec![source.clone()],
        },
    );
    app.update();
    let revision = app.world().resource::<Clipboard>().revision();
    let mut replacement = Clipboard::memory();
    replacement.write_files(&[source.clone()]).unwrap();
    assert_eq!(replacement.revision(), revision);
    app.insert_resource(replacement);
    let result = operate(
        &mut app,
        panel,
        Action::Paste {
            directory: destination.clone(),
        },
    );
    assert!(result.result.errors.is_empty());
    assert!(matches!(
        result.result.completed.as_slice(),
        [FileTreeChange::Copied { .. }]
    ));
    assert!(
        source.exists(),
        "A new clipboard service does not own the old Cut"
    );
    assert_eq!(
        fs::read(destination.join("source.txt")).unwrap(),
        b"preserve the source"
    );
    finish(&mut app);
}

#[test]
fn a_drop_during_inline_rename_cannot_consume_the_unsubmitted_draft() {
    let fixture = Fixture::new();
    let external = Fixture::new();
    let selected = fixture.file("selected.txt", b"selected");
    let incoming = external.file("incoming.txt", b"incoming");
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, panel, &fixture.0);
    queue(
        &mut app,
        panel,
        Action::Rename {
            path: selected.clone(),
        },
    );
    app.update();
    edit_name(&mut app, panel, "unfinished-name.txt");
    queue(
        &mut app,
        panel,
        Action::Transfer {
            sources: vec![incoming.clone()],
            directory: fixture.0.clone(),
            copy: true,
        },
    );
    app.update();
    assert_eq!(
        tree(&app, panel).editing.as_ref().unwrap().text,
        "unfinished-name.txt"
    );
    assert!(!tree(&app, panel).is_busy());
    assert!(
        tree(&app, panel).error().is_none(),
        "Invalid drag gestures are silent"
    );
    assert!(selected.exists() && incoming.exists());
    assert!(!fixture.path("incoming.txt").exists());
    assert!(drain::<FileTreeOperationFinished>(&mut app).is_empty());
    finish(&mut app);
}

#[test]
fn root_collapse_preserves_expansion_and_retires_invisible_selection_without_a_new_generation() {
    let fixture = Fixture::new();
    let folder = fixture.directory("folder");
    let child = fixture.file("folder/child.txt", b"child");
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, panel, &fixture.0);
    expand(&mut app, panel, &folder);
    eventually(&mut app, |app| visible(app, panel, &child));
    let generation = tree(&app, panel).generation;
    select(&mut app, panel, &child);
    expand(&mut app, panel, &fixture.0);
    eventually(&mut app, |app| tree(app, panel).snapshot.rows.len() == 1);
    assert!(!tree(&app, panel).root_expanded);
    assert!(tree(&app, panel).selection.is_empty());
    assert_eq!(
        tree(&app, panel).cursor.as_deref(),
        Some(fixture.0.as_path())
    );
    expand(&mut app, panel, &fixture.0);
    eventually(&mut app, |app| visible(app, panel, &child));
    assert_eq!(tree(&app, panel).generation, generation);
    assert!(tree(&app, panel).expanded.contains(&folder));
    finish(&mut app);
}

#[test]
fn read_only_admission_blocks_filesystem_mutations_but_keeps_copy_open_and_visibility() {
    let fixture = Fixture::new();
    let source = fixture.file("source.txt", b"source");
    let destination = fixture.directory("destination");
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, Some(&fixture.0));
    let settings = FileTreeSettings {
        read_only: true,
        indent_size: 5.0,
    };
    app.world_mut().entity_mut(panel).insert(settings);
    ready(&mut app, panel, &fixture.0);
    assert_eq!(
        *app.world().get::<FileTreeSettings>(panel).unwrap(),
        settings
    );
    for action in [
        Action::NewFile {
            directory: destination.clone(),
        },
        Action::NewFolder {
            directory: destination.clone(),
        },
        Action::Rename {
            path: source.clone(),
        },
        Action::SubmitEdit,
        Action::Cut {
            paths: vec![source.clone()],
        },
        Action::Paste {
            directory: destination.clone(),
        },
        Action::Duplicate {
            paths: vec![source.clone()],
        },
        Action::Trash {
            paths: vec![source.clone()],
        },
        Action::Transfer {
            sources: vec![source.clone()],
            directory: destination.clone(),
            copy: true,
        },
    ] {
        queue(&mut app, panel, action);
        app.update();
    }
    assert_eq!(filetree_operation_count(app.world()), 0);
    assert!(tree(&app, panel).error().is_none());
    assert!(tree(&app, panel).editing.is_none());
    assert!(source.exists() && fs::read_dir(&destination).unwrap().next().is_none());
    assert_eq!(app.world().resource::<Clipboard>().revision(), 0);
    queue(
        &mut app,
        panel,
        Action::Copy {
            paths: vec![source.clone()],
        },
    );
    queue(
        &mut app,
        panel,
        Action::Open {
            path: source.clone(),
            activation: FileTreeActivation::Keyboard,
        },
    );
    queue(
        &mut app,
        panel,
        Action::Hide {
            paths: vec![source.clone()],
        },
    );
    app.update();
    assert_eq!(
        app.world()
            .resource::<Clipboard>()
            .clone()
            .read_files()
            .unwrap(),
        [source.clone()]
    );
    let opens = drain::<FileTreeOpenRequested>(&mut app);
    assert_eq!(opens.len(), 1);
    assert_eq!(opens[0].path, source);
    assert!(
        app.world()
            .get_resource::<FileTreePreferences>()
            .unwrap()
            .roots[&fixture.0]
            .hidden_paths
            .contains(Path::new("source.txt"))
    );
    finish(&mut app);
}

#[test]
fn invalid_drop_self_descendant_and_same_parent_are_silent_and_admit_no_work() {
    let fixture = Fixture::new();
    let folder = fixture.directory("folder");
    let descendant = fixture.directory("folder/nested");
    let file = fixture.file("file.txt", b"unchanged");
    let (mut app, workspace) = host();
    let panel = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, panel, &fixture.0);
    for action in [
        Action::Transfer {
            sources: vec![folder.clone()],
            directory: folder.clone(),
            copy: false,
        },
        Action::Transfer {
            sources: vec![folder.clone()],
            directory: descendant,
            copy: true,
        },
        Action::Transfer {
            sources: vec![file.clone()],
            directory: fixture.0.clone(),
            copy: false,
        },
    ] {
        queue(&mut app, panel, action);
        app.update();
    }
    assert_eq!(filetree_operation_count(app.world()), 0);
    assert!(tree(&app, panel).error().is_none());
    assert!(file.exists() && folder.exists());
    assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 2);
    finish(&mut app);
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn owned_clipboard_transfers_selected_symlinks_not_their_canonicalized_targets() {
    use std::os::unix::fs::symlink;
    for is_cut in [false, true] {
        let fixture = Fixture::new();
        let external = Fixture::new();
        let target = external.file("target.txt", b"never move this target");
        let selected = fixture.path("selected-link");
        symlink(&target, &selected).unwrap();
        let destination = fixture.directory("destination");
        let (mut app, workspace) = host();
        let panel = panel(&mut app, workspace, Some(&fixture.0));
        ready(&mut app, panel, &fixture.0);
        queue(
            &mut app,
            panel,
            if is_cut {
                Action::Cut {
                    paths: vec![selected.clone()],
                }
            } else {
                Action::Copy {
                    paths: vec![selected.clone()],
                }
            },
        );
        app.update();
        // Model the real macOS arboard boundary: its successful file-list write
        // round-trips canonical paths, while the selected entry remains the link.
        let mut clipboard = app.world().resource::<Clipboard>().clone();
        clipboard.write_files(&[target.clone()]).unwrap();
        let snapshot = Arc::new(clipboard.read_files_snapshot().unwrap());
        let mut cuts = app.world_mut().resource_mut::<FileTreeClipboard>();
        let cut = cuts.0.as_mut().unwrap();
        cut.clipboard_snapshot = snapshot;
        cut.clipboard_revision = clipboard.revision();
        drop(cuts);
        let result = operate(
            &mut app,
            panel,
            Action::Paste {
                directory: destination.clone(),
            },
        );
        assert!(result.result.errors.is_empty());
        assert!(match (is_cut, result.result.completed.as_slice()) {
            (true, [FileTreeChange::Moved { source, .. }])
            | (false, [FileTreeChange::Copied { source, .. }]) => source == &selected,
            _ => false,
        });
        assert_eq!(selected.exists(), !is_cut);
        assert!(
            fs::symlink_metadata(destination.join("selected-link"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&target).unwrap(), b"never move this target");
        finish(&mut app);
    }
}

#[test]
fn explicit_viewer_choices_are_validated_and_keep_the_origin_workspace() {
    let fixture = Fixture::new();
    let file = fixture.file("picture.svg", b"image");
    let (mut app, workspace) = host();
    app.insert_resource(FileTreeOpenWith(|_| {
        vec![FileTreeOpenChoice {
            id: "image".into(),
            label: "Image".into(),
        }]
    }));
    let panel = panel(&mut app, workspace, Some(&fixture.0));
    ready(&mut app, panel, &fixture.0);
    queue(
        &mut app,
        panel,
        Action::OpenWith {
            path: file.clone(),
            choice: "unknown".into(),
        },
    );
    app.update();
    assert!(drain::<FileTreeOpenWithRequested>(&mut app).is_empty());
    queue(
        &mut app,
        panel,
        Action::OpenWith {
            path: file.clone(),
            choice: "image".into(),
        },
    );
    let other = crate::test_layout(app.world_mut());
    Workbench::new(app.world_mut())
        .activate_layout(other)
        .unwrap();
    app.update();
    let messages = drain::<FileTreeOpenWithRequested>(&mut app);
    assert_eq!(messages.len(), 1);

    assert_eq!(messages[0].path, file);
    assert_eq!(messages[0].choice, "image");
    queue(
        &mut app,
        panel,
        Action::OpenWith {
            path: fixture.0.join("missing.svg"),
            choice: "image".into(),
        },
    );
    app.update();
    assert!(drain::<FileTreeOpenWithRequested>(&mut app).is_empty());
}
