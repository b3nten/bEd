use super::*;
use bedtime_desktop2::{Activates, InMenu, Menu, MenuBar, MenuItem};

struct First;
impl CommandDefinition for First {
    const KEY: &'static str = "tests.first";
    const LABEL: &'static str = "First";
}

struct Second;
impl CommandDefinition for Second {
    const KEY: &'static str = "tests.second";
    const LABEL: &'static str = "Second";
}

struct Collision;
impl CommandDefinition for Collision {
    const KEY: &'static str = First::KEY;
    const LABEL: &'static str = "Collision";
}

#[derive(Resource, Default)]
struct Calls(Vec<CommandInvocation>);

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorkbenchPlugin).init_resource::<Calls>();
    app
}

#[test]
fn workbench_always_has_a_workspace_and_a_layout() {
    let mut app = app();
    let workspace = Workbench::new(app.world_mut()).active_workspace();
    let layout = Workbench::new(app.world_mut()).active_layout();
    assert_eq!(app.world().get::<InWorkspace>(layout).unwrap().0, workspace);
    assert!(
        !app.world()
            .get::<Workspace>(workspace)
            .unwrap()
            .settings_enabled
    );
    assert!(
        Workbench::new(app.world_mut())
            .delete_layout(layout)
            .is_err()
    );
    assert!(
        Workbench::new(app.world_mut())
            .delete_workspace(workspace)
            .is_err()
    );
}

#[derive(Component, PartialEq, Debug)]
struct Draft(String);

#[test]
fn switching_layouts_and_workspaces_retains_panel_instances_and_their_state() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let home = w.active_workspace();
    let first = w.active_layout();
    let panel = w.create_panel(first, "Notes").unwrap();
    let second = w.create_layout(home, "Other").unwrap();
    let away = w.create_workspace("Away", true);
    w.world
        .entity_mut(panel)
        .insert(Draft("keep this draft".into()));
    w.activate_layout(second).unwrap();
    w.activate_workspace(away).unwrap();
    assert!(w.activate_layout(first).is_err());
    w.activate_workspace(home).unwrap();
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
    let home = w.active_workspace();
    let first = w.active_layout();
    let panel = w.create_panel(first, "Disposable").unwrap();
    let second = w.create_layout(home, "Survives").unwrap();
    w.delete_layout(first).unwrap();
    assert_eq!(w.active_layout(), second);
    assert!(w.world.get_entity(panel).is_err());
    let away = w.create_workspace("Away", false);
    w.activate_workspace(away).unwrap();
    w.delete_workspace(home).unwrap();
    assert!(w.world.get_entity(second).is_err());
    assert_eq!(w.active_workspace(), away);
}

#[test]
fn rejected_moves_preserve_layouts_and_stale_requests_do_not_target_new_panels() {
    let mut app = app();
    let mut w = Workbench::new(app.world_mut());
    let home = w.active_workspace();
    let layout = w.active_layout();
    let first = w.create_panel(layout, "First").unwrap();
    let other = w.create_layout(home, "Other").unwrap();
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
fn record(In(invocation): In<CommandInvocation>, mut calls: ResMut<Calls>) -> CommandResult {
    calls.0.push(invocation);
    Ok(())
}
fn results(app: &mut App) -> Vec<CommandFinished> {
    app.world_mut()
        .resource_mut::<Messages<CommandFinished>>()
        .drain()
        .collect()
}

#[test]
fn typed_commands_run_in_submission_order_and_report_handler_errors() {
    let mut app = app();
    let (first, second) = {
        let mut workbench = Workbench::new(app.world_mut());
        let first = workbench.register_command::<First, _>(record).unwrap();
        let second = workbench
            .register_command::<Second, _>(|In(_): In<CommandInvocation>| -> CommandResult {
                Err("expected failure".into())
            })
            .unwrap();
        assert_eq!(workbench.command::<First>(), Some(first));
        (first, second)
    };
    let source = app.world_mut().spawn_empty().id();
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .submit(CommandInvocation {
            command: first,
            source: Some(source),
        });
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .invoke(second);
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .invoke(first);
    app.update();
    assert_eq!(
        app.world().resource::<Calls>().0,
        [
            CommandInvocation {
                command: first,
                source: Some(source)
            },
            CommandInvocation {
                command: first,
                source: None
            }
        ]
    );
    let completed = results(&mut app);
    assert_eq!(
        completed
            .iter()
            .map(|result| result.invocation.command)
            .collect::<Vec<_>>(),
        [first, second, first]
    );
    assert!(completed[0].result.is_ok());
    assert_eq!(completed[1].result, Err("expected failure".into()));
    assert!(completed[2].result.is_ok());
}

#[test]
fn stable_keys_round_trip_and_resolve_a_new_registration() {
    let mut old = app();
    let command = Workbench::new(old.world_mut())
        .register_command::<First, _>(record)
        .unwrap();
    let key = old.world().get::<CommandInfo>(command).unwrap().key();
    let saved = serde_json::to_string(key).unwrap();
    assert_eq!(saved, "\"tests.first\"");
    let loaded: CommandKey = serde_json::from_str(&saved).unwrap();
    let mut new = app();
    new.world_mut().spawn_empty();
    let registered = Workbench::new(new.world_mut())
        .register_command::<First, _>(record)
        .unwrap();
    assert_eq!(
        Workbench::new(new.world_mut()).command_by_key(loaded.as_str()),
        Some(registered)
    );
    assert_eq!(
        Workbench::new(new.world_mut()).command_by_key("missing.command"),
        None
    );
}

#[test]
fn key_collisions_do_not_replace_the_registered_handler() {
    let mut app = app();
    let mut workbench = Workbench::new(app.world_mut());
    let first = workbench.register_command::<First, _>(record).unwrap();
    assert!(workbench.register_command::<First, _>(record).is_err());
    assert!(workbench.register_command::<Collision, _>(record).is_err());
    assert_eq!(workbench.command_by_key(First::KEY), Some(first));
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .invoke(first);
    app.update();
    assert_eq!(app.world().resource::<Calls>().0.len(), 1);
}

#[test]
fn disabled_and_removed_commands_never_run_queued_handlers() {
    let mut app = app();
    let (first, second) = {
        let mut workbench = Workbench::new(app.world_mut());
        (
            workbench.register_command::<First, _>(record).unwrap(),
            workbench.register_command::<Second, _>(record).unwrap(),
        )
    };
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .invoke(first);
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .invoke(second);
    app.world_mut().get_mut::<ActionEnabled>(first).unwrap().0 = false;
    app.world_mut().entity_mut(second).despawn();
    app.update();
    assert!(app.world().resource::<Calls>().0.is_empty());
    assert!(
        results(&mut app)
            .iter()
            .all(|result| result.result.is_err())
    );
    app.world_mut().get_mut::<ActionEnabled>(first).unwrap().0 = true;
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .invoke(first);
    app.update();
    assert_eq!(app.world().resource::<Calls>().0.len(), 1);
}

#[test]
fn menu_activations_use_the_same_dispatcher_and_capture_their_source() {
    let mut app = app();
    let command = Workbench::new(app.world_mut())
        .register_command::<First, _>(record)
        .unwrap();
    let bar = app.world_mut().spawn(MenuBar).id();
    let menu = app
        .world_mut()
        .spawn((
            Menu {
                label: "File".into(),
            },
            InMenu(bar),
        ))
        .id();
    let item = app
        .world_mut()
        .spawn((MenuItem::new("First"), InMenu(menu), Activates(command)))
        .id();
    app.world_mut().write_message(DesktopAction::Activated {
        source: item,
        target: command,
    });
    app.update();
    assert_eq!(
        app.world().resource::<Calls>().0,
        [CommandInvocation {
            command,
            source: Some(item)
        }]
    );
    app.world_mut().entity_mut(menu).despawn();
    assert!(app.world().get::<CommandInfo>(command).is_some());
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .invoke(command);
    app.update();
    assert_eq!(app.world().resource::<Calls>().0.len(), 2);
}

#[test]
fn handlers_enqueue_followup_commands_without_reentrant_execution() {
    let mut app = app();
    let followup = Workbench::new(app.world_mut())
        .register_command::<Second, _>(record)
        .unwrap();
    let first = Workbench::new(app.world_mut())
        .register_command::<First, _>(
            move |In(_): In<CommandInvocation>,
                  mut requests: ResMut<CommandRequests>|
                  -> CommandResult {
                requests.invoke(followup);
                Ok(())
            },
        )
        .unwrap();
    app.world_mut()
        .resource_mut::<CommandRequests>()
        .invoke(first);
    app.update();
    assert!(app.world().resource::<Calls>().0.is_empty());
    assert_eq!(results(&mut app).len(), 1);
    app.update();
    assert_eq!(app.world().resource::<Calls>().0.len(), 1);
    assert_eq!(results(&mut app)[0].invocation.command, followup);
}

#[test]
fn application_owned_native_actions_are_not_treated_as_commands() {
    let mut app = app();
    let source = app.world_mut().spawn_empty().id();
    let target = app.world_mut().spawn_empty().id();
    app.world_mut()
        .write_message(DesktopAction::Activated { source, target });
    app.update();
    assert!(results(&mut app).is_empty());
}

#[test]
fn actions_produced_before_post_update_ingress_execute_in_the_same_frame() {
    let mut app = app();
    let command = Workbench::new(app.world_mut())
        .register_command::<First, _>(record)
        .unwrap();
    let source = app.world_mut().spawn_empty().id();
    app.add_systems(
        PostUpdate,
        (move |mut actions: MessageWriter<DesktopAction>| {
            actions.write(DesktopAction::Activated {
                source,
                target: command,
            });
        })
        .before(CommandSystems::Ingress),
    );
    app.update();
    assert_eq!(
        app.world().resource::<Calls>().0,
        [CommandInvocation {
            command,
            source: Some(source)
        }]
    );
}

#[cfg(feature = "ui")]
#[test]
fn ui_activations_execute_in_the_same_frame_without_application_ordering() {
    let mut app = app();
    let command = Workbench::new(app.world_mut())
        .register_command::<First, _>(record)
        .unwrap();
    let source = app.world_mut().spawn_empty().id();
    app.add_systems(
        PostUpdate,
        (move |mut actions: MessageWriter<DesktopAction>| {
            actions.write(DesktopAction::Activated {
                source,
                target: command,
            });
        })
        .in_set(bedtime_ui2::UiSystems::Draw),
    );
    app.update();
    assert_eq!(
        app.world().resource::<Calls>().0,
        [CommandInvocation {
            command,
            source: Some(source)
        }]
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
    let workspace = w.active_workspace();
    let area = w.destination_area(layout).unwrap();
    let stamp = w.stamp();
    let other = w.create_layout(workspace, "Other").unwrap();
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
