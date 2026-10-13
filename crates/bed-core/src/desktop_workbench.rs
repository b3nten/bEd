//! Native input and clipboard mechanics shared by every desktop workbench.
//! Input captures requests; the document/application reducer owns their effects.
use bed_api::{
    features::{HostAction, HostEffect, HostEffects, HostRequest},
    workbench as api,
};
use bed_desktop::{
    DesktopContext, DesktopFrame, DesktopFrameInfo, DesktopInputEvents, DesktopSystems,
    winit::event::WindowEvent,
};
use bevy::prelude::*;
use std::{io, path::PathBuf};

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// Translation follows optional input interception; geometry follows drawing.
pub enum DesktopWorkbenchSystems {
    Input,
    Geometry,
}

/// Installed by the owned launcher alongside the document and desktop services.
pub struct DesktopWorkbenchPlugin;
impl Plugin for DesktopWorkbenchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FileDrag>()
            .add_systems(
                PreUpdate,
                install_clipboard
                    .in_set(DesktopSystems::Prepare)
                    .after(bed_desktop::DesktopNativeSystems::Prepare),
            )
            .add_systems(
                PreUpdate,
                input
                    .after(DesktopSystems::InterceptInput)
                    .before(DesktopSystems::Input)
                    .in_set(DesktopWorkbenchSystems::Input),
            )
            .add_systems(
                PreUpdate,
                publish_geometry
                    .in_set(DesktopSystems::AfterUi)
                    .in_set(DesktopWorkbenchSystems::Geometry)
                    .before(crate::CoreSystems::Requests),
            );
        #[cfg(target_os = "macos")]
        app.add_systems(
            PreUpdate,
            prepare_toolbar_drag
                .in_set(DesktopSystems::Prepare)
                .after(bed_desktop::DesktopNativeSystems::Prepare),
        );
    }
}

#[cfg(target_os = "macos")]
fn prepare_toolbar_drag(world: &mut World) {
    if !world.contains_non_send::<DesktopContext>() {
        return;
    }
    if bed_desktop::native::macos_window::toolbar_drag().is_some() {
        let (position, _) = native_pointer(world);
        let pressed = bed_desktop::native::macos_window::toolbar_drag_pressed();
        let mut desktop = world.non_send_mut::<DesktopContext>();
        desktop.context.io_mut().add_mouse_pos_event(position);
        desktop
            .context
            .io_mut()
            .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, pressed);
    }
}

/// Bridge native toolbar gestures into the workbench's ordinary command payload.
/// Call before drawing panels when supplying an application's custom frontend.
pub fn begin_native_toolbar_drag(ui: &dear_imgui_rs::Ui) {
    #[cfg(target_os = "macos")]
    if let Some((command, label)) = bed_desktop::native::macos_window::toolbar_drag() {
        ui.with_bound_context(|| unsafe {
            if dear_imgui_rs::sys::igBeginDragDropSource(
                dear_imgui_rs::sys::ImGuiDragDropFlags_SourceExtern
                    | dear_imgui_rs::sys::ImGuiDragDropFlags_SourceNoPreviewTooltip,
            ) {
                dear_imgui_rs::sys::igSetDragDropPayload(
                    c"BED_TOOLBAR_COMMAND".as_ptr(),
                    command.as_ptr().cast(),
                    command.len(),
                    dear_imgui_rs::sys::ImGuiCond_Always,
                );
                api::draw_panel_drag_label(ui, &label);
                dear_imgui_rs::sys::igEndDragDropSource();
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = ui;
}

/// Complete native toolbar ownership after the frontend has observed its drops.
pub fn finish_native_toolbar_drag(ui: &dear_imgui_rs::Ui) {
    #[cfg(target_os = "macos")]
    if !ui.is_mouse_down(dear_imgui_rs::MouseButton::Left) {
        bed_desktop::native::macos_window::finish_toolbar_drag();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = ui;
}

/// Publish the desktop's textures and hide workbench canvases before drawing.
/// Other canvases in the same Bevy world keep their application's visibility.
pub fn prepare_workbench_ui(world: &mut World) {
    if let Some(textures) = world.get_resource::<bed_ui::gpu::CanvasTextures>() {
        let textures = textures.0.clone();
        world
            .resource_mut::<bed_api::presentation::PresentationFrame>()
            .textures = textures;
    }
    let canvases = world
        .query_filtered::<Entity, With<bed_ui::gpu::CanvasVisible>>()
        .iter(world)
        .filter(|entity| {
            api::panel_owner(world, *entity)
                .is_some_and(|owner| bed_workbench::owns_panel(world, owner))
        })
        .collect::<Vec<_>>();
    for entity in canvases {
        world
            .get_mut::<bed_ui::gpu::CanvasVisible>(entity)
            .unwrap()
            .0 = false;
    }
}

/// Publish drawn workbench canvases and discard a quarantined frontend's effects.
/// Applications still own admission and execution of the remaining effects.
pub fn finish_workbench_ui(world: &mut World) {
    let effects = world.resource_mut::<HostEffects>().take();
    for effect in effects {
        if effect.source.is_some_and(|source| {
            api::panel_owner(world, source)
                .filter(|owner| bed_workbench::owns_panel(world, *owner))
                .is_some_and(|owner| {
                    [source, owner].into_iter().any(|entity| {
                        world
                            .get::<bed_workbench::PanelRecord>(entity)
                            .is_some_and(|record| record.unavailable.is_some())
                    })
                })
        }) {
            continue;
        }
        world.resource_mut::<HostEffects>().push_effect(effect);
    }
    let canvases = world
        .query::<(Entity, &api::PanelGeometry, Option<&api::PanelFrontend>)>()
        .iter(world)
        .filter(|(owner, geometry, _)| {
            geometry.visible
                && bed_workbench::owns_panel(world, *owner)
                && api::panel_active(world, *owner)
        })
        .map(|(owner, _, frontend)| frontend.map_or(owner, |frontend| frontend.0))
        .collect::<Vec<_>>();
    for entity in canvases {
        world
            .entity_mut(entity)
            .insert(bed_ui::gpu::CanvasVisible(true));
    }
}

struct FileClipboard(bed_desktop::DesktopFileClipboard);
impl api::FileClipboardService for FileClipboard {
    fn read_files(&mut self) -> io::Result<Vec<PathBuf>> {
        self.0.read_files()
    }
    fn write_files(&mut self, paths: &[PathBuf]) -> io::Result<()> {
        self.0.write_files(paths)
    }
    fn read_paste_files(&mut self) -> io::Result<Vec<PathBuf>> {
        self.0.read_paste_files()
    }
}
fn install_clipboard(world: &mut World) {
    if !world.contains_non_send::<Box<dyn api::FileClipboardService>>()
        && let Some(clipboard) = world
            .get_non_send::<bed_desktop::DesktopFileClipboard>()
            .cloned()
    {
        world.insert_non_send(
            Box::new(FileClipboard(clipboard)) as Box<dyn api::FileClipboardService>
        );
    }
}

#[derive(Resource, Default)]
struct FileDrag {
    paths: Vec<PathBuf>,
    panel: Option<Entity>,
}

fn frontend(world: &World, panel: Entity) -> Option<Entity> {
    if let Some(frontend) = world.get::<api::PanelFrontend>(panel) {
        Some(frontend.0)
    } else {
        api::panel_owner(world, panel).map(|_| panel)
    }
}

fn emit(world: &mut World, source: Option<Entity>, action: HostAction, area: u32) {
    let request = HostRequest::capture(
        world
            .non_send::<bed_documents::DocumentSession>()
            .session_id(),
        source,
        action,
        area,
        world.resource::<bed_workbench::WorkbenchLayoutEpoch>().0,
    );
    world.write_message(request);
}

fn input(world: &mut World) {
    if !world.contains_non_send::<DesktopContext>() {
        return;
    }
    let mut events = Vec::new();
    let mut pinches = Vec::new();
    for event in &mut world.resource_mut::<DesktopInputEvents>().0 {
        if event.consumed {
            continue;
        }
        match &event.event.event {
            WindowEvent::DroppedFile(_)
            | WindowEvent::HoveredFile(_)
            | WindowEvent::HoveredFileCancelled => {
                event.consumed = true;
                events.push(event.event.event.clone());
            }
            WindowEvent::CursorMoved { .. } => events.push(event.event.event.clone()),
            WindowEvent::PinchGesture { delta, phase, .. } => {
                event.consumed = true;
                pinches.push((*delta, *phase));
            }
            _ => {}
        }
    }
    file_events(world, events);
    for (delta, phase) in pinches {
        let (position, _) = native_pointer(world);
        let phase = match phase {
            bed_desktop::winit::event::TouchPhase::Started => api::PinchPhase::Started,
            bed_desktop::winit::event::TouchPhase::Moved => api::PinchPhase::Moved,
            bed_desktop::winit::event::TouchPhase::Ended => api::PinchPhase::Ended,
            bed_desktop::winit::event::TouchPhase::Cancelled => api::PinchPhase::Cancelled,
        };
        world
            .resource_mut::<api::PinchEvents>()
            .0
            .push(api::PinchEvent {
                position,
                delta,
                phase,
            });
    }
}

fn publish_geometry(world: &mut World) {
    let projected: Vec<_> = world
        .query::<(Entity, &api::PanelGeometry)>()
        .iter(world)
        .map(|(panel, geometry)| bed_desktop::DesktopPanelGeometry {
            panel,
            content: bed_desktop::DesktopRect {
                min: geometry.content_min,
                max: geometry.content_max,
            },
            clip: bed_desktop::DesktopRect {
                min: geometry.clip_min,
                max: geometry.clip_max,
            },
            visible: geometry.visible,
        })
        .collect();
    // The desktop finalizes FrameInfo inside Ui. Preserve geometry published by
    // a custom desktop frontend; ECS geometry owns only its matching targets.
    // This bridge runs before captures/extraction and updates that current frame.
    let panels = {
        let mut frame = world.resource_mut::<DesktopFrame>();
        frame.panels.retain(|published| {
            !projected
                .iter()
                .any(|geometry| geometry.panel == published.panel)
        });
        frame.panels.extend(projected);
        frame.panels.clone()
    };
    world.resource_mut::<DesktopFrameInfo>().panels = panels;
}

/// Native edit commands preserve the focused presentation captured before UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeEditAction {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}
impl NativeEditAction {
    pub fn from_command(command: &str) -> Option<Self> {
        Some(match command {
            "bed.edit.undo" => Self::Undo,
            "bed.edit.redo" => Self::Redo,
            "bed.edit.cut" => Self::Cut,
            "bed.edit.copy" => Self::Copy,
            "bed.edit.paste" => Self::Paste,
            "bed.edit.select_all" => Self::SelectAll,
            _ => return None,
        })
    }
    fn shortcut(self) -> bed_keybindings::Stroke {
        let key = match self {
            Self::Undo => "Primary+Z",
            Self::Redo => "Primary+Shift+Z",
            Self::Cut => "Primary+X",
            Self::Copy => "Primary+C",
            Self::Paste => "Primary+V",
            Self::SelectAll => "Primary+A",
        };
        key.parse::<bed_keybindings::Stroke>()
            .expect("Valid native edit shortcut")
            .resolved(bed_keybindings::Platform::current())
    }
}

/// Native menus consumed the original event. Ordinary ImGui text inputs receive
/// its logical shortcut; registered panels produce captured reducer requests.
pub fn route_native_edit(world: &mut World, action: NativeEditAction) {
    let text_input = world
        .non_send::<DesktopContext>()
        .context
        .io()
        .want_text_input();
    let source = world
        .resource::<bed_workbench::WorkbenchLayout>()
        .focused
        .and_then(|panel| frontend(world, panel));
    if !text_input && let Some(panel) = source {
        let panel_action = match action {
            NativeEditAction::Undo => Some(api::PanelAction::Undo),
            NativeEditAction::Redo => Some(api::PanelAction::Redo),
            NativeEditAction::SelectAll => Some(api::PanelAction::SelectAll),
            _ => None,
        };
        if world.get::<api::PanelActions>(panel).is_some()
            && let Some(action) = panel_action
        {
            let area = world
                .resource::<bed_workbench::WorkbenchLayout>()
                .tiling
                .layout
                .largest_area();
            emit(
                world,
                Some(panel),
                HostAction::PanelAction { panel, action },
                area,
            );
            return;
        }
        if world.get::<api::PanelClipboard>(panel).is_some() {
            let operation = match action {
                NativeEditAction::Copy => Some(api::ClipboardAction::Copy),
                NativeEditAction::Cut => Some(api::ClipboardAction::Cut),
                NativeEditAction::Paste => Some(api::ClipboardAction::PasteFromClipboard(
                    world
                        .non_send_mut::<DesktopContext>()
                        .context
                        .clipboard_text(),
                )),
                NativeEditAction::SelectAll => Some(api::ClipboardAction::SelectAll),
                _ => None,
            };
            if let Some(action) = operation {
                let area = world
                    .resource::<bed_workbench::WorkbenchLayout>()
                    .tiling
                    .layout
                    .largest_area();
                emit(
                    world,
                    Some(panel),
                    HostAction::Clipboard { panel, action },
                    area,
                );
                return;
            }
        }
    }
    queue_native_shortcut(
        &mut world.non_send_mut::<DesktopContext>().context,
        action.shortcut(),
    );
}

pub fn queue_native_shortcut(
    context: &mut dear_imgui_rs::Context,
    stroke: bed_keybindings::Stroke,
) {
    use dear_imgui_rs::Key;
    let io = context.io_mut();
    for (key, pressed) in [
        (Key::ModCtrl, stroke.modifiers.ctrl),
        (Key::ModShift, stroke.modifiers.shift),
        (Key::ModAlt, stroke.modifiers.alt),
        (Key::ModSuper, stroke.modifiers.super_key),
    ] {
        if pressed {
            io.add_key_event(key, true);
        }
    }
    io.add_key_event(stroke.key, true);
    io.add_key_event(stroke.key, false);
    for (key, pressed) in [
        (Key::ModCtrl, stroke.modifiers.ctrl),
        (Key::ModShift, stroke.modifiers.shift),
        (Key::ModAlt, stroke.modifiers.alt),
        (Key::ModSuper, stroke.modifiers.super_key),
    ] {
        if pressed {
            io.add_key_event(key, false);
        }
    }
}

/// Execute one captured clipboard request. Feature handlers own their paste
/// interpretation; the shared bridge only transfers returned text to the OS.
pub fn dispatch_panel_clipboard(
    world: &mut World,
    panel: Entity,
    action: api::ClipboardAction,
) -> Result<Vec<HostEffect>, String> {
    let entity = frontend(world, panel).ok_or("Clipboard panel is unavailable")?;
    let handler = world
        .get::<api::PanelClipboard>(entity)
        .ok_or("Panel does not support clipboard actions")?
        .0;
    let previous = world.resource_mut::<HostEffects>().take();
    let result = world
        .run_system_with(handler, api::ClipboardInvocation { entity, action })
        .map_err(|error| error.to_string())
        .and_then(|result| result);
    let effects = world.resource_mut::<HostEffects>().take();
    for effect in previous {
        world.resource_mut::<HostEffects>().push_effect(effect);
    }
    let result = result?;
    if let Some(text) = result {
        world
            .non_send_mut::<DesktopContext>()
            .context
            .set_clipboard_text(text);
    }
    Ok(effects)
}

/// Invoke the captured panel and return its synchronous effects to its reducer.
pub fn dispatch_panel_action(
    world: &mut World,
    panel: Entity,
    action: api::PanelAction,
) -> Result<(bool, Vec<HostEffect>), String> {
    let entity = frontend(world, panel).ok_or("Action panel is unavailable")?;
    let handler = world
        .get::<api::PanelActions>(entity)
        .ok_or("Panel does not support this action")?
        .0;
    let previous = world.resource_mut::<HostEffects>().take();
    let result = world
        .run_system_with(handler, api::PanelActionInvocation { entity, action })
        .map_err(|error| error.to_string())
        .and_then(|result| result);
    let effects = world.resource_mut::<HostEffects>().take();
    for effect in previous {
        world.resource_mut::<HostEffects>().push_effect(effect);
    }
    if effects.iter().any(|effect| effect.source != Some(entity)) {
        return Err("Panel actions may only produce their own effects".into());
    }
    result.map(|accepted| (accepted, effects))
}

/// Resolve a panel's native drop handler without imposing an unhandled-drop policy.
pub fn dispatch_external_files(
    world: &mut World,
    panel: Option<Entity>,
    event: api::ExternalFileDrag,
) -> Result<(api::ExternalFileDropResponse, Vec<HostEffect>), String> {
    let Some(entity) = panel.and_then(|panel| frontend(world, panel)) else {
        return Ok((api::ExternalFileDropResponse::Ignored, Vec::new()));
    };
    let Some(handler) = world.get::<api::PanelExternalFiles>(entity).copied() else {
        return Ok((api::ExternalFileDropResponse::Ignored, Vec::new()));
    };
    let previous = world.resource_mut::<HostEffects>().take();
    let result = world
        .run_system_with(
            handler.0,
            api::PanelExternalFilesInvocation { entity, event },
        )
        .map_err(|error| error.to_string())
        .and_then(|result| result);
    let effects = world.resource_mut::<HostEffects>().take();
    for effect in previous {
        world.resource_mut::<HostEffects>().push_effect(effect);
    }
    result.map(|response| (response, effects))
}

fn file_events(world: &mut World, events: Vec<WindowEvent>) {
    use bed_api::workbench::{ExternalFileDrag, ExternalFileDragPhase};
    let mut dropped = Vec::new();
    for event in events {
        let phase = match event {
            WindowEvent::DroppedFile(path) => {
                dropped.push(path);
                continue;
            }
            WindowEvent::HoveredFile(path) => {
                let mut drag = world.resource_mut::<FileDrag>();
                if !drag.paths.contains(&path) {
                    drag.paths.push(path);
                }
                ExternalFileDragPhase::Hover
            }
            WindowEvent::HoveredFileCancelled => ExternalFileDragPhase::Cancel,
            WindowEvent::CursorMoved { .. } if !world.resource::<FileDrag>().paths.is_empty() => {
                ExternalFileDragPhase::Hover
            }
            _ => continue,
        };
        let (position, modifiers) = native_pointer(world);
        let previous = world.resource::<FileDrag>().panel;
        let panel = if phase == ExternalFileDragPhase::Cancel {
            previous
        } else {
            panel_at(world, position)
        };
        let paths = world.resource::<FileDrag>().paths.clone();
        if previous != panel
            && let Some(previous) = previous
        {
            submit_drag(
                world,
                Some(previous),
                ExternalFileDrag {
                    paths: Vec::new(),
                    position,
                    modifiers,
                    viewport: world
                        .get::<bed_api::workbench::PanelPresentation>(previous)
                        .map_or(0, |panel| panel.viewport),
                    phase: ExternalFileDragPhase::Cancel,
                },
            );
        }
        let viewport = panel
            .and_then(|panel| world.get::<bed_api::workbench::PanelPresentation>(panel))
            .map_or(0, |panel| panel.viewport);
        submit_drag(
            world,
            panel,
            ExternalFileDrag {
                paths,
                position,
                viewport,
                phase,
                modifiers,
            },
        );
        let mut drag = world.resource_mut::<FileDrag>();
        drag.panel = panel;
        if phase == ExternalFileDragPhase::Cancel {
            drag.paths.clear();
            drag.panel = None;
        }
    }
    if !dropped.is_empty() {
        let (position, modifiers) = native_pointer(world);
        let panel = panel_at(world, position);
        let viewport = panel
            .and_then(|panel| world.get::<bed_api::workbench::PanelPresentation>(panel))
            .map_or(0, |panel| panel.viewport);
        submit_drag(
            world,
            panel,
            ExternalFileDrag {
                paths: dropped,
                position,
                viewport,
                phase: ExternalFileDragPhase::Drop,
                modifiers,
            },
        );
        let mut drag = world.resource_mut::<FileDrag>();
        drag.paths.clear();
        drag.panel = None;
    }
}

fn panel_at(world: &mut World, position: [f32; 2]) -> Option<Entity> {
    world
        .query::<(Entity, &api::PanelGeometry)>()
        .iter(world)
        .find(|(entity, rect)| {
            rect.visible
                && api::panel_active(world, *entity)
                && (0..2).all(|axis| {
                    position[axis] >= rect.content_min[axis].max(rect.clip_min[axis])
                        && position[axis] < rect.content_max[axis].min(rect.clip_max[axis])
                })
        })
        .map(|(entity, _)| entity)
}

fn submit_drag(world: &mut World, panel: Option<Entity>, event: api::ExternalFileDrag) {
    let source = panel.and_then(|panel| frontend(world, panel));
    let area = bed_workbench::area_at(world, event.position, event.viewport).unwrap_or_else(|| {
        world
            .resource::<bed_workbench::WorkbenchLayout>()
            .tiling
            .layout
            .largest_area()
    });
    emit(
        world,
        source,
        HostAction::ExternalFiles { panel, event },
        area,
    );
}

/// Observe window-local logical coordinates and the native drag modifier state.
pub fn native_pointer(world: &mut World) -> ([f32; 2], bed_api::workbench::ExternalFileModifiers) {
    let io = world.non_send::<DesktopContext>().context.io();
    let mut position = io.mouse_pos();
    let mut modifiers = bed_api::workbench::ExternalFileModifiers {
        control: io.key_ctrl(),
        shift: io.key_shift(),
        alt: io.key_alt(),
        super_key: io.key_super(),
    };
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let primary = world
            .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
            .iter(world)
            .next();
        bevy::winit::WINIT_WINDOWS.with(|windows| {
            let windows = windows.borrow();
            let Some(window) = primary.and_then(|entity| windows.get_window(entity)) else {
                return;
            };
            #[cfg(target_os = "macos")]
            {
                if let Some(mut native) =
                    bed_desktop::native::macos_window::external_drag_position()
                    && let Ok(origin) = window.inner_position()
                {
                    let origin = origin.to_logical::<f32>(window.scale_factor());
                    native[0] -= origin.x;
                    native[1] -= origin.y;
                    position = native;
                }
                if let Some(native) = bed_desktop::native::macos_window::external_drag_modifiers() {
                    modifiers = bed_api::workbench::ExternalFileModifiers {
                        control: native.ctrl,
                        shift: native.shift,
                        alt: native.alt,
                        super_key: native.super_key,
                    };
                }
            }
            #[cfg(target_os = "linux")]
            {
                if let Some(native) =
                    bed_desktop::native::linux_window::external_drag_position(window, false)
                {
                    position = native;
                }
                if let Some([control, shift, alt, super_key]) =
                    bed_desktop::native::linux_window::external_drag_modifiers(window)
                {
                    modifiers = bed_api::workbench::ExternalFileModifiers {
                        control,
                        shift,
                        alt,
                        super_key,
                    };
                }
            }
        });
    }
    (position, modifiers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::message::MessageCursor;
    static IMGUI_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn bridge_world() -> World {
        let mut app = App::new();
        app.add_plugins(bed_workbench::WorkbenchPlugin);
        let mut world = std::mem::take(app.world_mut());
        world.init_resource::<bed_api::presentation::PresentationFrame>();
        world.init_resource::<HostEffects>();
        world
    }

    fn owned_panel(world: &mut World) -> Entity {
        bed_workbench::spawn_panel(
            world,
            serde_json::json!({"panel_type":"test.canvas"}),
            None,
            None,
        )
        .unwrap()
    }

    #[test]
    fn shared_ui_bridge_projects_textures_and_changes_only_workbench_canvas_visibility() {
        let mut world = bridge_world();
        let handle = bed_ui::TextureHandle(7);
        let texture = dear_imgui_rs::TextureId::new(23);
        world.insert_resource(bed_ui::gpu::CanvasTextures([(handle, texture)].into()));
        let owner = owned_panel(&mut world);
        let active = world
            .spawn((api::PanelOwner(owner), bed_ui::gpu::CanvasVisible(true)))
            .id();
        let retained = world
            .spawn((api::PanelOwner(owner), bed_ui::gpu::CanvasVisible(true)))
            .id();
        world.entity_mut(owner).insert((
            api::PanelFrontend(active),
            api::PanelGeometry {
                visible: true,
                ..Default::default()
            },
        ));
        let suspended = owned_panel(&mut world);
        world.entity_mut(suspended).insert((
            api::PanelSuspended,
            bed_ui::gpu::CanvasVisible(true),
            api::PanelGeometry {
                visible: true,
                ..Default::default()
            },
        ));
        let direct = owned_panel(&mut world);
        world.entity_mut(direct).insert((
            bed_ui::gpu::CanvasVisible(true),
            api::PanelGeometry {
                visible: true,
                ..Default::default()
            },
        ));
        let game_canvas = world.spawn(bed_ui::gpu::CanvasVisible(true)).id();
        let other_frontend = world.spawn(bed_ui::gpu::CanvasVisible(false)).id();
        world.spawn((
            api::PanelPresentation::new(100),
            api::PanelFrontend(other_frontend),
            api::PanelGeometry {
                visible: true,
                ..Default::default()
            },
        ));

        prepare_workbench_ui(&mut world);
        assert_eq!(
            world
                .resource::<bed_api::presentation::PresentationFrame>()
                .textures[&handle],
            texture
        );
        for canvas in [active, retained, suspended, direct] {
            assert!(!world.get::<bed_ui::gpu::CanvasVisible>(canvas).unwrap().0);
        }
        assert!(
            world
                .get::<bed_ui::gpu::CanvasVisible>(game_canvas)
                .unwrap()
                .0
        );

        finish_workbench_ui(&mut world);
        assert!(world.get::<bed_ui::gpu::CanvasVisible>(active).unwrap().0);
        assert!(world.get::<bed_ui::gpu::CanvasVisible>(direct).unwrap().0);
        assert!(!world.get::<bed_ui::gpu::CanvasVisible>(retained).unwrap().0);
        assert!(
            !world
                .get::<bed_ui::gpu::CanvasVisible>(suspended)
                .unwrap()
                .0
        );
        assert!(
            world
                .get::<bed_ui::gpu::CanvasVisible>(game_canvas)
                .unwrap()
                .0
        );
        assert!(
            !world
                .get::<bed_ui::gpu::CanvasVisible>(other_frontend)
                .unwrap()
                .0
        );
    }

    #[test]
    fn shared_ui_bridge_discards_quarantined_effects_and_preserves_other_producers_order() {
        let mut world = bridge_world();
        let damaged_owner = owned_panel(&mut world);
        world
            .get_mut::<bed_workbench::PanelRecord>(damaged_owner)
            .unwrap()
            .unavailable = Some("owner failed".into());
        let owner = owned_panel(&mut world);
        let damaged_frontend = world
            .spawn((
                api::PanelOwner(owner),
                bed_workbench::PanelRecord {
                    panel_type: "test.canvas".into(),
                    serialized: serde_json::Value::Null,
                    unavailable: Some("frontend failed".into()),
                },
            ))
            .id();
        let healthy = world.spawn(api::PanelOwner(owner)).id();
        let unrelated = world
            .spawn((
                api::PanelPresentation::new(100),
                bed_workbench::PanelRecord {
                    panel_type: "test.other".into(),
                    serialized: serde_json::Value::Null,
                    unavailable: Some("another application's failure".into()),
                },
            ))
            .id();
        for (source, message) in [
            (Some(damaged_owner), "damaged owner"),
            (Some(healthy), "healthy"),
            (Some(damaged_frontend), "damaged frontend"),
            (None, "application"),
            (Some(unrelated), "unrelated"),
        ] {
            world.resource_mut::<HostEffects>().push(
                source,
                HostAction::Notify {
                    message: message.into(),
                },
            );
        }

        finish_workbench_ui(&mut world);
        let messages = world
            .resource_mut::<HostEffects>()
            .take()
            .into_iter()
            .map(|effect| {
                let HostAction::Notify { message } = effect.action else {
                    panic!("Expected notification")
                };
                message
            })
            .collect::<Vec<_>>();
        assert_eq!(messages, ["healthy", "application", "unrelated"]);
    }

    fn world() -> World {
        let mut context = dear_imgui_rs::Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context.io_mut().set_mouse_pos([20.0, 20.0]);
        let platform = bed_desktop::DesktopPlatform::new(&mut context);
        let mut app = App::new();
        app.add_plugins(bed_workbench::WorkbenchPlugin);
        let mut world = std::mem::take(app.world_mut());
        world.insert_non_send(DesktopContext {
            context,
            platform,
            snapshot_context: None,
        });
        world.insert_non_send(bed_documents::DocumentSession::new());
        world.init_resource::<bed_workbench::WorkbenchLayout>();
        world.init_resource::<bed_workbench::WorkbenchLayoutEpoch>();
        world.init_resource::<Messages<HostRequest>>();
        world.init_resource::<HostEffects>();
        world.init_resource::<FileDrag>();
        world.init_resource::<api::PinchEvents>();
        world.init_resource::<DesktopFrameInfo>();
        world.init_resource::<DesktopInputEvents>();
        world.init_resource::<DesktopFrame>();
        world
    }
    fn requests(world: &World) -> Vec<HostRequest> {
        MessageCursor::default()
            .read(world.resource::<Messages<HostRequest>>())
            .cloned()
            .collect()
    }
    fn panel(world: &mut World, id: u64, min: [f32; 2], max: [f32; 2]) -> Entity {
        world
            .spawn((
                api::PanelPresentation::new(id),
                api::PanelGeometry {
                    content_min: min,
                    content_max: max,
                    clip_min: min,
                    clip_max: max,
                    visible: true,
                },
            ))
            .id()
    }

    #[test]
    fn native_file_drops_capture_the_panel_and_one_grouped_request() {
        let _lock = IMGUI_LOCK.lock().unwrap();
        let mut world = world();
        let panel = panel(&mut world, 1, [0.0; 2], [100.0; 2]);
        let one = PathBuf::from("/tmp/one.txt");
        let two = PathBuf::from("/tmp/two.txt");
        world.resource_mut::<DesktopInputEvents>().0 = [
            WindowEvent::HoveredFile(one.clone()),
            WindowEvent::DroppedFile(one.clone()),
            WindowEvent::DroppedFile(two.clone()),
        ]
        .into_iter()
        .map(|event| bed_desktop::DesktopInputEvent {
            event: bevy::winit::RawWinitWindowEvent {
                window_id: bed_desktop::winit::window::WindowId::dummy(),
                event,
            },
            consumed: false,
            gameplay_consumed: false,
        })
        .collect();
        input(&mut world);
        let captured = requests(&world);
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[1].source(), Some(panel));
        assert_eq!(
            captured[1].session_id(),
            world
                .non_send::<bed_documents::DocumentSession>()
                .session_id()
        );
        let HostAction::ExternalFiles {
            panel: destination,
            event,
        } = captured[1].action()
        else {
            panic!("Drop request")
        };
        assert_eq!(*destination, Some(panel));
        assert_eq!(event.paths, [one, two]);
        assert_eq!(event.phase, api::ExternalFileDragPhase::Drop);
        assert!(
            world
                .resource::<DesktopInputEvents>()
                .0
                .iter()
                .all(|event| event.consumed)
        );
        assert!(world.resource::<FileDrag>().paths.is_empty());
        assert!(world.resource::<FileDrag>().panel.is_none());
    }

    #[test]
    fn drag_leaving_a_clipped_panel_cancels_that_panels_hover() {
        let _lock = IMGUI_LOCK.lock().unwrap();
        let mut world = world();
        let first = panel(&mut world, 1, [0.0; 2], [100.0; 2]);
        world.get_mut::<api::PanelGeometry>(first).unwrap().clip_max = [50.0; 2];
        file_events(
            &mut world,
            vec![WindowEvent::HoveredFile("/tmp/hover".into())],
        );
        world
            .non_send_mut::<DesktopContext>()
            .context
            .io_mut()
            .set_mouse_pos([80.0; 2]);
        file_events(
            &mut world,
            vec![WindowEvent::CursorMoved {
                device_id: bed_desktop::winit::event::DeviceId::dummy(),
                position: bed_desktop::winit::dpi::PhysicalPosition::new(80.0, 80.0),
            }],
        );
        let captured = requests(&world);
        assert_eq!(captured.len(), 3);
        let HostAction::ExternalFiles { panel, event } = captured[1].action() else {
            panic!("Cancel request")
        };
        assert_eq!(*panel, Some(first));
        assert_eq!(event.phase, api::ExternalFileDragPhase::Cancel);
        let HostAction::ExternalFiles { panel, event } = captured[2].action() else {
            panic!("Hover request")
        };
        assert!(panel.is_none());
        assert_eq!(event.phase, api::ExternalFileDragPhase::Hover);
    }

    #[test]
    fn native_pinches_reach_shared_frame_data_and_geometry_projects_after_custom_ui() {
        let _lock = IMGUI_LOCK.lock().unwrap();
        let mut world = world();
        let panel = panel(&mut world, 1, [1.0; 2], [99.0; 2]);
        world
            .resource_mut::<DesktopInputEvents>()
            .0
            .push(bed_desktop::DesktopInputEvent {
                event: bevy::winit::RawWinitWindowEvent {
                    window_id: bed_desktop::winit::window::WindowId::dummy(),
                    event: WindowEvent::PinchGesture {
                        device_id: bed_desktop::winit::event::DeviceId::dummy(),
                        delta: 0.25,
                        phase: bed_desktop::winit::event::TouchPhase::Started,
                    },
                },
                consumed: false,
                gameplay_consumed: false,
            });
        input(&mut world);
        let event = world.resource::<api::PinchEvents>().0[0];
        assert_eq!(event.phase, api::PinchPhase::Started);
        assert_eq!(event.delta, 0.25);
        assert_eq!(event.position, [20.0; 2]);
        publish_geometry(&mut world);
        let geometry = &world.resource::<DesktopFrame>().panels[0];
        assert_eq!(geometry.panel, panel);
        assert_eq!(geometry.content.min, [1.0; 2]);
        assert_eq!(geometry.clip.max, [99.0; 2]);
        let current = &world.resource::<DesktopFrameInfo>().panels[0];
        assert_eq!(current.panel, panel);
        assert_eq!(current.content, geometry.content);
        assert_eq!(current.clip, geometry.clip);
        assert_eq!(current.visible, geometry.visible);
    }

    #[test]
    fn geometry_bridge_preserves_desktop_targets_and_refreshes_ecs_targets_each_frame() {
        let mut world = World::new();
        world.init_resource::<DesktopFrame>();
        world.init_resource::<DesktopFrameInfo>();
        let desktop_panel = world.spawn_empty().id();
        let published = bed_desktop::DesktopPanelGeometry {
            panel: desktop_panel,
            content: bed_desktop::DesktopRect {
                min: [0.0; 2],
                max: [320.0, 180.0],
            },
            clip: bed_desktop::DesktopRect {
                min: [2.0; 2],
                max: [8.0; 2],
            },
            visible: true,
        };
        world
            .resource_mut::<DesktopFrame>()
            .publish_panel(published.clone());
        publish_geometry(&mut world);
        assert_eq!(
            world.resource::<DesktopFrameInfo>().panels,
            std::slice::from_ref(&published)
        );

        let workbench_panel = panel(&mut world, 1, [1.0; 2], [99.0; 2]);
        world
            .resource_mut::<DesktopFrame>()
            .publish_panel(bed_desktop::DesktopPanelGeometry {
                panel: workbench_panel,
                visible: false,
                ..published.clone()
            });
        publish_geometry(&mut world);
        let current = &world.resource::<DesktopFrameInfo>().panels;
        assert_eq!(current.len(), 2);
        assert_eq!(current[0], published);
        assert_eq!(current[1].panel, workbench_panel);
        assert_eq!(current[1].content.min, [1.0; 2]);
        assert_eq!(current[1].clip.max, [99.0; 2]);
        assert!(current[1].visible);
        assert_eq!(world.resource::<DesktopFrame>().panels, *current);

        // Desktop draw_frontend clears geometry before each new UI transaction.
        // An explicitly published target must disappear if it is not redrawn.
        world.resource_mut::<DesktopFrame>().panels.clear();
        world
            .get_mut::<api::PanelGeometry>(workbench_panel)
            .unwrap()
            .visible = false;
        publish_geometry(&mut world);
        let current = &world.resource::<DesktopFrameInfo>().panels;
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].panel, workbench_panel);
        assert!(!current[0].visible);
        assert_eq!(world.resource::<DesktopFrame>().panels, *current);
    }

    #[test]
    fn optional_input_interceptors_run_before_native_workbench_translation() {
        let _lock = IMGUI_LOCK.lock().unwrap();
        let mut world = world();
        world
            .resource_mut::<DesktopInputEvents>()
            .0
            .push(bed_desktop::DesktopInputEvent {
                event: bevy::winit::RawWinitWindowEvent {
                    window_id: bed_desktop::winit::window::WindowId::dummy(),
                    event: WindowEvent::DroppedFile("/tmp/intercepted".into()),
                },
                consumed: false,
                gameplay_consumed: false,
            });
        let mut app = App::new();
        *app.world_mut() = world;
        app.add_plugins(DesktopWorkbenchPlugin);
        app.add_systems(
            PreUpdate,
            (|mut events: ResMut<DesktopInputEvents>| {
                for event in &mut events.0 {
                    event.consumed = true;
                }
            })
            .in_set(DesktopSystems::InterceptInput),
        );
        app.update();
        assert!(requests(app.world()).is_empty());
        assert!(app.world().resource::<DesktopInputEvents>().0[0].consumed);
    }

    #[test]
    fn native_panel_edit_requests_retain_the_original_focus() {
        let _lock = IMGUI_LOCK.lock().unwrap();
        let mut world = world();
        let action = world.register_system(
            |_: In<api::PanelActionInvocation>| -> api::PanelActionResult { Ok(true) },
        );
        let original = panel(&mut world, 1, [0.0; 2], [100.0; 2]);
        world.entity_mut(original).insert(api::PanelActions(action));
        world
            .resource_mut::<bed_workbench::WorkbenchLayout>()
            .focused = Some(original);
        route_native_edit(&mut world, NativeEditAction::Undo);
        let later = panel(&mut world, 2, [100.0; 2], [200.0; 2]);
        world
            .resource_mut::<bed_workbench::WorkbenchLayout>()
            .focused = Some(later);
        let captured = requests(&world);
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].source(), Some(original));
        let HostAction::PanelAction { panel, action } = captured[0].action() else {
            panic!("Panel action")
        };
        assert_eq!(*panel, original);
        assert!(matches!(action, api::PanelAction::Undo));
    }

    #[test]
    fn native_edits_leave_imgui_text_inputs_in_control_of_editing() {
        let _lock = IMGUI_LOCK.lock().unwrap();
        let mut world = world();
        let action = world.register_system(
            |_: In<api::PanelActionInvocation>| -> api::PanelActionResult { Ok(true) },
        );
        let panel = panel(&mut world, 1, [0.0; 2], [100.0; 2]);
        world.entity_mut(panel).insert(api::PanelActions(action));
        world
            .resource_mut::<bed_workbench::WorkbenchLayout>()
            .focused = Some(panel);
        {
            let mut desktop = world.non_send_mut::<DesktopContext>();
            desktop
                .context
                .font_atlas()
                .try_claim_legacy_renderer()
                .unwrap()
                .build();
            desktop
                .context
                .prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                    [640.0, 480.0],
                    1.0 / 60.0,
                ));
            let ui = desktop.context.frame();
            ui.with_bound_context(|| unsafe {
                // Exercise the native ingress against Dear ImGui's published
                // text-input ownership, independently of panel focus.
                (*dear_imgui_rs::sys::igGetCurrentContext())
                    .IO
                    .WantTextInput = true;
            });
            drop(desktop.context.render_legacy());
        }
        route_native_edit(&mut world, NativeEditAction::Undo);
        assert!(
            requests(&world).is_empty(),
            "The focused panel must not also receive Undo"
        );
    }

    #[test]
    fn clipboard_only_pastes_have_a_native_origin_even_without_text() {
        let _lock = IMGUI_LOCK.lock().unwrap();
        let mut world = world();
        let clipboard =
            world.register_system(|_: In<api::ClipboardInvocation>| -> api::ClipboardResult {
                Ok(None)
            });
        let panel = panel(&mut world, 1, [0.0; 2], [100.0; 2]);
        world
            .entity_mut(panel)
            .insert(api::PanelClipboard(clipboard));
        world
            .resource_mut::<bed_workbench::WorkbenchLayout>()
            .focused = Some(panel);
        route_native_edit(&mut world, NativeEditAction::Paste);
        let captured = requests(&world);
        let HostAction::Clipboard {
            panel: destination,
            action,
        } = captured[0].action()
        else {
            panic!("Clipboard request")
        };
        assert_eq!(*destination, panel);
        assert!(matches!(
            action,
            api::ClipboardAction::PasteFromClipboard(None)
        ));
    }

    #[test]
    fn failed_clipboard_handlers_discard_their_effects_and_keep_the_previous_outbox() {
        let _lock = IMGUI_LOCK.lock().unwrap();
        let mut world = world();
        let handler = world.register_system(
            |In(invocation): In<api::ClipboardInvocation>,
             mut effects: ResMut<HostEffects>|
             -> api::ClipboardResult {
                effects.push(
                    Some(invocation.entity),
                    HostAction::Notify {
                        message: "failed child".into(),
                    },
                );
                Err("Clipboard failure".into())
            },
        );
        let panel = panel(&mut world, 1, [0.0; 2], [100.0; 2]);
        world.entity_mut(panel).insert(api::PanelClipboard(handler));
        world.resource_mut::<HostEffects>().push(
            None,
            HostAction::Notify {
                message: "previous".into(),
            },
        );
        assert_eq!(
            dispatch_panel_clipboard(&mut world, panel, api::ClipboardAction::Copy).unwrap_err(),
            "Clipboard failure"
        );
        let effects = world.resource_mut::<HostEffects>().take();
        assert_eq!(effects.len(), 1);
        assert!(
            matches!(&effects[0].action, HostAction::Notify { message } if message == "previous")
        );
    }
}
