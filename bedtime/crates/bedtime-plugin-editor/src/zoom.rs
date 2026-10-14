//! Zoom belongs to one text presentation. Commands carry its captured entity;
//! native gestures retain the presentation beneath their starting position.
use crate::{EditorConfig, EditorViews};
use bedtime_api::documents::DocumentFrame;
use bedtime_api::workbench::{CommandInvocation, CommandResult, PanelFrontend, WorkbenchOverlay};
use bedtime_api::workbench::{CommandPresentation, CommandPresentations, PinchEvents, PinchPhase};
use bedtime_editor_widgets::EditorView;
use bevy::prelude::*;
#[cfg(test)]
use dear_imgui_rs::Key;
use dear_imgui_rs::{PopupQueryFlags, Ui, sys};

pub const ZOOM_IN: &str = "bedtime.editor.zoom_in";
pub const ZOOM_OUT: &str = "bedtime.editor.zoom_out";
pub const RESET_ZOOM: &str = "bedtime.editor.reset_zoom";

#[derive(Component, Clone, Copy)]
pub(crate) struct EditorWindow {
    root: u32,
    frame: usize,
}

#[derive(Resource, Default)]
struct GestureTarget(Option<Entity>);

pub(crate) fn install(app: &mut App) {
    app.init_resource::<PinchEvents>()
        .init_resource::<GestureTarget>()
        .init_resource::<CommandPresentations>();
    app.world_mut().spawn(WorkbenchOverlay(gestures));
}

pub(crate) fn command(In(invocation): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let entity = invocation
        .source
        .ok_or("Zoom requires a captured text presentation")?;
    let entity = world
        .get::<PanelFrontend>(entity)
        .map_or(entity, |front| front.0);
    let mut views = world.non_send_mut::<EditorViews>();
    let view = views
        .text_mut(entity)
        .ok_or("Zoom requires a text presentation")?;
    match invocation.command.as_str() {
        ZOOM_IN => view.zoom_in(),
        ZOOM_OUT => view.zoom_out(),
        RESET_ZOOM => view.reset_zoom(),
        _ => return Err("Unknown editor zoom command".into()),
    }
    Ok(Vec::new())
}

pub(crate) fn window(ui: &Ui) -> EditorWindow {
    ui.with_bound_context(|| unsafe {
        // This UI scope owns the current native window; only its stable ID escapes.
        EditorWindow {
            root: (*(*sys::igGetCurrentWindowRead()).RootWindow).ID,
            frame: ui.frame_count(),
        }
    })
}

fn popup(ui: &Ui) -> bool {
    ui.is_popup_open_with_flags(
        "",
        PopupQueryFlags::ANY_POPUP_ID | PopupQueryFlags::ANY_POPUP_LEVEL,
    )
}

/// Suppress only characters belonging to the held zoom chord, even at limits.
/// This runs before the focused editor's input code can mutate its document.
pub(crate) fn keyboard(
    ui: &Ui,
    view: &EditorView,
    blocked: bool,
    shortcuts: &bedtime_keybindings::ShortcutInput,
) -> Option<&'static str> {
    if blocked
        || (view.find_visible() || view.line_jump_visible())
        || popup(ui)
        || ui.io().want_text_input()
        || !ui.is_window_focused_with_flags(dear_imgui_rs::FocusedFlags::CHILD_WINDOWS)
    {
        return None;
    }
    let zoom_in = shortcuts.held(ui, bedtime_keybindings::ZOOM_IN) || shortcuts.held(ui, ZOOM_IN);
    let zoom_out =
        shortcuts.held(ui, bedtime_keybindings::ZOOM_OUT) || shortcuts.held(ui, ZOOM_OUT);
    let reset_zoom =
        shortcuts.held(ui, bedtime_keybindings::RESET_ZOOM) || shortcuts.held(ui, RESET_ZOOM);
    if zoom_in || zoom_out || reset_zoom {
        ui.with_bound_context(|| unsafe {
            let queue = &mut (*sys::igGetIO_Nil()).InputQueueCharacters;
            if queue.Size > 0 {
                let characters = std::slice::from_raw_parts_mut(queue.Data, queue.Size as usize);
                let mut kept = 0;
                for index in 0..characters.len() {
                    let codepoint = characters[index];
                    if (zoom_in && (codepoint == '=' as u32 || codepoint == '+' as u32))
                        || (zoom_out && codepoint == '-' as u32)
                        || (reset_zoom && codepoint == '0' as u32)
                    {
                        continue;
                    }
                    characters[kept] = codepoint;
                    kept += 1;
                }
                queue.Size = kept as i32;
            }
        });
    }
    [
        bedtime_keybindings::ZOOM_IN,
        bedtime_keybindings::ZOOM_OUT,
        bedtime_keybindings::RESET_ZOOM,
        ZOOM_IN,
        ZOOM_OUT,
        RESET_ZOOM,
    ]
    .into_iter()
    .find(|id| shortcuts.take(ui, id))
}

fn gestures(world: &mut World, ui: &Ui) {
    let events = world.resource::<PinchEvents>().0.clone();
    let blocked = world.non_send::<EditorConfig>().options.block_input || popup(ui);
    let mut target = world.resource::<GestureTarget>().0;
    if blocked {
        target = None;
    }
    for event in events {
        if event.phase == PinchPhase::Started {
            target = if blocked
                || !event
                    .position
                    .iter()
                    .all(|coordinate| coordinate.is_finite())
            {
                None
            } else {
                let root = ui.with_bound_context(|| unsafe {
                    let mut hovered = std::ptr::null_mut();
                    sys::igFindHoveredWindowEx(
                        event.position.into(),
                        true,
                        &mut hovered,
                        std::ptr::null_mut(),
                    );
                    (!hovered.is_null()).then(|| (*(*hovered).RootWindow).ID)
                });
                let views = world.non_send::<EditorViews>();
                views.widgets.keys().find_map(|entity| {
                    let view = views.text(*entity)?;
                    let window = world.get::<EditorWindow>(*entity)?;
                    if (view.find_visible() || view.line_jump_visible())
                        || window.frame != ui.frame_count()
                        || Some(window.root) != root
                    {
                        return None;
                    }
                    let layout = view.presentation().layout;
                    (event.position[0] >= layout.pane_pos[0]
                        && event.position[0] < layout.pane_pos[0] + layout.pane_size[0]
                        && event.position[1] >= layout.pane_pos[1]
                        && event.position[1] < layout.pane_pos[1] + layout.pane_size[1])
                        .then_some(*entity)
                })
            };
        }
        let factor = (1.0 + event.delta) as f32;
        if !blocked
            && event.phase != PinchPhase::Cancelled
            && factor.is_finite()
            && factor > 0.0
            && let Some(entity) = target
        {
            if let Some(view) = world.non_send_mut::<EditorViews>().text_mut(entity)
                && !view.find_visible()
                && !view.line_jump_visible()
            {
                view.zoom_by(factor);
            } else {
                target = None;
            }
        }
        if matches!(event.phase, PinchPhase::Ended | PinchPhase::Cancelled) {
            target = None;
        }
    }
    world.resource_mut::<GestureTarget>().0 = target;
    let active = world.resource::<DocumentFrame>().active_view;
    let enabled = active.is_some_and(|active| {
        world.non_send::<EditorViews>().widgets.values().any(
            |widget| matches!(widget, crate::feature::Widget::Text(view) if view.id() == active),
        )
    });
    for command in [ZOOM_IN, ZOOM_OUT, RESET_ZOOM] {
        world.resource_mut::<CommandPresentations>().0.insert(
            command.into(),
            CommandPresentation {
                enabled,
                ..Default::default()
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EditorPlugin, TEXT_PANEL_TYPE};
    use bedtime_api::workbench::WorkbenchAction;
    use bedtime_api::{
        documents::PanelBinding,
        workbench::{PanelFocus, PinchEvent},
    };
    use bedtime_documents::DocumentSession;
    use bedtime_workbench::{
        WorkbenchLayout, WorkbenchPlugin, WorkbenchRequests, capture_command, process_requests,
    };
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions};
    use serde_json::Value;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((WorkbenchPlugin, EditorPlugin));
        app
    }

    fn open(app: &mut App) -> Entity {
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(WorkbenchAction::OpenPanel {
                panel_type: TEXT_PANEL_TYPE.into(),
                state: Value::Null,
                area: None,
            });
        process_requests(app.world_mut());
        assert!(
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .take_errors()
                .is_empty()
        );
        app.world().resource::<WorkbenchLayout>().focused.unwrap()
    }

    fn focus(app: &mut App, entity: Entity) {
        let system = app.world().get::<PanelFocus>(entity).unwrap().0;
        app.world_mut()
            .run_system_with(system, entity)
            .unwrap()
            .unwrap();
    }

    fn context() -> Context {
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context
    }

    fn draw(app: &mut App, context: &mut Context, entities: &[Entity]) {
        context.prepare_frame(FramePrepareOptions::new([900.0, 420.0], 1.0 / 60.0));
        let ui = context.frame();
        let mut actions = Vec::new();
        for (index, entity) in entities.iter().copied().enumerate() {
            ui.window(format!("Text {index}"))
                .position([index as f32 * 450.0, 0.0], Condition::Always)
                .size([450.0, 420.0], Condition::Always)
                .build(|| {
                    actions.extend(
                        bedtime_workbench::render_panel(app.world_mut(), entity, ui).unwrap(),
                    )
                });
        }
        gestures(app.world_mut(), ui);
        app.world_mut().resource_mut::<PinchEvents>().0.clear();
        drop(context.render_legacy());
        for action in actions {
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .push(action);
        }
        process_requests(app.world_mut());
        assert!(
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .take_errors()
                .is_empty()
        );
    }

    fn zoom(app: &App, entity: Entity) -> f32 {
        app.world()
            .non_send::<EditorViews>()
            .text(entity)
            .unwrap()
            .zoom()
    }

    fn point(app: &App, entity: Entity) -> [f32; 2] {
        let layout = app
            .world()
            .non_send::<EditorViews>()
            .text(entity)
            .unwrap()
            .presentation()
            .layout;
        [
            layout.text_pos[0] + 12.0,
            layout.text_pos[1] + layout.line_height * 0.5,
        ]
    }

    fn pinch(app: &mut App, point: [f32; 2], delta: f64, phase: PinchPhase) {
        app.world_mut()
            .resource_mut::<PinchEvents>()
            .0
            .push(PinchEvent {
                position: point,
                delta,
                phase,
            });
    }

    #[test]
    fn registered_zoom_uses_captured_source_after_focus_moves() {
        let mut app = app();
        let first = open(&mut app);
        let invocation = capture_command(app.world(), ZOOM_IN, Value::Null);
        let second = open(&mut app);
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(WorkbenchAction::Invoke(invocation));
        process_requests(app.world_mut());
        assert!((zoom(&app, first) - 1.1).abs() < 0.00001);
        assert_eq!(zoom(&app, second), 1.0);
        assert_eq!(
            app.world().resource::<WorkbenchLayout>().focused,
            Some(second)
        );
        let document = app
            .world()
            .get::<PanelBinding>(first)
            .unwrap()
            .document
            .unwrap();
        assert!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(document)
                .unwrap()
                .bytes
                .is_empty()
        );
    }

    #[test]
    fn native_zoom_chords_consume_only_their_characters_at_limits() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut app = app();
        let entity = open(&mut app);
        let mut context = context();
        focus(&mut app, entity);
        for _ in 0..3 {
            draw(&mut app, &mut context, &[entity]);
        }
        let document = app
            .world()
            .get::<PanelBinding>(entity)
            .unwrap()
            .document
            .unwrap();
        let modifier = if cfg!(target_os = "macos") {
            Key::ModSuper
        } else {
            Key::ModCtrl
        };
        for (key, text, factor, limit) in
            [(Key::Equal, '=', 10.0, 1.4), (Key::Minus, '-', 0.01, 0.6)]
        {
            app.world_mut()
                .non_send_mut::<EditorViews>()
                .text_mut(entity)
                .unwrap()
                .zoom_by(factor);
            context.io_mut().add_key_event(modifier, true);
            for _ in 0..45 {
                context.io_mut().add_input_character(text);
                context.io_mut().add_key_event(key, true);
                draw(&mut app, &mut context, &[entity]);
                assert!((zoom(&app, entity) - limit).abs() < 0.00001);
                assert!(
                    app.world()
                        .non_send::<DocumentSession>()
                        .snapshot(document)
                        .unwrap()
                        .bytes
                        .is_empty()
                );
            }
            context.io_mut().add_key_event(key, false);
            context.io_mut().add_key_event(modifier, false);
            draw(&mut app, &mut context, &[entity]);
        }
        app.world_mut()
            .non_send_mut::<EditorViews>()
            .text_mut(entity)
            .unwrap()
            .zoom_by(10.0);
        context.io_mut().add_key_event(modifier, true);
        context.io_mut().add_key_event(Key::Key0, true);
        for _ in 0..45 {
            context.io_mut().add_input_character('0');
            draw(&mut app, &mut context, &[entity]);
            assert!(
                app.world()
                    .non_send::<DocumentSession>()
                    .snapshot(document)
                    .unwrap()
                    .bytes
                    .is_empty()
            );
        }
        assert_eq!(zoom(&app, entity), 1.0);
        context.io_mut().add_input_characters_utf8("é0雪0🙂");
        draw(&mut app, &mut context, &[entity]);
        assert_eq!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(document)
                .unwrap()
                .bytes,
            "é雪🙂".as_bytes()
        );
        context.io_mut().add_key_event(Key::Key0, false);
        context.io_mut().add_key_event(modifier, false);
        draw(&mut app, &mut context, &[entity]);
        context.io_mut().add_key_event(modifier, true);
        context.io_mut().add_input_characters_utf8("é=雪+🙂-");
        context.io_mut().add_key_event(Key::Equal, true);
        draw(&mut app, &mut context, &[entity]);
        assert_eq!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(document)
                .unwrap()
                .bytes,
            "é雪🙂é雪🙂-".as_bytes()
        );
        context.io_mut().add_key_event(Key::Equal, false);
        context.io_mut().add_key_event(modifier, false);
        draw(&mut app, &mut context, &[entity]);
        context.io_mut().add_input_characters_utf8("=+-0");
        context.io_mut().add_key_event(Key::Equal, true);
        context.io_mut().add_key_event(Key::Key0, true);
        draw(&mut app, &mut context, &[entity]);
        assert_eq!(
            app.world()
                .non_send::<DocumentSession>()
                .snapshot(document)
                .unwrap()
                .bytes,
            "é雪🙂é雪🙂-=+-0".as_bytes()
        );
    }

    #[test]
    fn native_pinch_retains_its_starting_pane_and_cancels_for_overlay_or_retirement() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut app = app();
        let first = open(&mut app);
        let second = open(&mut app);
        let mut context = context();
        for _ in 0..4 {
            draw(&mut app, &mut context, &[first, second]);
        }
        let start = point(&app, first);
        context.io_mut().add_mouse_pos_event(start);
        pinch(&mut app, start, 0.25, PinchPhase::Started);
        draw(&mut app, &mut context, &[first, second]);
        assert!((zoom(&app, first) - 1.25).abs() < 0.00001);
        let other = point(&app, second);
        context.io_mut().add_mouse_pos_event(other);
        focus(&mut app, second);
        for delta in [f64::NAN, f64::INFINITY, -2.0] {
            pinch(&mut app, other, delta, PinchPhase::Moved);
        }
        pinch(&mut app, other, 0.2, PinchPhase::Moved);
        pinch(&mut app, other, 0.0, PinchPhase::Ended);
        pinch(&mut app, other, 1.0, PinchPhase::Moved);
        draw(&mut app, &mut context, &[first, second]);
        assert_eq!(zoom(&app, first), 1.4);
        assert_eq!(zoom(&app, second), 1.0);
        pinch(&mut app, other, 0.1, PinchPhase::Started);
        draw(&mut app, &mut context, &[first, second]);
        app.world_mut()
            .non_send_mut::<EditorViews>()
            .text_mut(second)
            .unwrap()
            .open_line_jump();
        pinch(&mut app, other, 1.0, PinchPhase::Moved);
        draw(&mut app, &mut context, &[first, second]);
        assert!((zoom(&app, second) - 1.1).abs() < 0.00001);
        assert!(app.world().resource::<GestureTarget>().0.is_none());
        let start = point(&app, first);
        context.io_mut().add_mouse_pos_event(start);
        pinch(&mut app, start, 0.0, PinchPhase::Started);
        draw(&mut app, &mut context, &[first]);
        crate::close_panel(app.world_mut(), first).unwrap();
        pinch(&mut app, start, 1.0, PinchPhase::Moved);
        draw(&mut app, &mut context, &[second]);
        assert!(app.world().resource::<GestureTarget>().0.is_none());
    }
}
