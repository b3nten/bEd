use super::*;

struct First;
impl ContextCommand for First {
    const KEY: &'static str = "tests.first";
    const LABEL: &'static str = "First";
}

struct Second;
impl ContextCommand for Second {
    const KEY: &'static str = "tests.second";
    const LABEL: &'static str = "Second";
}

struct Collision;
impl ContextCommand for Collision {
    const KEY: &'static str = First::KEY;
    const LABEL: &'static str = "Collision";
}

#[derive(Resource, Default)]
struct Calls(Vec<ContextCommandInvocation>);

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorkspacePlugin).init_resource::<Calls>();
    app
}

#[test]
fn workbench_always_has_a_layout() {
    let mut app = app();
    let layout = Workbench::new(app.world_mut()).active_layout();
    assert_eq!(app.world().resource::<Workspace>().active_layout(), layout);
    assert!(
        Workbench::new(app.world_mut())
            .delete_layout(layout)
            .is_err()
    );
}

#[derive(Component, PartialEq, Debug)]
struct Draft(String);

#[test]
fn switching_layouts_retains_panel_instances_and_their_state() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let first = w.active_layout();
    let panel = w.create_panel(first, "Notes").unwrap();
    let second = w.create_layout("Other").unwrap();
    w.world
        .entity_mut(panel)
        .insert(Draft("keep this draft".into()));
    w.activate_layout(second).unwrap();
    assert_eq!(w.active_layout(), second);
    w.activate_layout(first).unwrap();
    assert_eq!(
        w.world.get::<Draft>(panel),
        Some(&Draft("keep this draft".into()))
    );
    assert_eq!(w.panels(first), vec![panel]);
}

#[test]
fn panels_can_split_regroup_and_close_without_retiring_other_panels() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let layout = w.active_layout();
    let first = w.create_panel(layout, "First").unwrap();
    let second = w.create_panel(layout, "Second").unwrap();
    let third = w.create_panel(layout, "Third").unwrap();
    w.world.entity_mut(first).insert(Draft("first".into()));
    w.move_panel(second, first, Some(SplitAxis::Horizontal))
        .unwrap();
    w.move_panel(third, second, Some(SplitAxis::Vertical))
        .unwrap();
    w.move_panel(first, third, None).unwrap();
    assert_eq!(w.panels(layout).len(), 3);
    w.close_panel(third).unwrap();
    w.close_panel(second).unwrap();
    assert!(w.world.get_entity(third).is_err());
    assert_eq!(w.world.get::<Draft>(first), Some(&Draft("first".into())));
    assert_eq!(w.panels(layout), vec![first]);
    w.focus_panel(first).unwrap();
    assert_eq!(w.focused_panel(layout), Some(first));
}

#[test]
fn deletion_retires_owned_panels_and_selects_a_surviving_layout() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let first = w.active_layout();
    let panel = w.create_panel(first, "Disposable").unwrap();
    let second = w.create_layout("Survives").unwrap();
    w.delete_layout(first).unwrap();
    assert_eq!(w.active_layout(), second);
    assert!(w.world.get_entity(panel).is_err());
}

#[test]
fn rejected_moves_preserve_layouts_and_stale_requests_do_not_target_new_panels() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let layout = w.active_layout();
    let first = w.create_panel(layout, "First").unwrap();
    let other = w.create_layout("Other").unwrap();
    let second = w.create_panel(other, "Second").unwrap();
    assert!(w.move_panel(first, second, None).is_err());
    assert_eq!(w.panels(layout), vec![first]);
    w.world
        .resource_mut::<WorkbenchRequests>()
        .submit(WorkbenchRequest::ClosePanel(first));
    w.world
        .resource_mut::<WorkbenchRequests>()
        .submit(WorkbenchRequest::FocusPanel(first));
    app.update();
    let results: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<WorkbenchFinished>>()
        .drain()
        .collect();
    assert!(results[0].result.is_ok());
    assert!(results[1].result.is_err());
    assert!(app.world().get::<Panel>(second).is_some());
}

#[test]
fn a_splitter_request_cannot_mutate_a_replaced_layout_tree() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let layout = w.active_layout();
    let first = w.create_panel(layout, "First").unwrap();
    let second = w.create_panel(layout, "Second").unwrap();
    w.move_panel(second, first, Some(SplitAxis::Horizontal))
        .unwrap();
    let stamp = w.stamp();
    w.world
        .resource_mut::<WorkbenchRequests>()
        .submit(WorkbenchRequest::Edit {
            stamp,
            edit: LayoutEdit::ResizeBorder {
                axis: SplitAxis::Horizontal,
                at: 5000,
                span: [0, LAYOUT_EXTENT],
                to: 8000,
                minimum: 1,
            },
        });
    w.move_panel(second, first, Some(SplitAxis::Vertical))
        .unwrap();
    app.update();
    let results: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<WorkbenchFinished>>()
        .drain()
        .collect();
    assert!(results[0].result.is_err());
    assert_eq!(Workbench::new(app.world_mut()).panels(layout).len(), 2);
}
fn record(
    In(invocation): In<ContextCommandInvocation>,
    mut calls: ResMut<Calls>,
) -> ContextCommandResult {
    calls.0.push(invocation);
    Ok(())
}
fn results(app: &mut App) -> Vec<ContextCommandFinished> {
    app.world_mut()
        .resource_mut::<Messages<ContextCommandFinished>>()
        .drain()
        .collect()
}

#[test]
fn typed_commands_run_in_submission_order_and_report_handler_errors() {
    let mut app = app();
    let first = Workbench::new(app.world_mut())
        .register_handler::<First, _>(record)
        .unwrap();
    let second = Workbench::new(app.world_mut())
        .register_handler::<Second, _>(
            |In(_): In<ContextCommandInvocation>| -> ContextCommandResult {
                Err("expected failure".into())
            },
        )
        .unwrap();
    let source = app.world_mut().spawn_empty().id();
    let mut requests = app.world_mut().resource_mut::<ContextCommandRequests>();
    requests.submit(ContextCommandRequest {
        command: first.clone(),
        source: Some(source),
        context: InvocationContext::Global,
    });
    requests.invoke_id(second.clone());
    requests.invoke::<First>();
    app.update();
    assert_eq!(
        app.world().resource::<Calls>().0,
        [
            ContextCommandInvocation {
                command: first.clone(),
                source: Some(source),
                panel: None
            },
            ContextCommandInvocation {
                command: first.clone(),
                source: None,
                panel: None
            }
        ]
    );
    let completed = results(&mut app);
    assert_eq!(
        completed
            .iter()
            .map(|r| r.invocation.command.clone())
            .collect::<Vec<_>>(),
        [first.clone(), second, first]
    );
    assert!(completed[0].result.is_ok());
    assert_eq!(completed[1].result, Err("expected failure".into()));
    assert!(completed[2].result.is_ok());
}
#[test]
fn stable_keys_round_trip_without_persisting_types_or_handler_entities() {
    let mut old = app();
    let command = Workbench::new(old.world_mut())
        .register_handler::<First, _>(record)
        .unwrap();
    let saved = serde_json::to_string(&command).unwrap();
    assert_eq!(saved, "\"tests.first\"");
    let loaded: ContextCommandId = serde_json::from_str(&saved).unwrap();
    let mut new = app();
    new.world_mut().spawn_empty();
    Workbench::new(new.world_mut())
        .register_handler::<First, _>(record)
        .unwrap();
    assert_eq!(
        Workbench::new(new.world_mut()).command_by_key(loaded.as_str()),
        Some(ContextCommandId::of::<First>())
    );
    assert_eq!(
        Workbench::new(new.world_mut()).command_by_key("missing.command"),
        None
    );
}
#[test]
fn key_and_scope_collisions_do_not_replace_handlers() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let first = w.register_handler::<First, _>(record).unwrap();
    assert!(w.register_handler::<First, _>(record).is_err());
    assert!(w.register_handler::<Collision, _>(record).is_err());
    assert_eq!(w.command_by_key(First::KEY), Some(first));
    let panel = w.create_panel(w.active_layout(), "Panel").unwrap();
    w.register_panel_handler::<First, _>(panel, record).unwrap();
    assert!(w.register_panel_handler::<First, _>(panel, record).is_err());
}
#[test]
fn disabled_and_removed_handlers_never_run_queued_requests() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let first = w.register_handler::<First, _>(record).unwrap();
    let second = w.register_handler::<Second, _>(record).unwrap();
    let h1 = w.handler(first.clone(), None).unwrap();
    let h2 = w.handler(second.clone(), None).unwrap();
    w.world.get_mut::<ActionEnabled>(h1).unwrap().0 = false;
    w.world.despawn(h2);
    let mut requests = w.world.resource_mut::<ContextCommandRequests>();
    requests.invoke_id(first.clone());
    requests.invoke_id(second.clone());
    app.update();
    assert!(app.world().resource::<Calls>().0.is_empty());
    assert!(results(&mut app).iter().all(|r| r.result.is_err()));
}
#[test]
fn panel_handlers_override_globals_and_disabled_panel_handlers_block_fallback() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let command = w.register_handler::<First, _>(record).unwrap();
    let first = w.create_panel(w.active_layout(), "First").unwrap();
    let second = w.create_panel(w.active_layout(), "Second").unwrap();
    let local = w.register_panel_handler::<First, _>(first, record).unwrap();
    w.focus_panel(first).unwrap();
    w.world
        .resource_mut::<ContextCommandRequests>()
        .invoke::<First>();
    app.update();
    assert_eq!(app.world().resource::<Calls>().0[0].panel, Some(first));
    app.world_mut().get_mut::<ActionEnabled>(local).unwrap().0 = false;
    app.world_mut()
        .resource_mut::<ContextCommandRequests>()
        .invoke::<First>();
    app.update();
    assert_eq!(app.world().resource::<Calls>().0.len(), 1);
    assert!(results(&mut app).last().unwrap().result.is_err());
    Workbench::new(app.world_mut()).focus_panel(second).unwrap();
    assert!(command_available(
        app.world_mut(),
        command,
        InvocationContext::Focused
    ));
    app.world_mut()
        .resource_mut::<ContextCommandRequests>()
        .invoke::<First>();
    app.update();
    assert_eq!(
        app.world().resource::<Calls>().0.last().unwrap().panel,
        Some(second)
    );
    Workbench::new(app.world_mut()).close_panel(first).unwrap();
    assert!(app.world().get_entity(local).is_err());
}
#[test]
fn handler_replacement_does_not_rewire_menu_action_types() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let command = w.register_handler::<First, _>(record).unwrap();
    let root = w
        .world
        .spawn(Menu {
            label: "Context".into(),
        })
        .id();
    let item = w
        .world
        .spawn((MenuItem::new("First"), InMenu(root), Invokes::of::<First>()))
        .id();
    let old = w.handler(command.clone(), None).unwrap();
    w.world.despawn(old);
    assert!(
        menu_snapshot(w.world, root, InvocationContext::Global)
            .unwrap()
            .request(item, command.clone())
            .is_none()
    );
    w.register_handler::<First, _>(record).unwrap();
    let request = menu_snapshot(w.world, root, InvocationContext::Global)
        .unwrap()
        .request(item, command.clone())
        .unwrap();
    w.world
        .resource_mut::<ContextCommandRequests>()
        .submit(request);
    app.update();
    assert_eq!(app.world().resource::<Calls>().0[0].source, Some(item));
}
#[test]
fn handlers_enqueue_followups_without_reentrant_execution() {
    let mut app = app();
    Workbench::new(app.world_mut())
        .register_handler::<Second, _>(record)
        .unwrap();
    Workbench::new(app.world_mut())
        .register_handler::<First, _>(
            |In(_): In<ContextCommandInvocation>,
             mut requests: ResMut<ContextCommandRequests>|
             -> ContextCommandResult {
                requests.invoke::<Second>();
                Ok(())
            },
        )
        .unwrap();
    app.world_mut()
        .resource_mut::<ContextCommandRequests>()
        .invoke::<First>();
    app.update();
    assert!(app.world().resource::<Calls>().0.is_empty());
    assert_eq!(results(&mut app).len(), 1);
    app.update();
    assert_eq!(app.world().resource::<Calls>().0.len(), 1);
}
#[test]
fn a_batch_captures_all_panels_before_handlers_change_focus_or_destroy_targets() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let a = w.create_panel(w.active_layout(), "A").unwrap();
    let b = w.create_panel(w.active_layout(), "B").unwrap();
    w.register_panel_handler::<Second, _>(a, record).unwrap();
    w.register_handler::<First, _>(
        move |In(_): In<ContextCommandInvocation>, world: &mut World| -> ContextCommandResult {
            Workbench::new(world).focus_panel(b)
        },
    )
    .unwrap();
    w.focus_panel(a).unwrap();
    let mut requests = w.world.resource_mut::<ContextCommandRequests>();
    requests.invoke::<First>();
    requests.invoke::<Second>();
    app.update();
    assert_eq!(app.world().resource::<Calls>().0[0].panel, Some(a));
}
#[test]
fn keybindings_resolve_independently_from_handlers_and_conflicts_reserve_input() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    w.register_handler::<First, _>(record).unwrap();
    w.register_handler::<Second, _>(record).unwrap();
    let panel = w.create_panel(w.active_layout(), "Panel").unwrap();
    w.register_panel_handler::<Second, _>(panel, record)
        .unwrap();
    let chord = KeyChord::new(Modifiers::CONTROL, KeyCode::KeyK);
    w.world.spawn(Binding::new::<First>(chord));
    w.world
        .spawn((Binding::new::<Second>(chord), InPanel(panel)));
    w.world.resource_mut::<BindingInput>().0.push(BindingPress {
        event: 1,
        chord,
        repeat: false,
        context: InvocationContext::Focused,
    });
    app.update();
    assert_eq!(
        app.world().resource::<Calls>().0[0].command,
        ContextCommandId::of::<Second>()
    );
    assert_eq!(app.world().resource::<BindingClaims>().0, vec![1]);
    app.world_mut()
        .spawn((Binding::new::<First>(chord), InPanel(panel)));
    app.world_mut()
        .resource_mut::<BindingInput>()
        .0
        .push(BindingPress {
            event: 2,
            chord,
            repeat: false,
            context: InvocationContext::Focused,
        });
    app.update();
    assert_eq!(app.world().resource::<Calls>().0.len(), 1);
    assert_eq!(app.world().resource::<BindingClaims>().0, vec![2]);
}

#[test]
fn retiring_an_admitted_panel_handler_does_not_redirect_to_a_global_handler() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let panel = w.create_panel(w.active_layout(), "Closing").unwrap();
    w.register_handler::<Second, _>(record).unwrap();
    w.register_panel_handler::<Second, _>(panel, record)
        .unwrap();
    w.register_handler::<First, _>(
        move |_: In<ContextCommandInvocation>, world: &mut World| -> ContextCommandResult {
            Workbench::new(world).close_panel(panel)
        },
    )
    .unwrap();
    w.world
        .resource_mut::<ContextCommandRequests>()
        .invoke::<First>();
    w.world
        .resource_mut::<ContextCommandRequests>()
        .invoke::<Second>();
    app.update();
    assert!(app.world().resource::<Calls>().0.is_empty());
    let results = results(&mut app);
    assert!(results[0].result.is_ok());
    assert!(results[1].result.is_err());
}

#[test]
fn normalized_shortcuts_reserve_repeats_without_reexecuting_by_default() {
    let mut app = app();
    Workbench::new(app.world_mut())
        .register_handler::<First, _>(record)
        .unwrap();
    let chord = KeyChord::new(Modifiers::CONTROL, KeyCode::KeyK);
    let binding = app.world_mut().spawn(Binding::new::<First>(chord)).id();
    for (event, repeat) in [(1, false), (2, true)] {
        app.world_mut()
            .resource_mut::<BindingInput>()
            .0
            .push(BindingPress {
                event,
                chord,
                repeat,
                context: InvocationContext::Focused,
            });
    }
    app.update();
    assert_eq!(app.world().resource::<Calls>().0.len(), 1);
    assert_eq!(app.world().resource::<BindingClaims>().0, vec![1, 2]);
    app.world_mut().get_mut::<Binding>(binding).unwrap().repeat = true;
    app.world_mut()
        .resource_mut::<BindingInput>()
        .0
        .push(BindingPress {
            event: 3,
            chord,
            repeat: true,
            context: InvocationContext::Focused,
        });
    app.update();
    assert_eq!(app.world().resource::<Calls>().0.len(), 2);
}
#[cfg(feature = "ui")]
#[test]
fn ui_requests_execute_in_the_same_frame_without_application_ordering() {
    let mut app = App::new();
    app.add_plugins((WorkbenchCorePlugin, WorkbenchUiPlugin));
    {
        let mut w = Workbench::new(app.world_mut());
        w.world_mut().init_resource::<Calls>();
        w.register_handler::<First, _>(record).unwrap();
    }
    app.add_systems(
        PreUpdate,
        (|host: &mut World| {
            Workbench::new(host)
                .world_mut()
                .resource_mut::<ContextCommandRequests>()
                .invoke::<First>();
        })
        .in_set(bedtime_ui2::UiSystems::Draw),
    );
    app.update();
    assert_eq!(
        Workbench::new(app.world_mut())
            .world()
            .resource::<Calls>()
            .0
            .len(),
        1
    );
}

#[test]
fn closing_content_retains_its_area_but_moving_the_last_tab_closes_its_footprint() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let layout = w.active_layout();
    let first = w.destination_area(layout).unwrap();
    let second = w
        .split_area(first, SplitAxis::Horizontal, 5000, [100, 100])
        .unwrap();
    let panel = w.create_panel_in_area(second, "Second").unwrap();
    let rect = w.world.get::<Area>(second).unwrap().rect();
    w.close_panel(panel).unwrap();
    assert_eq!(w.world.get::<Area>(second).unwrap().rect(), rect);
    assert!(panels_in_area(w.world, second).is_empty());
    let panel = w.create_panel_in_area(second, "Moved").unwrap();
    w.world.entity_mut(panel).insert(Draft("retained".into()));
    w.move_tab(panel, first, 0).unwrap();
    assert!(w.world.get_entity(second).is_err());
    assert_eq!(w.world.get::<InArea>(panel).unwrap().0, first);
    assert_eq!(
        w.world.get::<Area>(first).unwrap().rect(),
        AreaRect {
            min: [0, 0],
            max: [LAYOUT_EXTENT; 2]
        }
    );
    assert_eq!(w.world.get::<Draft>(panel), Some(&Draft("retained".into())));
}

#[test]
fn tab_reordering_uses_destination_order_after_removing_the_source() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let layout = w.active_layout();
    let area = w.destination_area(layout).unwrap();
    let a = w.create_panel_in_area(area, "A").unwrap();
    let b = w.create_panel_in_area(area, "B").unwrap();
    let c = w.create_panel_in_area(area, "C").unwrap();
    w.world.get_mut::<Panel>(a).unwrap().preview = true;
    w.move_tab(a, area, 2).unwrap();
    assert_eq!(panels_in_area(w.world, area), &[b, c, a]);
    assert!(!w.world.get::<Panel>(a).unwrap().preview);
    w.move_tab(c, area, 0).unwrap();
    assert_eq!(panels_in_area(w.world, area), &[c, b, a]);
    assert_eq!(w.focused_panel(layout), Some(c));
}

#[test]
fn all_four_tab_edges_split_without_recreating_the_panel_and_keep_an_empty_source() {
    for axis in [SplitAxis::Horizontal, SplitAxis::Vertical] {
        for high in [false, true] {
            let mut app = app();
            let mut w = Workbench::new(app.world_mut());
            let layout = w.active_layout();
            let original = w.destination_area(layout).unwrap();
            let panel = w.create_panel_in_area(original, "One").unwrap();
            w.world.entity_mut(panel).insert(Draft("canonical".into()));
            let destination = w
                .split_tab(panel, original, axis, high, [100, 100])
                .unwrap();
            assert_eq!(
                w.world.get::<LayoutAreas>(layout).unwrap().entities().len(),
                2
            );
            assert!(panels_in_area(w.world, original).is_empty());
            assert_eq!(w.world.get::<InArea>(panel).unwrap().0, destination);
            assert_eq!(
                w.world.get::<Draft>(panel),
                Some(&Draft("canonical".into()))
            );
            let r = w.world.get::<Area>(destination).unwrap().rect();
            let i = axis.index();
            assert_eq!(if high { r.min[i] } else { r.max[i] }, 5000);
        }
    }
}

#[test]
fn partial_area_join_preserves_the_target_tail_and_its_live_content() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let layout = w.active_layout();
    let left = w.destination_area(layout).unwrap();
    let right = w
        .split_area(left, SplitAxis::Horizontal, 5000, [100, 100])
        .unwrap();
    let lower = w
        .split_area(left, SplitAxis::Vertical, 5000, [100, 100])
        .unwrap();
    let source = w.create_panel_in_area(lower, "Lower").unwrap();
    let target = w.create_panel_in_area(right, "Right").unwrap();
    w.join_areas(lower, right, [100, 100]).unwrap();
    assert_eq!(
        w.world.get::<Area>(lower).unwrap().rect(),
        AreaRect {
            min: [0, 5000],
            max: [10000, 10000]
        }
    );
    assert_eq!(
        w.world.get::<Area>(right).unwrap().rect(),
        AreaRect {
            min: [5000, 0],
            max: [10000, 5000]
        }
    );
    assert_eq!(w.world.get::<InArea>(source).unwrap().0, lower);
    assert_eq!(w.world.get::<InArea>(target).unwrap().0, right);
    assert_eq!(w.focused_panel(layout), Some(source));
}

#[test]
fn switching_away_and_back_invalidates_a_pending_gesture_even_without_a_geometry_change() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let layout = w.active_layout();
    let area = w.destination_area(layout).unwrap();
    let stamp = w.stamp();
    let other = w.create_layout("Other").unwrap();
    w.activate_layout(other).unwrap();
    w.activate_layout(layout).unwrap();
    let before = w.world.get::<Area>(area).unwrap().rect();
    assert!(
        w.edit(
            stamp,
            &LayoutEdit::SplitArea {
                area,
                axis: SplitAxis::Horizontal,
                at: 5000,
                minimum: [100, 100]
            }
        )
        .is_err()
    );
    assert_eq!(w.world.get::<Area>(area).unwrap().rect(), before);
}

struct TestPanelType;
impl PanelDefinition for TestPanelType {
    const KEY: &'static str = "test.panel";
    const LABEL: &'static str = "Test panel";
}
#[test]
fn factories_are_entities_and_failed_drops_do_not_modify_layout_geometry() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let layout = w.active_layout();
    let area = w.destination_area(layout).unwrap();
    let kind = w
        .register_panel_type::<TestPanelType, _>(|In(_): In<Entity>| -> Result<Entity, String> {
            Err("load failed".into())
        })
        .unwrap();
    let stamp = w.stamp();
    let original = w.world.get::<Area>(area).unwrap().rect();
    assert!(
        w.edit(
            stamp,
            &LayoutEdit::DropPanel {
                area,
                kind,
                index: 0,
                split: Some((SplitAxis::Horizontal, false)),
                minimum: [100, 100]
            }
        )
        .is_err()
    );
    assert_eq!(w.world.get::<Area>(area).unwrap().rect(), original);
    assert!(w.panels(layout).is_empty());
    assert_eq!(w.stamp(), stamp);
    assert!(w.world.get::<PanelType>(kind).is_some());
}

#[test]
fn global_bindings_use_panel_handlers_and_panel_bindings_use_workbench_handlers() {
    #[derive(Resource, Default)]
    struct Scopes(Vec<&'static str>);
    let mut app = app();
    app.init_resource::<Scopes>();
    let mut w = Workbench::new(app.world_mut());
    let panel = w.create_panel(w.active_layout(), "Panel").unwrap();
    w.register_context_command::<Second>().unwrap();
    assert!(!command_available(
        w.world,
        ContextCommandId::of::<Second>(),
        InvocationContext::Focused
    ));
    w.register_handler::<First, _>(
        |_: In<ContextCommandInvocation>, mut scopes: ResMut<Scopes>| -> ContextCommandResult {
            scopes.0.push("workbench");
            Ok(())
        },
    )
    .unwrap();
    w.register_panel_handler::<Second, _>(
        panel,
        |_: In<ContextCommandInvocation>, mut scopes: ResMut<Scopes>| -> ContextCommandResult {
            scopes.0.push("panel");
            Ok(())
        },
    )
    .unwrap();
    let global = KeyChord::new(Modifiers::CONTROL, KeyCode::Equal);
    let local = KeyChord::new(Modifiers::CONTROL, KeyCode::KeyD);
    w.world.spawn(Binding::new::<Second>(global));
    w.world
        .spawn((Binding::new::<First>(local), InPanel(panel)));
    for (event, chord) in [(1, global), (2, local)] {
        w.world.resource_mut::<BindingInput>().0.push(BindingPress {
            event,
            chord,
            repeat: false,
            context: InvocationContext::Focused,
        });
    }
    app.update();
    assert_eq!(
        app.world().resource::<Scopes>().0,
        vec!["panel", "workbench"]
    );
}
#[test]
fn unavailable_panel_binding_does_not_retry_the_shadowed_workbench_binding() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    w.register_handler::<First, _>(record).unwrap();
    w.register_context_command::<Second>().unwrap();
    let panel = w.create_panel(w.active_layout(), "Panel").unwrap();
    let chord = KeyChord::new(Modifiers::CONTROL, KeyCode::KeyS);
    w.world.spawn(Binding::new::<First>(chord));
    w.world
        .spawn((Binding::new::<Second>(chord), InPanel(panel)));
    w.world.resource_mut::<BindingInput>().0.push(BindingPress {
        event: 1,
        chord,
        repeat: false,
        context: InvocationContext::Focused,
    });
    app.update();
    assert!(app.world().resource::<Calls>().0.is_empty());
    assert_eq!(app.world().resource::<BindingClaims>().0, vec![1]);
    assert!(results(&mut app)[0].result.is_err());
}

#[test]
fn extra_modifiers_do_not_match_a_shortcut() {
    let mut app = app();
    Workbench::new(app.world_mut())
        .register_handler::<First, _>(record)
        .unwrap();
    let chord = KeyChord::new(Modifiers::PRIMARY.resolved(), KeyCode::KeyS);
    app.world_mut().spawn(Binding::new::<First>(chord));
    app.world_mut()
        .resource_mut::<BindingInput>()
        .0
        .push(BindingPress {
            event: 1,
            chord: KeyChord::new(
                Modifiers::PRIMARY.resolved() | Modifiers::SHIFT,
                KeyCode::KeyS,
            ),
            repeat: false,
            context: InvocationContext::Focused,
        });
    app.update();
    assert!(app.world().resource::<Calls>().0.is_empty());
    assert!(app.world().resource::<BindingClaims>().0.is_empty());
    app.world_mut()
        .resource_mut::<BindingInput>()
        .0
        .push(BindingPress {
            event: 2,
            chord,
            repeat: false,
            context: InvocationContext::Focused,
        });
    app.update();
    assert_eq!(app.world().resource::<Calls>().0.len(), 1);
}
