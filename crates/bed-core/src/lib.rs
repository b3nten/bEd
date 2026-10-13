//! Desktop, workbench and document composition for ordinary Bevy applications.
pub use bed_api as api;
pub use bed_desktop;
pub use bed_documents;
pub use bed_ui;
pub use bed_workbench;
pub use bevy;
mod commands;
pub mod documents;
mod files;
#[cfg(feature = "language-services")]
mod format_saves;
pub mod models;
mod saves;
mod transitions;
#[cfg(feature = "language-services")]
pub use format_saves::*;
pub use saves::*;
pub use transitions::*;
mod native;
#[cfg(test)]
mod tests;
pub use documents::*;
pub use models::*;

use bed_desktop::{DesktopPlugins, DesktopSystems};
use bevy::prelude::*;

/// One complete desktop, layout and document foundation. Application configuration
/// and persistence are supplied by the host; default documents start empty.
pub struct BedCorePlugin {
    pub desktop: DesktopPlugins,
}
impl Default for BedCorePlugin {
    fn default() -> Self {
        Self {
            desktop: DesktopPlugins {
                close_when_requested: false,
                ..Default::default()
            },
        }
    }
}

impl Plugin for BedCorePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(DesktopPlugins {
            window: self.desktop.window.clone(),
            wgpu: self.desktop.wgpu.clone(),
            close_when_requested: self.desktop.close_when_requested,
        })
        .add_plugins(BedCoreIntegrationPlugin);
    }
}

/// Attach shared editing and workbench services to an existing Bevy desktop.
/// DesktopIntegrationPlugin must already be installed; the host owns its
/// DefaultPlugins, renderer configuration, window and runner.
pub struct BedCoreIntegrationPlugin;
impl Plugin for BedCoreIntegrationPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<bed_desktop::DesktopIntegrationPlugin>(),
            "BedCoreIntegrationPlugin requires DesktopIntegrationPlugin"
        );
        app.init_non_send::<bed_ui::icons::IconDirectories>();
        let directory = app
            .world()
            .non_send::<bed_ui::icons::IconDirectories>()
            .0
            .get("bed.core")
            .cloned();
        app.world_mut().init_non_send::<bed_ui::icons::Icons>();
        app.world_mut()
            .non_send_mut::<bed_ui::icons::Icons>()
            .register_core_defaults(directory.as_deref())
            .expect("valid core icon directory");
        app.add_plugins((bed_runtime::BedRuntimePlugin, DocumentHostPlugin))
            .configure_sets(
                PreUpdate,
                CoreSystems::Services
                    .after(DesktopSystems::Configure)
                    .before(DesktopSystems::Prepare),
            )
            .configure_sets(
                PreUpdate,
                bed_api::features::FeatureSystems::Prepare
                    .after(DesktopSystems::Prepare)
                    .before(bed_desktop::DesktopCanvasSystems::Prepare),
            )
            .configure_sets(
                PreUpdate,
                CoreSystems::Requests.in_set(DesktopSystems::AfterUi),
            )
            .add_systems(
                PreUpdate,
                (
                    route_native_commands.in_set(DesktopSystems::InterceptInput),
                    after_ui
                        .after(CoreSystems::Requests)
                        .in_set(DesktopSystems::AfterUi),
                ),
            )
            .add_systems(Update, (report_notices, handle_close_requests));
        native::install(app);
        if !app.world().contains_resource::<bed_desktop::DesktopUi>() {
            app.insert_resource(bed_desktop::DesktopUi(draw));
        }
    }
}

fn draw(world: &mut World, ui: &dear_imgui_rs::Ui) {
    if let Some(textures) = world.get_resource::<bed_ui::gpu::CanvasTextures>() {
        let textures = textures.0.clone();
        world
            .resource_mut::<api::presentation::PresentationFrame>()
            .textures = textures;
    }
    if let Some(_menu) = ui.begin_main_menu_bar() {
        let commands = world
            .resource::<api::workbench::WorkbenchRegistry>()
            .menus()
            .iter()
            .filter_map(|id| {
                world
                    .resource::<api::workbench::WorkbenchRegistry>()
                    .command(id)
            })
            .cloned()
            .collect::<Vec<_>>();
        for command in commands {
            let presentation =
                command.presentation(world.get_resource::<api::workbench::CommandPresentations>());
            if presentation.visible
                && ui.menu_item_enabled_selected_no_shortcut(
                    presentation.label,
                    false,
                    presentation.enabled,
                )
            {
                let invocation =
                    bed_workbench::capture_command(world, command.id, serde_json::Value::Null);
                world
                    .resource_mut::<bed_workbench::WorkbenchRequests>()
                    .push(api::workbench::WorkbenchAction::Invoke(invocation));
            }
        }
    }
    let inset = world.resource::<bed_desktop::DesktopContentInsets>().top;
    world
        .resource_mut::<bed_workbench::WorkbenchViewport>()
        .top_inset = inset;
    let has_status = world
        .query::<&api::status::StatusItems>()
        .iter(world)
        .next()
        .is_some();
    world
        .resource_mut::<bed_workbench::WorkbenchViewport>()
        .bottom_inset = if has_status {
        bed_workbench::status_bar_height(ui)
    } else {
        0.0
    };
    bed_workbench::render_ui(world, ui);
    if has_status {
        bed_workbench::render_status_bar(
            world,
            ui,
            bed_workbench::StatusBarHost {
                icon: "bed.core.default",
                summary: "",
                details: "",
            },
            |world, key| {
                world
                    .get_non_send::<bed_ui::icons::Icons>()
                    .and_then(|icons| icons.get(key))
            },
        );
    }
    let geometry = world
        .query::<(Entity, &api::workbench::PanelGeometry)>()
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
    world.resource_mut::<bed_desktop::DesktopFrame>().panels = geometry;
}
fn route_native_commands(world: &mut World) {
    if *world.resource::<CorePresentationMode>() == CorePresentationMode::Application {
        return;
    }
    let commands = std::mem::take(&mut world.resource_mut::<bed_desktop::DesktopNativeActions>().0);
    for command in commands {
        if command.quit {
            let mut invocation =
                bed_workbench::capture_command(world, "bed.core.quit", serde_json::Value::Null);
            invocation.source = None;
            world
                .resource_mut::<bed_workbench::WorkbenchRequests>()
                .push_unscoped(api::workbench::WorkbenchAction::Invoke(invocation));
            continue;
        }
        if world
            .resource::<api::workbench::WorkbenchRegistry>()
            .command(&command.command)
            .is_some()
        {
            let invocation =
                bed_workbench::capture_command(world, command.command, serde_json::Value::Null);
            world
                .resource_mut::<bed_workbench::WorkbenchRequests>()
                .push(api::workbench::WorkbenchAction::Invoke(invocation));
        }
    }
}
fn after_ui(world: &mut World) {
    let capture = *world.resource::<bed_desktop::DesktopCapture>();
    *world.resource_mut::<bed_runtime::InputCapture>() = bed_runtime::InputCapture {
        keyboard: capture.keyboard,
        pointer: capture.pointer,
    };
    if *world.resource::<CorePresentationMode>() == CorePresentationMode::Application {
        return;
    }
    for error in world
        .resource_mut::<bed_workbench::WorkbenchRequests>()
        .take_errors()
    {
        world.write_message(CoreNotice {
            source: None,
            message: error,
        });
    }
    for error in std::mem::take(&mut world.resource_mut::<bed_desktop::DesktopCanvasErrors>().0) {
        world.write_message(CoreNotice {
            source: error.entity,
            message: error.message,
        });
    }
    for error in std::mem::take(
        &mut world
            .resource_mut::<bed_desktop::DesktopAppearanceErrors>()
            .0,
    ) {
        world.write_message(CoreNotice {
            source: None,
            message: error,
        });
    }
    for error in std::mem::take(&mut world.resource_mut::<bed_desktop::DesktopNativeErrors>().0) {
        world.write_message(CoreNotice {
            source: None,
            message: error,
        });
    }
    if let Some(error) = world
        .resource::<bed_desktop::DesktopRenderStatus>()
        .take_error()
    {
        world.write_message(CoreNotice {
            source: None,
            message: error,
        });
        world.write_message(AppExit::error());
    }
}
fn report_notices(mut notices: MessageReader<CoreNotice>) {
    for notice in notices.read() {
        eprintln!("{}", notice.message);
    }
}

fn handle_close_requests(world: &mut World) {
    if *world.resource::<CorePresentationMode>() == CorePresentationMode::Application {
        return;
    }
    let closing = world
        .resource_mut::<Messages<bevy::window::WindowCloseRequested>>()
        .drain()
        .next()
        .is_some();
    if closing || documents::quit_save_pending(world) {
        match quit(world) {
            Ok(()) => {
                world.write_message(AppExit::Success);
            }
            Err(_) if documents::quit_save_pending(world) => {}
            Err(message) => {
                world.write_message(CoreNotice {
                    source: None,
                    message,
                });
            }
        }
    }
}
