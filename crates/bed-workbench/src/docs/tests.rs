use super::*;
use crate::{WorkbenchPlugin, WorkbenchRequests, focus_panel, open_panel, process_requests};
use bed_keybindings::{Action as KeyAction, Overrides, Platform};

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((WorkbenchPlugin, DocsPlugin));
    app
}

#[test]
fn docs_reuses_its_panel_and_tracks_live_frontends_across_focus_and_layout_changes() {
    let mut app = app();
    let world = app.world_mut();
    let work = open_panel(world, json!({"panel_type":"fixture.work"}), None, None).unwrap();
    let first = world
        .spawn((
            api::PanelOwner(work),
            PanelRecord {
                panel_type: "fixture.first".into(),
                serialized: Value::Null,
                unavailable: None,
            },
        ))
        .id();
    let second = world
        .spawn((
            api::PanelOwner(work),
            PanelRecord {
                panel_type: "fixture.second".into(),
                serialized: Value::Null,
                unavailable: None,
            },
        ))
        .id();
    world.entity_mut(work).insert(api::PanelFrontend(first));
    assert_eq!(target(world), Some(first));
    let docs = open_panel(
        world,
        json!({"panel_type":DOCS_PANEL_ID,"state":{"zoom":0.7,"expanded":[false,true,true]}}),
        None,
        None,
    )
    .unwrap();
    assert_eq!(target(world), Some(first), "Docs keeps the last work panel");
    world.entity_mut(work).insert(api::PanelFrontend(second));
    assert_eq!(
        target(world),
        Some(second),
        "resolve the current viewer, not a retained frontend"
    );
    world
        .resource_mut::<WorkbenchRequests>()
        .push(Action::Invoke(CommandInvocation {
            command: SHOW_DOCS_COMMAND.into(),
            source: Some(work),
            area: 1,
            arguments: Value::Null,
        }));
    process_requests(world);
    assert_eq!(world.resource::<WorkbenchLayout>().focused, Some(docs));
    let saved = crate::panel_state(world, docs).unwrap();
    assert!((saved["zoom"].as_f64().unwrap() - 0.7).abs() < 0.00001);
    assert_eq!(saved["expanded"], json!([true, true]));
    world.entity_mut(work).insert(api::PanelSuspended);
    assert_eq!(
        target(world),
        None,
        "another layout's retained panels are excluded"
    );
    world.entity_mut(work).remove::<api::PanelSuspended>();
    focus_panel(world, work);
    assert_eq!(target(world), Some(second));
    crate::commit_close(world, work);
    assert_eq!(
        target(world),
        None,
        "closed frontends cannot remain a target"
    );
}

#[test]
fn rows_use_supported_scopes_effective_overrides_and_binding_precedence() {
    let local = Scope::panel("fixture.panel");
    let widget = Scope::panel("fixture.widget");
    let actions = vec![
        KeyAction::new(
            "fixture.open",
            "Open",
            "fixture",
            local.clone(),
            &["Primary+O", "F2"],
        ),
        KeyAction::new(
            "fixture.remapped",
            "Remapped",
            "fixture",
            local.clone(),
            &["F3"],
        ),
        KeyAction::new(
            "fixture.removed",
            "Removed",
            "fixture",
            local.clone(),
            &["F4"],
        ),
        KeyAction::new("fixture.widget", "Widget", "fixture", widget, &["F5"]),
        KeyAction::new(
            "fixture.copy",
            "Copy",
            "fixture",
            Scope::Focused,
            &["Primary+C"],
        ),
        KeyAction::new(
            "fixture.cut",
            "Unsupported Cut",
            "fixture",
            Scope::Focused,
            &["Primary+X"],
        ),
        KeyAction::new(
            "fixture.global",
            "Global Open",
            "fixture",
            Scope::Global,
            &["Primary+O"],
        ),
        KeyAction::new("fixture.unbound", "Unbound", "fixture", Scope::Global, &[]),
    ];
    let mut overrides = Overrides::default();
    overrides
        .replace(&actions[1], Some(0), "Primary+R".parse().unwrap())
        .unwrap();
    overrides.actions.insert(actions[2].id.clone(), Vec::new());
    let docs = PanelDocumentation::default()
        .shortcut_scope("fixture.widget")
        .shortcut_actions(&["fixture.copy"]);
    for (platform, primary) in [(Platform::Mac, "Command"), (Platform::Other, "Ctrl")] {
        let bindings = Bindings::new(actions.clone(), &overrides, platform);
        let rows = key_rows(&bindings, Some("fixture.panel"), Some(&docs), false);
        assert_eq!(
            rows,
            vec![
                KeyRow {
                    label: "Open".into(),
                    keys: "F2".into()
                },
                KeyRow {
                    label: "Remapped".into(),
                    keys: format!("{primary}+R")
                },
                KeyRow {
                    label: "Widget".into(),
                    keys: "F5".into()
                },
                KeyRow {
                    label: "Copy".into(),
                    keys: format!("{primary}+C")
                },
            ]
        );
        assert_eq!(
            key_rows(&bindings, None, None, true),
            vec![KeyRow {
                label: "Global Open".into(),
                keys: format!("{primary}+O")
            }]
        );
        assert_eq!(
            key_rows(&bindings, Some("fixture.panel"), None, false).len(),
            2,
            "shortcuts work without usage notes"
        );
    }
}

#[test]
fn zoom_and_sections_round_trip_and_external_zoom_is_bounded() {
    let world = World::new();
    let mut panel = DocsPanel::from_state(&Value::Null);
    assert_eq!(panel.zoom, 1.0);
    assert_eq!(panel.expanded, [true, false]);
    panel.expanded = [false, true];
    panel.symbol_expanded = false;
    panel.zoom_by(1.0 / 1.1);
    let restored = DocsPanel::from_state(&panel.save(&world));
    assert_eq!(restored.zoom, panel.zoom);
    assert_eq!(restored.expanded, panel.expanded);
    assert!(!restored.symbol_expanded);
    assert_eq!(
        DocsPanel::from_state(&json!({"expanded":[false,false,true]})).expanded,
        [false, true],
    );
    assert_eq!(
        DocsPanel::from_state(&json!({"expanded":[false,true,false]})).expanded,
        [true, false],
    );
    panel.zoom_by(1e6);
    assert_eq!(panel.zoom, 2.0);
    panel.zoom_by(1e-6);
    assert_eq!(panel.zoom, 0.5);
    assert_eq!(DocsPanel::from_state(&json!({"zoom":99})).zoom, 2.0);
    assert_eq!(DocsPanel::from_state(&json!({"zoom":-1})).zoom, 0.5);
    assert_eq!(DocsPanel::from_state(&json!({"zoom":"invalid"})).zoom, 1.0);
}

#[test]
fn narrow_docs_wraps_and_keyboard_and_captured_pinch_zoom_keep_the_reading_position() {
    use dear_imgui_rs::{Condition, Context, Key, sys};
    let _lock = crate::tests::IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = Context::create();
    context
        .set_ini_filename(None::<std::path::PathBuf>)
        .unwrap();
    context.io_mut().set_display_size([600.0, 450.0]);
    context.io_mut().set_delta_time(1.0 / 60.0);
    context.io_mut().set_config_macosx_behaviors(true);
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let mut app = app();
    let world = app.world_mut();
    let work = open_panel(world, json!({"panel_type":"fixture.work"}), None, None).unwrap();
    world.entity_mut(work).insert(PanelDocumentation::new(&[(
        "Guide",
        &"A short explanation of how to use this focused panel. ".repeat(40),
    )]));
    world.get_mut::<PanelDocumentation>(work).unwrap().symbol = Some(
        "pub fn documented_symbol(name: &str) -> usize\n\nDocumentation for the clicked symbol. "
            .repeat(8)
            .into(),
    );
    let mut icons = bed_ui::icons::Icons::default();
    for key in ["key-command", "key-shift", "key-option", "key-control"] {
        icons.set_texture(key, dear_imgui_rs::TextureId::new(99));
    }
    world.insert_non_send(icons);
    let actions = bed_keybindings::common_actions()
        .into_iter()
        .chain([KeyAction::new(
            "fixture.mods",
            "Copy the current selection",
            "fixture",
            Scope::panel("fixture.work"),
            &["Ctrl+Command+Alt+Shift+KeypadSubtract", "Command+Shift+C"],
        )])
        .collect();
    let input = ShortcutInput::new(Bindings::new(actions, &Overrides::default(), Platform::Mac))
        .with_scope(Scope::panel(DOCS_PANEL_ID));
    let mut panel = DocsPanel::from_state(&Value::Null);
    let mut frame = |context: &mut Context,
                     panel: &mut DocsPanel,
                     focus_docs: bool,
                     events: Vec<api::PinchEvent>,
                     suppressed: bool| {
        world.resource_mut::<api::PinchEvents>().0 = events;
        let ui = context.frame();
        input.begin_frame(ui);
        if suppressed {
            input.suppress(ui);
        }
        ui.window("Docs test")
            .position([0.0, 0.0], Condition::Always)
            .size([220.0, 350.0], Condition::Always)
            .build(|| {
                if focus_docs {
                    ui.with_bound_context(|| unsafe {
                        sys::igSetWindowFocus_Nil();
                    });
                }
                panel.draw(ui, world, &mut Vec::new(), &input);
                ui.with_bound_context(|| unsafe {
                    let root = sys::igGetCurrentWindow();
                    let windows = &(*sys::igGetCurrentContext()).Windows;
                    for index in 0..windows.Size as usize {
                        let child = *windows.Data.add(index);
                        if (*child).ParentWindow == root {
                            assert_eq!(
                                (*child).ScrollMax.x,
                                0.0,
                                "narrow docs must wrap at zoom {}",
                                panel.zoom
                            );
                        }
                    }
                });
            });
        ui.window("Other panel")
            .position([240.0, 0.0], Condition::Always)
            .size([220.0, 350.0], Condition::Always)
            .build(|| {
                if !focus_docs {
                    ui.with_bound_context(|| unsafe {
                        sys::igSetWindowFocus_Nil();
                    });
                }
                ui.text("Another panel");
            });
        world.resource_mut::<api::PinchEvents>().0.clear();
        drop(context.render_legacy());
    };
    for zoom in [0.5, 1.0, 2.0] {
        panel.zoom = zoom;
        for _ in 0..3 {
            frame(&mut context, &mut panel, true, vec![], false);
        }
    }
    panel.zoom = 1.0;
    context.io_mut().add_mouse_pos_event([110.0, 200.0]);
    for _ in 0..3 {
        frame(&mut context, &mut panel, true, vec![], false);
    }
    context.io_mut().add_mouse_wheel_event([0.0, -3.0]);
    for _ in 0..3 {
        frame(&mut context, &mut panel, true, vec![], false);
    }
    let scroll_before = panel.scroll;
    assert!(scroll_before > 0.0, "ordinary wheel scrolls the guide");
    context.io_mut().add_key_event(Key::ModSuper, true);
    frame(&mut context, &mut panel, true, vec![], false);
    for (key, shift, expected) in [
        (Key::Minus, false, 1.0 / 1.1),
        (Key::Equal, true, 1.0),
        (Key::KeypadAdd, false, 1.1),
        (Key::KeypadSubtract, false, 1.0),
        (Key::Equal, false, 1.1),
        (Key::Key0, false, 1.0),
    ] {
        context.io_mut().add_key_event(Key::ModShift, shift);
        frame(&mut context, &mut panel, true, vec![], false);
        context.io_mut().add_key_event(key, true);
        frame(&mut context, &mut panel, true, vec![], false);
        assert!((panel.zoom - expected).abs() < 0.00001, "{key:?}");
        assert!(
            (panel.scroll - scroll_before).abs() < 1.0,
            "zoom keeps reading position"
        );
        context.io_mut().add_key_event(key, false);
        frame(&mut context, &mut panel, true, vec![], false);
    }
    context.io_mut().add_key_event(Key::Equal, true);
    frame(&mut context, &mut panel, true, vec![], true);
    assert_eq!(
        panel.zoom, 1.0,
        "recording or modal shortcut suppression blocks zoom"
    );
    context.io_mut().add_key_event(Key::Equal, false);
    context.io_mut().add_key_event(Key::ModSuper, false);
    frame(&mut context, &mut panel, false, vec![], false);
    context.io_mut().add_key_event(Key::ModSuper, true);
    context.io_mut().add_key_event(Key::Equal, true);
    frame(&mut context, &mut panel, false, vec![], false);
    assert_eq!(panel.zoom, 1.0, "another panel's keypress cannot zoom Docs");
    context.io_mut().add_key_event(Key::Equal, false);
    context.io_mut().add_key_event(Key::ModSuper, false);
    let event = |position, delta, phase| api::PinchEvent {
        position,
        delta,
        phase,
    };
    frame(
        &mut context,
        &mut panel,
        false,
        vec![event([110.0, 200.0], 0.25, api::PinchPhase::Started)],
        false,
    );
    assert_eq!(panel.zoom, 1.25, "pinch does not require keyboard focus");
    context.io_mut().add_mouse_pos_event([350.0, 200.0]);
    frame(
        &mut context,
        &mut panel,
        false,
        vec![
            event([350.0, 200.0], f64::NAN, api::PinchPhase::Moved),
            event([350.0, 200.0], -2.0, api::PinchPhase::Moved),
            event([350.0, 200.0], 0.2, api::PinchPhase::Moved),
            event([350.0, 200.0], 0.0, api::PinchPhase::Ended),
            event([350.0, 200.0], 1.0, api::PinchPhase::Moved),
        ],
        false,
    );
    assert_eq!(
        panel.zoom, 1.5,
        "pinch retains its starting panel until ended"
    );
    frame(
        &mut context,
        &mut panel,
        false,
        vec![event([350.0, 200.0], 1.0, api::PinchPhase::Started)],
        false,
    );
    assert_eq!(
        panel.zoom, 1.5,
        "a pinch starting elsewhere cannot zoom Docs"
    );
    frame(
        &mut context,
        &mut panel,
        false,
        vec![event([110.0, 200.0], 0.0, api::PinchPhase::Started)],
        false,
    );
    frame(
        &mut context,
        &mut panel,
        false,
        vec![event([110.0, 200.0], 1.0, api::PinchPhase::Cancelled)],
        false,
    );
    frame(
        &mut context,
        &mut panel,
        false,
        vec![event([110.0, 200.0], 1.0, api::PinchPhase::Moved)],
        false,
    );
    assert_eq!(
        panel.zoom, 1.5,
        "cancelled gestures cannot continue zooming"
    );
}

#[test]
fn native_pinch_facts_remain_available_to_all_overlays_and_expire_at_frame_end() {
    #[derive(Resource, Default)]
    struct Observations(Vec<usize>);

    fn observe(world: &mut World, _: &Ui) {
        let count = world.resource::<api::PinchEvents>().0.len();
        world.resource_mut::<Observations>().0.push(count);
    }
    let _lock = crate::tests::IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = dear_imgui_rs::Context::create();
    context
        .set_ini_filename(None::<std::path::PathBuf>)
        .unwrap();
    context.io_mut().set_display_size([600.0, 450.0]);
    context.io_mut().set_delta_time(1.0 / 60.0);
    context
        .io_mut()
        .set_config_flags(dear_imgui_rs::ConfigFlags::DOCKING_ENABLE);
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let mut app = app();
    open_panel(
        app.world_mut(),
        json!({"panel_type":DOCS_PANEL_ID}),
        None,
        None,
    )
    .unwrap();
    app.init_resource::<Observations>();
    for _ in 0..2 {
        app.world_mut().spawn(api::WorkbenchOverlay(observe));
    }
    app.world_mut()
        .resource_mut::<api::PinchEvents>()
        .0
        .push(api::PinchEvent {
            position: [-100.0, -100.0],
            delta: 0.25,
            phase: api::PinchPhase::Started,
        });
    for _ in 0..2 {
        let ui = context.frame();
        crate::render_ui(app.world_mut(), ui);
        assert!(app.world().resource::<api::PinchEvents>().0.is_empty());
        drop(context.render_legacy());
    }
    assert_eq!(app.world().resource::<Observations>().0, [1, 1, 0, 0]);
}

#[test]
fn embedded_widgets_can_publish_only_the_actions_they_handle() {
    let scope = Scope::panel("fixture.widget");
    let bindings = Bindings::new(
        vec![
            KeyAction::new("fixture.move", "Move", "fixture", scope.clone(), &["Left"]),
            KeyAction::new("fixture.zoom", "Zoom", "fixture", scope, &["Ctrl+Equal"]),
        ],
        &Overrides::default(),
        Platform::Other,
    );
    let docs = PanelDocumentation::default().shortcut_actions(&["fixture.move"]);
    assert_eq!(
        key_rows(&bindings, Some("fixture.host"), Some(&docs), false),
        vec![KeyRow {
            label: "Move".into(),
            keys: "LeftArrow".into()
        },]
    );
}

#[test]
fn symbol_documentation_survives_focus_changes_and_updates_to_the_next_symbol() {
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions};
    let _lock = crate::tests::IMGUI_TEST_LOCK.lock().unwrap();
    let mut app = app();
    let world = app.world_mut();
    let first = open_panel(world, json!({"panel_type":"fixture.first"}), None, None).unwrap();
    let second = open_panel(world, json!({"panel_type":"fixture.second"}), None, None).unwrap();
    let documentation = bed_ui::documentation::Documentation {
        text: "```rust\npub fn selected_symbol()\n```\n\nA **documented** `symbol`.".into(),
        markdown: true,
        language: "rust".into(),
        ..Default::default()
    };
    world.entity_mut(first).insert(PanelDocumentation {
        symbol: Some(documentation.clone()),
        ..Default::default()
    });
    let mut context = Context::create();
    context
        .set_ini_filename(None::<std::path::PathBuf>)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let input = ShortcutInput::default();
    let mut panel = DocsPanel::from_state(&Value::Null);
    let draw = |context: &mut Context, panel: &mut DocsPanel, world: &mut World| {
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("Symbol retention fixture")
            .size([260.0, 380.0], Condition::Always)
            .build(|| panel.draw(ui, world, &mut Vec::new(), &input));
        drop(context.render_legacy());
    };
    focus_panel(world, first);
    draw(&mut context, &mut panel, world);
    assert_eq!(panel.symbol.as_ref(), Some(&documentation));
    focus_panel(world, second);
    draw(&mut context, &mut panel, world);
    assert_eq!(panel.source, Some(second));
    assert_eq!(panel.symbol.as_ref(), Some(&documentation));
    let next = bed_ui::documentation::Documentation::from("Literal **punctuation**.".to_owned());
    world.entity_mut(second).insert(PanelDocumentation {
        symbol: Some(next.clone()),
        ..Default::default()
    });
    draw(&mut context, &mut panel, world);
    assert_eq!(panel.symbol.as_ref(), Some(&next));
}
