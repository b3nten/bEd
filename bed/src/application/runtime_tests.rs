use super::*;
use crate::application::test_support::TempDir;

#[path = "host_request_tests.rs"]
mod document_requests;

fn app(dir: &TempDir) -> App {
    let mut app = unstarted_app(dir);
    app.update();
    app
}

fn unstarted_app(dir: &TempDir) -> App {
    let mut app = App::new();
    install_test_infrastructure(&mut app, &dir.root().join("config"));
    app.add_plugins((
        (bevy::MinimalPlugins, bevy::input::InputPlugin),
        AppRuntimePlugin {
            options: AppOptions {
                working_directory: Some(dir.root().to_owned()),
                ..Default::default()
            },
        },
        bedtime_plugin_editor::EditorPlugin,
        bedtime_plugin_color_picker::ColorPickerPlugin,
        bedtime_plugin_json_tree::JsonTreePlugin,
    ));
    app
}

fn submit(world: &mut World, action: AppAction, source: Option<Entity>) -> AppRequestId {
    let request = capture_app_request_from(world, action, PanelTarget::LastFocused, source);
    submit_app_request(world, request)
}

fn apply(world: &mut World, action: AppAction, source: Option<Entity>) -> AppRequestOutcome {
    let id = submit(world, action, source);
    wait_app_request(world, id).unwrap()
}

#[test]
fn picker_open_saves_workspace_and_leaves_startup_without_losing_open_documents() {
    let dir = TempDir::new();
    let root = dir.path("project");
    std::fs::create_dir(&root).unwrap();
    let mut app = unstarted_app(&dir);
    app.add_plugins(bedtime_plugin_workspaces::WorkspacesPlugin);
    app.world_mut()
        .resource_mut::<InitialAppOptions>()
        .0
        .as_mut()
        .unwrap()
        .initial_layout = Some(LayoutSlot::Startup);
    app.update();
    let picker = active_panel_entities(app.world_mut())[0];
    let path = dir.write("carried.txt", b"before");
    let panel = open_file(app.world_mut(), &path, None, false, None).unwrap();
    let document = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    let session = app.world().non_send::<DocumentSession>().session_id();
    let revision = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .apply_edits(
            document,
            revision,
            &[ByteEdit {
                range: 0..6,
                bytes: b"retained draft".to_vec(),
            }],
        )
        .unwrap();

    assert_eq!(
        apply(
            app.world_mut(),
            AppAction::Workspace(WorkspaceAction::OpenLocalWorkspace(root.clone())),
            Some(picker),
        ),
        AppRequestOutcome::Applied
    );
    let spec = WorkspaceSpec::local(root.canonicalize().unwrap().to_string_lossy());
    assert_eq!(
        app.world()
            .resource::<bedtime_plugin_workspaces::WorkspaceAttachment>()
            .saved(),
        Some(&spec)
    );
    assert_eq!(
        app.world().resource::<LayoutSlots>().active(),
        LayoutSlot::Layout1
    );
    assert!(!app.world().entities().contains(picker));
    assert!(bedtime_api::workbench::panel_active(app.world(), panel));
    assert_eq!(
        app.world().non_send::<DocumentSession>().session_id(),
        session
    );
    let snapshot = app
        .world()
        .non_send::<DocumentSession>()
        .snapshot(document)
        .unwrap();
    assert_eq!(snapshot.bytes, b"retained draft");
    assert!(snapshot.dirty);
    assert_eq!(std::fs::read(path).unwrap(), b"before");
    let stored = WorkspaceStore::load(&test_application_paths(&dir.path("config"))).unwrap();
    assert_eq!(stored.recent_workspaces(), vec![spec.clone()]);
    assert_eq!(stored.active_layout_slot(&spec), LayoutSlot::Layout1);
    assert!(root.join(".bed/workspace.json").is_file());
}

#[test]
fn opening_another_new_project_saves_both_workspaces_and_uses_layout_one() {
    let dir = TempDir::new();
    let first = dir.path("first");
    let second = dir.path("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let mut app = app(&dir);
    for root in [&first, &second] {
        assert_eq!(
            apply(
                app.world_mut(),
                AppAction::Workspace(WorkspaceAction::OpenLocalWorkspace(root.clone())),
                None,
            ),
            AppRequestOutcome::Applied
        );
        assert_eq!(
            app.world().resource::<LayoutSlots>().active(),
            LayoutSlot::Layout1
        );
        assert_eq!(
            app.world()
                .resource::<bedtime_plugin_workspaces::WorkspaceAttachment>()
                .saved()
                .unwrap()
                .root,
            root.canonicalize().unwrap().to_string_lossy()
        );
        assert!(root.join(".bed/workspace.json").is_file());
    }
    let stored = WorkspaceStore::load(&test_application_paths(&dir.path("config"))).unwrap();
    assert_eq!(
        stored.recent_workspaces(),
        vec![
            WorkspaceSpec::local(second.canonicalize().unwrap().to_string_lossy()),
            WorkspaceSpec::local(first.canonicalize().unwrap().to_string_lossy()),
        ]
    );
}

fn navigation_context() -> dear_imgui_rs::Context {
    let mut imgui = dear_imgui_rs::Context::create();
    imgui.set_ini_filename(None::<PathBuf>).unwrap();
    imgui
        .io_mut()
        .set_config_flags(dear_imgui_rs::ConfigFlags::DOCKING_ENABLE);
    imgui.io_mut().set_display_size([800., 600.]);
    imgui.io_mut().set_delta_time(1. / 60.);
    imgui
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    imgui
}

fn preview(app: &mut App, path: &Path, area: u32) -> Entity {
    let mut request = capture_app_request_from(
        app.world(),
        AppAction::HostAction(HostAction::PreviewFileIn {
            path: path.to_string_lossy().into_owned(),
            viewer: None,
            target: PanelTarget::LargestArea,
        }),
        PanelTarget::LargestArea,
        None,
    );
    request.area = area;
    let id = submit_app_request(app.world_mut(), request);
    process_app_requests(app.world_mut()).unwrap();
    match poll_app_request(app.world_mut(), id).unwrap().unwrap() {
        AppRequestOutcome::Opened { panel, .. } => panel,
        outcome => panic!("{outcome:?}"),
    }
}

#[test]
fn syntax_spans_use_the_selected_theme_and_update_existing_documents() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    app.add_plugins(crate::themes::ThemePlugin::new(PathBuf::from(env!(
        "CARGO_MANIFEST_DIR"
    ))));
    let path = dir.root().join("syntax.rs");
    std::fs::write(
        &path,
        "fn main() { let count = 42; println!(\"hello\"); }\n",
    )
    .unwrap();
    let panel = open_file(app.world_mut(), &path, None, false, None).unwrap();
    let view = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .view
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        app.update();
        let ready = app
            .world_mut()
            .non_send_mut::<DocumentSession>()
            .with_view(view, |view| !view.highlight.spans_for_line(0).is_empty())
            .unwrap();
        if ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "syntax parser never delivered spans"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    for theme in ["tokyo", "catppuccin-latte"] {
        app.world_mut()
            .write_message(bedtime_api::settings::SettingsRequest::new(
                "bed.application",
                "theme",
                theme,
            ));
        // The ordered reducer commits preferences after this frame's preparation.
        app.update();
        app.update();
        let palette = app
            .world()
            .resource::<bedtime_core::SyntaxAppearance>()
            .0
            .clone();
        app.world_mut()
            .non_send_mut::<DocumentSession>()
            .with_view(view, |view| {
                let spans = view.highlight.spans_for_line(0);
                assert!(spans.len() > 3);
                for span in spans {
                    assert_eq!(
                        view.highlight.color_for_slot(span.slot),
                        palette.slots[span.slot as usize]
                    );
                }
                let colors: std::collections::HashSet<_> = spans
                    .iter()
                    .map(|span| view.highlight.color_for_slot(span.slot).map(f32::to_bits))
                    .collect();
                assert!(
                    colors.len() >= 3,
                    "keywords, strings and numbers lost their syntax colors"
                );
            })
            .unwrap();
    }
}

#[test]
fn structure_navigation_animates_to_the_item_and_respects_disabled_animations() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let path = dir.root().join("outline.rs");
    std::fs::write(&path, vec!["// padding"; 300].join("\n")).unwrap();
    let panel = open_file(app.world_mut(), &path, None, false, None).unwrap();
    let binding = app.world().get::<PanelBinding>(panel).unwrap().clone();
    let view = binding.view.unwrap();
    let document = binding.document.unwrap();
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut imgui = navigation_context();
    for _ in 0..6 {
        app.update();
        crate::application::testing::draw_ui(app.world_mut(), imgui.frame()).unwrap();
        drop(imgui.render_legacy());
    }
    let navigate = |app: &mut App, row| {
        let (byte_offset, revision) = {
            let session = app.world().non_send::<DocumentSession>();
            (
                session
                    .with_document(document, |state| state.offset_from_row_col(row, 0))
                    .unwrap(),
                session.document_revision(document).unwrap(),
            )
        };
        apply(
            app.world_mut(),
            AppAction::HostAction(HostAction::Navigate {
                document,
                byte_offset,
                revision,
            }),
            Some(panel),
        );
    };
    navigate(&mut app, 220);
    let mut samples = Vec::new();
    for _ in 0..40 {
        app.update();
        crate::application::testing::draw_ui(app.world_mut(), imgui.frame()).unwrap();
        drop(imgui.render_legacy());
        samples.push(
            app.world()
                .non_send::<DocumentSession>()
                .view_snapshot(view)
                .unwrap()
                .scroll_position[1],
        );
    }
    let settled = *samples.last().unwrap();
    assert!(settled > 2000.);
    assert!(
        samples[4] > 0. && samples[4] < settled - 1.,
        "jump did not animate: {samples:?}"
    );
    assert!(samples.windows(2).all(|pair| pair[1] >= pair[0] - 1.));
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .view_snapshot(view)
            .unwrap()
            .row,
        220
    );

    bedtime_core::settings::apply_settings_request(
        app.world_mut(),
        &bedtime_api::settings::SettingsRequest::new("bedtime.ui", "ui_animations", false),
    )
    .unwrap();
    navigate(&mut app, 0);
    for _ in 0..2 {
        app.update();
        crate::application::testing::draw_ui(app.world_mut(), imgui.frame()).unwrap();
        drop(imgui.render_legacy());
    }
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .view_snapshot(view)
            .unwrap()
            .scroll_position[1],
        0.
    );
}

#[test]
fn location_navigation_previews_new_files_and_preserves_existing_tab_modes() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let source_path = dir.write("source.rs", b"fn source() {}\nfn local_definition() {}\n");
    let source = open_file(app.world_mut(), &source_path, None, false, Some(1)).unwrap();
    let old_preview = preview(&mut app, &dir.write("old.rs", b"old"), 1);
    let old_id = app
        .world()
        .get::<PanelPresentation>(old_preview)
        .unwrap()
        .id;
    let navigate = |app: &mut App, path: &Path, source, row| {
        let outcome = apply(
            app.world_mut(),
            AppAction::HostAction(HostAction::NavigateLocation {
                path: path.to_string_lossy().into_owned(),
                row,
                utf16_column: 3,
                origin: None,
            }),
            Some(source),
        );
        let AppRequestOutcome::Opened { panel, .. } = outcome else {
            panic!("{outcome:?}")
        };
        assert_eq!(
            app.world().resource::<WorkbenchLayout>().focused,
            Some(panel)
        );
        panel
    };
    assert_eq!(navigate(&mut app, &source_path, source, 1), source);
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        Some(old_id),
        "same-file navigation must leave the kept source and other preview alone"
    );
    let first_path = dir.write("first.rs", b"// first\nfn definition() {}\n");
    let first = navigate(&mut app, &first_path, source, 1);
    let first_id = app.world().get::<PanelPresentation>(first).unwrap().id;
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        Some(first_id)
    );
    assert!(!app.world().entities().contains(old_preview));
    assert!(app.world().entities().contains(source));
    assert_eq!(navigate(&mut app, &first_path, first, 0), first);
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        Some(first_id),
        "same-file navigation must also preserve an existing preview"
    );
    let second_path = dir.write("second.rs", b"// second\nfn definition() {}\n");
    let second = navigate(&mut app, &second_path, first, 1);
    let second_id = app.world().get::<PanelPresentation>(second).unwrap().id;
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        Some(second_id)
    );
    assert!(!app.world().entities().contains(first));
    let view = app
        .world()
        .get::<PanelBinding>(second)
        .unwrap()
        .view
        .unwrap();
    let snapshot = app
        .world()
        .non_send::<DocumentSession>()
        .view_snapshot(view)
        .unwrap();
    assert_eq!((snapshot.row, snapshot.column), (1, 3));
    assert_eq!(navigate(&mut app, &source_path, second, 1), source);
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        Some(second_id),
        "cross-file navigation must preserve an already-kept destination"
    );
}

#[test]
fn location_navigation_reveals_the_target_in_a_new_and_existing_view() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let path = dir.root().join("definition.rs");
    let mut lines = vec!["// padding"; 300];
    lines[0] = "pub struct Definition {";
    lines[200] = "// 🦀 definition";
    lines[299] = "}";
    std::fs::write(&path, lines.join("\n")).unwrap();
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut imgui = navigation_context();
    apply(
        app.world_mut(),
        AppAction::HostAction(HostAction::NavigateLocation {
            path: path.to_string_lossy().into_owned(),
            row: 200,
            utf16_column: 6,
            origin: None,
        }),
        None,
    );
    let panel = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let binding = app.world().get::<PanelBinding>(panel).unwrap().clone();
    let view = binding.view.unwrap();
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .view_snapshot(view)
            .unwrap()
            .row,
        200
    );
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .view_snapshot(view)
            .unwrap()
            .column,
        8
    );
    for _ in 0..40 {
        app.update();
        crate::application::testing::draw_ui(app.world_mut(), imgui.frame()).unwrap();
        drop(imgui.render_legacy());
        process_app_requests(app.world_mut()).unwrap();
    }
    let snapshot = app
        .world()
        .non_send::<DocumentSession>()
        .view_snapshot(view)
        .unwrap();
    assert_eq!(snapshot.row, 200);
    assert!(
        snapshot.scroll_position[1] > 2000.,
        "target never became visible: {snapshot:?}"
    );
    let assert_target_visible = |world: &World| {
        let views = world.non_send::<bedtime_plugin_editor::EditorViews>();
        let surface = panel_presentation(world, panel).unwrap();
        let presentation = views.text(surface).unwrap().presentation();
        let geometry = world
            .get::<bedtime_api::workbench::PanelGeometry>(panel)
            .unwrap();
        assert!(
            presentation.caret_position[1] > geometry.content_min[1]
                && presentation.caret_position[1] < geometry.content_max[1],
            "definition caret is outside the rendered viewport"
        );
    };
    assert_target_visible(app.world());

    // A saved viewport waiting for the first draw must yield to newer navigation.
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .set_scroll(view, 0., 0.)
        .unwrap();
    apply(
        app.world_mut(),
        AppAction::HostAction(HostAction::NavigateLocation {
            path: path.to_string_lossy().into_owned(),
            row: 180,
            utf16_column: 3,
            origin: None,
        }),
        Some(panel),
    );
    for _ in 0..40 {
        app.update();
        crate::application::testing::draw_ui(app.world_mut(), imgui.frame()).unwrap();
        drop(imgui.render_legacy());
        process_app_requests(app.world_mut()).unwrap();
    }
    let snapshot = app
        .world()
        .non_send::<DocumentSession>()
        .view_snapshot(view)
        .unwrap();
    assert_eq!(snapshot.row, 180);
    assert!(
        snapshot.scroll_position[1] > 2000.,
        "saved viewport swallowed navigation: {snapshot:?}"
    );
    assert_target_visible(app.world());
}

#[test]
fn the_ui_and_registered_handlers_borrow_permanent_domain_resources() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let panel = match apply(
        app.world_mut(),
        AppAction::Command(WindowCommand::NewDocument),
        None,
    ) {
        AppRequestOutcome::Opened { panel, .. } => panel,
        _ => panic!(),
    };
    let child = panel_presentation(app.world(), panel).unwrap();
    let view = app
        .world()
        .get::<PanelBinding>(child)
        .unwrap()
        .view
        .unwrap();
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut imgui = dear_imgui_rs::Context::create();
    imgui
        .io_mut()
        .set_config_flags(dear_imgui_rs::ConfigFlags::DOCKING_ENABLE);
    imgui.io_mut().set_display_size([800., 600.]);
    imgui.io_mut().set_delta_time(1. / 60.);
    imgui
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    crate::application::testing::draw_ui(app.world_mut(), imgui.frame()).unwrap();
    drop(imgui.render_legacy());
    assert!(app.world().contains_non_send::<DocumentSession>());
    assert!(
        app.world()
            .contains_non_send::<bedtime_plugin_editor::EditorViews>()
    );
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .document_for_view(view),
        app.world().get::<PanelBinding>(panel).unwrap().document
    );
    assert!(
        app.world()
            .get::<bedtime_api::workbench::PanelGeometry>(panel)
            .unwrap()
            .visible
    );
}

#[test]
fn attachment_preserves_the_session_and_dirty_panel_entities_across_saved_id_collisions() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let root = dir.root().join("project");
    std::fs::create_dir(&root).unwrap();
    let saved = root.join("saved.txt");
    std::fs::write(&saved, b"saved").unwrap();
    let carried = match apply(
        app.world_mut(),
        AppAction::Command(WindowCommand::NewDocument),
        None,
    ) {
        AppRequestOutcome::Opened { panel, .. } => panel,
        _ => panic!(),
    };
    let document = app
        .world()
        .get::<PanelBinding>(carried)
        .unwrap()
        .document
        .unwrap();
    let revision = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    apply(
        app.world_mut(),
        AppAction::ApplyEdits {
            document,
            revision,
            edits: vec![ByteEdit {
                range: 0..0,
                bytes: b"unsaved".to_vec(),
            }],
        },
        Some(carried),
    );
    let session = app.world().non_send::<DocumentSession>().session_id();
    let id = app.world().get::<PanelPresentation>(carried).unwrap().id;
    let spec = WorkspaceSpec::local(root.to_string_lossy());
    app.world_mut()
        .non_send_mut::<WorkspacePersistence>()
        .store
        .record_workspace(spec.clone())
        .unwrap();
    app.world_mut().non_send_mut::<WorkspacePersistence>().store.set_layout(&spec,json!({"version":2,"panels":[{"id":id,"panel_type":"bedtime.editor.text-panel","viewer":"bedtime.editor.text","path":saved,"state":{}}],"focused":id})).unwrap();
    attach_workspace(app.world_mut(), &root).unwrap();
    assert_eq!(
        app.world().non_send::<DocumentSession>().session_id(),
        session
    );
    assert!(app.world().entities().contains(carried));
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"unsaved"
    );
    let panels = panel_entities(app.world_mut());
    let ids = panels
        .iter()
        .map(|entity| app.world().get::<PanelPresentation>(*entity).unwrap().id)
        .collect::<HashSet<_>>();
    assert_eq!(ids.len(), panels.len());
}

fn target_fixture(app: &mut App) -> (Entity, Entity, Entity, u32, u32) {
    let first = open_panel(
        app.world_mut(),
        &bedtime_api::PanelTypeId::from(bedtime_plugin_editor::TEXT_PANEL_TYPE),
        PanelInput::None,
        Value::Null,
        None,
        None,
        false,
    )
    .unwrap();
    let last = open_panel(
        app.world_mut(),
        &bedtime_api::PanelTypeId::from(bedtime_plugin_editor::TEXT_PANEL_TYPE),
        PanelInput::None,
        Value::Null,
        None,
        None,
        false,
    )
    .unwrap();
    let singleton = open_panel(
        app.world_mut(),
        &bedtime_api::PanelTypeId::new("bedtime.color.panel"),
        PanelInput::None,
        Value::Null,
        None,
        None,
        false,
    )
    .unwrap();
    let mut layout = bedtime_workbench::workspace::tiling::Layout::default();
    let largest = layout.split(1, 0, 3000, 100).unwrap();
    let last_area = layout.split(1, 1, 5000, 100).unwrap();
    let mut tiling = bedtime_workbench::workspace::tiling_state::TilingState::new(layout);
    for (panel, area) in [(first, 1), (last, last_area), (singleton, 1)] {
        tiling.assign(
            app.world().get::<PanelPresentation>(panel).unwrap().id,
            area,
        );
    }
    app.world_mut().resource_mut::<WorkbenchLayout>().tiling = tiling;
    focus_panel(app.world_mut(), first, true).unwrap();
    focus_panel(app.world_mut(), last, true).unwrap();
    (first, last, singleton, largest, last_area)
}

#[test]
fn default_text_opening_prefers_text_groups_while_explicit_areas_override_it() {
    for explicit in [false, true] {
        let dir = TempDir::new();
        let mut app = app(&dir);
        let (first, last, _, largest, last_area) = target_fixture(&mut app);
        let invocation =
            bedtime_workbench::capture_command(app.world(), "bedtime.color.new", Value::Null);
        assert_eq!(invocation.source, panel_presentation(app.world(), last));
        assert_eq!(invocation.area, largest);
        app.world_mut().resource_mut::<HostEffects>().push(
            Some(last),
            HostAction::OpenFile {
                path: dir
                    .write("default.txt", b"default")
                    .to_string_lossy()
                    .into_owned(),
                viewer: None,
            },
        );
        // Equal-size text groups may choose the later group. An explicit area
        // takes precedence even when that area has no text documents.
        if explicit {
            let intents = app.world_mut().resource_mut::<HostEffects>().take();
            for intent in intents {
                app.world_mut().resource_mut::<HostEffects>().push_at(
                    intent.source,
                    largest,
                    intent.action,
                );
            }
        }
        drain_feature_intents(app.world_mut());
        focus_panel(app.world_mut(), first, true).unwrap();
        process_app_requests(app.world_mut()).unwrap();
        let opened = app.world().resource::<WorkbenchLayout>().focused.unwrap();
        assert_ne!(opened, first);
        let id = app.world().get::<PanelPresentation>(opened).unwrap().id;
        assert_eq!(
            app.world()
                .resource::<WorkbenchLayout>()
                .tiling
                .area_for(id),
            Some(if explicit { largest } else { last_area })
        );
    }
}

#[test]
fn shortcut_panel_opening_captures_the_focused_area_while_menu_opening_uses_the_default() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    for keyboard in [false, true] {
        let dir = TempDir::new();
        let mut app = app(&dir);
        let (first, _, _, largest, focused_area) = target_fixture(&mut app);
        let actions = app
            .world()
            .resource::<bedtime_api::workbench::WorkbenchRegistry>()
            .shortcuts();
        for label in [
            "Find",
            "Copy",
            "Cut",
            "Paste",
            "Select All",
            "Undo",
            "Redo",
            "Save",
            "Save As",
            "Open File",
        ] {
            assert_eq!(
                actions.iter().filter(|a| a.label == label).count(),
                1,
                "{label} has one configurable binding"
            );
        }
        let mut imgui = navigation_context();
        imgui.io_mut().set_config_macosx_behaviors(false);
        if keyboard {
            imgui.io_mut().add_key_event(
                if cfg!(target_os = "macos") {
                    dear_imgui_rs::Key::ModSuper
                } else {
                    dear_imgui_rs::Key::ModCtrl
                },
                true,
            );
            imgui.io_mut().add_key_event(dear_imgui_rs::Key::N, true);
            let ui = imgui.frame();
            bedtime_workbench::prepare_shortcuts(app.world_mut(), ui);
            bedtime_workbench::global_shortcuts(app.world_mut(), ui);
            drop(imgui.render_legacy());
        } else {
            let invocation = bedtime_workbench::capture_command(
                app.world(),
                "bedtime.documents.new",
                Value::Null,
            );
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .push(WorkbenchAction::Invoke(invocation));
        }
        focus_panel(app.world_mut(), first, true).unwrap();
        process_app_requests(app.world_mut()).unwrap();
        let opened = app.world().resource::<WorkbenchLayout>().focused.unwrap();
        let id = app.world().get::<PanelPresentation>(opened).unwrap().id;
        assert_eq!(
            app.world()
                .resource::<WorkbenchLayout>()
                .tiling
                .area_for(id),
            Some(if keyboard { focused_area } else { largest })
        );
    }
}

#[test]
fn scheduled_frames_and_shared_save_commands_preserve_a_tools_remembered_document() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    apply(
        app.world_mut(),
        AppAction::Settings(bedtime_api::settings::SettingsRequest::new(
            "bedtime.documents",
            "autosave",
            false,
        )),
        None,
    );
    let path = dir.write("remembered.txt", b"before");
    let editor = open_file(app.world_mut(), &path, None, false, None).unwrap();
    let document = app
        .world()
        .get::<PanelBinding>(editor)
        .unwrap()
        .document
        .unwrap();
    let tool = match apply(
        app.world_mut(),
        AppAction::PluginCommand(bedtime_plugin_color_picker::OPEN_COMMAND.into()),
        Some(editor),
    ) {
        AppRequestOutcome::Opened { panel, .. } => panel,
        outcome => panic!("color tool did not open: {outcome:?}"),
    };
    for _ in 0..3 {
        app.update();
        assert_eq!(
            app.world().resource::<WorkbenchLayout>().focused,
            Some(tool)
        );
        assert_eq!(
            app.world()
                .resource::<bedtime_api::documents::DocumentFrame>()
                .active_document,
            Some(document)
        );
    }
    let revision = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    apply(
        app.world_mut(),
        AppAction::ApplyEdits {
            document,
            revision,
            edits: vec![ByteEdit {
                range: 0..6,
                bytes: b"after".to_vec(),
            }],
        },
        Some(editor),
    );
    let outcome = apply(
        app.world_mut(),
        AppAction::PluginCommand("bedtime.documents.save".into()),
        Some(tool),
    );
    assert!(matches!(outcome, AppRequestOutcome::Applied));
    assert_eq!(std::fs::read(path).unwrap(), b"after");
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().focused,
        Some(tool)
    );
}

#[test]
fn queued_shared_save_keeps_the_document_captured_while_a_tool_was_focused() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    apply(
        app.world_mut(),
        AppAction::Settings(bedtime_api::settings::SettingsRequest::new(
            "bedtime.documents",
            "autosave",
            false,
        )),
        None,
    );
    let first_path = dir.write("first-save.txt", b"before-a");
    let first = open_file(app.world_mut(), &first_path, None, false, None).unwrap();
    let first_document = app
        .world()
        .get::<PanelBinding>(first)
        .unwrap()
        .document
        .unwrap();
    let second_path = dir.write("second-save.txt", b"before-b");
    let second = open_file(app.world_mut(), &second_path, None, false, None).unwrap();
    let second_document = app
        .world()
        .get::<PanelBinding>(second)
        .unwrap()
        .document
        .unwrap();
    for (panel, document, bytes) in [
        (first, first_document, b"after-a".to_vec()),
        (second, second_document, b"after-b".to_vec()),
    ] {
        let revision = app
            .world()
            .non_send::<DocumentSession>()
            .document_revision(document)
            .unwrap();
        apply(
            app.world_mut(),
            AppAction::ApplyEdits {
                document,
                revision,
                edits: vec![ByteEdit { range: 0..8, bytes }],
            },
            Some(panel),
        );
    }
    focus_panel(app.world_mut(), first, true).unwrap();
    let tool = match apply(
        app.world_mut(),
        AppAction::PluginCommand(bedtime_plugin_color_picker::OPEN_COMMAND.into()),
        Some(first),
    ) {
        AppRequestOutcome::Opened { panel, .. } => panel,
        outcome => panic!("tool did not open: {outcome:?}"),
    };
    let invocation =
        bedtime_workbench::capture_command(app.world(), "bedtime.documents.save", Value::Null);
    let captured = capture_app_request_from(
        app.world(),
        AppAction::WorkbenchAction(WorkbenchAction::Invoke(invocation)),
        PanelTarget::LastFocused,
        Some(tool),
    );
    assert_eq!(captured.context.document, Some(first_document));
    let request = submit_app_request(app.world_mut(), captured);
    focus_panel(app.world_mut(), second, true).unwrap();
    publish_app_state(app.world_mut());
    process_app_requests(app.world_mut()).unwrap();
    let outcome = poll_app_request(app.world_mut(), request).unwrap().unwrap();
    assert!(matches!(outcome, AppRequestOutcome::Applied), "{outcome:?}");
    assert_eq!(std::fs::read(first_path).unwrap(), b"after-a");
    assert_eq!(std::fs::read(second_path).unwrap(), b"before-b");
    assert!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(second_document)
            .unwrap()
            .dirty
    );
}

#[test]
fn location_navigation_uses_the_source_area_after_focus_changes() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let (first, _, _, largest, source_area) = target_fixture(&mut app);
    assert_ne!(source_area, largest);
    let source = open_file(
        app.world_mut(),
        &dir.write("source.rs", b"fn source() {}"),
        None,
        false,
        Some(source_area),
    )
    .unwrap();
    let surface = panel_presentation(app.world(), source).unwrap();
    app.world_mut().resource_mut::<HostEffects>().push(
        Some(surface),
        HostAction::NavigateLocation {
            path: dir
                .write("definition.rs", b"fn definition() {}")
                .to_string_lossy()
                .into_owned(),
            row: 0,
            utf16_column: 3,
            origin: None,
        },
    );
    drain_feature_intents(app.world_mut());
    focus_panel(app.world_mut(), first, true).unwrap();
    process_app_requests(app.world_mut()).unwrap();
    let panel = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let id = app.world().get::<PanelPresentation>(panel).unwrap().id;
    let tiling = &app.world().resource::<WorkbenchLayout>().tiling;
    assert_eq!(tiling.area_for(id), Some(source_area));
    assert_eq!(
        tiling
            .areas
            .iter()
            .find(|group| group.area == source_area)
            .unwrap()
            .preview,
        Some(id)
    );
    assert!(app.world().entities().contains(source));
}

#[test]
fn registered_explicit_show_moves_the_existing_panel_and_plain_show_preserves_its_area() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let (_, last, singleton, largest, last_area) = target_fixture(&mut app);
    let geometry = app
        .world()
        .resource::<WorkbenchLayout>()
        .tiling
        .layout
        .clone();
    let child = panel_presentation(app.world(), singleton).unwrap();
    bedtime_api::workbench::register(app.world_mut(), "bedtime.test-target", |registrar| {
        registrar.command(
            "bedtime.test-target.show",
            "Show",
            |In(invocation): In<CommandInvocation>,
             mut intents: ResMut<HostEffects>|
             -> bedtime_api::workbench::CommandResult {
                intents.push(
                    invocation.source,
                    HostAction::ShowPanelIn {
                        panel_type: "bedtime.color.panel".into(),
                        document: None,
                        state: Value::Null,
                        action: None,
                        target: PanelTarget::LargestArea,
                    },
                );
                Ok(Vec::new())
            },
        );
    })
    .unwrap();
    let outcome = apply(
        app.world_mut(),
        AppAction::PluginCommand("bedtime.test-target.show".into()),
        Some(last),
    );
    assert!(matches!(outcome, AppRequestOutcome::Opened{panel,..} if panel == singleton));
    assert_eq!(
        app.world()
            .resource::<WorkbenchLayout>()
            .tiling
            .area_for(app.world().get::<PanelPresentation>(singleton).unwrap().id),
        Some(largest)
    );
    assert_eq!(panel_presentation(app.world(), singleton), Some(child));
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.layout,
        geometry
    );
    assert_eq!(panel_entities(app.world_mut()).len(), 3);
    apply(
        app.world_mut(),
        AppAction::HostAction(HostAction::ShowPanel {
            panel_type: "bedtime.color.panel".into(),
            document: None,
            state: Value::Null,
            action: None,
        }),
        Some(last),
    );
    assert_eq!(
        app.world()
            .resource::<WorkbenchLayout>()
            .tiling
            .area_for(app.world().get::<PanelPresentation>(singleton).unwrap().id),
        Some(largest)
    );
    app.world_mut().resource_mut::<HostEffects>().push_at(
        Some(last),
        last_area,
        HostAction::ShowPanelIn {
            panel_type: "bedtime.color.panel".into(),
            document: None,
            state: Value::Null,
            action: None,
            target: PanelTarget::LargestArea,
        },
    );
    process_app_requests(app.world_mut()).unwrap();
    assert_eq!(
        app.world()
            .resource::<WorkbenchLayout>()
            .tiling
            .area_for(app.world().get::<PanelPresentation>(singleton).unwrap().id),
        Some(last_area)
    );
}

#[test]
fn feature_intent_largest_area_is_captured_before_the_next_transition() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let (first, last, singleton, largest, _) = target_fixture(&mut app);
    app.world_mut().resource_mut::<HostEffects>().push(
        Some(last),
        HostAction::ShowPanelIn {
            panel_type: "bedtime.color.panel".into(),
            document: None,
            state: Value::Null,
            action: None,
            target: PanelTarget::LargestArea,
        },
    );
    drain_feature_intents(app.world_mut());
    focus_panel(app.world_mut(), first, true).unwrap();
    process_app_requests(app.world_mut()).unwrap();
    assert_eq!(
        app.world()
            .resource::<WorkbenchLayout>()
            .tiling
            .area_for(app.world().get::<PanelPresentation>(singleton).unwrap().id),
        Some(largest)
    );
}

#[test]
fn opening_a_file_reuses_only_a_pristine_scratch_and_keeps_its_identity_and_order() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let first = match apply(
        app.world_mut(),
        AppAction::Command(WindowCommand::NewDocument),
        None,
    ) {
        AppRequestOutcome::Opened { panel, .. } => panel,
        _ => panic!(),
    };
    let scratch = app
        .world()
        .get::<PanelBinding>(first)
        .unwrap()
        .document
        .unwrap();
    let second = match apply(
        app.world_mut(),
        AppAction::Command(WindowCommand::NewDocument),
        None,
    ) {
        AppRequestOutcome::Opened { panel, .. } => panel,
        _ => panic!(),
    };
    focus_panel(app.world_mut(), first, true).unwrap();
    let layout = app.world().resource::<WorkbenchLayout>().tiling.clone();
    let file = dir.write("file.txt", b"file contents");
    assert_eq!(
        open_file(app.world_mut(), &file, None, false, None).unwrap(),
        first
    );
    assert_eq!(app.world().resource::<WorkbenchLayout>().tiling, layout);
    assert!(
        !app.world()
            .non_send::<DocumentSession>()
            .document_ids()
            .contains(&scratch)
    );
    let document = app
        .world()
        .get::<PanelBinding>(second)
        .unwrap()
        .document
        .unwrap();
    let view = app
        .world()
        .get::<PanelBinding>(second)
        .unwrap()
        .view
        .unwrap();
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .with_commands(view, |commands| commands.type_text(b"x"))
        .unwrap();
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .undo_document(document)
        .unwrap();
    focus_panel(app.world_mut(), second, true).unwrap();
    let count = panel_entities(app.world_mut()).len();
    let other = dir.write("other.txt", b"other");
    let opened = open_file(app.world_mut(), &other, None, false, None).unwrap();
    assert_ne!(opened, second);
    assert_eq!(panel_entities(app.world_mut()).len(), count + 1);
    assert!(
        app.world()
            .non_send::<DocumentSession>()
            .document_ids()
            .contains(&document)
    );
}

fn registered_close(
    In(invocation): In<CommandInvocation>,
) -> bedtime_api::workbench::CommandResult {
    Ok(vec![WorkbenchAction::ClosePanel(
        invocation.source.unwrap(),
    )])
}

#[test]
fn registered_command_receipts_follow_actual_save_and_close_effects() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let path = dir.write("close.txt", b"before");
    bedtime_api::workbench::register(app.world_mut(), "receipt", |registrar| {
        registrar.command("receipt.close", "Close", registered_close)
    })
    .unwrap();
    let panel = open_file(app.world_mut(), &path, None, false, None).unwrap();
    let document = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    let revision = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .apply_edits(
            document,
            revision,
            &[ByteEdit {
                range: 0..6,
                bytes: b"after".to_vec(),
            }],
        )
        .unwrap();
    let id = submit(
        app.world_mut(),
        AppAction::PluginCommand("receipt.close".into()),
        Some(panel),
    );
    app.update();
    assert!(
        matches!(poll_app_request_result(app.world_mut(),id),Some(Ok(AppRequestOutcome::Closed{panels})) if panels==vec![panel])
    );
    assert!(!app.world().entities().contains(panel));
    assert_eq!(std::fs::read(&path).unwrap(), b"after");
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas.len(),
        1,
        "closing content preserves an empty destination area"
    );
}

#[test]
fn saving_a_failed_destination_preserves_the_old_binding_history_and_dirty_buffer() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let path = dir.write("before.txt", b"before");
    let panel = open_file(app.world_mut(), &path, None, false, None).unwrap();
    let document = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    let view = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .view
        .unwrap();
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .with_commands(view, |commands| commands.type_text(b"x"))
        .unwrap();
    let revision = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    let bytes = app
        .world()
        .non_send::<DocumentSession>()
        .snapshot(document)
        .unwrap()
        .bytes;
    let id = submit(
        app.world_mut(),
        AppAction::SaveAs {
            document,
            path: dir.root().join("missing-parent/after.txt"),
        },
        Some(panel),
    );
    app.update();
    assert!(
        poll_app_request_result(app.world_mut(), id)
            .unwrap()
            .is_err()
    );
    let session = app.world().non_send::<DocumentSession>();
    assert_eq!(session.document_revision(document).unwrap(), revision);
    let snapshot = session.snapshot(document).unwrap();
    assert_eq!(
        snapshot.path,
        path.canonicalize().unwrap().to_string_lossy()
    );
    assert_eq!(snapshot.bytes, bytes);
    assert!(snapshot.dirty);
    assert_eq!(std::fs::read(path).unwrap(), b"before");
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .undo_document(document)
        .unwrap();
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"before"
    );
}

#[test]
fn explicitly_refocusing_the_selected_panel_requests_widget_focus_again() {
    #[derive(Resource, Default)]
    struct Focuses(usize);
    let dir = TempDir::new();
    let mut app = app(&dir);
    let panel = open_file(
        app.world_mut(),
        &dir.write("focus.txt", b"focus"),
        None,
        false,
        None,
    )
    .unwrap();
    let surface = panel_presentation(app.world(), panel).unwrap();
    app.init_resource::<Focuses>();
    let hook = app.world_mut().register_system(
        |_: In<Entity>, mut focuses: ResMut<Focuses>| -> Result<(), String> {
            focuses.0 += 1;
            Ok(())
        },
    );
    app.world_mut().entity_mut(surface).insert(PanelFocus(hook));
    apply(app.world_mut(), AppAction::FocusPanel(panel), Some(panel));
    apply(app.world_mut(), AppAction::ObserveFocus(panel), Some(panel));
    assert_eq!(
        app.world().resource::<Focuses>().0,
        1,
        "observing the current panel cannot repeatedly steal native focus"
    );
    apply(app.world_mut(), AppAction::FocusPanel(panel), Some(panel));
    assert_eq!(
        app.world().resource::<Focuses>().0,
        2,
        "explicit focus restores a widget after native focus was stolen"
    );
}

#[test]
fn accepted_file_drop_finishes_its_editor_effects_before_acknowledging_and_later_requests() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let path = dir.write("drop.txt", b"before");
    let panel = open_file(app.world_mut(), &path, None, false, None).unwrap();
    let surface = panel_presentation(app.world(), panel).unwrap();
    let document = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    let revision = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    let handler = app.world_mut().register_system(
        move |In(invocation): In<bedtime_api::workbench::PanelExternalFilesInvocation>,
              mut intents: ResMut<HostEffects>|
              -> Result<bedtime_api::workbench::ExternalFileDropResponse, String> {
            intents.push(
                Some(invocation.entity),
                HostAction::ApplyEdits {
                    document,
                    revision,
                    edits: vec![ByteEdit {
                        range: 0..6,
                        bytes: b"dropped".to_vec(),
                    }],
                },
            );
            intents.push(Some(invocation.entity), HostAction::Save { document });
            Ok(bedtime_api::workbench::ExternalFileDropResponse::Accepted)
        },
    );
    app.world_mut()
        .entity_mut(surface)
        .insert(bedtime_api::workbench::PanelExternalFiles(handler));
    let event = bedtime_api::workbench::ExternalFileDrag {
        paths: vec![dir.root().join("handler-owns-this-path")],
        position: [0., 0.],
        viewport: 0,
        phase: bedtime_api::workbench::ExternalFileDragPhase::Drop,
        modifiers: Default::default(),
    };
    let drop = submit(
        app.world_mut(),
        AppAction::ExternalFiles {
            panel: Some(panel),
            event,
        },
        Some(panel),
    );
    let later = submit(
        app.world_mut(),
        AppAction::ApplyEdits {
            document,
            revision,
            edits: vec![ByteEdit {
                range: 0..0,
                bytes: b"stale".to_vec(),
            }],
        },
        Some(panel),
    );
    app.update();
    assert!(matches!(
        poll_app_request_result(app.world_mut(), drop),
        Some(Ok(AppRequestOutcome::Saved { .. }))
    ));
    assert_eq!(
        poll_app_request_result(app.world_mut(), later)
            .unwrap()
            .unwrap_err()
            .message,
        "Document changed; refresh before applying edits"
    );
    assert_eq!(std::fs::read(path).unwrap(), b"dropped");
}

#[test]
fn dropping_a_directory_stays_temporary_without_replacing_carried_documents() {
    let dir = TempDir::new();
    let mut app = unstarted_app(&dir);
    app.world_mut()
        .resource_mut::<InitialAppOptions>()
        .0
        .as_mut()
        .unwrap()
        .initial_layout = Some(LayoutSlot::Startup);
    app.update();
    let path = dir.write("carried.txt", b"carried");
    let panel = open_file(app.world_mut(), &path, None, false, None).unwrap();
    let document = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    let view = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .view
        .unwrap();
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .with_commands(view, |commands| commands.type_text(b"dirty "))
        .unwrap();
    let session = app.world().non_send::<DocumentSession>().session_id();
    let project = dir.root().join("project");
    std::fs::create_dir(&project).unwrap();
    let event = bedtime_api::workbench::ExternalFileDrag {
        paths: vec![project.clone()],
        position: [0., 0.],
        viewport: 0,
        phase: bedtime_api::workbench::ExternalFileDragPhase::Drop,
        modifiers: Default::default(),
    };
    apply(
        app.world_mut(),
        AppAction::ExternalFiles { panel: None, event },
        Some(panel),
    );
    assert_eq!(
        app.world().non_send::<DocumentSession>().session_id(),
        session
    );
    assert!(
        app.world()
            .resource::<bedtime_plugin_workspaces::WorkspaceAttachment>()
            .saved()
            .is_none()
    );
    assert_eq!(
        app.world().resource::<LayoutSlots>().active(),
        LayoutSlot::Layout1
    );
    assert!(!project.join(".bed/workspace.json").exists());
    assert_eq!(
        app.world()
            .resource::<bedtime_files::FileContext>()
            .directory,
        project.canonicalize().unwrap()
    );
    assert!(app.world().entities().contains(panel));
    assert!(bedtime_api::workbench::panel_active(app.world(), panel));
    assert!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .dirty
    );
    assert_eq!(std::fs::read(path).unwrap(), b"carried");
}

#[test]
fn saving_without_a_ui_frame_restores_geometry_order_shared_views_and_last_document() {
    let dir = TempDir::new();
    let mut original = app(&dir);
    let path = dir.write("shared.txt", b"shared");
    let first = open_file(original.world_mut(), &path, None, false, None).unwrap();
    let second = match apply(
        original.world_mut(),
        AppAction::Command(WindowCommand::DuplicateView),
        Some(first),
    ) {
        AppRequestOutcome::Opened { panel, .. } => panel,
        _ => panic!(),
    };
    let color = open_panel(
        original.world_mut(),
        &bedtime_api::PanelTypeId::new("bedtime.color.panel"),
        PanelInput::None,
        Value::Null,
        None,
        None,
        false,
    )
    .unwrap();
    let mut layout = bedtime_workbench::workspace::tiling::Layout::default();
    let right = layout.split(1, 0, 7000, 1).unwrap();
    let mut tiling = bedtime_workbench::workspace::tiling_state::TilingState::new(layout);
    tiling.assign(
        original
            .world()
            .get::<PanelPresentation>(second)
            .unwrap()
            .id,
        1,
    );
    tiling.assign(
        original.world().get::<PanelPresentation>(first).unwrap().id,
        1,
    );
    tiling.assign(
        original.world().get::<PanelPresentation>(color).unwrap().id,
        right,
    );
    original
        .world_mut()
        .resource_mut::<WorkbenchLayout>()
        .tiling = tiling.clone();
    focus_panel(original.world_mut(), second, true).unwrap();
    focus_panel(original.world_mut(), color, true).unwrap();
    let saved = layouts::snapshot(original.world_mut()).unwrap()["layouts"]
        [original.world().resource::<LayoutSlots>().active().key()]
    .clone();
    let fresh = TempDir::new();
    let mut restored = app(&fresh);
    restore_layout(restored.world_mut(), &saved, false).unwrap();
    assert_eq!(
        restored.world().resource::<WorkbenchLayout>().tiling,
        tiling
    );
    let document = restored
        .world()
        .resource::<DocumentNavigation>()
        .active_document
        .unwrap();
    assert_eq!(
        restored
            .world()
            .non_send::<DocumentSession>()
            .view_count(document),
        2
    );
    assert_eq!(
        restored
            .world()
            .get::<PanelRecord>(
                restored
                    .world()
                    .resource::<WorkbenchLayout>()
                    .focused
                    .unwrap()
            )
            .unwrap()
            .panel_type,
        "bedtime.color.panel"
    );
    assert_eq!(
        restored
            .world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"shared"
    );
}

#[test]
fn splitting_a_terminal_creates_an_independent_shell_in_the_inherited_directory() {
    use bedtime_plugin_terminal::TerminalPanelState;
    use bedtime_terminal::bedtime_terminal::{BedTerminal, TerminalSessionState};

    let dir = TempDir::new();
    let cwd = dir.path("shell-directory");
    std::fs::create_dir(&cwd).unwrap();
    let mut app = unstarted_app(&dir);
    app.add_plugins(bedtime_plugin_terminal::TerminalPlugin);
    app.update();
    let source = open_panel(
        app.world_mut(),
        &bedtime_api::PanelTypeId::from(bedtime_plugin_terminal::PANEL_ID),
        PanelInput::None,
        json!({ "cwd": cwd }),
        None,
        Some(1),
        false,
    )
    .unwrap();
    let source_frontend = panel_presentation(app.world(), source).unwrap();
    let source_session = app
        .world()
        .get::<TerminalPanelState>(source_frontend)
        .unwrap()
        .session_id;
    let source_id = app.world().get::<PanelPresentation>(source).unwrap().id;
    let source_area = app
        .world()
        .resource::<WorkbenchLayout>()
        .tiling
        .area_for(source_id)
        .unwrap();

    let copy = split_panel(app.world_mut(), source_frontend, 0, true).unwrap();
    app.update();
    let copy_frontend = panel_presentation(app.world(), copy).unwrap();
    let copy_session = app
        .world()
        .get::<TerminalPanelState>(copy_frontend)
        .unwrap()
        .session_id;
    let copy_id = app.world().get::<PanelPresentation>(copy).unwrap().id;
    assert_ne!(source, copy);
    assert_ne!(source_frontend, copy_frontend);
    assert_ne!(source_session, copy_session);
    assert_eq!(
        panel_presentation(app.world(), source),
        Some(source_frontend)
    );
    assert_eq!(
        app.world().get::<PanelBinding>(source).unwrap().terminal,
        Some(source_session)
    );
    assert_eq!(
        app.world().get::<PanelBinding>(copy).unwrap().terminal,
        Some(copy_session)
    );
    let layout = app.world().resource::<WorkbenchLayout>();
    assert_eq!(layout.tiling.area_for(source_id), Some(source_area));
    let copy_area = layout.tiling.area_for(copy_id).unwrap();
    assert_ne!(copy_area, source_area);
    assert_eq!(layout.focused, Some(copy));
    assert_eq!(layout.focused_area, Some(copy_area));
    for (area, id) in [(source_area, source_id), (copy_area, copy_id)] {
        let group = layout
            .tiling
            .areas
            .iter()
            .find(|group| group.area == area)
            .unwrap();
        assert_eq!(group.tabs, [id]);
        assert_eq!(group.selected, Some(id));
    }

    let terminal = app.world().non_send::<BedTerminal>();
    assert_eq!(terminal.session_ids(), [source_session, copy_session]);
    assert_eq!(terminal.active_session_id(), Some(copy_session));
    for session in [source_session, copy_session] {
        assert_eq!(terminal.working_directory(session), Some(cwd.as_path()));
        assert_eq!(
            terminal.session_state(session),
            Some(TerminalSessionState::Pending)
        );
        assert_eq!(terminal.process_id(session), None);
    }
}

#[test]
fn dropped_files_and_toolbar_commands_keep_the_captured_tab_insertion() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let existing = open_file(
        app.world_mut(),
        &dir.write("existing.txt", b"existing"),
        None,
        false,
        Some(1),
    )
    .unwrap();
    let existing_id = app.world().get::<PanelPresentation>(existing).unwrap().id;
    let path = dir.write("dropped.txt", b"dropped");
    app.world_mut()
        .resource_mut::<HostEffects>()
        .push_effect(bedtime_api::features::HostEffect {
            source: Some(existing),
            area: Some(1),
            tab_index: Some(0),
            action: HostAction::DropFile {
                path: path.to_string_lossy().into_owned(),
            },
        });
    process_app_requests(app.world_mut()).unwrap();
    let dropped = app.world().resource::<WorkbenchLayout>().focused.unwrap();
    let dropped_id = app.world().get::<PanelPresentation>(dropped).unwrap().id;
    assert_ne!(dropped, existing);
    let mut command = capture_app_request_from(
        app.world(),
        AppAction::PluginCommand(bedtime_plugin_color_picker::OPEN_COMMAND.into()),
        PanelTarget::LargestArea,
        Some(existing),
    );
    command.area = 1;
    command.tab_index = Some(1);
    let id = submit_app_request(app.world_mut(), command);
    process_app_requests(app.world_mut()).unwrap();
    let toolbar_panel = match poll_app_request(app.world_mut(), id).unwrap().unwrap() {
        AppRequestOutcome::Opened { panel, .. } => panel,
        outcome => panic!("{outcome:?}"),
    };
    let toolbar_id = app
        .world()
        .get::<PanelPresentation>(toolbar_panel)
        .unwrap()
        .id;
    let tabs = &app
        .world()
        .resource::<WorkbenchLayout>()
        .tiling
        .areas
        .iter()
        .find(|group| group.area == 1)
        .unwrap()
        .tabs;
    assert_eq!(tabs[0..3], [dropped_id, toolbar_id, existing_id]);
    // Dropping an already open file also moves its existing tab into position.
    let mut request = capture_app_request_from(
        app.world(),
        AppAction::HostAction(HostAction::DropFile {
            path: path.to_string_lossy().into_owned(),
        }),
        PanelTarget::LargestArea,
        Some(existing),
    );
    request.area = 1;
    request.tab_index = Some(2);
    let id = submit_app_request(app.world_mut(), request);
    process_app_requests(app.world_mut()).unwrap();
    assert!(
        matches!(poll_app_request(app.world_mut(), id).unwrap().unwrap(), AppRequestOutcome::Opened { panel, .. } if panel == dropped)
    );
    let tabs = &app
        .world()
        .resource::<WorkbenchLayout>()
        .tiling
        .areas
        .iter()
        .find(|group| group.area == 1)
        .unwrap()
        .tabs;
    assert_eq!(tabs[0..3], [toolbar_id, existing_id, dropped_id]);
}

#[test]
fn temporary_sessions_never_write_or_replace_global_templates() {
    let dir = TempDir::new();
    let config = dir.root().join("config");
    let mut store = WorkspaceStore::load(&crate::application::runtime::test_application_paths(
        &config,
    ))
    .unwrap();
    store
        .set_layout_default(
            LayoutSlot::Startup,
            Some(json!({"version":2,"panels":[],"tiling":bedtime_workbench::workspace::tiling_state::TilingState::default().to_value([]),"marker":"global template"})),
        )
        .unwrap();
    let before = std::fs::read(config.join("layouts.json")).unwrap();
    let mut app = app(&dir);
    let path = dir.write("temporary.txt", b"temporary");
    let panel = open_file(app.world_mut(), &path, None, false, None).unwrap();
    let session = app.world().non_send::<DocumentSession>().session_id();
    persist_workspace(app.world_mut()).unwrap();
    dispatch_app_command(
        app.world_mut(),
        WindowCommand::UseLayout(LayoutSlot::Layout4),
        PanelTarget::LastFocused,
    )
    .unwrap();
    persist_workspace(app.world_mut()).unwrap();
    assert!(
        app.world()
            .resource::<bedtime_plugin_workspaces::WorkspaceAttachment>()
            .saved()
            .is_none()
    );
    assert_eq!(
        app.world().non_send::<DocumentSession>().session_id(),
        session
    );
    assert!(app.world().entities().contains(panel));
    assert_eq!(std::fs::read(config.join("layouts.json")).unwrap(), before);
    assert!(
        !app.world()
            .non_send::<DocumentSession>()
            .options()
            .history_path
            .is_some()
    );
    assert_eq!(
        app.world()
            .resource::<bedtime_files::FileContext>()
            .directory,
        dir.root().canonicalize().unwrap()
    );
}

#[test]
fn save_workspace_retains_dirty_documents_history_and_suspended_panels() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let path = dir.write("draft.txt", b"before");
    let panel = open_file(app.world_mut(), &path, None, false, None).unwrap();
    let document = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    let revision = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    apply(
        app.world_mut(),
        AppAction::ApplyEdits {
            document,
            revision,
            edits: vec![ByteEdit {
                range: 0..6,
                bytes: b"draft".to_vec(),
            }],
        },
        Some(panel),
    );
    let edited = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    let session = app.world().non_send::<DocumentSession>().session_id();
    dispatch_app_command(
        app.world_mut(),
        WindowCommand::UseLayout(LayoutSlot::Layout4),
        PanelTarget::LastFocused,
    )
    .unwrap();
    let slot = app.world().resource::<LayoutSlots>().active();
    let spec = save_current_workspace(app.world_mut(), Some("Saved Example".into())).unwrap();
    assert_eq!(
        app.world()
            .resource::<bedtime_plugin_workspaces::WorkspaceAttachment>()
            .saved(),
        Some(&spec)
    );
    assert_eq!(
        app.world().non_send::<DocumentSession>().session_id(),
        session
    );
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .document_revision(document)
            .unwrap(),
        edited
    );
    assert_eq!(app.world().resource::<LayoutSlots>().active(), slot);
    assert!(app.world().entities().contains(panel));
    assert!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .dirty
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"before");
    dispatch_app_command(
        app.world_mut(),
        WindowCommand::UseLayout(LayoutSlot::Layout1),
        PanelTarget::LastFocused,
    )
    .unwrap();
    apply(
        app.world_mut(),
        AppAction::HostAction(HostAction::Undo { document }),
        Some(panel),
    );
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"before"
    );
}

#[test]
fn saving_a_workspace_persists_live_undo_in_its_repository_snapshot() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let path = dir.write("draft.txt", b"before");
    let panel = open_file(app.world_mut(), &path, None, false, None).unwrap();
    let document = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    let revision = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    apply(
        app.world_mut(),
        AppAction::ApplyEdits {
            document,
            revision,
            edits: vec![ByteEdit {
                range: 0..6,
                bytes: b"draft".to_vec(),
            }],
        },
        Some(panel),
    );
    let session = app.world().non_send::<DocumentSession>().session_id();
    let spec = save_current_workspace(app.world_mut(), None).unwrap();
    assert!(
        matches!(app.world().resource::<bedtime_plugin_workspaces::WorkspaceAttachment>(),
        bedtime_plugin_workspaces::WorkspaceAttachment::Saved(saved) if saved == &spec)
    );
    assert_eq!(
        app.world()
            .resource::<bedtime_plugin_workspaces::WorkspaceAttachment>()
            .saved(),
        Some(&spec)
    );
    let snapshot = app
        .world()
        .resource::<bedtime_plugin_workspaces::WorkspaceRepositoryResource>()
        .0
        .load(&spec)
        .unwrap()
        .unwrap();
    let history = snapshot
        .history
        .expect("saved workspace owns its undo history");
    assert_eq!(
        history,
        app.world_mut()
            .non_send_mut::<DocumentSession>()
            .history_snapshot()
            .unwrap()
    );
    assert!(app.world().resource::<ApplicationState>().error.is_none());
    assert_eq!(
        app.world().non_send::<DocumentSession>().session_id(),
        session
    );
    assert!(
        app.world()
            .non_send::<DocumentSession>()
            .options()
            .history_path
            .is_none()
    );
    assert!(app.world().entities().contains(panel));
    apply(
        app.world_mut(),
        AppAction::HostAction(HostAction::Undo { document }),
        Some(panel),
    );
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"before"
    );
}

#[test]
fn global_settings_requests_persist_in_emission_order_without_a_workspace() {
    #[derive(Resource, Default)]
    struct Observed(Vec<Value>);
    let dir = TempDir::new();
    let mut app = app(&dir);
    app.init_resource::<Observed>();
    bedtime_api::workbench::register(app.world_mut(), "fixture.settings", |registrar| {
        registrar.command(
            "fixture.settings.observe",
            "Observe settings",
            |In(_): In<CommandInvocation>, world: &mut World| {
                let value = world
                    .resource::<bedtime_api::settings::SettingsSnapshot>()
                    .get("bedtime.workbench", "tab_bar_add_button")
                    .unwrap()
                    .clone();
                world.resource_mut::<Observed>().0.push(value);
                Ok(Vec::new())
            },
        );
    })
    .unwrap();
    app.world_mut()
        .write_message(bedtime_api::settings::SettingsRequest::new(
            "bedtime.workbench",
            "tab_bar_add_button",
            json!(true),
        ));
    app.world_mut()
        .resource_mut::<WorkbenchRequests>()
        .submit(WorkbenchAction::Invoke(CommandInvocation {
            command: "fixture.settings.observe".into(),
            source: None,
            area: 1,
            arguments: Value::Null,
        }));
    process_app_requests(app.world_mut()).unwrap();
    assert_eq!(app.world().resource::<Observed>().0, vec![json!(true)]);
    assert!(
        app.world()
            .resource::<bedtime_plugin_workspaces::WorkspaceAttachment>()
            .saved()
            .is_none()
    );
    let saved: Value =
        serde_json::from_slice(&std::fs::read(dir.root().join("config/settings.json")).unwrap())
            .unwrap();
    assert_eq!(
        saved["overrides"]["bedtime.workbench"]["tab_bar_add_button"],
        true
    );
    assert!(!dir.root().join("config/registry.json").exists());
}
