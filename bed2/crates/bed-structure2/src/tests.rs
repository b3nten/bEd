use super::*;
use bedtime_editor2::{
    BufferEdit,
    workbench::{
        CodeLanguage, EditorAnalysisPlugin, WorkbenchEditor, analysis_for_panel, bind_text_editor,
    },
};
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
struct Host {
    app: App,
    workspace: Entity,
    layout: Entity,
    a: Entity,
    b: Entity,
    structure: Entity,
    document: DocumentHandle<TextBuffer>,
}
fn host() -> Host {
    let mut app = App::new();
    app.add_plugins((WorkbenchCorePlugin, EditorAnalysisPlugin, StructurePlugin));
    let workspace = Workspaces::new(app.world_mut()).open("Code").entity();
    let document = open_document(
        app.world_mut(),
        workspace,
        TextBuffer::from("fn cat() {\n let age=42;\n}\nfn dog() {}\n"),
        DocumentOptions::loaded("cats.rs"),
    )
    .unwrap();
    app.world_mut()
        .entity_mut(document.id().0)
        .insert(CodeLanguage("rust".into()));
    let mut w = Workbench::for_entity(app.world_mut(), workspace).unwrap();
    let layout = w.active_layout();
    let a = w.create_panel(layout, "A").unwrap();
    let b = w.create_panel(layout, "B").unwrap();
    let structure = w.create_panel(layout, "Structure").unwrap();
    bind_text_editor(w.world_mut(), a, document).unwrap();
    bind_text_editor(w.world_mut(), b, document).unwrap();
    Host {
        app,
        workspace,
        layout,
        a,
        b,
        structure,
        document,
    }
}
fn focus(h: &mut Host, panel: Entity) {
    Workbench::for_entity(h.app.world_mut(), panel)
        .unwrap()
        .focus_panel(panel)
        .unwrap();
    refresh_sources(h.app.world_mut());
}
fn source(h: &Host) -> Option<Entity> {
    h.app
        .world()
        .get::<StructureSource>(h.workspace)
        .and_then(|s| s.source)
        .map(|s| s.panel)
}
fn wait(h: &mut Host) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        h.app.update();
        if analysis_for_panel(h.app.world(), h.a).is_some() {
            return;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn current_focus_wins_unrelated_focus_retains_and_no_history_is_scanned() {
    let mut h = host();
    let (a, b, structure) = (h.a, h.b, h.structure);
    focus(&mut h, a);
    assert_eq!(source(&h), Some(a));
    focus(&mut h, b);
    focus(&mut h, structure);
    assert_eq!(source(&h), Some(b));
    Workbench::for_entity(h.app.world_mut(), b)
        .unwrap()
        .close_panel(b)
        .unwrap();
    refresh_sources(h.app.world_mut());
    assert_eq!(source(&h), None);
    assert!(h.app.world().get::<PanelInstance>(a).is_some());
    focus(&mut h, a);
    assert_eq!(source(&h), Some(a));
}
#[test]
fn automatic_tab_focus_can_establish_an_eligible_replacement() {
    let mut h = host();
    let (a, b) = (h.a, h.b);
    // Remove the ineligible tab so closure selects A through normal tab policy.
    Workbench::for_entity(h.app.world_mut(), h.structure)
        .unwrap()
        .close_panel(h.structure)
        .unwrap();
    focus(&mut h, a);
    focus(&mut h, b);
    Workbench::for_entity(h.app.world_mut(), b)
        .unwrap()
        .close_panel(b)
        .unwrap();
    refresh_sources(h.app.world_mut());
    assert_eq!(source(&h), Some(a));
}
#[test]
fn automatic_ineligible_focus_clears_closed_source_and_rebinding_is_not_a_fallback() {
    let mut h = host();
    let (a, b, structure) = (h.a, h.b, h.structure);
    focus(&mut h, b);
    Workbench::for_entity(h.app.world_mut(), a)
        .unwrap()
        .close_panel(a)
        .unwrap();
    Workbench::for_entity(h.app.world_mut(), b)
        .unwrap()
        .close_panel(b)
        .unwrap();
    refresh_sources(h.app.world_mut());
    assert_eq!(source(&h), None);
    assert_eq!(
        Workbench::for_entity(h.app.world_mut(), structure)
            .unwrap()
            .focused_panel(h.layout),
        Some(structure)
    );
    let mut h = host();
    let (a, structure) = (h.a, h.structure);
    focus(&mut h, a);
    focus(&mut h, structure);
    let other = open_document(
        h.app.world_mut(),
        h.workspace,
        TextBuffer::from("not code"),
        DocumentOptions::loaded("text"),
    )
    .unwrap();
    bind_text_editor(h.app.world_mut(), a, other).unwrap();
    refresh_sources(h.app.world_mut());
    assert_eq!(source(&h), None);
}
#[test]
fn shared_analysis_targets_the_source_view_and_reveals_its_existing_layout() {
    let mut h = host();
    wait(&mut h);
    let a = analysis_for_panel(h.app.world(), h.a).unwrap();
    let b = analysis_for_panel(h.app.world(), h.b).unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    let activation = Activation {
        source: SourceBinding {
            panel: h.b,
            document: h.document.id(),
        },
        observation: a.key,
        item: a.outline[0].id,
    };
    let other = Workbench::for_entity(h.app.world_mut(), h.workspace)
        .unwrap()
        .create_layout("Other")
        .unwrap();
    Workbench::for_entity(h.app.world_mut(), h.workspace)
        .unwrap()
        .activate_layout(other)
        .unwrap();
    assert!(activate(h.app.world_mut(), activation).unwrap());
    assert_eq!(active_layout(h.app.world(), h.workspace), Some(h.layout));
    assert_eq!(
        Workbench::for_entity(h.app.world_mut(), h.workspace)
            .unwrap()
            .focused_panel(h.layout),
        Some(h.b)
    );
    // The sibling remains independent, and stale activation after edits has no effects.
    assert_eq!(
        h.app
            .world()
            .get::<WorkbenchEditor>(h.a)
            .unwrap()
            .view
            .selection(),
        TextSelection::default()
    );
    Workbench::for_entity(h.app.world_mut(), h.workspace)
        .unwrap()
        .activate_layout(other)
        .unwrap();
    edit_document(
        h.app.world_mut(),
        h.document,
        0,
        BufferEdit::new(0..0, "// edit\n"),
        EditOptions::new("Edit"),
    )
    .unwrap();
    assert!(!activate(h.app.world_mut(), activation).unwrap());
    assert_eq!(active_layout(h.app.world(), h.workspace), Some(other));
}
#[test]
fn closed_rebound_and_unsupported_sources_reject_obsolete_activation() {
    let mut h = host();
    wait(&mut h);
    let observation = analysis_for_panel(h.app.world(), h.b).unwrap();
    let activation = Activation {
        source: SourceBinding {
            panel: h.b,
            document: h.document.id(),
        },
        observation: observation.key,
        item: observation.outline[0].id,
    };
    let other = open_document(
        h.app.world_mut(),
        h.workspace,
        TextBuffer::from("fn other() {}"),
        DocumentOptions::loaded("other.rs"),
    )
    .unwrap();
    h.app
        .world_mut()
        .entity_mut(other.id().0)
        .insert(CodeLanguage("rust".into()));
    bind_text_editor(h.app.world_mut(), h.b, other).unwrap();
    assert!(!activate(h.app.world_mut(), activation).unwrap());
    Workbench::for_entity(h.app.world_mut(), h.b)
        .unwrap()
        .close_panel(h.b)
        .unwrap();
    assert!(!activate(h.app.world_mut(), activation).unwrap());
    h.app
        .world_mut()
        .entity_mut(h.document.id().0)
        .insert(CodeLanguage("text".into()));
    refresh_sources(h.app.world_mut());
    assert_eq!(source(&h), None);
}
#[test]
fn closing_the_last_view_retires_analysis_but_retains_the_document() {
    let mut h = host();
    wait(&mut h);
    Workbench::for_entity(h.app.world_mut(), h.a)
        .unwrap()
        .close_panel(h.a)
        .unwrap();
    Workbench::for_entity(h.app.world_mut(), h.b)
        .unwrap()
        .close_panel(h.b)
        .unwrap();
    h.app.update();
    assert!(
        h.app
            .world()
            .get::<bedtime_editor2::workbench::DocumentAnalysis>(h.document.id().0)
            .is_none()
    );
    assert!(read_document(h.app.world(), h.document).is_ok());
    let deadline = Instant::now() + Duration::from_secs(2);
    while !h
        .app
        .world()
        .get::<WorkspaceWork>(h.workspace)
        .unwrap()
        .is_finished()
    {
        h.app.update();
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn composition_rejects_activation_without_switching_layout_or_focus() {
    let mut h = host();
    wait(&mut h);
    let observation = analysis_for_panel(h.app.world(), h.b).unwrap();
    let activation = Activation {
        source: SourceBinding {
            panel: h.b,
            document: h.document.id(),
        },
        observation: observation.key,
        item: observation.outline[0].id,
    };
    let other = Workbench::for_entity(h.app.world_mut(), h.workspace)
        .unwrap()
        .create_layout("Other")
        .unwrap();
    Workbench::for_entity(h.app.world_mut(), h.workspace)
        .unwrap()
        .activate_layout(other)
        .unwrap();
    h.app.world_mut().entity_mut(h.b).insert(DocumentDraft);
    assert!(!activate(h.app.world_mut(), activation).unwrap());
    assert_eq!(active_layout(h.app.world(), h.workspace), Some(other));
}

#[test]
fn outline_stays_visible_on_the_edit_frame_and_through_pending_refresh() {
    let mut h = host();
    wait(&mut h);
    let source = SourceBinding {
        panel: h.a,
        document: h.document.id(),
    };
    let mut panel = StructurePanel::default();
    let (_, before) = panel.refresh_outline(h.app.world(), Some(source)).unwrap();
    let item = before.outline[0].id;
    panel.collapsed.insert(item);
    panel.selected = Some(item);
    edit_document(
        h.app.world_mut(),
        h.document,
        before.key.revision,
        BufferEdit::new(3..6, "kitten"),
        EditOptions::new("Rename"),
    )
    .unwrap();
    // The edit can be drawn before Update has requested the new analysis.
    assert!(analysis_for_panel(h.app.world(), h.a).is_none());
    let (_, editing) = panel.refresh_outline(h.app.world(), Some(source)).unwrap();
    assert!(Arc::ptr_eq(&editing, &before));
    assert!(
        !activate(
            h.app.world_mut(),
            Activation {
                source,
                observation: before.key,
                item
            }
        )
        .unwrap()
    );
    let text = read_document(h.app.world(), h.document).unwrap().clone();
    let revision = inspect_document(h.app.world(), h.document.id())
        .unwrap()
        .revision;
    h.app
        .world_mut()
        .get_mut::<bedtime_editor2::workbench::DocumentAnalysis>(h.document.id().0)
        .unwrap()
        .session
        .request(Arc::new(text), revision, "rust", Some(before.key.revision));
    for _ in 0..3 {
        let (_, refreshing) = panel.refresh_outline(h.app.world(), Some(source)).unwrap();
        assert!(Arc::ptr_eq(&refreshing, &before));
        assert!(panel.collapsed.contains(&item));
        assert_eq!(panel.selected, Some(item));
    }
    wait(&mut h);
    let (_, after) = panel.refresh_outline(h.app.world(), Some(source)).unwrap();
    assert_eq!(after.key.revision, revision);
    assert_eq!(after.outline[0].label, "kitten");
    assert!(panel.collapsed.contains(&item));
    // An actual empty result replaces the cached declarations normally.
    let length = read_document(h.app.world(), h.document).unwrap().len();
    edit_document(
        h.app.world_mut(),
        h.document,
        revision,
        BufferEdit::new(0..length, ""),
        EditOptions::new("Clear"),
    )
    .unwrap();
    assert!(
        !panel
            .refresh_outline(h.app.world(), Some(source))
            .unwrap()
            .1
            .outline
            .is_empty()
    );
    wait(&mut h);
    assert!(
        panel
            .refresh_outline(h.app.world(), Some(source))
            .unwrap()
            .1
            .outline
            .is_empty()
    );
}

#[test]
fn cached_outline_clears_on_source_or_language_changes_and_analysis_refusal() {
    let mut h = host();
    wait(&mut h);
    let source = SourceBinding {
        panel: h.a,
        document: h.document.id(),
    };
    let mut panel = StructurePanel::default();
    let before = panel
        .refresh_outline(h.app.world(), Some(source))
        .unwrap()
        .1;
    panel.selected = Some(before.outline[0].id);
    assert!(panel.refresh_outline(h.app.world(), None).is_none());
    assert_eq!(panel.selected, None);
    panel.refresh_outline(h.app.world(), Some(source)).unwrap();
    h.app
        .world_mut()
        .entity_mut(h.document.id().0)
        .insert(CodeLanguage("python".into()));
    assert!(panel.refresh_outline(h.app.world(), Some(source)).is_none());
    h.app
        .world_mut()
        .entity_mut(h.document.id().0)
        .insert(CodeLanguage("rust".into()));
    panel.refresh_outline(h.app.world(), Some(source)).unwrap();
    let limited = TextBuffer::from("x".repeat(bedtime_treesitter2::MAX_SOURCE_BYTES + 1));
    h.app
        .world_mut()
        .get_mut::<bedtime_editor2::workbench::DocumentAnalysis>(h.document.id().0)
        .unwrap()
        .session
        .request(
            Arc::new(limited),
            before.key.revision + 1,
            "rust",
            Some(before.key.revision),
        );
    assert!(panel.refresh_outline(h.app.world(), Some(source)).is_none());
}
