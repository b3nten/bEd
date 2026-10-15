//! Host supplies overrides; the workbench owns frame input and global dispatch.
use crate::{WorkbenchLayout, WorkbenchRequests, api};
use bedtime_keybindings::{Bindings, Platform, Scope};
use bevy::prelude::*;
use dear_imgui_rs::{PopupQueryFlags, Ui};
use serde_json::Value;

pub use api::ShortcutRuntime as WorkbenchShortcuts;

/// One immutable resolved map feeds UI routing, native menus and terminal input.
pub struct ResolvedKeymap {
    pub bindings: std::rc::Rc<Bindings>,
    definition_count: usize,
    overrides: bedtime_keybindings::Overrides,
}

pub fn resolve_keymap(world: &mut World) {
    world.init_non_send::<WorkbenchShortcuts>();
    let registry = world.resource::<api::WorkbenchRegistry>();
    let definition_count = registry.commands().len() + registry.shortcuts().len();
    let overrides = world
        .get_resource::<bedtime_api::settings::SettingsSnapshot>()
        .filter(|snapshot| snapshot.get("bedtime.workbench", "keybindings").is_some())
        .map(bedtime_api::settings::SettingsSnapshot::keybindings)
        .unwrap_or_else(|| world.non_send::<WorkbenchShortcuts>().overrides.clone());
    if world
        .get_non_send::<ResolvedKeymap>()
        .is_some_and(|map| map.definition_count == definition_count && map.overrides == overrides)
    {
        return;
    }
    let bindings = std::rc::Rc::new(Bindings::new(
        registry.shortcuts().to_vec(),
        &overrides,
        Platform::current(),
    ));
    world.non_send_mut::<WorkbenchShortcuts>().overrides = overrides.clone();
    world.insert_non_send(ResolvedKeymap {
        bindings,
        definition_count,
        overrides,
    });
}

pub fn prepare_shortcuts(world: &mut World, ui: &Ui) {
    resolve_keymap(world);
    if world.non_send::<WorkbenchShortcuts>().prepared_frame == Some(ui.frame_count()) {
        return;
    }
    let bindings = world.non_send::<ResolvedKeymap>().bindings.clone();
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
    runtime.input.bindings = bindings;
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
                command: action.id.clone().into(),
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
