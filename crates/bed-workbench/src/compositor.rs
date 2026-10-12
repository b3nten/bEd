//! The generic tiling frontend reads one frame-entry layout and returns intents.
//! It owns gesture state, never panel membership or authoritative geometry.
use super::*;
use bevy::ecs::system::SystemState;
use dear_imgui_rs::{FocusedFlags, StyleVar, sys};
use tiling_ui::{
    ChromePanel, ChromeSettings, ChromeTab, TabPlacement, TilingAction, TilingFrontend,
};
use ui_animations::UiAnimations;

#[derive(Resource, Clone, Debug)]
pub struct WorkbenchAppearance {
    pub animations: bool,
    pub add_button: bool,
    pub background: [f32; 4],
    pub hidden_picker: HashSet<String>,
}
impl Default for WorkbenchAppearance {
    fn default() -> Self {
        Self {
            animations: true,
            add_button: false,
            background: [0.08, 0.08, 0.08, 1.0],
            hidden_picker: HashSet::new(),
        }
    }
}
#[derive(Component, Clone, Copy, Default)]
pub struct PanelDirty(pub bool);

pub fn tab_bounds(world: &World, panel: Entity) -> Option<([f32; 2], [f32; 2])> {
    let id = world.get::<PanelPresentation>(panel)?.id;
    world
        .non_send::<TilingFrontend>()
        .tiling_ui
        .tab_rects
        .get(&id)
        .copied()
}
pub fn area_bounds(world: &World, area: u32) -> Option<([f32; 2], [f32; 2])> {
    world
        .non_send::<TilingFrontend>()
        .tiling_ui
        .area_bounds
        .get(&area)
        .copied()
}
pub fn area_at(world: &World, position: [f32; 2], viewport: u32) -> Option<u32> {
    world
        .non_send::<TilingFrontend>()
        .tiling_area_at(position, viewport)
}

pub fn render_ui(world: &mut World, ui: &Ui) {
    crate::prepare_shortcuts(world, ui);
    assert!(
        !world.contains_resource::<UiLoan>(),
        "frontend is not nested"
    );
    world.insert_resource(UiLoan);
    let result = catch_unwind(AssertUnwindSafe(|| draw(world, ui)));
    world.remove_resource::<UiLoan>().unwrap();
    crate::global_shortcuts(world, ui);
    if let Err(panic) = result {
        resume_unwind(panic);
    }
}
fn draw(world: &mut World, ui: &Ui) {
    assert!(
        ui.io()
            .config_flags()
            .contains(dear_imgui_rs::ConfigFlags::DOCKING_ENABLE),
        "enable ImGui docking before the first workbench frame"
    );
    let layout = world.resource::<WorkbenchLayout>().clone();
    let owned = world
        .query::<(Entity, &PanelPresentation, &PanelRecord)>()
        .iter(world)
        .filter(|(entity, _, _)| owns_panel(world, *entity) && api::panel_active(world, *entity))
        .map(|(entity, panel, record)| (entity, panel.id, record.panel_type.clone()))
        .collect::<Vec<_>>();
    let tabs = owned
        .into_iter()
        .map(|(entity, id, kind)| {
            let surface = world
                .get::<api::PanelFrontend>(entity)
                .map(|front| front.0)
                .unwrap_or(entity);
            ChromeTab {
                entity,
                id,
                title: panel_title(world, surface).unwrap_or(kind),
                dirty: world.get::<PanelDirty>(entity).is_some_and(|dirty| dirty.0),
            }
        })
        .collect::<Vec<_>>();
    let appearance = world.resource::<WorkbenchAppearance>().clone();
    if world
        .get_non_send::<UiAnimations>()
        .is_none_or(|animations| !animations.matches_context(ui))
    {
        world.insert_non_send(UiAnimations::new(ui.binding()));
    }
    world
        .non_send_mut::<UiAnimations>()
        .begin(ui, appearance.animations);
    let definitions = world
        .resource::<Registry>()
        .panels()
        .iter()
        .map(|definition| ChromePanel {
            id: definition.id.into(),
            label: definition.label.into(),
            allow_empty: !appearance.hidden_picker.contains(definition.id),
        })
        .collect();
    let requested = layout.focused.filter(|entity| {
        world
            .get::<PanelPresentation>(*entity)
            .is_some_and(|panel| panel.focus)
    });
    let viewport = ui.main_viewport();
    let config = *world.resource::<WorkbenchViewport>();
    let (mut origin, mut size) = config
        .bounds
        .unwrap_or((viewport.work_pos(), viewport.work_size()));
    let inset = config.top_inset.clamp(0.0, size[1]);
    if inset > 0.0 {
        // Full-size native windows expose these pixels behind the title bar.
        ui.get_background_draw_list()
            .add_rect(
                origin,
                [origin[0] + size[0], origin[1] + inset],
                appearance.background,
            )
            .filled(true)
            .build();
    }
    origin[1] += inset;
    size[1] -= inset;
    size[1] -= config.bottom_inset.clamp(0.0, size[1]);
    for mut geometry in world.query::<&mut api::PanelGeometry>().iter_mut(world) {
        geometry.visible = false;
    }
    let _rounding = ui.push_style_var(StyleVar::WindowRounding(0.0));
    let docks = {
        let mut state =
            SystemState::<(NonSendMut<TilingFrontend>, NonSendMut<UiAnimations>)>::new(world);
        let (mut chrome, mut animations) = state
            .get_mut(world)
            .expect("tiling frontend and opening animations are installed");
        // A removed initiating panel cancels its transient gesture before use.
        chrome.refresh(layout.tiling.clone(), tabs.clone());
        chrome.focused = layout
            .focused
            .and_then(|entity| tabs.iter().find(|tab| tab.entity == entity))
            .map(|tab| tab.id);
        chrome.focused_area = layout.focused_area;
        chrome.definitions = definitions;
        chrome.settings = ChromeSettings {
            animations: appearance.animations,
            add_button: appearance.add_button,
            background: appearance.background,
            opacity: appearance.background[3],
        };
        let _padding = ui.push_style_var(StyleVar::WindowPadding([0.0; 2]));
        let _border = ui.push_style_var(StyleVar::WindowBorderSize(0.0));
        ui.set_next_window_viewport(viewport.id());
        ui.window("##bed_workspace")
            .position(origin, Condition::Always)
            .size(size, Condition::Always)
            .flags(
                WindowFlags::NO_DECORATION
                    | WindowFlags::NO_BACKGROUND
                    | WindowFlags::NO_DOCKING
                    | WindowFlags::NO_MOVE
                    | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS
                    | WindowFlags::NO_NAV_FOCUS,
            )
            .build(|| {
                animations.exclude_current(ui);
                chrome.draw_tiling(ui, origin, size);
            });
        chrome.tiling_ui.docks.clone()
    };
    let mut requests = Vec::new();
    for tab in &tabs {
        let surface = world
            .get::<api::PanelFrontend>(tab.entity)
            .map(|front| front.0)
            .unwrap_or(tab.entity);
        let mut presentation = world.get_mut::<PanelPresentation>(tab.entity).unwrap();
        ui.set_next_window_viewport(viewport.id());
        if std::mem::take(&mut presentation.dock_next) {
            let area = layout
                .tiling
                .area_for(tab.id)
                .expect("owned panel belongs to an area");
            unsafe {
                sys::igSetNextWindowDockID(docks[&area], sys::ImGuiCond_Always);
            }
        }
        if std::mem::take(&mut presentation.focus) && requested == Some(tab.entity) {
            unsafe {
                sys::igSetNextWindowFocus();
            }
        }
        let mut flags = WindowFlags::NO_COLLAPSE
            | WindowFlags::NO_MOVE
            | WindowFlags::NO_BACKGROUND
            | WindowFlags::NO_FOCUS_ON_APPEARING;
        if tab.dirty {
            flags |= WindowFlags::UNSAVED_DOCUMENT;
        }
        let mut opened = true;
        let _padding = world
            .get::<api::PanelPadding>(surface)
            .map(|padding| ui.push_style_var(StyleVar::WindowPadding(padding.0)));
        ui.window(format!("{}###bed_tab_{}", tab.title, tab.id))
            .opened(&mut opened)
            .flags(flags)
            .size([700.0, 520.0], Condition::FirstUseEver)
            .build(|| {
                world.non_send_mut::<UiAnimations>().register_panel(ui);
                if ui.is_window_focused_with_flags(FocusedFlags::ROOT_AND_CHILD_WINDOWS)
                    && requested.is_none_or(|requested| requested == tab.entity)
                    && layout.focused != Some(tab.entity)
                {
                    requests.push(Action::ObserveFocus(tab.entity));
                }
                let min = ui.cursor_screen_pos();
                let extent = ui.content_region_avail();
                let max = [min[0] + extent[0], min[1] + extent[1]];
                let native = unsafe { sys::igGetCurrentWindow() };
                let (clip_min, clip_max) =
                    unsafe { ((*native).ClipRect.Min.into(), (*native).ClipRect.Max.into()) };
                world.entity_mut(tab.entity).insert(api::PanelGeometry {
                    content_min: min,
                    content_max: max,
                    clip_min,
                    clip_max,
                    visible: true,
                });
                let mut presentation = world.get_mut::<PanelPresentation>(tab.entity).unwrap();
                presentation.viewport = unsafe { (*sys::igGetWindowViewport()).ID };
                presentation.dock_id = unsafe { sys::igGetWindowDockID() };
                match render_panel(world, surface, ui) {
                    Ok(actions) => requests.extend(actions),
                    Err(error) => ui.text_wrapped(&error),
                }
            });
        if !opened {
            requests.push(Action::ClosePanel(tab.entity));
        }
    }
    let (draft, actions, chrome_requests, minimum) = {
        let mut state = SystemState::<NonSendMut<TilingFrontend>>::new(world);
        let mut chrome = state.get_mut(world).expect("tiling frontend is installed");
        chrome.draw_tab_context_menu(ui);
        chrome.draw_empty_areas(ui);
        chrome.accept_panel_drop(ui);
        chrome.finish_tiling(ui);
        (
            chrome.tiling.clone(),
            std::mem::take(&mut chrome.tiling_ui.actions),
            std::mem::take(&mut chrome.requests),
            chrome.tiling_ui.minimum,
        )
    };
    requests.extend(chrome_requests);
    let (gestures, drops) = resolve_gestures(&layout.tiling, draft, &tabs, actions, minimum);
    requests.extend(gestures);
    // Reserve incoming split destinations before features enqueue their opens.
    for request in requests {
        world.resource_mut::<WorkbenchRequests>().push(request);
    }
    for drop in drops {
        world.write_message(drop);
    }
    let mut overlays = world
        .query::<(Entity, &api::WorkbenchOverlay)>()
        .iter(world)
        .filter(|(entity, _)| world.get::<api::OverlayError>(*entity).is_none())
        .map(|(entity, overlay)| (entity, *overlay))
        .collect::<Vec<_>>();
    overlays.sort_by_key(|(entity, _)| entity.to_bits());
    for (entity, overlay) in overlays {
        let mut stacks = sys::ImGuiErrorRecoveryState::default();
        unsafe {
            sys::igErrorRecoveryStoreState(&mut stacks);
        }
        if catch_unwind(AssertUnwindSafe(|| (overlay.0)(world, ui))).is_err() {
            unsafe {
                let io = sys::igGetIO_Nil();
                let assert = (*io).ConfigErrorRecoveryEnableAssert;
                (*io).ConfigErrorRecoveryEnableAssert = false;
                sys::igErrorRecoveryTryToRecoverState(&stacks);
                (*io).ConfigErrorRecoveryEnableAssert = assert;
            }
            if world.entities().contains(entity) {
                world
                    .entity_mut(entity)
                    .insert(api::OverlayError("Feature overlay panicked".into()));
            }
        }
    }
    world
        .non_send_mut::<UiAnimations>()
        .end(ui, appearance.animations);
}

fn resolve_gestures(
    original: &TilingState,
    mut draft: TilingState,
    tabs: &[ChromeTab],
    actions: Vec<TilingAction>,
    minimum: [i32; 2],
) -> (Vec<Action>, Vec<api::PanelDrop>) {
    let closing = actions
        .iter()
        .filter_map(|action| match action {
            TilingAction::CloseTab(id) => Some(*id),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let entity = |id| tabs.iter().find(|tab| tab.id == id).map(|tab| tab.entity);
    let mut requests = Vec::new();
    let mut close = Vec::new();
    let mut drops = Vec::new();
    for action in actions {
        match action {
            TilingAction::Split { area, axis, at } => {
                if let Some(area) = draft.layout.split(area, axis, at, minimum[axis]) {
                    draft.add_area(area);
                }
            }
            TilingAction::Plan { area, plan } => {
                if let Some(panel) = draft
                    .areas
                    .iter()
                    .find(|group| group.area == area)
                    .and_then(|group| group.selected)
                    .and_then(entity)
                {
                    requests.push(Action::FocusPanel(panel));
                } else {
                    requests.push(Action::FocusArea(area));
                }
                let removed = draft
                    .areas
                    .iter()
                    .filter(|group| !plan.layout.areas.iter().any(|area| area.id == group.area))
                    .flat_map(|group| group.tabs.iter().copied())
                    .collect::<Vec<_>>();
                for id in removed {
                    draft.remove(id);
                    if let Some(entity) = entity(id) {
                        close.push(entity);
                    }
                }
                draft.apply_plan(plan);
            }
            TilingAction::MoveTab { panel, area, index }
                if !closing.contains(&panel) && draft.area_for(panel).is_some() =>
            {
                draft.move_tab(panel, area, index)
            }
            TilingAction::SplitTab {
                panel,
                area,
                axis,
                high,
            } if !closing.contains(&panel) && draft.area_for(panel).is_some() => {
                draft.split_tab(panel, area, axis, high, minimum);
            }
            TilingAction::Drop {
                placement,
                payload_type,
                data,
            } => {
                let (area, tab_index) = match placement {
                    TabPlacement::Insert { area, index } => {
                        (area, (index != usize::MAX).then_some(index))
                    }
                    TabPlacement::Split { area, axis, high } => {
                        let Some(area) = draft.split_area(area, axis, high, minimum) else {
                            continue;
                        };
                        (area, None)
                    }
                };
                let panel = draft
                    .areas
                    .iter()
                    .find(|group| group.area == area)
                    .and_then(|group| group.selected)
                    .and_then(entity);
                drops.push(api::PanelDrop {
                    area,
                    panel,
                    tab_index,
                    payload_type,
                    data,
                });
            }
            TilingAction::CloseTab(panel) => {
                if let Some(entity) = entity(panel) {
                    requests.push(Action::ClosePanel(entity));
                }
                draft.remove(panel);
            }
            TilingAction::CloseArea(area) => {
                draft.close_empty_area(area);
            }
            TilingAction::AddPanel { area, kind } => requests.push(Action::OpenPanel {
                panel_type: kind,
                state: Value::Null,
                area: Some(area),
            }),
            _ => {}
        }
    }
    // A close intention has its own save barrier. Geometry cannot describe its
    // absence before that barrier commits, so include it in the same transition.
    if draft != *original {
        for id in closing {
            if let Some(entity) = entity(id) {
                close.push(entity);
            }
        }
        requests.retain(
            |request| !matches!(request,Action::ClosePanel(entity) if close.contains(entity)),
        );
        requests.insert(
            0,
            Action::SetLayout {
                tiling: draft.to_value(
                    tabs.iter()
                        .filter_map(|tab| draft.area_for(tab.id).map(|_| tab.id)),
                ),
                close,
            },
        );
    }
    (requests, drops)
}
