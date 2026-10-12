//! Application composition. Native adapters produce captured editor requests.
use bed_config::Settings;
use bed_desktop::{
    DesktopContext, DesktopFrame, DesktopInputEvents, DesktopPlugins, DesktopRenderStatus,
    DesktopSystems, DesktopUi,
};
use bed_editor_api::PanelTarget;
use bed_editor_workbench::{
    EditorAction, EditorWorkbenchOptions, EditorWorkbenchPlugin, EditorWorkspace, WindowCommand,
    capture_editor_request,
};
use bevy::prelude::*;
use dear_imgui_rs::{Context, Ui};
use std::{
    error::Error,
    io,
    path::{Path, PathBuf},
};
use winit::event::WindowEvent;

type HostResult<T> = Result<T, Box<dyn Error>>;

struct NativeFileClipboard(bed_desktop::DesktopFileClipboard);
impl bed_editor_api::FileClipboardService for NativeFileClipboard {
    fn read_files(&mut self) -> io::Result<Vec<PathBuf>> {
        self.0.read_files()
    }
    fn write_files(&mut self, paths: &[PathBuf]) -> io::Result<()> {
        self.0.write_files(paths)
    }
}

#[derive(Resource, Default)]
struct NativeFileDrag {
    paths: Vec<PathBuf>,
    panel: Option<Entity>,
}
#[derive(Resource, Default)]
struct NativeFailure(std::sync::Arc<std::sync::Mutex<Option<String>>>);

/// Platform code observes native events; the editor reducer owns transitions.
#[cfg(target_os = "macos")]
#[derive(Resource, Default)]
struct ToolbarDropCursor(bevy::ecs::message::MessageCursor<bed_workbench::api::PanelDrop>);

pub struct BedAppPlugin;
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BedPlatformSystems {
    Prepare,
    Input,
}
impl Plugin for BedAppPlugin {
    fn build(&self, app: &mut App) {
        bed_workbench::register(app.world_mut(), "bed.desktop", |registrar| {
            registrar.command("bed.desktop.new_window", "New Window", new_window_command);
            registrar.global_shortcut("bed.desktop.new_window", &["Primary+Shift+N"]);
        })
        .expect("desktop shortcuts are registered once");
        app.init_resource::<NativeFailure>()
            .init_resource::<NativeFileDrag>()
            .insert_resource(DesktopUi(draw))
            .add_systems(PreUpdate, configure.in_set(DesktopSystems::Configure))
            .add_systems(
                PreUpdate,
                prepare
                    .after(bed_desktop::DesktopNativeSystems::Prepare)
                    .in_set(DesktopSystems::Prepare)
                    .in_set(BedPlatformSystems::Prepare),
            )
            .add_systems(PreUpdate, after_ui.in_set(DesktopSystems::AfterUi))
            .add_systems(PreUpdate, begin_previews.before(DesktopSystems::Ui))
            .add_systems(
                PreUpdate,
                retire_previews
                    .after(bed_desktop::DesktopCanvasSystems::Render)
                    .before(DesktopSystems::RouteInput),
            );
        #[cfg(target_os = "macos")]
        app.init_resource::<ToolbarDropCursor>();
        #[cfg(not(feature = "terminal"))]
        app.add_systems(
            PreUpdate,
            native_input
                .in_set(DesktopSystems::InterceptInput)
                .in_set(BedPlatformSystems::Input),
        );
        #[cfg(feature = "terminal")]
        app.add_systems(
            PreUpdate,
            native_input
                .after(bed_terminal::TerminalInputSystems::Intercept)
                .in_set(BedPlatformSystems::Input)
                .in_set(DesktopSystems::InterceptInput),
        );
        #[cfg(feature = "idle")]
        app.configure_sets(
            PreUpdate,
            bed_idle::IdleSystems::Intercept.before(BedPlatformSystems::Input),
        );
        #[cfg(all(feature = "idle", feature = "terminal"))]
        app.configure_sets(
            PreUpdate,
            bed_idle::IdleSystems::Intercept.before(bed_terminal::TerminalInputSystems::Intercept),
        );
        #[cfg(feature = "automation")]
        app.init_resource::<NativeSmokeMode>()
            .init_resource::<NativeSpawnedInstances>();
        #[cfg(all(feature = "automation", target_os = "macos"))]
        app.init_resource::<NativeMenuObservations>();
        app.configure_sets(
            PreUpdate,
            BedPlatformSystems::Prepare.before(bed_editor_api::EditorFeatureSystems::Prepare),
        );
        app.configure_sets(
            PreUpdate,
            bed_editor_workbench::EditorSystemSet::Services
                .after(DesktopSystems::Configure)
                .before(bed_editor_api::EditorFeatureSystems::Prepare),
        );
        app.configure_sets(
            PreUpdate,
            bed_desktop::DesktopCanvasSystems::Prepare
                .after(bed_editor_api::EditorFeatureSystems::Prepare),
        );
        app.configure_sets(
            PreUpdate,
            bed_editor_api::EditorFeatureSystems::Prepare
                .after(DesktopSystems::Input)
                .before(DesktopSystems::Ui),
        );
    }
}

fn begin_previews(world: &mut World) {
    bed_runtime::render::begin_preview_frame(world);
}

fn retire_previews(world: &mut World) {
    let live = world
        .query::<&bed_ui::gpu::CanvasOutput>()
        .iter(world)
        .map(|output| output.0.handle.0)
        .collect();
    bed_runtime::render::retire_previews(world, &live);
}

fn fail(world: &mut World, error: impl std::fmt::Display) {
    world
        .resource::<NativeFailure>()
        .0
        .lock()
        .expect("native failure lock")
        .get_or_insert_with(|| error.to_string());
    world.write_message(AppExit::error());
}
fn configure(world: &mut World) {
    {
        let mut settings = world.non_send_mut::<Settings>();
        settings.refresh();
    }
    let settings = world.non_send::<Settings>();
    let appearance = settings.appearance();
    let background = settings.window_background_color();
    let mut hidden_picker = world
        .resource::<bed_editor_api::Registry>()
        .panels()
        .iter()
        .filter(|panel| !panel.empty_input)
        .map(|panel| panel.id.to_owned())
        .collect::<std::collections::HashSet<_>>();
    hidden_picker.insert("bed.duck.panel".into());
    let workbench_appearance = bed_workbench::WorkbenchAppearance {
        animations: settings.bool("ui_animations", true),
        add_button: settings.bool("tab_bar_add_button", false),
        background: [
            background[0],
            background[1],
            background[2],
            settings.background_opacity(),
        ],
        hidden_picker,
    };

    #[cfg(feature = "terminal")]
    let terminal_appearance = bed_terminal::TerminalAppearance {
        background: settings.background_color(),
        foreground: settings.text_color(),
        ansi: settings.theme.terminal,
    };
    #[cfg(feature = "terminal")]
    let terminal_policy = {
        let bindings = bed_keybindings::Bindings::new(
            world
                .resource::<bed_workbench::Registry>()
                .shortcuts()
                .to_vec(),
            &settings.keybinds.overrides,
            bed_keybindings::Platform::current(),
        );
        let strokes = |id| {
            bed_terminal::TerminalInputPolicy::clipboard_shortcuts(
                &bindings,
                id,
                settings.keybinds.overrides.actions.contains_key(id),
            )
        };
        bed_terminal::TerminalInputPolicy {
            application_shortcuts: bindings
                .entries
                .iter()
                .filter(|b| {
                    (b.scope == bed_keybindings::Scope::Global
                        || b.scope == bed_keybindings::Scope::panel(bed_terminal::PANEL_ID))
                        && bindings
                            .winner(&b.scope, b.shortcut)
                            .is_some_and(|winner| winner.action == b.action)
                })
                .map(|b| b.shortcut)
                .collect(),
            copy: strokes("bed.edit.copy"),
            paste: strokes("bed.edit.paste"),
            select_all: strokes("bed.edit.select_all"),
            recording: world
                .get_non_send::<bed_workbench::WorkbenchShortcuts>()
                .is_some_and(|runtime| runtime.recording.is_some()),
        }
    };
    #[cfg(feature = "explorer")]
    let explorer = bed_explorer::ExplorerPresentation {
        tree: bed_explorer::file_tree::FileTreeStyle {
            text_color: settings.text_color(),
            rainbow: settings.rainbow(),
            rainbow_time: 0.0,
            animations: settings.bool("ui_animations", true),
        },
        finder: bed_explorer::file_finder::FileFinderStyle {
            background_color: settings.background_color(),
            embedded_pane: None,
        },
    };
    #[cfg(feature = "effects")]
    let effects = settings.shader_settings();
    #[cfg(feature = "idle")]
    let idle = settings.bool("bedtime_enabled", true);
    #[cfg(feature = "editor")]
    let navigation_animations = settings.bool("ui_animations", true);
    #[cfg(feature = "editor")]
    let cursor_smear = settings.bool("cursor_smear", true);
    world.resource_mut::<bed_desktop::DesktopAppearance>().0 = appearance;
    *world.resource_mut::<bed_workbench::WorkbenchAppearance>() = workbench_appearance;
    #[cfg(feature = "editor")]
    {
        let mut config = world.non_send_mut::<bed_editor::EditorConfig>();
        config.navigation_animations = navigation_animations;
        config.options.cursor_smear = cursor_smear;
    }
    #[cfg(feature = "terminal")]
    {
        *world.resource_mut::<bed_terminal::TerminalAppearance>() = terminal_appearance;
        *world.resource_mut::<bed_terminal::TerminalInputPolicy>() = terminal_policy;
    }
    #[cfg(feature = "explorer")]
    {
        *world.resource_mut::<bed_explorer::ExplorerPresentation>() = explorer;
    }
    #[cfg(feature = "effects")]
    {
        world
            .resource_mut::<bed_effects::desktop::EffectsState>()
            .settings = effects;
    }
    #[cfg(feature = "idle")]
    {
        world.resource_mut::<bed_idle::IdleSettings>().enabled = idle;
    }
    #[cfg(target_os = "macos")]
    update_native_presentation(world);
}
fn prepare(world: &mut World) {
    if !world.contains_non_send::<DesktopContext>() {
        return;
    }
    if !world.contains_non_send::<Box<dyn bed_editor_api::FileClipboardService>>()
        && let Some(clipboard) = world
            .get_non_send::<bed_desktop::DesktopFileClipboard>()
            .cloned()
    {
        world.insert_non_send(Box::new(NativeFileClipboard(clipboard))
            as Box<dyn bed_editor_api::FileClipboardService>);
    }
    world
        .non_send_mut::<DesktopContext>()
        .platform
        .set_ime_auto_management(false);
    #[cfg(target_os = "macos")]
    if bed_desktop::native::macos_window::toolbar_drag().is_some() {
        let (position, _) = native_drag_observation(world);
        let pressed = bed_desktop::native::macos_window::toolbar_drag_pressed();
        let mut desktop = world.non_send_mut::<DesktopContext>();
        desktop.context.io_mut().add_mouse_pos_event(position);
        desktop
            .context
            .io_mut()
            .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, pressed);
    }
    let top = world.resource::<bed_desktop::DesktopContentInsets>().top;
    bed_editor_workbench::set_root_top_inset(world, top);
}
fn draw(world: &mut World, ui: &Ui) {
    #[cfg(target_os = "macos")]
    if let Some((command, label)) = bed_desktop::native::macos_window::toolbar_drag() {
        ui.with_bound_context(|| unsafe {
            if dear_imgui_sys::igBeginDragDropSource(
                dear_imgui_sys::ImGuiDragDropFlags_SourceExtern
                    | dear_imgui_sys::ImGuiDragDropFlags_SourceNoPreviewTooltip,
            ) {
                dear_imgui_sys::igSetDragDropPayload(
                    c"BED_TOOLBAR_COMMAND".as_ptr(),
                    command.as_ptr().cast(),
                    command.len(),
                    dear_imgui_sys::ImGuiCond_Always,
                );
                bed_workbench::api::draw_panel_drag_label(ui, &label);
                dear_imgui_sys::igEndDragDropSource();
            }
        });
    }
    if let Err(error) = bed_editor_workbench::draw_ui(world, ui) {
        fail(world, error);
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let drops = world.resource_scope(|world, mut cursor: Mut<ToolbarDropCursor>| {
            cursor
                .0
                .read(world.resource::<Messages<bed_workbench::api::PanelDrop>>())
                .filter(|drop| drop.payload_type == "BED_TOOLBAR_COMMAND")
                .cloned()
                .collect::<Vec<_>>()
        });
        for drop in drops {
            let Ok(command) = String::from_utf8(drop.data) else {
                continue;
            };
            let mut request = bed_editor_workbench::capture_editor_request_from(
                world,
                EditorAction::PluginCommand(command),
                PanelTarget::LargestArea,
                drop.panel,
            );
            request.area = drop.area;
            request.tab_index = drop.tab_index;
            world
                .resource_mut::<bed_editor_workbench::EditorRequests>()
                .enqueue(request);
        }
        if !ui.is_mouse_down(dear_imgui_rs::MouseButton::Left) {
            bed_desktop::native::macos_window::finish_toolbar_drag();
        }
    }
    let geometries = world
        .query::<(Entity, &bed_editor_workbench::PanelGeometry)>()
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
        .collect::<Vec<_>>();
    world.resource_mut::<DesktopFrame>().panels = geometries;
    #[cfg(feature = "idle")]
    {
        let focused = world
            .query::<&bevy::window::Window>()
            .iter(world)
            .any(|window| window.focused);
        world.resource_mut::<bed_idle::IdleSettings>().eligible =
            focused.then_some(ui.main_viewport().id().raw());
        bed_idle::draw_ui(world, ui);
    }
}
fn native_input(world: &mut World) {
    let mut actions = Vec::new();
    let mut file_events = Vec::new();
    let mut pinches = Vec::new();
    #[cfg(target_os = "macos")]
    let mut edits = Vec::new();
    #[cfg(target_os = "macos")]
    let mut new_windows = 0;
    #[cfg(all(feature = "automation", target_os = "macos"))]
    let mut observations = Vec::new();
    {
        let mut events = world.resource_mut::<DesktopInputEvents>();
        for event in &mut events.0 {
            if event.consumed {
                continue;
            }
            match &event.event.event {
                WindowEvent::CloseRequested => {
                    event.consumed = true;
                    actions.push(EditorAction::Command(WindowCommand::Quit));
                }
                WindowEvent::DroppedFile(_)
                | WindowEvent::HoveredFile(_)
                | WindowEvent::HoveredFileCancelled => {
                    event.consumed = true;
                    file_events.push(event.event.event.clone());
                }
                WindowEvent::CursorMoved { .. } => file_events.push(event.event.event.clone()),
                WindowEvent::PinchGesture { delta, phase, .. } => pinches.push((*delta, *phase)),
                _ => {}
            }
        }
    }

    let x = String::
    for (delta, phase) in pinches {
        let (position, _) = native_drag_observation(world);
        let phase = match phase {
            winit::event::TouchPhase::Started => bed_workbench::api::PinchPhase::Started,
            winit::event::TouchPhase::Moved => bed_workbench::api::PinchPhase::Moved,
            winit::event::TouchPhase::Ended => bed_workbench::api::PinchPhase::Ended,
            winit::event::TouchPhase::Cancelled => bed_workbench::api::PinchPhase::Cancelled,
        };
        if let Some(mut events) = world.get_resource_mut::<bed_workbench::api::PinchEvents>() {
            events.0.push(bed_workbench::api::PinchEvent {
                position,
                delta,
                phase,
            });
        }
    }
    #[cfg(target_os = "macos")]
    {
        let dispatches =
            std::mem::take(&mut world.resource_mut::<bed_desktop::DesktopNativeActions>().0);
        for dispatch in dispatches {
            use crate::native_commands::MenuAction;
            if dispatch.keyboard {
                let bindings = bed_keybindings::Bindings::new(
                    world
                        .resource::<bed_workbench::Registry>()
                        .shortcuts()
                        .to_vec(),
                    &world.non_send::<Settings>().keybinds.overrides,
                    bed_keybindings::Platform::current(),
                );
                let stroke = if let Some(action) = MenuAction::from_command(&dispatch.command) {
                    crate::native_commands::menu_shortcut(
                        action,
                        &bindings,
                        editor_shortcuts_focused(world),
                        world
                            .non_send::<DesktopContext>()
                            .context
                            .io()
                            .want_text_input(),
                    )
                } else {
                    bindings
                        .for_action(&dispatch.command)
                        .into_iter()
                        .find(|binding| {
                            (binding.scope == bed_keybindings::Scope::Global
                                || (editor_shortcuts_focused(world)
                                    && binding.scope
                                        == bed_keybindings::Scope::panel(
                                            bed_keybindings::EDITOR_SCOPE,
                                        )))
                                && bindings
                                    .winner(&binding.scope, binding.shortcut)
                                    .is_some_and(|winner| winner.action == binding.action)
                        })
                        .map(|binding| binding.shortcut)
                };
                if let Some(stroke) = stroke {
                    #[cfg(feature = "automation")]
                    if let Some(action) = MenuAction::from_command(&dispatch.command) {
                        observations.push(crate::native_commands::MenuDispatch {
                            action,
                            keyboard: true,
                            shift: dispatch.modifiers.shift,
                        });
                    }
                    queue_native_shortcut(
                        &mut world.non_send_mut::<DesktopContext>().context,
                        stroke,
                    );
                    continue;
                }
            }
            let Some(action) = MenuAction::from_command(&dispatch.command) else {
                actions.push(EditorAction::PluginCommand(dispatch.command));
                continue;
            };
            #[cfg(feature = "automation")]
            observations.push(crate::native_commands::MenuDispatch {
                action,
                keyboard: dispatch.keyboard,
                shift: dispatch.modifiers.shift,
            });
            if let Some(command) = native_menu_command(action, dispatch.keyboard) {
                actions.push(EditorAction::Command(command));
            } else if action == MenuAction::NewWindow {
                new_windows += 1;
            } else if action == MenuAction::Quit {
                actions.push(EditorAction::Command(WindowCommand::Quit));
            } else if let NativeEditRoute::Shortcut(key, shift) = native_edit_route(
                action,
                world
                    .non_send::<DesktopContext>()
                    .context
                    .io()
                    .want_text_input(),
            ) {
                edits.push((action, key, shift));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        #[cfg(feature = "automation")]
        world
            .resource_mut::<NativeMenuObservations>()
            .0
            .extend(observations);
        for (action, key, shift) in edits {
            route_native_edit(world, action, key, shift);
        }
        for _ in 0..new_windows {
            if let Err(error) = spawn_instance(world, None) {
                world.resource_mut::<EditorWorkspace>().error = Some(error.to_string());
            }
        }
    }
    for action in actions {
        let request = capture_editor_request(world, action, PanelTarget::LargestArea);
        world
            .resource_mut::<bed_editor_workbench::EditorRequests>()
            .enqueue(request);
    }
}

fn native_file_events(world: &mut World, events: Vec<WindowEvent>) {
    use bed_editor_api::{ExternalFileDrag, ExternalFileDragPhase};
    let mut dropped = Vec::new();
    for event in events {
        let phase = match event {
            WindowEvent::DroppedFile(path) => {
                dropped.push(path);
                continue;
            }
            WindowEvent::HoveredFile(path) => {
                let mut drag = world.resource_mut::<NativeFileDrag>();
                if !drag.paths.contains(&path) {
                    drag.paths.push(path);
                }
                ExternalFileDragPhase::Hover
            }
            WindowEvent::HoveredFileCancelled => ExternalFileDragPhase::Cancel,
            WindowEvent::CursorMoved { .. }
                if !world.resource::<NativeFileDrag>().paths.is_empty() =>
            {
                ExternalFileDragPhase::Hover
            }
            _ => continue,
        };
        let (position, modifiers) = native_drag_observation(world);
        let previous = world.resource::<NativeFileDrag>().panel;
        let panel = if phase == ExternalFileDragPhase::Cancel {
            previous
        } else {
            native_panel_at(world, position)
        };
        let paths = world.resource::<NativeFileDrag>().paths.clone();
        if previous != panel
            && let Some(previous) = previous
        {
            submit_native_drag(
                world,
                Some(previous),
                ExternalFileDrag {
                    paths: Vec::new(),
                    position,
                    modifiers,
                    viewport: world
                        .get::<bed_editor_workbench::EditorPanel>(previous)
                        .map_or(0, |panel| panel.viewport),
                    phase: ExternalFileDragPhase::Cancel,
                },
            );
        }
        let viewport = panel
            .and_then(|panel| world.get::<bed_editor_workbench::EditorPanel>(panel))
            .map_or(0, |panel| panel.viewport);
        submit_native_drag(
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
        let mut drag = world.resource_mut::<NativeFileDrag>();
        drag.panel = panel;
        if phase == ExternalFileDragPhase::Cancel {
            drag.paths.clear();
            drag.panel = None;
        }
    }
    if !dropped.is_empty() {
        let (position, modifiers) = native_drag_observation(world);
        let panel = native_panel_at(world, position);
        let viewport = panel
            .and_then(|panel| world.get::<bed_editor_workbench::EditorPanel>(panel))
            .map_or(0, |panel| panel.viewport);
        submit_native_drag(
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
        let mut drag = world.resource_mut::<NativeFileDrag>();
        drag.paths.clear();
        drag.panel = None;
    }
}
fn native_panel_at(world: &mut World, position: [f32; 2]) -> Option<Entity> {
    world
        .query::<(Entity, &bed_editor_workbench::PanelGeometry)>()
        .iter(world)
        .find(|(_, rect)| {
            rect.visible
                && (0..2).all(|axis| {
                    position[axis] >= rect.content_min[axis].max(rect.clip_min[axis])
                        && position[axis] < rect.content_max[axis].min(rect.clip_max[axis])
                })
        })
        .map(|(entity, _)| entity)
}
fn submit_native_drag(
    world: &mut World,
    panel: Option<Entity>,
    event: bed_editor_api::ExternalFileDrag,
) {
    let source = panel.and_then(|panel| bed_editor_workbench::panel_presentation(world, panel));
    let request = bed_editor_workbench::capture_editor_request_from(
        world,
        EditorAction::ExternalFiles { panel, event },
        PanelTarget::LargestArea,
        source,
    );
    world
        .resource_mut::<bed_editor_workbench::EditorRequests>()
        .enqueue(request);
}
fn native_drag_observation(world: &mut World) -> ([f32; 2], bed_editor_api::ExternalFileModifiers) {
    let io = world.non_send::<DesktopContext>().context.io();
    let mut position = io.mouse_pos();
    let mut modifiers = bed_editor_api::ExternalFileModifiers {
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
                    modifiers = bed_editor_api::ExternalFileModifiers {
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
                    modifiers = bed_editor_api::ExternalFileModifiers {
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
fn editor_shortcuts_focused(world: &World) -> bool {
    world
        .resource::<bed_workbench::WorkbenchLayout>()
        .focused
        .and_then(|panel| bed_editor_workbench::panel_presentation(world, panel))
        .and_then(|panel| world.get::<bed_workbench::PanelRecord>(panel))
        .is_some_and(|record| {
            record.panel_type == bed_editor::TEXT_PANEL_TYPE
                || record.panel_type == bed_editor::HEX_PANEL_TYPE
        })
}

fn after_ui(world: &mut World) {
    let capture = *world.resource::<bed_desktop::DesktopCapture>();
    *world.resource_mut::<bed_runtime::InputCapture>() = bed_runtime::InputCapture {
        keyboard: capture.keyboard,
        pointer: capture.pointer,
    };
    if let Err(error) = bed_editor_workbench::process_editor_requests(world) {
        world.resource_mut::<EditorWorkspace>().error = Some(error.to_string());
    }
    let errors = std::mem::take(&mut world.resource_mut::<bed_desktop::DesktopCanvasErrors>().0);
    for error in errors {
        world.resource_mut::<EditorWorkspace>().error = Some(error.message);
    }
    if let Some(error) = world.resource::<DesktopRenderStatus>().take_error() {
        fail(world, error);
    }
    if world.resource::<EditorWorkspace>().close_requested {
        world.write_message(AppExit::Success);
    }
    let appearance_errors = std::mem::take(
        &mut world
            .resource_mut::<bed_desktop::DesktopAppearanceErrors>()
            .0,
    );
    for error in appearance_errors {
        fail(world, error);
    }
    let native_errors =
        std::mem::take(&mut world.resource_mut::<bed_desktop::DesktopNativeErrors>().0);
    for error in native_errors {
        fail(world, error);
    }
    let windows = std::mem::take(&mut world.resource_mut::<EditorWorkspace>().windows);
    for workspace in windows {
        if let Err(error) = spawn_instance(world, Some(&workspace)) {
            world.resource_mut::<EditorWorkspace>().error = Some(error.to_string());
        }
    }
}
#[cfg(target_os = "macos")]
fn update_native_presentation(world: &mut World) {
    let toolbar = bed_editor_workbench::toolbar_commands(world);
    let application = bed_editor_workbench::application_commands(world);
    let available = world.resource::<EditorWorkspace>().spec.is_some();
    let has_document = world
        .resource::<bed_editor_workbench::EditorNavigation>()
        .active_document
        .is_some();
    let clipboard_focused = world
        .resource::<bed_editor_workbench::WorkbenchLayout>()
        .focused
        .and_then(|panel| bed_editor_workbench::panel_presentation(world, panel))
        .is_some_and(|panel| {
            world
                .get::<bed_editor_api::EditorPanelClipboard>(panel)
                .is_some()
                && world
                    .get::<bed_editor_api::EditorPanelActions>(panel)
                    .is_none()
        });
    let text_input_focused = world
        .non_send::<DesktopContext>()
        .context
        .io()
        .want_text_input();
    let settings = world.non_send::<Settings>();
    let bindings = bed_keybindings::Bindings::new(
        world
            .resource::<bed_workbench::Registry>()
            .shortcuts()
            .to_vec(),
        &settings.keybinds.overrides,
        bed_keybindings::Platform::current(),
    );
    let menus = crate::native_commands::menus(
        &bindings,
        available,
        has_document,
        clipboard_focused,
        text_input_focused,
        editor_shortcuts_focused(world),
        &application,
        world
            .resource::<bed_editor_workbench::LayoutSlots>()
            .active(),
    );
    let presentation = bed_desktop::DesktopNativePresentation {
        opacity: settings.background_opacity(),
        blur: settings.bool("mac_blur_enabled", true),
        foreground: settings.text_color(),
        background: settings.background_color(),
        menus,
        toolbar: toolbar
            .into_iter()
            .map(|item| bed_desktop::DesktopToolbarItem {
                id: item.id,
                label: item.label,
                icon: item.icon,
                enabled: item.enabled,
            })
            .collect(),
        dock: vec![bed_desktop::DesktopMenuCommand {
            id: crate::native_commands::MenuAction::NewWindow.command_id(),
            label: "New Window".into(),
            enabled: true,
            shortcut: None,
        }],
    };
    *world.resource_mut::<bed_desktop::DesktopNativePresentation>() = presentation;
}

/// Build a headful app. Embedders can compose the same editor plugins in their own app.
pub fn build_app(mut options: EditorWorkbenchOptions, settings: Settings) -> io::Result<App> {
    let store = bed_editor_workbench::WorkspaceStore::load(&settings.config_dir)?;
    let icons = bed_ui::icons::Icons::load(&settings.resources_root);
    if options.working_directory.is_none() {
        options.working_directory = Some(std::env::current_dir()?);
    }
    let mut app = App::new();
    app.insert_non_send(settings)
        .insert_non_send(icons)
        .insert_non_send(bed_editor_workbench::WorkspacePersistence::new(store))
        .insert_resource(bed_editor_workbench::RemoteHelperDirectory(
            crate::remote_helpers::helpers_directory(),
        ));
    #[cfg(feature = "models")]
    app.add_plugins(bed_model_viewer::ModelAssetsPlugin);
    app.add_plugins(DesktopPlugins {
        close_when_requested: false,
        window: bevy::window::Window {
            title: "bEd".into(),
            resolution: (1200, 800).into(),
            transparent: true,
            ime_enabled: true,
            present_mode: bevy::window::PresentMode::AutoNoVsync,
            composite_alpha_mode: if cfg!(target_os = "macos") {
                bevy::window::CompositeAlphaMode::PostMultiplied
            } else {
                bevy::window::CompositeAlphaMode::Auto
            },
            titlebar_transparent: true,
            titlebar_show_title: false,
            fullsize_content_view: true,
            ..default()
        },
        ..default()
    })
    .add_plugins(bed_runtime::BedRuntimePlugin)
    .add_plugins(EditorWorkbenchPlugin { options })
    .add_plugins(crate::builtins::BedEditorPlugins)
    .add_plugins(bed_runtime::render::BedPreviewPlugin)
    .add_plugins(BedAppPlugin);
    #[cfg(feature = "effects")]
    app.add_plugins(bed_effects::EffectsPlugin);
    #[cfg(feature = "idle")]
    app.add_plugins(bed_idle::IdlePlugin);
    #[cfg(feature = "automation")]
    app.add_plugins(bed_automation::AutomationPlugin);
    Ok(app)
}

pub fn run() -> HostResult<()> {
    let launch = crate::startup::Launch::parse(std::env::args_os().skip(1))?;
    let Some(mut launch) = launch else {
        return Ok(());
    };
    if launch.smoke.requested() && !cfg!(feature = "automation") {
        return Err(io::Error::other(
            "smoke and frame capture commands require building with --features automation",
        )
        .into());
    }
    if launch.smoke.requested() && launch.config_dir.is_none() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        launch.config_dir = Some(
            std::env::temp_dir().join(format!("bed-smoke-config-{}-{unique}", std::process::id())),
        );
    }
    let settings = match launch.config_dir.clone() {
        Some(path) => Settings::with_paths(path, Settings::get_app_resources_path())?,
        None => Settings::new()?,
    };
    let startup = launch
        .startup
        .resolve(launch.paths, std::env::current_dir().ok().as_deref())?;
    let (options, error) = startup.into_options(&settings)?;
    let mut app = build_app(options, settings)?;
    if let Some(error) = error {
        app.add_systems(
            PostStartup,
            move |mut workspace: ResMut<EditorWorkspace>| {
                workspace.error = Some(error.clone());
            },
        );
    }
    #[cfg(feature = "automation")]
    app.add_plugins(crate::smoke::SmokePlugin(launch.smoke));
    let native_failure = app.world().resource::<NativeFailure>().0.clone();
    #[cfg(feature = "automation")]
    let smoke_failure = app
        .world()
        .get_resource::<crate::smoke::SmokeFailure>()
        .map(|failure| failure.0.clone());
    let exit = app.run();
    if let Some(error) = native_failure.lock().expect("native failure lock").clone() {
        return Err(io::Error::other(error).into());
    }
    #[cfg(feature = "automation")]
    if let Some(error) =
        smoke_failure.and_then(|failure| failure.lock().expect("smoke failure lock").clone())
    {
        return Err(io::Error::other(error).into());
    }
    if exit.is_error() {
        return Err(io::Error::other("Bevy desktop exited with an error").into());
    }
    Ok(())
}
fn new_instance_command(
    executable: &Path,
    config_dir: &Path,
    wait: bool,
    workspace: Option<&bed_editor_api::workspace::WorkspaceSpec>,
) -> io::Result<std::process::Command> {
    let config_dir = std::fs::canonicalize(config_dir)?;
    // Launch Services needs -n to start another process for a running bundle.
    // Source builds run the executable directly. Neither path inherits the
    // parent's project arguments or smoke flags.
    #[cfg(target_os = "macos")]
    let bundle = executable
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "MacOS"))
        .and_then(Path::parent)
        .filter(|path| path.file_name().is_some_and(|name| name == "Contents"))
        .and_then(Path::parent)
        .filter(|path| path.extension().is_some_and(|extension| extension == "app"));
    #[cfg(target_os = "macos")]
    let mut command = if let Some(bundle) = bundle {
        let mut command = std::process::Command::new("/usr/bin/open");
        command.arg("-n");
        if wait {
            command.arg("-W");
        }
        command.arg(bundle).arg("--args");
        command
    } else {
        std::process::Command::new(executable)
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = std::process::Command::new(executable);
    command
        .arg("--new-window")
        .arg("--config-dir")
        .arg(config_dir)
        .stdin(std::process::Stdio::null());
    if let Some(workspace) = workspace {
        command
            .arg("--workspace")
            .arg(workspace.to_value().to_string());
    }
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    if !wait {
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }
    Ok(command)
}

#[cfg(target_os = "macos")]
fn native_menu_command(
    action: crate::native_commands::MenuAction,
    keyboard: bool,
) -> Option<WindowCommand> {
    use crate::native_commands::MenuAction;
    Some(match action {
        MenuAction::NewDocument => WindowCommand::NewDocument,
        MenuAction::NewTerminal => WindowCommand::NewTerminal,
        MenuAction::NewExplorer => WindowCommand::NewExplorer,
        MenuAction::NewSettings => WindowCommand::NewSettings,
        MenuAction::NewProjects | MenuAction::Projects => WindowCommand::NewProjects,
        MenuAction::NewDiagnostics | MenuAction::Diagnostics => WindowCommand::NewDiagnostics,
        MenuAction::Debug => WindowCommand::Debug,
        MenuAction::Structure | MenuAction::NewStructure => WindowCommand::NewStructure,
        MenuAction::NewReferences => WindowCommand::NewReferences,
        MenuAction::NewLspDashboard | MenuAction::LspDashboard => WindowCommand::NewLspDashboard,
        MenuAction::NewContentSearch => WindowCommand::NewContentSearch,
        MenuAction::DuplicateView => WindowCommand::DuplicateView,
        MenuAction::SplitRight => WindowCommand::SplitRight,
        MenuAction::SplitDown => WindowCommand::SplitDown,
        MenuAction::UseLayout(slot) => WindowCommand::UseLayout(slot),
        MenuAction::SetLayoutDefault(slot) => WindowCommand::SetLayoutDefault(slot),
        MenuAction::RestoreBuiltinLayout(slot) => WindowCommand::RestoreBuiltinLayout(slot),
        MenuAction::ResetLayout => WindowCommand::ResetLayout,
        MenuAction::SaveDefaultLayout => WindowCommand::SaveDefaultLayout,
        MenuAction::ResetDefaultLayout => WindowCommand::ResetDefaultLayout,
        MenuAction::OpenFolder => WindowCommand::OpenFolder,
        MenuAction::OpenFile => WindowCommand::OpenFile,
        MenuAction::Save => WindowCommand::Save,
        MenuAction::SaveAs => WindowCommand::SaveAs,
        MenuAction::Close => WindowCommand::Close,
        MenuAction::Find => WindowCommand::Find,
        MenuAction::GoToLine => WindowCommand::GoToLine,
        MenuAction::FindFile => WindowCommand::FindFile,
        MenuAction::FindProject if keyboard => WindowCommand::FindProject,
        MenuAction::FindProject => WindowCommand::NewContentSearch,
        MenuAction::Explorer if keyboard => WindowCommand::Explorer,
        MenuAction::Explorer => WindowCommand::NewExplorer,
        MenuAction::Terminal if keyboard => WindowCommand::Terminal,
        MenuAction::Terminal => WindowCommand::NewTerminal,
        MenuAction::Settings if keyboard => WindowCommand::Settings,
        MenuAction::Settings => WindowCommand::NewSettings,
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
#[derive(Debug, PartialEq, Eq)]
enum NativeEditRoute {
    Ignore,
    Shortcut(dear_imgui_rs::Key, bool),
}
#[cfg(target_os = "macos")]
fn native_edit_route(
    action: crate::native_commands::MenuAction,
    text_input_focused: bool,
) -> NativeEditRoute {
    use crate::native_commands::MenuAction;
    use dear_imgui_rs::Key;
    match action {
        MenuAction::Undo => NativeEditRoute::Shortcut(Key::Z, false),
        MenuAction::Redo => NativeEditRoute::Shortcut(Key::Z, true),
        MenuAction::Cut => NativeEditRoute::Shortcut(Key::X, false),
        MenuAction::Copy => NativeEditRoute::Shortcut(Key::C, false),
        MenuAction::Paste => NativeEditRoute::Shortcut(Key::V, false),
        MenuAction::SelectAll if text_input_focused => NativeEditRoute::Shortcut(Key::A, false),
        MenuAction::SelectAll => NativeEditRoute::Shortcut(Key::A, false),
        _ => NativeEditRoute::Ignore,
    }
}

#[cfg(target_os = "macos")]
fn queue_native_edit_shortcut(context: &mut Context, key: dear_imgui_rs::Key, shift: bool) {
    use dear_imgui_rs::Key;
    let io = context.io_mut();
    // Native menus consume the key-equivalent event. Route edit actions through
    // the existing focused ImGui/editor input handler. Dear ImGui's
    // macOS behavior maps physical Command (Super) to logical Ctrl.
    io.add_key_event(Key::ModSuper, true);
    io.add_key_event(Key::LeftSuper, true);
    if shift {
        io.add_key_event(Key::ModShift, true);
        io.add_key_event(Key::LeftShift, true);
    }
    io.add_key_event(key, true);
    io.add_key_event(key, false);
    if shift {
        io.add_key_event(Key::LeftShift, false);
        io.add_key_event(Key::ModShift, false);
    }
    io.add_key_event(Key::LeftSuper, false);
    io.add_key_event(Key::ModSuper, false);
}

#[cfg(target_os = "macos")]
fn route_native_edit(
    world: &mut World,
    action: crate::native_commands::MenuAction,
    key: dear_imgui_rs::Key,
    shift: bool,
) {
    use crate::native_commands::MenuAction;
    use bed_editor_api::{ClipboardAction, ClipboardInvocation, EditorPanelClipboard};
    let text_input = world
        .non_send::<DesktopContext>()
        .context
        .io()
        .want_text_input();
    let source = world
        .resource::<bed_editor_workbench::WorkbenchLayout>()
        .focused
        .and_then(|panel| bed_editor_workbench::panel_presentation(world, panel));
    if !text_input
        && let Some(panel) = source
        && world
            .get::<bed_editor_api::EditorPanelActions>(panel)
            .is_some()
        && let Some(action) = match action {
            MenuAction::Undo => Some(bed_editor_api::PanelAction::Undo),
            MenuAction::Redo => Some(bed_editor_api::PanelAction::Redo),
            MenuAction::SelectAll => Some(bed_editor_api::PanelAction::SelectAll),
            _ => None,
        }
    {
        let request = bed_editor_workbench::capture_editor_request_from(
            world,
            EditorAction::PanelAction { panel, action },
            PanelTarget::LargestArea,
            Some(panel),
        );
        world
            .resource_mut::<bed_editor_workbench::EditorRequests>()
            .enqueue(request);
        return;
    }
    let clipboard = if text_input {
        None
    } else {
        source.and_then(|source| {
            world
                .get::<EditorPanelClipboard>(source)
                .map(|handler| (source, handler.0))
        })
    };
    if let Some((entity, handler)) = clipboard {
        let operation = match action {
            MenuAction::Copy => Some(ClipboardAction::Copy),
            MenuAction::Cut => Some(ClipboardAction::Cut),
            MenuAction::Paste => {
                let files = if world
                    .get::<bed_terminal::TerminalPanelState>(entity)
                    .is_some()
                {
                    let mut clipboard =
                        world.get_non_send_mut::<bed_desktop::DesktopFileClipboard>();
                    match bed_terminal::clipboard_file_paste(clipboard.as_deref_mut()) {
                        Ok(text) => text,
                        Err(error) => {
                            world.resource_mut::<EditorWorkspace>().error = Some(error.to_string());
                            return;
                        }
                    }
                } else {
                    None
                };
                files
                    .or_else(|| {
                        world
                            .non_send_mut::<DesktopContext>()
                            .context
                            .clipboard_text()
                    })
                    .map(ClipboardAction::Paste)
            }
            MenuAction::SelectAll => Some(ClipboardAction::SelectAll),
            _ => None,
        };
        if let Some(action) = operation {
            match world.run_system_with(handler, ClipboardInvocation { entity, action }) {
                Ok(Ok(Some(value))) => {
                    world
                        .non_send_mut::<DesktopContext>()
                        .context
                        .set_clipboard_text(value);
                }
                Ok(Ok(None)) => {}
                result => {
                    world.resource_mut::<EditorWorkspace>().error =
                        Some(format!("Native clipboard operation failed: {result:?}"))
                }
            }
            return;
        }
    }
    queue_native_edit_shortcut(
        &mut world.non_send_mut::<DesktopContext>().context,
        key,
        shift,
    );
}

#[cfg(all(feature = "automation", target_os = "macos"))]
pub(crate) fn smoke_native_menu(
    world: &World,
    action: crate::native_commands::MenuAction,
) -> io::Result<()> {
    world
        .non_send::<bed_desktop::DesktopNativePlatform>()
        .menu
        .as_ref()
        .ok_or_else(|| io::Error::other("native menu is not initialized"))?
        .perform_for_smoke(&action.command_id())
}
#[cfg(all(feature = "automation", target_os = "macos"))]
pub(crate) fn smoke_native_toolbar(world: &World, command: &str) -> bool {
    world
        .non_send::<bed_desktop::DesktopNativePlatform>()
        .window
        .as_ref()
        .is_some_and(|window| window.click_command(command))
}
#[cfg(all(feature = "automation", target_os = "macos"))]
pub(crate) fn smoke_native_chrome(world: &World) -> io::Result<()> {
    let window = world
        .non_send::<bed_desktop::DesktopNativePlatform>()
        .window
        .as_ref()
        .ok_or_else(|| io::Error::other("native window is not initialized"))?;
    window.validate_control_layout()?;
    if !window.preserves_winit_content_view() {
        return Err(io::Error::other(
            "native chrome replaced Bevy's content view",
        ));
    }
    Ok(())
}

fn spawn_instance(
    world: &mut World,
    workspace: Option<&bed_editor_api::workspace::WorkspaceSpec>,
) -> io::Result<()> {
    let config = world.non_send::<Settings>().config_dir.clone();
    let mut command = new_instance_command(&std::env::current_exe()?, &config, false, workspace)?;
    #[cfg(feature = "automation")]
    if world.resource::<NativeSmokeMode>().0 {
        command.arg("--smoke-test");
    }
    let child = command.spawn()?;
    #[cfg(feature = "automation")]
    if world.resource::<NativeSmokeMode>().0 {
        world.resource_mut::<NativeSpawnedInstances>().0.push(child);
        return Ok(());
    }
    drop(child);
    Ok(())
}
#[cfg(feature = "automation")]
#[derive(Resource, Default)]
pub(crate) struct NativeSmokeMode(pub bool);
#[cfg(feature = "automation")]
#[derive(Resource, Default)]
pub(crate) struct NativeSpawnedInstances(pub Vec<std::process::Child>);
#[cfg(all(feature = "automation", target_os = "macos"))]
#[derive(Resource, Default)]
pub(crate) struct NativeMenuObservations(pub Vec<crate::native_commands::MenuDispatch>);
#[cfg(all(feature = "automation", target_os = "macos"))]
pub(crate) fn smoke_native_key_equivalent(
    world: &World,
    characters: &str,
    key_code: u16,
    shift: bool,
) -> io::Result<()> {
    world
        .non_send::<bed_desktop::DesktopNativePlatform>()
        .menu
        .as_ref()
        .ok_or_else(|| io::Error::other("native menu is not initialized"))?
        .key_equivalent_for_smoke(characters, key_code, shift)
}
#[cfg(all(feature = "automation", target_os = "macos"))]
pub(crate) fn smoke_native_dock_new_window(world: &World) -> io::Result<()> {
    world
        .non_send::<bed_desktop::DesktopNativePlatform>()
        .menu
        .as_ref()
        .ok_or_else(|| io::Error::other("native menu is not initialized"))?
        .perform_dock_for_smoke(&crate::native_commands::MenuAction::NewWindow.command_id())
}
#[cfg(feature = "automation")]
pub(crate) fn smoke_focus_window(world: &mut World) {
    let entity = world
        .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
        .iter(world)
        .next();
    bevy::winit::WINIT_WINDOWS.with(|windows| {
        let windows = windows.borrow();
        if let Some(window) = entity.and_then(|entity| windows.get_window(entity)) {
            window.focus_window();
        }
    });
}

#[cfg(test)]
#[path = "../tests/unit/native/host_tests.rs"]
mod tests;

#[cfg(all(feature = "automation", target_os = "macos"))]
pub(crate) fn smoke_native_material(world: &World) -> (bool, Option<bool>, Option<f32>) {
    world
        .non_send::<bed_desktop::DesktopNativePlatform>()
        .window
        .as_ref()
        .map_or((false, None, None), |window| {
            (
                window.material_is_visible(),
                window.content_is_opaque(),
                window.content_opacity(),
            )
        })
}

fn new_window_command(
    _invocation: In<bed_workbench::CommandInvocation>,
    world: &mut World,
) -> bed_workbench::api::CommandResult {
    spawn_instance(world, None).map_err(|error| error.to_string())?;
    Ok(Vec::new())
}
#[cfg(target_os = "macos")]
fn queue_native_shortcut(context: &mut Context, stroke: bed_keybindings::Stroke) {
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
