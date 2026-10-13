//! Host supplies overrides; the workbench owns frame input and global dispatch.
use crate::{WorkbenchLayout, WorkbenchRequests, api};
use bed_keybindings::{Bindings, Platform, Scope};
use bevy::prelude::*;
use dear_imgui_rs::{PopupQueryFlags, Ui};
use serde_json::Value;

pub use api::ShortcutRuntime as WorkbenchShortcuts;

pub fn prepare_shortcuts(world: &mut World, ui: &Ui) {
    world.init_non_send::<WorkbenchShortcuts>();
    if world.non_send::<WorkbenchShortcuts>().prepared_frame == Some(ui.frame_count()) {
        return;
    }
    let actions = world
        .resource::<api::WorkbenchRegistry>()
        .shortcuts()
        .to_vec();
    let live_recording = world
        .non_send::<WorkbenchShortcuts>()
        .recording
        .filter(|entity| {
            api::panel_owner(world, *entity)
                .is_some_and(|owner| world.resource::<WorkbenchLayout>().focused == Some(owner))
        });
    let mut runtime = world.non_send_mut::<WorkbenchShortcuts>();
    runtime.prepared_frame = Some(ui.frame_count());
    runtime.recording = live_recording;
    runtime.input.bindings = std::rc::Rc::new(Bindings::new(
        actions,
        &runtime.overrides,
        Platform::current(),
    ));
    runtime.input.begin_frame(ui);
    runtime.focus = None;
    if runtime.recording.is_some()
        || ui.is_popup_open_with_flags(
            "",
            PopupQueryFlags::ANY_POPUP_ID | PopupQueryFlags::ANY_POPUP_LEVEL,
        )
    {
        runtime.input.suppress(ui);
    }
}

pub fn global_shortcuts(world: &mut World, ui: &Ui) {
    let input = world
        .non_send::<WorkbenchShortcuts>()
        .input
        .with_scope(Scope::Global);
    let observed = world.non_send::<WorkbenchShortcuts>().focus;
    let source = observed.map(|(source, _)| source).or_else(|| {
        world.resource::<WorkbenchLayout>().focused.map(|panel| {
            world
                .get::<api::PanelFrontend>(panel)
                .map_or(panel, |front| front.0)
        })
    });
    let layout = world.resource::<WorkbenchLayout>();
    let area = observed
        .map(|(_, area)| area)
        .or(layout.focused_area)
        .unwrap_or_else(|| layout.tiling.layout.largest_area());
    let mut requests = Vec::new();
    for action in &input.bindings.actions {
        if action.scope == Scope::Global && input.take(ui, &action.id) {
            requests.push(api::WorkbenchAction::Invoke(api::CommandInvocation {
                command: action.id.clone(),
                source,
                area,
                arguments: Value::Null,
            }));
        }
    }
    for request in requests {
        world.resource_mut::<WorkbenchRequests>().push(request);
    }
}
