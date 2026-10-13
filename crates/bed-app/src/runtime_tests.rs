use super::*;
use crate::test_support::TempDir;

#[path = "viewer_persistence_tests.rs"]
mod viewer_persistence;

#[path = "host_request_tests.rs"]
mod document_requests;

fn app(dir: &TempDir) -> App {
    let mut app = App::new();
    install_test_infrastructure(&mut app, &dir.root().join("config"));
    app.add_plugins((
        bed_runtime::BedRuntimePlugin,
        AppRuntimePlugin {
            options: AppOptions {
                working_directory: Some(dir.root().to_owned()),
                ..Default::default()
            },
        },
        bed_plugin_editor::EditorPlugin,
        bed_plugin_color_picker::ColorPickerPlugin,
        bed_plugin_json_tree::JsonTreePlugin,
    ));
    app.update();
    app
}

fn submit(world: &mut World, action: AppAction, source: Option<Entity>) -> AppRequestId {
    let request = capture_app_request_from(world, action, PanelTarget::LastFocused, source);
    submit_app_request(world, request)
}

fn apply(world: &mut World, action: AppAction, source: Option<Entity>) -> AppRequestOutcome {
    let id = submit(world, action, source);
    process_app_requests(world).unwrap();
    poll_app_request(world, id).unwrap().unwrap()
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
fn previews_replace_across_viewer_types_and_keep_existing_tab_order() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    app.add_plugins(bed_plugin_image_viewer::ImageViewerPlugin);
    let kept = open_file(
        app.world_mut(),
        &dir.write("kept.txt", b"kept"),
        None,
        false,
        Some(1),
    )
    .unwrap();
    let image = preview(&mut app, &dir.write("image.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="1" height="1"/></svg>"#), 1);
    assert_eq!(
        app.world()
            .get::<PanelBinding>(image)
            .unwrap()
            .viewer
            .as_deref(),
        Some(bed_plugin_image_viewer::VIEWER_ID)
    );
    let image_surface = panel_presentation(app.world(), image).unwrap();
    let text = preview(&mut app, &dir.write("preview.txt", b"preview"), 1);
    assert!(!app.world().entities().contains(image));
    assert!(!app.world().entities().contains(image_surface));
    assert!(app.world().entities().contains(kept));
    let group = &app.world().resource::<WorkbenchLayout>().tiling.areas[0];
    assert_eq!(
        group.tabs,
        vec![
            app.world().get::<PanelPresentation>(kept).unwrap().id,
            app.world().get::<PanelPresentation>(text).unwrap().id
        ]
    );
    assert_eq!(
        group.preview,
        Some(app.world().get::<PanelPresentation>(text).unwrap().id)
    );
    assert_eq!(group.selected, group.preview);
}

#[test]
fn local_file_viewers_share_the_preview_slot_with_document_viewers() {
    struct LocalViewer;

    impl bed_api::workbench::Panel for LocalViewer {
        fn title(&self, _: &World) -> String {
            "Local viewer".into()
        }

        fn draw(
            &mut self,
            _: &dear_imgui_rs::Ui,
            _: &mut World,
            _: &mut Vec<WorkbenchAction>,
            _: &bed_api::workbench::ShortcutInput,
        ) {
        }
    }
    let dir = TempDir::new();
    let mut app = app(&dir);
    app.world_mut()
        .resource_mut::<bed_api::features::FeatureRegistry>()
        .register_with("fixture.local", |registrar| {
            registrar.panel_ref("fixture.local.panel");
            registrar.local_file_viewer(
                "fixture.local.viewer",
                "Local viewer",
                "fixture.local.panel",
                &["local"],
            );
        })
        .unwrap();
    bed_api::workbench::register(app.world_mut(), "fixture.local", |registrar| {
        registrar.panel(
            "fixture.local.panel",
            "Local viewer",
            |_: In<PanelCreation>| -> bed_api::workbench::PanelResult { Ok(Box::new(LocalViewer)) },
        );
    })
    .unwrap();
    let local = preview(&mut app, &dir.write("first.local", b"\0local"), 1);
    let binding = app.world().get::<PanelBinding>(local).unwrap();
    assert!(binding.document.is_none());
    assert!(binding.input.local_file().is_some());
    let text = preview(&mut app, &dir.write("text.txt", b"text"), 1);
    assert!(!app.world().entities().contains(local));
    let second = preview(&mut app, &dir.write("second.local", b"\0local"), 1);
    assert!(!app.world().entities().contains(text));
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        Some(app.world().get::<PanelPresentation>(second).unwrap().id)
    );
    let path = dir.root().join("second.local");
    let duplicate = open_file(app.world_mut(), &path, None, true, Some(1)).unwrap();
    assert_ne!(duplicate, second);
    assert_eq!(
        open_file(app.world_mut(), &path, None, false, Some(1)).unwrap(),
        duplicate,
        "ordinary local-file opening still chooses its most recently focused presentation"
    );
}

#[test]
fn preview_requests_do_not_demote_kept_files_and_explicit_opens_retain_previews() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let kept_path = dir.write("kept.txt", b"kept");
    let kept = open_file(app.world_mut(), &kept_path, None, false, Some(1)).unwrap();
    let path = dir.write("preview.txt", b"preview");
    let first = preview(&mut app, &path, 1);
    assert_eq!(preview(&mut app, &kept_path, 1), kept);
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        Some(app.world().get::<PanelPresentation>(first).unwrap().id)
    );
    assert_eq!(
        open_file(app.world_mut(), &path, None, false, Some(1)).unwrap(),
        first
    );
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        None
    );
    let second = preview(&mut app, &dir.write("second.txt", b"second"), 1);
    // Drop/menu-style ordinary openings create kept tabs beside the preview.
    let other = open_file(
        app.world_mut(),
        &dir.write("other.txt", b"other"),
        None,
        false,
        Some(1),
    )
    .unwrap();
    assert!(app.world().entities().contains(first));
    assert!(app.world().entities().contains(second));
    assert!(app.world().entities().contains(other));
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        Some(app.world().get::<PanelPresentation>(second).unwrap().id)
    );
}

#[test]
fn plugin_keep_requests_and_edits_retain_the_owning_preview_tab() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    app.world_mut().non_send_mut::<Settings>().settings["autosave"] = json!(false);
    let first = preview(&mut app, &dir.write("first.txt", b"first"), 1);
    let surface = panel_presentation(app.world(), first).unwrap();
    apply(
        app.world_mut(),
        AppAction::WorkbenchAction(WorkbenchAction::KeepPanel(surface)),
        Some(surface),
    );
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        None
    );
    let second = preview(&mut app, &dir.write("second.txt", b"second"), 1);
    let document = app
        .world()
        .get::<PanelBinding>(second)
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
                bytes: b"edited".to_vec(),
            }],
        },
        Some(second),
    );
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().tiling.areas[0].preview,
        None
    );
    // Saving cannot restore replaceability once editing promoted the tab.
    apply(
        app.world_mut(),
        AppAction::SaveDocuments(vec![document]),
        Some(second),
    );
    preview(&mut app, &dir.write("third.txt", b"third"), 1);
    assert!(app.world().entities().contains(first));
    assert!(app.world().entities().contains(second));
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"edited"
    );
}

#[test]
fn each_area_has_its_own_preview_and_failed_opens_preserve_it() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let area = app
        .world_mut()
        .resource_mut::<WorkbenchLayout>()
        .tiling
        .split_area(1, 0, true, [100, 100])
        .unwrap();
    let left = preview(&mut app, &dir.write("left.txt", b"left"), 1);
    let right = preview(&mut app, &dir.write("right.txt", b"right"), area);
    let before = app.world().resource::<WorkbenchLayout>().tiling.clone();
    assert!(
        open_file_with_origin(
            app.world_mut(),
            &dir.root().join("missing.txt"),
            None,
            false,
            Some(1),
            None,
            FileOpenMode::Preview
        )
        .is_err()
    );
    assert_eq!(app.world().resource::<WorkbenchLayout>().tiling, before);
    preview(&mut app, &dir.write("replacement.txt", b"replacement"), 1);
    assert!(!app.world().entities().contains(left));
    assert!(app.world().entities().contains(right));
}

#[test]
fn syntax_spans_use_the_selected_theme_and_update_existing_documents() {
    let dir = TempDir::new();
    let mut app = app(&dir);
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
            .non_send_mut::<Settings>()
            .select_theme(theme)
            .unwrap();
        app.update();
        let palette = app.world().non_send::<Settings>().highlight_colors();
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
        crate::draw_ui(app.world_mut(), imgui.frame()).unwrap();
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
        crate::draw_ui(app.world_mut(), imgui.frame()).unwrap();
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

    app.world_mut()
        .non_send_mut::<bed_plugin_editor::EditorConfig>()
        .navigation_animations = false;
    navigate(&mut app, 0);
    for _ in 0..2 {
        app.update();
        crate::draw_ui(app.world_mut(), imgui.frame()).unwrap();
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
        assert_eq!(
            apply(
                app.world_mut(),
                AppAction::HostAction(HostAction::NavigateLocation {
                    path: path.to_string_lossy().into_owned(),
                    row,
                    utf16_column: 3,
                    origin: None,
                }),
                Some(source),
            ),
            AppRequestOutcome::Applied
        );
        app.world().resource::<WorkbenchLayout>().focused.unwrap()
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
        crate::draw_ui(app.world_mut(), imgui.frame()).unwrap();
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
        let views = world.non_send::<bed_plugin_editor::EditorViews>();
        let surface = panel_presentation(world, panel).unwrap();
        let presentation = views.text(surface).unwrap().presentation();
        let geometry = world
            .get::<bed_api::workbench::PanelGeometry>(panel)
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
        crate::draw_ui(app.world_mut(), imgui.frame()).unwrap();
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
    crate::draw_ui(app.world_mut(), imgui.frame()).unwrap();
    drop(imgui.render_legacy());
    assert!(app.world().contains_non_send::<DocumentSession>());
    assert!(
        app.world()
            .contains_non_send::<bed_plugin_editor::EditorViews>()
    );
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .document_for_view(view),
        app.world().get::<PanelBinding>(panel).unwrap().document
    );
    assert!(
        app.world()
            .get::<bed_api::workbench::PanelGeometry>(panel)
            .unwrap()
            .visible
    );
}

#[test]
fn saving_a_captured_panel_never_redirects_to_newer_focus() {
    for from_widget in [false, true] {
        let dir = TempDir::new();
        let mut app = app(&dir);
        let first = dir.root().join("first.txt");
        let second = dir.root().join("second.txt");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let a = open_file(app.world_mut(), &first, None, false, None).unwrap();
        let document = app
            .world()
            .get::<PanelBinding>(a)
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
                    range: 0..5,
                    bytes: b"changed".to_vec(),
                }],
            },
            Some(a),
        );
        let request = capture_app_request(
            app.world(),
            if from_widget {
                AppAction::HostAction(HostAction::Save { document })
            } else {
                AppAction::Command(WindowCommand::Save)
            },
            PanelTarget::LastFocused,
        );
        let b = open_file(app.world_mut(), &second, None, false, None).unwrap();
        let id = submit_app_request(app.world_mut(), request);
        process_app_requests(app.world_mut()).unwrap();
        assert!(matches!(
            poll_app_request(app.world_mut(), id),
            Some(Ok(AppRequestOutcome::Saved { .. }))
        ));
        assert_eq!(std::fs::read(&first).unwrap(), b"changed");
        assert_eq!(std::fs::read(&second).unwrap(), b"second");
        assert_eq!(app.world().resource::<WorkbenchLayout>().focused, Some(b));
    }
}

#[test]
fn inactive_viewers_remain_owned_until_the_layout_panel_closes() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let path = dir.root().join("values.json");
    std::fs::write(&path, b"{\"value\":1}").unwrap();
    let panel = open_file(app.world_mut(), &path, Some("bed.editor.text"), false, None).unwrap();
    let text = panel_presentation(app.world(), panel).unwrap();
    switch_viewer(app.world_mut(), panel, "bed.json.viewer").unwrap();
    let json = panel_presentation(app.world(), panel).unwrap();
    assert_ne!(text, json);
    switch_viewer(app.world_mut(), panel, "bed.editor.text").unwrap();
    assert_eq!(panel_presentation(app.world(), panel), Some(text));
    assert!(app.world().entities().contains(json));
    apply(app.world_mut(), AppAction::ClosePanel(panel), Some(panel));
    assert!(!app.world().entities().contains(text));
    assert!(!app.world().entities().contains(json));
}

#[test]
fn a_registered_viewer_factory_failure_retains_the_active_presentation_and_document() {
    #[derive(Component)]
    struct FailedAllocation;
    let dir = TempDir::new();
    let mut app = app(&dir);
    bed_workbench::register(app.world_mut(), "fixture", |registrar| {
        registrar.panel(
            "fixture.panel",
            "Rejected viewer",
            |In(creation): In<PanelCreation>,
             world: &mut World|
             -> bed_api::workbench::PanelResult {
                world.entity_mut(creation.entity).insert(FailedAllocation);
                Err("Factory rejected the input".into())
            },
        );
    })
    .unwrap();
    app.world_mut()
        .resource_mut::<bed_api::features::FeatureRegistry>()
        .register_with("fixture", |registrar| {
            registrar.panel_ref("fixture.panel");
            registrar.viewer(
                "fixture.viewer",
                "Rejected viewer",
                "fixture.panel",
                &[],
                bed_documents::DocumentKind::Text,
            );
        })
        .unwrap();
    let path = dir.write("document.txt", b"original");
    let tab = open_file(app.world_mut(), &path, None, false, None).unwrap();
    let surface = panel_presentation(app.world(), tab).unwrap();
    let binding = app.world().get::<PanelBinding>(tab).unwrap().clone();
    let document = binding.document.unwrap();
    let view = binding.view.unwrap();
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .with_commands(view, |commands| commands.type_text(b"unsaved "))
        .unwrap();
    let before = app
        .world()
        .non_send::<DocumentSession>()
        .snapshot(document)
        .unwrap();
    let selections = app
        .world()
        .non_send::<DocumentSession>()
        .view_snapshot(view)
        .unwrap()
        .selections;
    let id = submit(
        app.world_mut(),
        AppAction::SwitchViewer {
            panel: tab,
            viewer: "fixture.viewer".into(),
        },
        Some(tab),
    );
    app.update();
    assert!(
        poll_app_request_result(app.world_mut(), id)
            .unwrap()
            .unwrap_err()
            .message
            .contains("Factory rejected")
    );
    assert_eq!(panel_presentation(app.world(), tab), Some(surface));
    assert_eq!(app.world().get::<PanelBinding>(tab).unwrap(), &binding);
    assert!(
        app.world()
            .get::<PanelRecord>(tab)
            .unwrap()
            .unavailable
            .is_none()
    );
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        before.bytes
    );
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .view_snapshot(view)
            .unwrap()
            .selections,
        selections
    );
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .view_count(document),
        1
    );
    assert_eq!(
        app.world_mut()
            .query::<&FailedAllocation>()
            .iter(app.world())
            .count(),
        0
    );
    assert_eq!(std::fs::read(path).unwrap(), b"original");
}

#[test]
fn missing_feature_records_roundtrip_without_discarding_opaque_data() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let record = json!({"id":37,"panel_type":"missing.feature.panel","viewer":"missing.feature.viewer","state":{"draft":"keep"},"extension":{"future":[1,2,3]}});
    let value = json!({"version":2,"panels":[record.clone()],"focused":37});
    restore_layout(app.world_mut(), &value, false).unwrap();
    let panel = panel_entities(app.world_mut())[0];
    assert_eq!(app.world().get::<PanelPresentation>(panel).unwrap().id, 37);
    let saved = layouts::snapshot(app.world_mut()).unwrap()["layouts"]
        [app.world().resource::<LayoutSlots>().active().key()]
    .clone();
    assert_eq!(saved["panels"][0]["extension"], record["extension"]);
    assert_eq!(saved["panels"][0]["state"], record["state"]);
}

#[test]
#[cfg(feature = "workspaces")]
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
    app.world_mut().non_send_mut::<WorkspacePersistence>().store.set_layout(&spec,json!({"version":2,"panels":[{"id":id,"panel_type":"bed.editor.text-panel","viewer":"bed.editor.text","path":saved,"state":{}}],"focused":id})).unwrap();
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
        bed_plugin_editor::TEXT_PANEL_TYPE,
        PanelInput::None,
        Value::Null,
        None,
        None,
        false,
    )
    .unwrap();
    let last = open_panel(
        app.world_mut(),
        bed_plugin_editor::TEXT_PANEL_TYPE,
        PanelInput::None,
        Value::Null,
        None,
        None,
        false,
    )
    .unwrap();
    let singleton = open_panel(
        app.world_mut(),
        "bed.color.panel",
        PanelInput::None,
        Value::Null,
        None,
        None,
        false,
    )
    .unwrap();
    let mut layout = bed_workbench::workspace::tiling::Layout::default();
    let largest = layout.split(1, 0, 3000, 100).unwrap();
    let last_area = layout.split(1, 1, 5000, 100).unwrap();
    let mut tiling = bed_workbench::workspace::tiling_state::TilingState::new(layout);
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
fn default_opening_uses_geometry_while_explicit_areas_override_it() {
    for explicit in [false, true] {
        let dir = TempDir::new();
        let mut app = app(&dir);
        let (first, last, _, largest, last_area) = target_fixture(&mut app);
        let invocation = bed_workbench::capture_command(app.world(), "bed.color.new", Value::Null);
        assert_eq!(invocation.source, Some(last));
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
        // Intent capture uses the default destination, even when its source is
        // the focused sidebar. A drop's explicit area takes precedence.
        if explicit {
            let intents = app.world_mut().resource_mut::<HostEffects>().take();
            for intent in intents {
                app.world_mut().resource_mut::<HostEffects>().push_at(
                    intent.source,
                    last_area,
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
            Some(if explicit { last_area } else { largest })
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
            .resource::<bed_api::workbench::WorkbenchRegistry>()
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
            bed_workbench::prepare_shortcuts(app.world_mut(), ui);
            bed_workbench::global_shortcuts(app.world_mut(), ui);
            drop(imgui.render_legacy());
        } else {
            let invocation =
                bed_workbench::capture_command(app.world(), "bed.app.NewDocument", Value::Null);
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
fn explicit_open_file_targets_are_captured_before_later_focus_changes() {
    for target in [PanelTarget::LastFocused, PanelTarget::LargestArea] {
        let dir = TempDir::new();
        let mut app = app(&dir);
        let (first, last, _, largest, last_area) = target_fixture(&mut app);
        let path = dir.write("target.txt", b"target");
        let request = capture_app_request_from(
            app.world(),
            AppAction::HostAction(HostAction::OpenFileIn {
                path: path.to_string_lossy().into_owned(),
                viewer: None,
                target,
            }),
            PanelTarget::LargestArea,
            Some(last),
        );
        let expected = match target {
            PanelTarget::LastFocused => last_area,
            PanelTarget::LargestArea => largest,
        };
        assert_eq!(request.area, expected);
        focus_panel(app.world_mut(), first, true).unwrap();
        let id = submit_app_request(app.world_mut(), request);
        process_app_requests(app.world_mut()).unwrap();
        let panel = match poll_app_request_result(app.world_mut(), id)
            .unwrap()
            .unwrap()
        {
            AppRequestOutcome::Opened { panel, .. } => panel,
            _ => panic!(),
        };
        assert_eq!(
            app.world()
                .resource::<WorkbenchLayout>()
                .tiling
                .area_for(app.world().get::<PanelPresentation>(panel).unwrap().id),
            Some(expected)
        );
    }
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
    bed_api::workbench::register(app.world_mut(), "bed.test-target", |registrar| {
        registrar.command(
            "bed.test-target.show",
            "Show",
            |In(invocation): In<CommandInvocation>,
             mut intents: ResMut<HostEffects>|
             -> bed_api::workbench::CommandResult {
                intents.push(
                    invocation.source,
                    HostAction::ShowPanelIn {
                        panel_type: "bed.color.panel".into(),
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
        AppAction::PluginCommand("bed.test-target.show".into()),
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
            panel_type: "bed.color.panel".into(),
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
            panel_type: "bed.color.panel".into(),
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
            panel_type: "bed.color.panel".into(),
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
fn plugin_only_apps_drive_requests_and_keep_revision_errors_typed() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let path = dir.write("revision.txt", b"original");
    let request = capture_app_request(
        app.world(),
        AppAction::OpenFile(path),
        PanelTarget::LargestArea,
    );
    let id = submit_app_request(app.world_mut(), request);
    app.update();
    let panel = match poll_app_request_result(app.world_mut(), id)
        .unwrap()
        .unwrap()
    {
        AppRequestOutcome::Opened { panel, .. } => panel,
        _ => panic!(),
    };
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
    let stale = capture_app_request_from(
        app.world(),
        AppAction::ApplyEdits {
            document,
            revision,
            edits: vec![ByteEdit {
                range: 0..1,
                bytes: b"x".to_vec(),
            }],
        },
        PanelTarget::LastFocused,
        Some(panel),
    );
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .apply_edits(
            document,
            revision,
            &[ByteEdit {
                range: 0..1,
                bytes: b"y".to_vec(),
            }],
        )
        .unwrap();
    let id = submit_app_request(app.world_mut(), stale);
    app.update();
    assert_eq!(
        poll_app_request_result(app.world_mut(), id)
            .unwrap()
            .unwrap_err()
            .kind,
        io::ErrorKind::WouldBlock
    );
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"yriginal"
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

#[test]
fn a_failed_batch_retains_panels_and_never_applies_later_intentions() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let path = dir.write("batch.txt", b"before");
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
        AppAction::HostActions {
            actions: vec![
                HostAction::ApplyEdits {
                    document,
                    revision,
                    edits: vec![],
                },
                HostAction::Save { document },
            ],
        },
        Some(panel),
    );
    app.update();
    assert_eq!(
        poll_app_request_result(app.world_mut(), id)
            .unwrap()
            .unwrap_err()
            .kind,
        io::ErrorKind::WouldBlock
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"before");
    assert!(app.world().entities().contains(panel));
}

fn registered_close(In(invocation): In<CommandInvocation>) -> bed_api::workbench::CommandResult {
    Ok(vec![WorkbenchAction::ClosePanel(
        invocation.source.unwrap(),
    )])
}

#[test]
fn registered_command_receipts_follow_actual_save_and_close_effects() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let path = dir.write("close.txt", b"before");
    bed_api::workbench::register(app.world_mut(), "receipt", |registrar| {
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
fn generic_and_editor_ingress_use_one_submission_order_without_redirecting_sources() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let a = open_file(
        app.world_mut(),
        &dir.write("a.txt", b"a"),
        None,
        false,
        None,
    )
    .unwrap();
    let b = open_file(
        app.world_mut(),
        &dir.write("b.txt", b"b"),
        None,
        false,
        None,
    )
    .unwrap();
    let first = submit(app.world_mut(), AppAction::FocusPanel(a), None);
    let second = app
        .world_mut()
        .resource_mut::<WorkbenchRequests>()
        .submit(WorkbenchAction::FocusPanel(b));
    let third = submit(app.world_mut(), AppAction::ObserveFocus(a), None);
    app.update();
    assert!(
        poll_app_request_result(app.world_mut(), first)
            .unwrap()
            .is_ok()
    );
    assert!(
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .take_result(second)
            .unwrap()
            .is_ok()
    );
    assert!(
        poll_app_request_result(app.world_mut(), third)
            .unwrap()
            .is_ok()
    );
    assert_eq!(app.world().resource::<WorkbenchLayout>().focused, Some(a));
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
        move |In(invocation): In<bed_api::workbench::PanelExternalFilesInvocation>,
              mut intents: ResMut<HostEffects>|
              -> Result<bed_api::workbench::ExternalFileDropResponse, String> {
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
            Ok(bed_api::workbench::ExternalFileDropResponse::Accepted)
        },
    );
    app.world_mut()
        .entity_mut(surface)
        .insert(bed_api::workbench::PanelExternalFiles(handler));
    let event = bed_api::workbench::ExternalFileDrag {
        paths: vec![dir.root().join("handler-owns-this-path")],
        position: [0., 0.],
        viewport: 0,
        phase: bed_api::workbench::ExternalFileDragPhase::Drop,
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
            .kind,
        io::ErrorKind::WouldBlock
    );
    assert_eq!(std::fs::read(path).unwrap(), b"dropped");
}

#[test]
fn dropping_a_directory_stays_temporary_without_replacing_carried_documents() {
    let dir = TempDir::new();
    let mut app = app(&dir);
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
    let event = bed_api::workbench::ExternalFileDrag {
        paths: vec![project.clone()],
        position: [0., 0.],
        viewport: 0,
        phase: bed_api::workbench::ExternalFileDragPhase::Drop,
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
    assert!(app.world().resource::<WorkspaceState>().spec.is_none());
    assert_eq!(
        app.world().resource::<bed_files::FileContext>().directory,
        project.canonicalize().unwrap()
    );
    assert!(app.world().entities().contains(panel));
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
fn unavailable_viewers_and_missing_files_keep_their_original_records() {
    for missing_file in [false, true] {
        let dir = TempDir::new();
        let mut app = app(&dir);
        let path = if missing_file {
            dir.root().join("missing.txt")
        } else {
            dir.write("exists.txt", b"retained")
        };
        let viewer = if missing_file {
            bed_plugin_editor::TEXT_VIEWER
        } else {
            "missing.viewer"
        };
        let record = json!({"id":41,"panel_type":bed_plugin_editor::TEXT_PANEL_TYPE,"viewer":viewer,"path":path,"state":{"position":17},"future":{"keep":[1,2,3]}});
        restore_layout(
            app.world_mut(),
            &json!({"version":2,"panels":[record.clone()],"focused":41}),
            false,
        )
        .unwrap();
        let panel = panel_entities(app.world_mut())[0];
        let surface = panel_presentation(app.world(), panel).unwrap();
        assert!(
            app.world()
                .get::<PanelRecord>(surface)
                .unwrap()
                .unavailable
                .is_some()
        );
        assert!(
            app.world()
                .get::<bed_plugin_editor::EditorPanelState>(surface)
                .is_none(),
            "unavailable content cannot silently create an unrelated scratch editor"
        );
        let saved = layouts::snapshot(app.world_mut()).unwrap()["layouts"]
            [app.world().resource::<LayoutSlots>().active().key()]
        .clone();
        assert_eq!(saved["panels"][0], record);
        assert_eq!(
            bed_workbench::panel_state(app.world_mut(), panel).unwrap(),
            record["state"]
        );
    }
}

#[test]
fn invalid_saved_identities_and_membership_recover_all_file_contents() {
    for identities in [[1, 1], [0, 2], [1, 2]] {
        let dir = TempDir::new();
        let mut app = app(&dir);
        let files = [
            dir.write("first.txt", b"first"),
            dir.write("second.txt", b"second"),
        ];
        let records=files.iter().zip(identities).map(|(path,id)|json!({"id":id,"panel_type":bed_plugin_editor::TEXT_PANEL_TYPE,"viewer":bed_plugin_editor::TEXT_VIEWER,"path":path,"state":{}})).collect::<Vec<_>>();
        restore_layout(
            app.world_mut(),
            &json!({"version":2,"panels":records,"tiling":{"version":1,"layout":{},"areas":[]}}),
            false,
        )
        .unwrap();
        let panels = panel_entities(app.world_mut());
        assert_eq!(panels.len(), 2);
        let ids = panels
            .iter()
            .map(|panel| app.world().get::<PanelPresentation>(*panel).unwrap().id)
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), 2);
        assert!(!ids.contains(&0));
        for (panel, expected) in panels
            .iter()
            .zip([b"first".as_slice(), b"second".as_slice()])
        {
            let document = app
                .world()
                .get::<PanelBinding>(*panel)
                .unwrap()
                .document
                .unwrap();
            assert_eq!(
                app.world()
                    .non_send::<DocumentSession>()
                    .snapshot(document)
                    .unwrap()
                    .bytes,
                expected
            );
        }
        assert!(app.world().resource::<WorkspaceState>().error.is_some());
        let saved = layouts::snapshot(app.world_mut()).unwrap()["layouts"]
            [app.world().resource::<LayoutSlots>().active().key()]
        .clone();
        let fresh = TempDir::new();
        let mut restored = self::app(&fresh);
        restore_layout(restored.world_mut(), &saved, false).unwrap();
        assert_eq!(panel_entities(restored.world_mut()).len(), 2);
    }
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
        "bed.color.panel",
        PanelInput::None,
        Value::Null,
        None,
        None,
        false,
    )
    .unwrap();
    let mut layout = bed_workbench::workspace::tiling::Layout::default();
    let right = layout.split(1, 0, 7000, 1).unwrap();
    let mut tiling = bed_workbench::workspace::tiling_state::TilingState::new(layout);
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
        "bed.color.panel"
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
fn restored_empty_areas_and_failed_splits_never_create_or_discard_content() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    let mut layout = bed_workbench::workspace::tiling::Layout::default();
    let empty = layout.split(1, 0, 100, 1).unwrap();
    let tiling = bed_workbench::workspace::tiling_state::TilingState::new(layout);
    restore_layout(
        app.world_mut(),
        &json!({"version":2,"panels":[],"tiling":tiling.to_value([]),"focused_area":empty}),
        false,
    )
    .unwrap();
    assert!(panel_entities(app.world_mut()).is_empty());
    assert_eq!(app.world().resource::<WorkbenchLayout>().tiling, tiling);
    assert_eq!(
        app.world().resource::<WorkbenchLayout>().focused_area,
        Some(empty)
    );
    let panel = open_file(
        app.world_mut(),
        &dir.write("narrow.txt", b"narrow"),
        None,
        false,
        Some(1),
    )
    .unwrap();
    let document = app
        .world()
        .get::<PanelBinding>(panel)
        .unwrap()
        .document
        .unwrap();
    let before = app.world().resource::<WorkbenchLayout>().tiling.clone();
    assert!(split_panel(app.world_mut(), panel, 0, true).is_err());
    assert_eq!(panel_entities(app.world_mut()), vec![panel]);
    assert_eq!(app.world().resource::<WorkbenchLayout>().tiling, before);
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .view_count(document),
        1
    );
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
        .push_effect(bed_api::features::HostEffect {
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
        AppAction::PluginCommand(bed_plugin_color_picker::OPEN_COMMAND.into()),
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
fn temporary_sessions_never_write_or_replace_legacy_session_state() {
    let dir = TempDir::new();
    let config = dir.root().join("config");
    let mut store = WorkspaceStore::load(&config).unwrap();
    store
        .save_standalone_module_settings(json!({"missing.plugin":{"draft":"legacy"}}))
        .unwrap();
    let before = std::fs::read(config.join("workspaces.json")).unwrap();
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
    assert!(app.world().resource::<WorkspaceState>().spec.is_none());
    assert_eq!(
        app.world().non_send::<DocumentSession>().session_id(),
        session
    );
    assert!(app.world().entities().contains(panel));
    assert_eq!(
        std::fs::read(config.join("workspaces.json")).unwrap(),
        before
    );
    assert!(
        !app.world()
            .non_send::<DocumentSession>()
            .options()
            .persistent_history
    );
    assert_eq!(
        app.world().resource::<bed_files::FileContext>().directory,
        dir.root().canonicalize().unwrap()
    );
}

#[test]
#[cfg(feature = "workspaces")]
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
        app.world().resource::<WorkspaceState>().spec.as_ref(),
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
#[cfg(not(feature = "workspaces"))]
fn omitting_workspace_integration_omits_workspace_commands_and_keeps_directory_services() {
    let dir = TempDir::new();
    let mut app = app(&dir);
    for command in [
        bed_plugin_workspaces::SAVE_COMMAND,
        bed_plugin_workspaces::PROMOTE_COMMAND,
        "bed.app.NewProjects",
        "bed.app.Projects",
    ] {
        assert!(
            app.world()
                .resource::<bed_api::workbench::WorkbenchRegistry>()
                .command(command)
                .is_none()
        );
    }
    let path = dir.write("quick.txt", b"quick edit");
    let panel = open_file(app.world_mut(), &path, None, false, None).unwrap();
    assert!(app.world().entities().contains(panel));
    assert!(app.world().resource::<WorkspaceState>().spec.is_none());
    assert!(
        app.world()
            .non_send::<DocumentSession>()
            .options()
            .project_root
            .is_some()
    );
    assert!(
        !app.world()
            .non_send::<DocumentSession>()
            .options()
            .persistent_history
    );
}

#[test]
fn global_settings_requests_persist_in_emission_order_without_a_workspace() {
    #[derive(Resource, Default)]
    struct Observed(Vec<Value>);
    let dir = TempDir::new();
    let mut app = app(&dir);
    app.init_resource::<Observed>();
    bed_api::workbench::register(app.world_mut(), "fixture.settings", |registrar| {
        registrar.command(
            "fixture.settings.observe",
            "Observe settings",
            |In(_): In<CommandInvocation>, world: &mut World| {
                let value = world.non_send::<Settings>().settings["architecture_probe"].clone();
                world.resource_mut::<Observed>().0.push(value);
                Ok(Vec::new())
            },
        );
    })
    .unwrap();
    app.world_mut()
        .write_message(bed_api::settings::SettingsRequest::new(
            "fixture.settings",
            "architecture_probe",
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
    assert!(app.world().resource::<WorkspaceState>().spec.is_none());
    let saved: Value =
        serde_json::from_slice(&std::fs::read(dir.root().join("config/settings.json")).unwrap())
            .unwrap();
    assert_eq!(saved["architecture_probe"], true);
    assert!(!dir.root().join("config/workspaces.json").exists());
}
