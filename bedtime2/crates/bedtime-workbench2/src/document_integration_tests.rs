use crate::*;
use std::{
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

const DEADLINE: Duration = Duration::from_secs(10);

struct TextViewer;
impl Panel for TextViewer {
    const KEY: &'static str = "document-tests.text-viewer";
    const LABEL: &'static str = "Text Viewer";

    #[cfg(feature = "ui")]
    fn draw(
        _: &mut bedtime_ui2::egui::Ui,
        _: &mut World,
        _: &bedtime_ui2::UiContext<'_>,
        _: Entity,
    ) {
    }
}

fn app() -> (App, WorkspaceId) {
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin)
        .register_panel_type::<TextViewer>();
    let workspace = Workspaces::new(app.world_mut()).open("Documents");
    (app, workspace)
}

fn loaded_text(world: &mut World, workspace: WorkspaceId) -> DocumentHandle<String> {
    open_document(
        world,
        workspace.entity(),
        "base".to_owned(),
        DocumentOptions::loaded("Text"),
    )
    .unwrap()
}

fn view_in_area(
    world: &mut World,
    workspace: WorkspaceId,
    area: Entity,
    document: DocumentHandle<String>,
) -> Entity {
    let kind = world
        .query::<(Entity, &PanelType)>()
        .iter(world)
        .find_map(|(entity, kind)| (kind.key == TextViewer::KEY).then_some(entity))
        .unwrap();
    let panel = Workbench::with_workspace(world, workspace)
        .unwrap()
        .open_panel(kind, area)
        .unwrap();
    bind_document_view(world, panel, document).unwrap();
    panel
}

fn text_view(world: &mut World, workspace: WorkspaceId) -> (DocumentHandle<String>, Entity) {
    let document = loaded_text(world, workspace);
    let area = {
        let workbench = Workbench::with_workspace(world, workspace).unwrap();
        workbench
            .destination_area(workbench.active_layout())
            .unwrap()
    };
    (document, view_in_area(world, workspace, area, document))
}

fn replace_text(world: &mut World, document: DocumentHandle<String>, text: &str) {
    let revision = inspect_document(world, document.id()).unwrap().revision;
    let len = read_document(world, document).unwrap().len();
    edit_document(
        world,
        document,
        revision,
        TextChange {
            range: 0..len,
            replacement: text.into(),
        },
        EditOptions::new("Replace text"),
    )
    .unwrap();
}

/// A conflict with checkpoint-equal local content exercises the conflict gate
/// independently of the dirty-content gate.
fn make_clean_conflict(world: &mut World, document: DocumentHandle<String>) {
    replace_text(world, document, "local");
    let generation = inspect_document(world, document.id())
        .unwrap()
        .source_generation;
    assert_eq!(
        offer_document_external_update(
            world,
            document,
            generation,
            "incoming".into(),
            Some("incoming-version".into()),
        )
        .unwrap(),
        DocumentExternalUpdate::Conflict,
    );
    undo_document(world, document.id()).unwrap();
    let info = inspect_document(world, document.id()).unwrap();
    assert!(info.conflict);
    assert!(!info.dirty);
}

/// Storage captures geometry, tab order and selections; the adjacent IDs ensure
/// an equal-looking replacement graph cannot pass an atomic-rejection assertion.
#[derive(Debug, PartialEq, Eq)]
struct GraphState {
    snapshot: WorkspaceSnapshot,
    entities: Vec<(Entity, Vec<(Entity, Vec<Entity>)>)>,
    active: bool,
}

fn graph(world: &World, workspace: WorkspaceId) -> GraphState {
    let entities = world
        .get::<WorkspaceMembers>(workspace.entity())
        .unwrap()
        .entities()
        .iter()
        .copied()
        .filter(|&entity| world.get::<Layout>(entity).is_some())
        .map(|layout| {
            let areas = world
                .get::<LayoutAreas>(layout)
                .unwrap()
                .entities()
                .iter()
                .map(|&area| (area, panels_in_area(world, area).to_vec()))
                .collect();
            (layout, areas)
        })
        .collect();
    GraphState {
        snapshot: capture_workspace(world, workspace.entity()).unwrap(),
        entities,
        active: world.get::<ActiveWorkspace>(workspace.entity()).is_some(),
    }
}

fn assert_open(world: &mut World, workspace: WorkspaceId) {
    let entry = Workspaces::new(world)
        .list()
        .into_iter()
        .find(|entry| entry.id == workspace)
        .unwrap();
    assert!(!entry.closing);
}

fn wait_for(description: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + DEADLINE;
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "Timed out waiting for {description}"
        );
        thread::yield_now();
    }
}

fn held_task(
    world: &mut World,
    workspace: WorkspaceId,
    owner: Option<Entity>,
) -> (WorkspaceTask<()>, mpsc::Sender<()>) {
    let (release, receive) = mpsc::channel();
    let task = Workbench::with_workspace(world, workspace)
        .unwrap()
        .work()
        .spawn_task(owner, "Keep working during failed preflight", move |_| {
            // Disconnection also releases the worker if the test fails.
            let _ = receive.recv_timeout(DEADLINE);
        })
        .unwrap();
    (task, release)
}

fn finish_task(task: &WorkspaceTask<()>, release: mpsc::Sender<()>) {
    release.send(()).unwrap();
    wait_for("uncancelled task completion", || {
        task.status() == WorkStatus::Finished
    });
}

fn controlled_save(
    world: &mut World,
    document: DocumentHandle<String>,
) -> (
    mpsc::Sender<()>,
    mpsc::Receiver<DocumentSaveSnapshot<String>>,
) {
    let (release, receive) = mpsc::channel();
    let receive = Mutex::new(receive);
    let (send_snapshot, snapshots) = mpsc::channel();
    bind_document_save(
        world,
        document,
        DocumentSaveBinding::new(None, move |snapshot| {
            send_snapshot.send(snapshot).unwrap();
            receive.lock().unwrap().recv_timeout(DEADLINE).unwrap();
            Ok(Some("saved-version".into()))
        }),
    )
    .unwrap();
    (release, snapshots)
}

fn finish_save(world: &mut World, document: DocumentId) -> DocumentSaveFinished {
    let mut result = None;
    wait_for("document save acknowledgement", || {
        result = poll_document_saves(world)
            .into_iter()
            .find(|completion| completion.document == document);
        result.is_some()
    });
    result.unwrap()
}

fn wait_for_retirement(app: &mut App, workspace: WorkspaceId) {
    wait_for("workspace retirement", || {
        app.update();
        app.world().get::<Workspace>(workspace.entity()).is_none()
    });
}

#[test]
fn close_preflights_dirty_and_clean_conflicted_documents_before_switching_or_stopping_work() {
    for conflict in [false, true] {
        let (mut app, workspace) = app();
        let (document, panel) = text_view(app.world_mut(), workspace);
        if conflict {
            make_clean_conflict(app.world_mut(), document);
        } else {
            replace_text(app.world_mut(), document, "local");
        }
        let other = Workspaces::new(app.world_mut()).open("Replacement");
        reconcile_workbench(app.world_mut()).unwrap();
        let before = graph(app.world(), workspace);
        let other_before = graph(app.world(), other);
        let revision = inspect_document(app.world(), document.id())
            .unwrap()
            .revision;
        let (task, release) = held_task(app.world_mut(), workspace, Some(panel));

        assert!(
            Workspaces::new(app.world_mut())
                .close(workspace, Some(other))
                .is_err()
        );

        assert_eq!(Workspaces::new(app.world_mut()).selected(), Some(workspace));
        assert_eq!(graph(app.world(), workspace), before);
        assert_eq!(graph(app.world(), other), other_before);
        assert_eq!(task.status(), WorkStatus::Running);
        assert_open(app.world_mut(), workspace);
        let info = inspect_document(app.world(), document.id()).unwrap();
        assert_eq!(info.revision, revision);
        assert_eq!(info.conflict, conflict);
        assert_eq!(info.dirty, !conflict);
        assert_eq!(
            read_document(app.world(), document).unwrap(),
            if conflict { "base" } else { "local" }
        );
        assert_eq!(
            app.world().get::<DocumentView>(panel).unwrap().document(),
            document.id()
        );
        finish_task(&task, release);
        app.update();
        assert!(app.world().get::<Workspace>(workspace.entity()).is_some());
    }
}

#[test]
fn close_and_explicit_discard_both_preserve_a_workspace_with_viewer_drafts() {
    let (mut app, workspace) = app();
    let (document, panel) = text_view(app.world_mut(), workspace);
    app.world_mut().entity_mut(panel).insert(DocumentDraft);
    let other = Workspaces::new(app.world_mut()).open("Replacement");
    reconcile_workbench(app.world_mut()).unwrap();
    let before = graph(app.world(), workspace);
    let other_before = graph(app.world(), other);
    let (task, release) = held_task(app.world_mut(), workspace, Some(panel));

    for discard in [false, true] {
        let result = if discard {
            Workspaces::new(app.world_mut()).discard_and_close(workspace, Some(other))
        } else {
            Workspaces::new(app.world_mut()).close(workspace, Some(other))
        };
        assert!(result.is_err());
        assert_eq!(Workspaces::new(app.world_mut()).selected(), Some(workspace));
        assert_eq!(graph(app.world(), workspace), before);
        assert_eq!(graph(app.world(), other), other_before);
        assert_eq!(task.status(), WorkStatus::Running);
        assert_open(app.world_mut(), workspace);
        assert!(
            inspect_document(app.world(), document.id())
                .unwrap()
                .has_drafts
        );
        assert!(!inspect_document(app.world(), document.id()).unwrap().dirty);
        assert!(app.world().get::<DocumentDraft>(panel).is_some());
    }

    finish_task(&task, release);
    app.world_mut().entity_mut(panel).remove::<DocumentDraft>();
    Workspaces::new(app.world_mut())
        .close(workspace, Some(other))
        .unwrap();
    wait_for_retirement(&mut app, workspace);
    assert_eq!(Workspaces::new(app.world_mut()).selected(), Some(other));
}

#[test]
fn pending_saves_block_close_discard_and_shutdown_without_cancelling_any_work() {
    let (mut app, workspace) = app();
    let (document, panel) = text_view(app.world_mut(), workspace);
    replace_text(app.world_mut(), document, "local");
    let other = Workspaces::new(app.world_mut()).open("Replacement");
    let (task, release_task) = held_task(app.world_mut(), workspace, Some(panel));
    let (other_task, release_other) = held_task(app.world_mut(), other, None);
    let (release_save, snapshots) = controlled_save(app.world_mut(), document);
    let save = save_document(app.world_mut(), document.id()).unwrap();
    let capture = snapshots.recv_timeout(DEADLINE).unwrap();
    assert_eq!(capture.document, document.id());
    assert_eq!(capture.model, "local");
    reconcile_workbench(app.world_mut()).unwrap();
    let before = graph(app.world(), workspace);
    let other_before = graph(app.world(), other);

    for operation in 0..3 {
        let result = match operation {
            0 => Workspaces::new(app.world_mut()).close(workspace, Some(other)),
            1 => Workspaces::new(app.world_mut()).discard_and_close(workspace, Some(other)),
            _ => Workspaces::new(app.world_mut()).shutdown_work(),
        };
        assert!(result.is_err());
        assert_eq!(graph(app.world(), workspace), before);
        assert_eq!(graph(app.world(), other), other_before);
        assert_eq!(Workspaces::new(app.world_mut()).selected(), Some(workspace));
        assert_open(app.world_mut(), workspace);
        assert_open(app.world_mut(), other);
        assert_eq!(task.status(), WorkStatus::Running);
        assert_eq!(other_task.status(), WorkStatus::Running);
        let work = app
            .world()
            .get::<WorkspaceWork>(workspace.entity())
            .unwrap()
            .list();
        assert_eq!(
            work.iter().find(|work| work.id == save).unwrap().status,
            WorkStatus::Running
        );
        assert!(inspect_document(app.world(), document.id()).unwrap().saving);
    }

    release_save.send(()).unwrap();
    let completion = finish_save(app.world_mut(), document.id());
    assert_eq!(completion.work, save);
    assert_eq!(completion.result, Ok(()));
    assert!(!inspect_document(app.world(), document.id()).unwrap().dirty);
    finish_task(&task, release_task);
    finish_task(&other_task, release_other);
    Workspaces::new(app.world_mut())
        .close(workspace, Some(other))
        .unwrap();
    wait_for_retirement(&mut app, workspace);
    assert!(app.world().get_entity(panel).is_err());
    assert!(app.world().get_entity(document.id().entity()).is_err());
    assert_eq!(
        graph(app.world(), other),
        GraphState {
            active: true,
            ..other_before
        }
    );
}

#[test]
fn shutdown_preflights_drafts_in_all_workspaces_before_stopping_any_workers() {
    let (mut app, first) = app();
    let second = Workspaces::new(app.world_mut()).open("Draft owner");
    let (document, panel) = text_view(app.world_mut(), second);
    app.world_mut().entity_mut(panel).insert(DocumentDraft);
    reconcile_workbench(app.world_mut()).unwrap();
    let first_before = graph(app.world(), first);
    let second_before = graph(app.world(), second);
    let (first_task, release_first) = held_task(app.world_mut(), first, None);
    let (second_task, release_second) = held_task(app.world_mut(), second, Some(panel));

    assert!(Workspaces::new(app.world_mut()).shutdown_work().is_err());
    assert_eq!(first_task.status(), WorkStatus::Running);
    assert_eq!(second_task.status(), WorkStatus::Running);
    assert_eq!(graph(app.world(), first), first_before);
    assert_eq!(graph(app.world(), second), second_before);
    assert_open(app.world_mut(), first);
    assert_open(app.world_mut(), second);

    app.world_mut().entity_mut(panel).remove::<DocumentDraft>();
    replace_text(
        app.world_mut(),
        document,
        "unsaved but explicitly allowed at shutdown",
    );
    Workspaces::new(app.world_mut()).shutdown_work().unwrap();
    assert_eq!(first_task.status(), WorkStatus::Cancelling);
    assert_eq!(second_task.status(), WorkStatus::Cancelling);
    release_first.send(()).unwrap();
    release_second.send(()).unwrap();
    wait_for("workspace workers to stop", || {
        Workspaces::new(app.world_mut()).work_finished()
    });
    app.update();
    assert!(app.world().get::<Workspace>(first.entity()).is_some());
    assert!(app.world().get::<Workspace>(second.entity()).is_some());
    assert!(app.world().get::<DocumentView>(panel).is_some());
    assert_eq!(
        read_document(app.world(), document).unwrap(),
        "unsaved but explicitly allowed at shutdown"
    );
}

#[test]
fn explicit_discard_retires_dirty_or_clean_conflicted_workspaces_and_their_documents() {
    for conflict in [false, true] {
        let (mut app, workspace) = app();
        let (document, panel) = text_view(app.world_mut(), workspace);
        if conflict {
            make_clean_conflict(app.world_mut(), document);
        } else {
            replace_text(app.world_mut(), document, "local");
        }
        let layout = active_layout(app.world(), workspace.entity()).unwrap();
        let other = Workspaces::new(app.world_mut()).open("Replacement");
        let other_before = graph(app.world(), other);

        Workspaces::new(app.world_mut())
            .discard_and_close(workspace, Some(other))
            .unwrap();

        assert_eq!(Workspaces::new(app.world_mut()).selected(), Some(other));
        wait_for_retirement(&mut app, workspace);
        for retired in [layout, panel, document.id().entity()] {
            assert!(app.world().get_entity(retired).is_err());
        }
        assert_eq!(
            graph(app.world(), other),
            GraphState {
                active: true,
                ..other_before
            }
        );
    }
}

#[derive(Component, Clone, Copy)]
enum InitializeAction {
    Edit,
    Save,
}

struct InitializingViewer;
impl Panel for InitializingViewer {
    const KEY: &'static str = "document-tests.initializing-viewer";
    const LABEL: &'static str = "Initializing Viewer";

    fn initialize(entity: &mut EntityWorldMut<'_>) {
        let action = *entity.get::<InitializeAction>().unwrap();
        let document = entity.get::<DocumentView>().unwrap().document();
        entity.world_scope(|world| match action {
            InitializeAction::Edit => {
                let document = cast_document::<String>(world, document).unwrap();
                replace_text(world, document, "edit accepted during initialization");
            }
            InitializeAction::Save => {
                save_document(world, document).unwrap();
            }
        });
    }

    #[cfg(feature = "ui")]
    fn draw(
        _: &mut bedtime_ui2::egui::Ui,
        _: &mut World,
        _: &bedtime_ui2::UiContext<'_>,
        _: Entity,
    ) {
    }
}

fn uninitialized_view(
    world: &mut World,
    area: Entity,
    document: DocumentHandle<String>,
    action: InitializeAction,
) -> Entity {
    let panel = world
        .spawn((
            PanelInstance::new(InitializingViewer::KEY),
            InArea(area),
            action,
        ))
        .id();
    bind_document_view(world, panel, document).unwrap();
    panel
}

#[test]
fn retirement_checks_edits_accepted_by_pending_panel_initialization_before_destroying_anything() {
    for retire_document in [false, true] {
        let (mut app, workspace) = app();
        app.register_panel_type::<InitializingViewer>();
        let (document, original) = text_view(app.world_mut(), workspace);
        let other = Workspaces::new(app.world_mut()).open("Replacement");
        let area = app.world().get::<InArea>(original).unwrap().0;
        let pending = uninitialized_view(app.world_mut(), area, document, InitializeAction::Edit);
        let (task, release) = held_task(app.world_mut(), workspace, Some(original));
        assert!(!inspect_document(app.world(), document.id()).unwrap().dirty);

        let result = if retire_document {
            close_document(app.world_mut(), document.id()).map_err(|error| error.to_string())
        } else {
            Workspaces::new(app.world_mut()).close(workspace, Some(other))
        };

        assert!(result.is_err());
        assert_eq!(Workspaces::new(app.world_mut()).selected(), Some(workspace));
        assert_open(app.world_mut(), workspace);
        assert_eq!(task.status(), WorkStatus::Running);
        assert_eq!(
            read_document(app.world(), document).unwrap(),
            "edit accepted during initialization"
        );
        let info = inspect_document(app.world(), document.id()).unwrap();
        assert!(info.dirty && info.can_undo);
        assert_eq!(info.revision, 1);
        for panel in [original, pending] {
            assert_eq!(
                app.world().get::<DocumentView>(panel).unwrap().document(),
                document.id()
            );
            assert_eq!(app.world().get::<InArea>(panel).unwrap().0, area);
        }
        finish_task(&task, release);
        app.update();
        assert!(app.world().get::<Workspace>(workspace.entity()).is_some());
        assert_eq!(
            inspect_document(app.world(), document.id())
                .unwrap()
                .revision,
            1
        );
    }
}

#[test]
fn workspace_exit_preflight_observes_saves_started_by_pending_panel_initialization() {
    for operation in 0..3 {
        let (mut app, workspace) = app();
        app.register_panel_type::<InitializingViewer>();
        let (document, original) = text_view(app.world_mut(), workspace);
        let other = Workspaces::new(app.world_mut()).open("Replacement");
        let (release_save, snapshots) = controlled_save(app.world_mut(), document);
        let area = app.world().get::<InArea>(original).unwrap().0;
        let pending = uninitialized_view(app.world_mut(), area, document, InitializeAction::Save);
        let (task, release_task) = held_task(app.world_mut(), workspace, Some(original));
        assert!(!inspect_document(app.world(), document.id()).unwrap().saving);

        let result = match operation {
            0 => Workspaces::new(app.world_mut()).close(workspace, Some(other)),
            1 => Workspaces::new(app.world_mut()).discard_and_close(workspace, Some(other)),
            _ => Workspaces::new(app.world_mut()).shutdown_work(),
        };

        assert!(result.is_err());
        assert_eq!(Workspaces::new(app.world_mut()).selected(), Some(workspace));
        assert_open(app.world_mut(), workspace);
        assert_eq!(task.status(), WorkStatus::Running);
        assert!(inspect_document(app.world(), document.id()).unwrap().saving);
        assert_eq!(
            app.world().get::<DocumentView>(pending).unwrap().document(),
            document.id()
        );
        let capture = snapshots.recv_timeout(DEADLINE).unwrap();
        assert_eq!(capture.document, document.id());
        assert_eq!(capture.model, "base");
        release_save.send(()).unwrap();
        assert_eq!(finish_save(app.world_mut(), document.id()).result, Ok(()));
        finish_task(&task, release_task);
        app.update();
        assert!(app.world().get::<Workspace>(workspace.entity()).is_some());
    }
}

struct SplitScene {
    app: App,
    workspace: WorkspaceId,
    layout: Entity,
    target: Entity,
    source: Entity,
    first: Entity,
    draft: Entity,
    survivor: Entity,
    document: DocumentHandle<String>,
}

fn split_scene() -> SplitScene {
    let (mut app, workspace) = app();
    let document = loaded_text(app.world_mut(), workspace);
    replace_text(app.world_mut(), document, "local");
    let (layout, target, source) = {
        let mut workbench = Workbench::with_workspace(app.world_mut(), workspace).unwrap();
        let layout = workbench.active_layout();
        let target = workbench.destination_area(layout).unwrap();
        let source = workbench
            .split_area(target, SplitAxis::Horizontal, LAYOUT_EXTENT / 2, [1, 1])
            .unwrap();
        (layout, target, source)
    };
    let first = view_in_area(app.world_mut(), workspace, target, document);
    let draft = view_in_area(app.world_mut(), workspace, target, document);
    let survivor = view_in_area(app.world_mut(), workspace, source, document);
    Workbench::with_workspace(app.world_mut(), workspace)
        .unwrap()
        .focus_panel(draft)
        .unwrap();
    app.world_mut().entity_mut(draft).insert(DocumentDraft);
    reconcile_workbench(app.world_mut()).unwrap();
    SplitScene {
        app,
        workspace,
        layout,
        target,
        source,
        first,
        draft,
        survivor,
        document,
    }
}

fn assert_draft_scene_unchanged(scene: &SplitScene, before: &GraphState) {
    assert_eq!(&graph(scene.app.world(), scene.workspace), before);
    assert_eq!(
        read_document(scene.app.world(), scene.document).unwrap(),
        "local"
    );
    let info = inspect_document(scene.app.world(), scene.document.id()).unwrap();
    assert_eq!(info.revision, 1);
    assert!(info.dirty && info.undo_label.is_some() && info.has_drafts);
    assert!(
        !info.can_undo,
        "Uncommitted drafts temporarily block document undo"
    );
    assert!(
        scene
            .app
            .world()
            .get::<DocumentDraft>(scene.draft)
            .is_some()
    );
    for panel in [scene.first, scene.draft, scene.survivor] {
        assert_eq!(
            scene
                .app
                .world()
                .get::<DocumentView>(panel)
                .unwrap()
                .document(),
            scene.document.id()
        );
        assert_eq!(
            scene.app.world().get::<InWorkspace>(panel).unwrap().0,
            scene.workspace.entity()
        );
    }
}

#[test]
fn closing_the_last_view_requires_resolving_its_draft_but_retains_document_history() {
    let (mut app, workspace) = app();
    let (document, panel) = text_view(app.world_mut(), workspace);
    replace_text(app.world_mut(), document, "local");
    app.world_mut().entity_mut(panel).insert(DocumentDraft);
    reconcile_workbench(app.world_mut()).unwrap();
    let before = graph(app.world(), workspace);

    assert!(
        Workbench::with_workspace(app.world_mut(), workspace)
            .unwrap()
            .close_panel(panel)
            .is_err()
    );
    assert_eq!(graph(app.world(), workspace), before);
    assert!(app.world().get::<DocumentDraft>(panel).is_some());
    assert_eq!(read_document(app.world(), document).unwrap(), "local");

    app.world_mut().entity_mut(panel).remove::<DocumentDraft>();
    Workbench::with_workspace(app.world_mut(), workspace)
        .unwrap()
        .close_panel(panel)
        .unwrap();
    app.update();
    assert!(app.world().get_entity(panel).is_err());
    assert_eq!(
        document_ids(app.world(), workspace.entity()),
        [document.id()]
    );
    assert_eq!(
        app.world()
            .get::<InWorkspace>(document.id().entity())
            .unwrap()
            .0,
        workspace.entity()
    );
    let info = inspect_document(app.world(), document.id()).unwrap();
    assert!(info.dirty && info.can_undo);
    assert!(!info.has_drafts);
    undo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(read_document(app.world(), document).unwrap(), "base");
    redo_document(app.world_mut(), document.id()).unwrap();
    assert_eq!(read_document(app.world(), document).unwrap(), "local");
}

#[test]
fn close_panels_batch_preflights_later_drafts_before_retiring_any_tab() {
    let mut scene = split_scene();
    let before = graph(scene.app.world(), scene.workspace);
    let mut workbench = Workbench::with_workspace(scene.app.world_mut(), scene.workspace).unwrap();
    let stamp = workbench.stamp();
    let edit = LayoutEdit::ClosePanels(vec![scene.first, scene.draft]);
    assert!(workbench.edit(stamp, &edit).is_err());
    assert_draft_scene_unchanged(&scene, &before);

    scene
        .app
        .world_mut()
        .entity_mut(scene.draft)
        .remove::<DocumentDraft>();
    Workbench::with_workspace(scene.app.world_mut(), scene.workspace)
        .unwrap()
        .edit(stamp, &edit)
        .unwrap();
    scene.app.update();
    assert!(scene.app.world().get_entity(scene.first).is_err());
    assert!(scene.app.world().get_entity(scene.draft).is_err());
    assert_eq!(
        panels_in_area(scene.app.world(), scene.source),
        [scene.survivor]
    );
    assert_eq!(
        read_document(scene.app.world(), scene.document).unwrap(),
        "local"
    );
}

#[test]
fn close_area_with_a_draft_preserves_geometry_focus_and_every_panel() {
    let mut scene = split_scene();
    let before = graph(scene.app.world(), scene.workspace);
    assert!(
        Workbench::with_workspace(scene.app.world_mut(), scene.workspace)
            .unwrap()
            .close_area(scene.target)
            .is_err()
    );
    assert_draft_scene_unchanged(&scene, &before);

    scene
        .app
        .world_mut()
        .entity_mut(scene.draft)
        .remove::<DocumentDraft>();
    Workbench::with_workspace(scene.app.world_mut(), scene.workspace)
        .unwrap()
        .close_area(scene.target)
        .unwrap();
    scene.app.update();
    for retired in [scene.target, scene.first, scene.draft] {
        assert!(scene.app.world().get_entity(retired).is_err());
    }
    assert_eq!(
        scene.app.world().get::<Area>(scene.source).unwrap().rect(),
        Area::default().rect()
    );
    assert_eq!(
        panels_in_area(scene.app.world(), scene.source),
        [scene.survivor]
    );
    assert_eq!(
        read_document(scene.app.world(), scene.document).unwrap(),
        "local"
    );
}

#[test]
fn deleting_a_draft_layout_is_atomic_and_later_deletion_preserves_its_workspace_document() {
    let mut scene = split_scene();
    let replacement = Workbench::with_workspace(scene.app.world_mut(), scene.workspace)
        .unwrap()
        .create_layout("Surviving layout")
        .unwrap();
    let before = graph(scene.app.world(), scene.workspace);
    assert!(
        Workbench::with_workspace(scene.app.world_mut(), scene.workspace)
            .unwrap()
            .delete_layout(scene.layout)
            .is_err()
    );
    assert_draft_scene_unchanged(&scene, &before);
    assert_eq!(
        active_layout(scene.app.world(), scene.workspace.entity()),
        Some(scene.layout)
    );

    scene
        .app
        .world_mut()
        .entity_mut(scene.draft)
        .remove::<DocumentDraft>();
    Workbench::with_workspace(scene.app.world_mut(), scene.workspace)
        .unwrap()
        .delete_layout(scene.layout)
        .unwrap();
    scene.app.update();
    assert_eq!(
        active_layout(scene.app.world(), scene.workspace.entity()),
        Some(replacement)
    );
    for retired in [
        scene.layout,
        scene.target,
        scene.source,
        scene.first,
        scene.draft,
        scene.survivor,
    ] {
        assert!(scene.app.world().get_entity(retired).is_err());
    }
    assert_eq!(
        document_ids(scene.app.world(), scene.workspace.entity()),
        [scene.document.id()]
    );
    assert_eq!(
        inspect_document(scene.app.world(), scene.document.id())
            .unwrap()
            .workspace,
        scene.workspace.entity()
    );
    undo_document(scene.app.world_mut(), scene.document.id()).unwrap();
    assert_eq!(
        read_document(scene.app.world(), scene.document).unwrap(),
        "base"
    );
}

#[derive(Component, Debug, PartialEq, Eq)]
struct LayoutNote(&'static str);

#[test]
fn replacing_a_draft_layout_is_atomic_and_a_later_reset_retains_the_workspace_document() {
    let mut scene = split_scene();
    scene
        .app
        .world_mut()
        .entity_mut(scene.layout)
        .insert(LayoutNote("Keep the slot identity"));
    let template = LayoutSnapshot {
        version: LAYOUT_FORMAT_VERSION,
        name: "Reset layout".into(),
        areas: vec![AreaSnapshot {
            rect: Area::default().rect(),
            panels: vec![PanelSnapshot {
                key: TextViewer::KEY.into(),
                title: "Replacement view".into(),
                preview: false,
            }],
            selected_panel: Some(0),
        }],
        focused_area: Some(0),
    };
    let before = graph(scene.app.world(), scene.workspace);
    assert!(replace_layout(scene.app.world_mut(), scene.layout, &template).is_err());
    assert_draft_scene_unchanged(&scene, &before);
    assert_eq!(
        scene.app.world().get::<LayoutNote>(scene.layout),
        Some(&LayoutNote("Keep the slot identity"))
    );

    scene
        .app
        .world_mut()
        .entity_mut(scene.draft)
        .remove::<DocumentDraft>();
    let restored = replace_layout(scene.app.world_mut(), scene.layout, &template).unwrap();
    scene.app.update();
    assert_eq!(restored.layout, scene.layout);
    assert_eq!(
        capture_layout(scene.app.world(), scene.layout).unwrap(),
        template
    );
    assert_eq!(
        active_layout(scene.app.world(), scene.workspace.entity()),
        Some(scene.layout)
    );
    assert_eq!(
        scene.app.world().get::<LayoutNote>(scene.layout),
        Some(&LayoutNote("Keep the slot identity"))
    );
    for retired in [
        scene.target,
        scene.source,
        scene.first,
        scene.draft,
        scene.survivor,
    ] {
        assert!(scene.app.world().get_entity(retired).is_err());
    }
    assert_eq!(
        document_ids(scene.app.world(), scene.workspace.entity()),
        [scene.document.id()]
    );
    assert_eq!(
        read_document(scene.app.world(), scene.document).unwrap(),
        "local"
    );
    let new_view = restored.areas[0].panels[0];
    bind_document_view(scene.app.world_mut(), new_view, scene.document).unwrap();
    undo_document(scene.app.world_mut(), scene.document.id()).unwrap();
    scene.app.update();
    assert_eq!(
        read_document(scene.app.world(), scene.document).unwrap(),
        "base"
    );
    assert!(
        !scene
            .app
            .world()
            .get::<PanelInstance>(new_view)
            .unwrap()
            .dirty
    );
}

#[test]
fn area_replace_and_join_preflight_target_drafts_before_geometry_or_selection_changes() {
    for dock in [false, true] {
        let mut scene = split_scene();
        let before = graph(scene.app.world(), scene.workspace);
        let relocate = |world: &mut World| {
            let mut workbench = Workbench::with_workspace(world, scene.workspace).unwrap();
            if dock {
                workbench.dock_area(scene.source, scene.target, AreaPlacement::Replace, [1, 1])
            } else {
                workbench.join_areas(scene.source, scene.target, [1, 1])
            }
        };
        assert!(relocate(scene.app.world_mut()).is_err());
        assert_draft_scene_unchanged(&scene, &before);

        scene
            .app
            .world_mut()
            .entity_mut(scene.draft)
            .remove::<DocumentDraft>();
        relocate(scene.app.world_mut()).unwrap();
        scene.app.update();
        for retired in [scene.target, scene.first, scene.draft] {
            assert!(scene.app.world().get_entity(retired).is_err());
        }
        assert_eq!(
            panels_in_area(scene.app.world(), scene.source),
            [scene.survivor]
        );
        assert_eq!(
            scene.app.world().get::<Area>(scene.source).unwrap().rect(),
            Area::default().rect()
        );
        assert_eq!(
            scene
                .app
                .world()
                .get::<Layout>(scene.layout)
                .unwrap()
                .focused_area(),
            Some(scene.source)
        );
        assert_eq!(
            read_document(scene.app.world(), scene.document).unwrap(),
            "local"
        );
    }
}

struct ChangeContext;
impl ContextCommand for ChangeContext {
    const KEY: &'static str = "document-tests.change-context";
    const LABEL: &'static str = "Change context";
}

struct RetireViewer;
impl ContextCommand for RetireViewer {
    const KEY: &'static str = "document-tests.retire-viewer";
    const LABEL: &'static str = "Retire viewer";
}

fn command_results(world: &mut World) -> Vec<ContextCommandFinished> {
    world
        .resource_mut::<Messages<ContextCommandFinished>>()
        .drain()
        .collect()
}

#[test]
fn document_commands_use_the_admitted_viewer_and_origin_even_after_focus_and_activation_change() {
    for inactive_at_admission in [false, true] {
        let (mut app, origin) = app();
        let (document, admitted) = text_view(app.world_mut(), origin);
        replace_text(app.world_mut(), document, "intended edit");
        let (distractor, new_focus) = text_view(app.world_mut(), origin);
        replace_text(app.world_mut(), distractor, "other origin edit");
        let other = Workspaces::new(app.world_mut()).open("Other active workspace");
        let (other_document, _) = text_view(app.world_mut(), other);
        replace_text(app.world_mut(), other_document, "other workspace edit");
        let (release, snapshots) = controlled_save(app.world_mut(), document);
        {
            let mut workbench = Workbench::with_workspace(app.world_mut(), origin).unwrap();
            workbench
                .register_handler::<ChangeContext, _>(
                    move |In(invocation): In<ContextCommandInvocation>,
                          world: &mut World|
                          -> ContextCommandResult {
                        Workbench::with_workspace(world, invocation.workspace)?
                            .focus_panel(new_focus)?;
                        Workspaces::new(world).activate(other)
                    },
                )
                .unwrap();
            workbench.focus_panel(admitted).unwrap();
            let mut requests = workbench.commands();
            requests.invoke::<ChangeContext>();
            requests.invoke::<UndoDocument>();
            requests.invoke::<RedoDocument>();
            requests.invoke::<SaveDocument>();
        }
        if inactive_at_admission {
            Workspaces::new(app.world_mut()).activate(other).unwrap();
        }

        app.update();

        let completed = command_results(app.world_mut());
        assert_eq!(completed.len(), 4);
        assert_eq!(
            completed
                .iter()
                .map(|result| result.invocation.command.clone())
                .collect::<Vec<_>>(),
            [
                ContextCommandId::of::<ChangeContext>(),
                ContextCommandId::of::<UndoDocument>(),
                ContextCommandId::of::<RedoDocument>(),
                ContextCommandId::of::<SaveDocument>(),
            ]
        );
        for result in completed {
            assert_eq!(result.invocation.workspace, origin);
            assert_eq!(result.invocation.panel, Some(admitted));
            assert_eq!(result.result, Ok(()));
        }
        assert_eq!(Workspaces::new(app.world_mut()).selected(), Some(other));
        let origin_layout = active_layout(app.world(), origin.entity()).unwrap();
        assert_eq!(
            Workbench::with_workspace(app.world_mut(), origin)
                .unwrap()
                .focused_panel(origin_layout),
            Some(new_focus)
        );
        let capture = snapshots.recv_timeout(DEADLINE).unwrap();
        assert_eq!(capture.document, document.id());
        assert_eq!(capture.model, "intended edit");
        assert_eq!(
            capture.revision, 3,
            "Undo and redo must both affect the admitted document"
        );
        for (untouched, contents) in [
            (distractor, "other origin edit"),
            (other_document, "other workspace edit"),
        ] {
            let info = inspect_document(app.world(), untouched.id()).unwrap();
            assert_eq!(info.revision, 1);
            assert!(info.dirty && !info.saving);
            assert_eq!(read_document(app.world(), untouched).unwrap(), contents);
        }
        release.send(()).unwrap();
        assert_eq!(finish_save(app.world_mut(), document.id()).result, Ok(()));
        assert!(!inspect_document(app.world(), document.id()).unwrap().dirty);
    }
}

#[test]
fn document_commands_for_a_removed_admitted_panel_error_instead_of_using_new_focus() {
    let (mut app, workspace) = app();
    let (document, admitted) = text_view(app.world_mut(), workspace);
    replace_text(app.world_mut(), document, "admitted edit");
    let (other_document, replacement) = text_view(app.world_mut(), workspace);
    replace_text(app.world_mut(), other_document, "replacement edit");
    let (saved, saves) = mpsc::channel();
    for document in [document, other_document] {
        let saved = saved.clone();
        bind_document_save(
            app.world_mut(),
            document,
            DocumentSaveBinding::new(None, move |snapshot| {
                saved.send(snapshot.document).unwrap();
                Ok(None)
            }),
        )
        .unwrap();
    }
    {
        let mut workbench = Workbench::with_workspace(app.world_mut(), workspace).unwrap();
        workbench
            .register_handler::<RetireViewer, _>(
                move |In(invocation): In<ContextCommandInvocation>,
                      world: &mut World|
                      -> ContextCommandResult {
                    let mut workbench = Workbench::with_workspace(world, invocation.workspace)?;
                    workbench.close_panel(admitted)?;
                    workbench.focus_panel(replacement)
                },
            )
            .unwrap();
        workbench.focus_panel(admitted).unwrap();
        let mut requests = workbench.commands();
        requests.invoke::<RetireViewer>();
        requests.invoke::<UndoDocument>();
        requests.invoke::<RedoDocument>();
        requests.invoke::<SaveDocument>();
    }

    app.update();

    let completed = command_results(app.world_mut());
    assert_eq!(completed.len(), 4);
    assert_eq!(completed[0].result, Ok(()));
    for result in &completed[1..] {
        assert_eq!(result.invocation.workspace, workspace);
        assert_eq!(result.invocation.panel, Some(admitted));
        assert!(result.result.is_err());
    }
    assert!(app.world().get_entity(admitted).is_err());
    let layout = active_layout(app.world(), workspace.entity()).unwrap();
    assert_eq!(
        Workbench::with_workspace(app.world_mut(), workspace)
            .unwrap()
            .focused_panel(layout),
        Some(replacement)
    );
    for (document, contents) in [
        (document, "admitted edit"),
        (other_document, "replacement edit"),
    ] {
        let info = inspect_document(app.world(), document.id()).unwrap();
        assert_eq!(info.revision, 1);
        assert!(info.can_undo && info.dirty && !info.saving);
        assert_eq!(read_document(app.world(), document).unwrap(), contents);
    }
    assert!(matches!(saves.try_recv(), Err(mpsc::TryRecvError::Empty)));
}

fn binding_state(
    world: &mut World,
) -> Vec<(
    Entity,
    ContextCommandId,
    KeyChord,
    bool,
    Option<Entity>,
    Option<Entity>,
)> {
    let mut bindings: Vec<_> = world
        .query::<(Entity, &Binding, Option<&InPanel>, Option<&InWorkspace>)>()
        .iter(world)
        .map(|(entity, binding, panel, workspace)| {
            (
                entity,
                binding.command.clone(),
                binding.chord,
                binding.repeat,
                panel.map(|panel| panel.0),
                workspace.map(|workspace| workspace.0),
            )
        })
        .collect();
    bindings.sort_by_key(|binding| binding.0.to_bits());
    bindings
}

#[test]
fn final_worker_shutdown_freezes_document_mutations_before_application_exit() {
    let (mut app, workspace) = app();
    let (document, _) = text_view(app.world_mut(), workspace);
    Workspaces::new(app.world_mut()).shutdown_work().unwrap();
    assert!(
        app.world()
            .get::<WorkspaceWork>(workspace.entity())
            .unwrap()
            .is_closing()
    );
    assert!(matches!(
        edit_document(
            app.world_mut(),
            document,
            0,
            TextChange {
                range: 0..0,
                replacement: "late edit".into(),
            },
            EditOptions::new("Late edit")
        ),
        Err(DocumentError::WorkspaceClosing(_))
    ));
    assert!(matches!(
        open_document(
            app.world_mut(),
            workspace.entity(),
            String::new(),
            DocumentOptions::unsaved("Late document")
        ),
        Err(DocumentError::WorkspaceClosing(_))
    ));
    assert!(matches!(
        offer_document_external_update(app.world_mut(), document, 0, "late external".into(), None),
        Err(DocumentError::WorkspaceClosing(_))
    ));
    assert_eq!(
        inspect_document(app.world(), document.id())
            .unwrap()
            .revision,
        0
    );
    assert!(!inspect_document(app.world(), document.id()).unwrap().dirty);
}

#[test]
fn conventional_document_shortcuts_are_opt_in_idempotent_and_preserve_host_bindings() {
    let (mut app, workspace) = app();
    let (document, panel) = text_view(app.world_mut(), workspace);
    replace_text(app.world_mut(), document, "local");
    let save_chord = KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyS);
    app.world_mut()
        .resource_mut::<BindingInput>()
        .0
        .push(BindingPress {
            chord: KeyChord::new(Modifiers::PRIMARY.resolved(), KeyCode::KeyS),
            repeat: false,
            context: InvocationContext::Focused,
        });
    app.update();
    assert!(
        binding_state(app.world_mut()).is_empty(),
        "Core document handlers must not install shortcut policy"
    );
    assert!(command_results(app.world_mut()).is_empty());
    assert!(inspect_document(app.world(), document.id()).unwrap().dirty);

    let host_save = app
        .world_mut()
        .spawn(Binding {
            command: ContextCommandId::of::<SaveDocument>(),
            chord: save_chord,
            repeat: true,
        })
        .id();
    let scoped_undo = app
        .world_mut()
        .spawn((
            Binding::new::<UndoDocument>(KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyZ)),
            InPanel(panel),
        ))
        .id();
    let unrelated = app
        .world_mut()
        .spawn(Binding::new::<ChangeContext>(KeyChord::new(
            Modifiers::ALT,
            KeyCode::KeyX,
        )))
        .id();
    register_document_shortcuts(app.world_mut());
    let installed = binding_state(app.world_mut());
    register_document_shortcuts(app.world_mut());
    app.update();
    register_document_shortcuts(app.world_mut());
    assert_eq!(binding_state(app.world_mut()), installed);
    assert!(app.world().get::<Binding>(host_save).unwrap().repeat);
    assert!(app.world().get::<Binding>(scoped_undo).is_some());
    assert!(app.world().get::<Binding>(unrelated).is_some());
    let expected = [
        (ContextCommandId::of::<SaveDocument>(), save_chord),
        (
            ContextCommandId::of::<UndoDocument>(),
            KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyZ),
        ),
        (
            ContextCommandId::of::<RedoDocument>(),
            KeyChord::new(Modifiers::PRIMARY | Modifiers::SHIFT, KeyCode::KeyZ),
        ),
        (
            ContextCommandId::of::<RedoDocument>(),
            KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyY),
        ),
    ];
    for (command, chord) in &expected {
        assert_eq!(
            installed
                .iter()
                .filter(|(_, id, key, _, panel, workspace)| id == command
                    && key == chord
                    && panel.is_none()
                    && workspace.is_none())
                .count(),
            1
        );
    }
    assert_eq!(installed.len(), 6);

    let (release, snapshots) = controlled_save(app.world_mut(), document);
    for (command, chord) in [
        expected[1].clone(),
        expected[2].clone(),
        expected[1].clone(),
        expected[3].clone(),
        expected[0].clone(),
    ] {
        app.world_mut()
            .resource_mut::<BindingInput>()
            .0
            .push(BindingPress {
                chord: KeyChord::new(chord.modifiers.resolved(), chord.key),
                repeat: false,
                context: InvocationContext::Focused,
            });
        app.update();
        let completed = command_results(app.world_mut());
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].invocation.command, command);
        assert_eq!(completed[0].invocation.workspace, workspace);
        assert_eq!(completed[0].invocation.panel, Some(panel));
        assert_eq!(completed[0].result, Ok(()));
        assert!(
            app.world_mut()
                .resource_mut::<Messages<BindingConflict>>()
                .drain()
                .next()
                .is_none()
        );
    }
    assert_eq!(snapshots.recv_timeout(DEADLINE).unwrap().model, "local");
    release.send(()).unwrap();
    assert_eq!(finish_save(app.world_mut(), document.id()).result, Ok(()));
}
