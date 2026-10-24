//! Document autosave policy. Metadata persistence is independent of this module.
use crate::settings::{self, Autosave};
use bedtime_editor2::workbench::WorkbenchEditor;
use bedtime_workbench2::*;
use bevy::{ecs::message::MessageCursor, prelude::*};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

pub struct AutosavePlugin;
impl Plugin for AutosavePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Autosaves>().add_systems(
            Update,
            update
                .after(crate::close::CloseSystems::Update)
                .after(crate::file_editor::reconcile_bindings),
        );
    }
}

#[derive(Resource, Default)]
struct Autosaves {
    documents: BTreeMap<DocumentId, Pending>,
    focused: Option<(Entity, DocumentId)>,
    saves: MessageCursor<DocumentSaveFinished>,
}

struct Pending {
    revision: u64,
    edited_at: Instant,
    focus_left: bool,
    attempted: Option<(u64, u64)>,
    work: Option<WorkId>,
}

fn focused_editor(world: &mut World) -> Option<(Entity, DocumentId)> {
    let contexts: Vec<_> = world
        .query_filtered::<&mut bevy_egui::EguiContext, With<bedtime_ui2::UiCamera>>()
        .iter_mut(world)
        .map(|mut c| c.get_mut().clone())
        .collect();
    let windows_focused = world.query::<&Window>().iter(world).all(|w| w.focused);
    if !windows_focused {
        return None;
    }
    let layout = active_layout(world)?;
    world
        .query_filtered::<(Entity, &DocumentView, &InArea), With<WorkbenchEditor>>()
        .iter(world)
        .find_map(|(panel, view, area)| {
            let visible = world
                .get::<Layout>(layout)
                .is_some_and(|layout| layout.focused_area == Some(area.0))
                && world
                    .get::<InLayout>(area.0)
                    .is_some_and(|owner| owner.0 == layout)
                && world
                    .get::<Area>(area.0)
                    .is_some_and(|area| area.selected == Some(panel));
            (visible
                && contexts.iter().any(|c| {
                    c.input(|i| i.focused)
                        && bedtime_editor2::workbench::editor_has_focus(world, panel, c)
                }))
            .then_some((panel, view.document()))
        })
}

fn update(world: &mut World) {
    let focused = focused_editor(world);
    tick(world, Instant::now(), focused);
}

fn tick(world: &mut World, now: Instant, focused: Option<(Entity, DocumentId)>) {
    if !crate::close::accepts_user_actions(world) {
        return;
    }
    let infos: Vec<_> = document_ids(world)
        .into_iter()
        .filter_map(|id| inspect_document(world, id).ok())
        .collect();
    world.resource_scope(|world, mut state: Mut<Autosaves>| {
        let outcomes: Vec<_> = state.saves.read(world.resource::<Messages<DocumentSaveFinished>>()).cloned().collect();
        for outcome in outcomes {
            if let Some(pending) = state.documents.get_mut(&outcome.document) && pending.work == Some(outcome.work) {
                pending.work = None;
                if let Err(error) = outcome.result {
                    request_error_dialog(world, ErrorDialog { title: "Autosave".into(), message: "The document couldn't be saved. Your edits remain open; use Save to retry.".into(), details: Some(error.to_string()) });
                }
            }
        }
        let live: BTreeSet<_> = infos.iter().map(|i| i.id).collect();
        state.documents.retain(|id, _| live.contains(id));
        let departed = if state.focused != focused { state.focused.map(|(_, id)| id) } else { None };
        state.focused = focused;
        for info in infos {
            let pending = state.documents.entry(info.id).or_insert(Pending { revision: info.revision,
                edited_at: now, focus_left: false, attempted: None, work: None });
            if pending.revision != info.revision {
                pending.revision = info.revision; pending.edited_at = now;
            }
            if departed == Some(info.id) { pending.focus_left = true; }
            if !info.dirty { pending.focus_left = false; continue; }
            let preferences = settings::effective_settings(world);
            let due = match preferences.autosave {
                Autosave::Never => { pending.focus_left = false; false },
                Autosave::ChangeFocus => pending.focus_left,
                Autosave::Timer => now.saturating_duration_since(pending.edited_at) >= Duration::from_secs_f64(preferences.autosave_timeout),
            };
            let attempt = (info.revision, info.source_generation);
            if crate::file_editor::source_path(world, info.id).is_some_and(|path|
                world.get_resource::<crate::file_editor::FileWrites>().is_some_and(|writes| writes.blocks(path))) { continue; }
            if !due || info.saving || info.has_drafts || info.conflict || info.requires_source_refresh || !info.has_save_binding
                || !crate::storage::ready(world) || pending.attempted == Some(attempt) { continue; }
            match save_document(world, info.id) {
                Ok(work) => {
                    pending.attempted = Some(attempt);
                    pending.focus_left = false;
                    pending.work = Some(work);
                }
                Err(DocumentError::DestinationBusy(_)) => continue,
                Err(error) => {
                    pending.attempted = Some(attempt);
                    pending.focus_left = false;
                request_error_dialog(world, ErrorDialog { title: "Autosave".into(), message: "The document couldn't be saved. Your edits remain open; use Save to retry.".into(), details: Some(error.to_string()) });
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn host(mode: Autosave) -> (App, Entity, DocumentHandle<String>, Arc<AtomicUsize>) {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugin)
            .init_resource::<Autosaves>();
        app.insert_resource(settings::GlobalPreferences(settings::Settings {
            autosave: mode,
            ..default()
        }));
        let workspace = crate::test_layout(app.world_mut());
        let document = open_document(
            app.world_mut(),
            String::new(),
            DocumentOptions::loaded("Test"),
        )
        .unwrap();
        let saves = Arc::new(AtomicUsize::new(0));
        let observed = saves.clone();
        bind_document_save(
            app.world_mut(),
            document,
            DocumentSaveBinding::new(None, move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            }),
        )
        .unwrap();
        edit(&mut app, document, "first");
        (app, workspace, document, saves)
    }

    fn edit(app: &mut App, document: DocumentHandle<String>, text: &str) {
        let revision = inspect_document(app.world(), document.id())
            .unwrap()
            .revision;
        let len = read_document(app.world(), document).unwrap().len();
        edit_document(
            app.world_mut(),
            document,
            revision,
            TextChange {
                range: 0..len,
                replacement: text.into(),
            },
            EditOptions::new("Edit"),
        )
        .unwrap();
    }

    fn finish(app: &mut App, document: DocumentId) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while inspect_document(app.world(), document).unwrap().saving {
            app.update();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn timer_waits_for_idle_and_saves_documents_without_viewers() {
        let (mut app, _, document, saves) = host(Autosave::Timer);
        let now = Instant::now();
        tick(app.world_mut(), now, None);
        tick(app.world_mut(), now + Duration::from_millis(900), None);
        assert_eq!(saves.load(Ordering::SeqCst), 0);
        edit(&mut app, document, "second");
        tick(app.world_mut(), now + Duration::from_millis(900), None);
        tick(app.world_mut(), now + Duration::from_millis(1500), None);
        assert_eq!(saves.load(Ordering::SeqCst), 0);
        tick(app.world_mut(), now + Duration::from_millis(1900), None);
        finish(&mut app, document.id());
        assert_eq!(saves.load(Ordering::SeqCst), 1);
        assert!(!inspect_document(app.world(), document.id()).unwrap().dirty);
    }

    #[test]
    fn focus_save_is_deduplicated_and_never_mode_does_not_write() {
        let (mut app, _, document, saves) = host(Autosave::ChangeFocus);
        let view = app.world_mut().spawn_empty().id();
        let now = Instant::now();
        tick(app.world_mut(), now, Some((view, document.id())));
        assert_eq!(saves.load(Ordering::SeqCst), 0);
        tick(app.world_mut(), now, None);
        tick(app.world_mut(), now, None);
        finish(&mut app, document.id());
        assert_eq!(saves.load(Ordering::SeqCst), 1);
        edit(&mut app, document, "never save");
        app.world_mut()
            .resource_mut::<settings::GlobalPreferences>()
            .0
            .autosave = Autosave::Never;
        tick(
            app.world_mut(),
            now + Duration::from_secs(100),
            Some((view, document.id())),
        );
        tick(app.world_mut(), now + Duration::from_secs(200), None);
        assert_eq!(saves.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failure_is_visible_and_does_not_retry_the_same_revision_forever() {
        let (mut app, _, document, saves) = host(Autosave::Timer);
        let observed = saves.clone();
        bind_document_save(
            app.world_mut(),
            document,
            DocumentSaveBinding::new(None, move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
                Err(DocumentSaveError::Failed("disk full".into()))
            }),
        )
        .unwrap();
        let now = Instant::now();
        tick(app.world_mut(), now, None);
        tick(app.world_mut(), now + Duration::from_secs(1), None);
        finish(&mut app, document.id());
        for n in 2..10 {
            tick(app.world_mut(), now + Duration::from_secs(n), None);
        }
        assert_eq!(saves.load(Ordering::SeqCst), 1);
        assert!(app.world().resource::<ErrorDialogs>().front().is_some());
        assert!(inspect_document(app.world(), document.id()).unwrap().dirty);
        edit(&mut app, document, "retry after edit");
        tick(app.world_mut(), now + Duration::from_secs(10), None);
        tick(app.world_mut(), now + Duration::from_secs(11), None);
        finish(&mut app, document.id());
        assert_eq!(saves.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn workspace_overrides_control_inactive_documents_and_unbound_documents_are_skipped() {
        let (mut app, workspace, document, saves) = host(Autosave::Timer);
        app.world_mut()
            .entity_mut(workspace)
            .insert(settings::ProjectPreferences(settings::SettingsOverrides {
                autosave: Some(Autosave::Never),
                ..default()
            }));
        let _other = crate::test_layout(app.world_mut());

        let now = Instant::now();
        tick(app.world_mut(), now, None);
        tick(app.world_mut(), now + Duration::from_secs(2), None);
        assert_eq!(saves.load(Ordering::SeqCst), 0);
        app.world_mut()
            .get_mut::<settings::ProjectPreferences>(workspace)
            .unwrap()
            .0
            .autosave = None;
        tick(app.world_mut(), now + Duration::from_secs(3), None);
        finish(&mut app, document.id());
        assert_eq!(saves.load(Ordering::SeqCst), 1);
        let unbound = open_document(
            app.world_mut(),
            "unsaved".to_owned(),
            DocumentOptions::unsaved("Unbound"),
        )
        .unwrap();
        tick(app.world_mut(), now + Duration::from_secs(3), None);
        tick(app.world_mut(), now + Duration::from_secs(10), None);
        assert!(inspect_document(app.world(), unbound.id()).unwrap().dirty);
    }

    #[test]
    fn composition_and_source_conflicts_block_autosave() {
        let (mut app, _workspace, document, saves) = host(Autosave::Timer);
        let layout = active_layout(app.world()).unwrap();
        let area = Workbench::for_entity(app.world_mut(), layout)
            .unwrap()
            .destination_area(layout)
            .unwrap();
        let panel = app
            .world_mut()
            .spawn((crate::settings::SettingsPanel, InArea(area)))
            .id();
        bind_document_view(app.world_mut(), panel, document).unwrap();
        app.world_mut().entity_mut(panel).insert(DocumentDraft);
        let now = Instant::now();
        tick(app.world_mut(), now, None);
        tick(app.world_mut(), now + Duration::from_secs(2), None);
        assert_eq!(saves.load(Ordering::SeqCst), 0);
        app.world_mut().entity_mut(panel).remove::<DocumentDraft>();
        let generation = inspect_document(app.world(), document.id())
            .unwrap()
            .source_generation;
        offer_document_external_update(
            app.world_mut(),
            document,
            generation,
            "external".to_owned(),
            None,
        )
        .unwrap();
        tick(app.world_mut(), now + Duration::from_secs(3), None);
        assert!(
            inspect_document(app.world(), document.id())
                .unwrap()
                .conflict
        );
        assert_eq!(saves.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_busy_destination_defers_without_suppressing_the_later_save() {
        let (mut app, _workspace, first, saves) = host(Autosave::Timer);
        let observed = saves.clone();
        let binding = DocumentSaveBinding::new(Some("shared destination".into()), move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        });
        bind_document_save(app.world_mut(), first, binding.clone()).unwrap();
        let second = open_document(
            app.world_mut(),
            String::new(),
            DocumentOptions::loaded("Second"),
        )
        .unwrap();
        bind_document_save(app.world_mut(), second, binding).unwrap();
        edit(&mut app, second, "second");
        save_document(app.world_mut(), first.id()).unwrap();
        let now = Instant::now();
        tick(app.world_mut(), now, None);
        tick(app.world_mut(), now + Duration::from_secs(1), None);
        assert!(!inspect_document(app.world(), second.id()).unwrap().saving);
        finish(&mut app, first.id());
        tick(app.world_mut(), now + Duration::from_secs(2), None);
        finish(&mut app, second.id());
        assert!(!inspect_document(app.world(), second.id()).unwrap().dirty);
        assert_eq!(saves.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn quit_decisions_suspend_new_autosaves() {
        let (mut app, _, document, saves) = host(Autosave::Timer);
        app.add_plugins(crate::close::ClosePlugin)
            .add_message::<AppExit>();
        let now = Instant::now();
        tick(app.world_mut(), now, None);
        app.world_mut()
            .write_message(CloseRequested { window: None });
        app.update();
        assert!(!crate::close::accepts_user_actions(app.world()));
        tick(app.world_mut(), now + Duration::from_secs(2), None);
        assert_eq!(saves.load(Ordering::SeqCst), 0);
        assert!(inspect_document(app.world(), document.id()).unwrap().dirty);
    }
}
