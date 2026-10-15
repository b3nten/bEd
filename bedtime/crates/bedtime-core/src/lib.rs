//! Desktop, workbench and document composition for ordinary Bevy applications.
pub use bedtime_api as api;
pub use bedtime_desktop;
pub use bedtime_documents;
pub use bedtime_ui;
pub use bedtime_workbench;
pub use bevy;
mod commands;
pub mod documents;
mod files;
#[cfg(feature = "layouts")]
pub mod layouts;
pub use files::RemoteHelperDirectory;
#[cfg(feature = "remote")]
pub use files::begin_remote_context;
#[cfg(feature = "language-services")]
mod format_saves;
pub mod models;
mod saves;
pub mod settings;
mod settings_view;
mod transitions;
#[cfg(feature = "language-services")]
pub use format_saves::*;
pub use saves::*;
pub use transitions::*;
mod configuration;
mod desktop_workbench;
mod menus;
mod native;
#[cfg(feature = "language-services")]
pub use configuration::SyntaxAppearance;
pub use configuration::{CoreConfigurationSystems, apply_ui_preferences};
pub use desktop_workbench::*;
pub use menus::{menu_shortcut, resolve_menus};
#[cfg(test)]
mod tests;
pub use documents::*;
pub use models::*;

use bedtime_desktop::{DesktopPlugins, DesktopSystems};
use bevy::prelude::*;

/// One complete desktop, layout and document foundation. Application configuration
/// and persistence are supplied by the host; default documents start empty.
#[doc(hidden)]
pub struct FoundationPlugin {
    pub desktop: DesktopPlugins,
}
impl Default for FoundationPlugin {
    fn default() -> Self {
        Self {
            desktop: DesktopPlugins {
                close_when_requested: false,
                ..Default::default()
            },
        }
    }
}

impl Plugin for FoundationPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(DesktopPlugins {
            window: self.desktop.window.clone(),
            wgpu: self.desktop.wgpu.clone(),
            // The document owner resolves window/native quit requests after
            // validation and saves, including when callers replace the window.
            close_when_requested: false,
        });
        app.add_plugins(DocumentHostPlugin)
            .add_message::<NativeCommandDispatched>()
            .configure_sets(
                PreUpdate,
                (
                    bedtime_workbench::WorkbenchSystems::Keymap,
                    bedtime_workbench::WorkbenchSystems::InputProjection,
                )
                    .chain()
                    .before(DesktopSystems::InterceptInput),
            )
            .add_systems(
                PreUpdate,
                settings::refresh_settings
                    .in_set(bedtime_workbench::WorkbenchSystems::Keymap)
                    .before(bedtime_workbench::resolve_keymap),
            )
            .configure_sets(
                PreUpdate,
                CoreSystems::Services
                    .after(DesktopSystems::Configure)
                    .before(DesktopSystems::Prepare),
            )
            .configure_sets(
                PreUpdate,
                bedtime_api::features::FeatureSystems::Prepare
                    .after(DesktopSystems::Prepare)
                    .before(bedtime_desktop::DesktopCanvasSystems::Prepare),
            )
            .configure_sets(
                PreUpdate,
                CoreSystems::Requests.in_set(DesktopSystems::AfterUi),
            )
            .add_systems(
                PreUpdate,
                (
                    route_native_commands
                        .in_set(DesktopSystems::InterceptInput)
                        .in_set(bedtime_desktop::DesktopInputStages::Commands)
                        .in_set(DesktopWorkbenchSystems::NativeCommands),
                    after_ui
                        .after(CoreSystems::Requests)
                        .in_set(DesktopSystems::AfterUi),
                ),
            )
            .add_systems(Update, (report_notices, handle_close_requests));
        app.add_systems(
            PreUpdate,
            configuration::configure
                .in_set(DesktopSystems::Configure)
                .in_set(CoreConfigurationSystems::Settings),
        );
        native::install(app);
        app.add_plugins(DesktopWorkbenchPlugin);
        app.world_mut().spawn((
            bedtime_api::workbench::WorkbenchOverlay(render_close_prompt),
            bedtime_api::workbench::OverlayPhase::AfterChrome,
            bedtime_api::workbench::OverlayOrder(100),
        ));
        if !app
            .world()
            .contains_resource::<bedtime_desktop::DesktopUi>()
        {
            app.insert_resource(bedtime_desktop::DesktopUi(draw_workbench));
        }
    }
}

/// Draw the complete shared frontend; applications contribute menus, identity and overlays.
pub fn draw_workbench(world: &mut World, ui: &dear_imgui_rs::Ui) {
    begin_native_toolbar_drag(ui);
    prepare_workbench_ui(world);
    let font = world
        .get_non_send::<bedtime_ui::font::UiFonts>()
        .and_then(|fonts| fonts.main);
    let _font = font.map(|font| ui.push_font(font));
    if let Err(message) = menus::draw_menus(world, ui) {
        world.write_message(CoreNotice {
            source: None,
            message,
        });
    }
    let inset = world
        .get_resource::<bedtime_desktop::DesktopContentInsets>()
        .map_or(0.0, |insets| insets.top);
    world
        .resource_mut::<bedtime_workbench::WorkbenchViewport>()
        .top_inset = inset;
    let identity = world
        .get_resource::<api::status::StatusBarIdentity>()
        .cloned();
    let has_status = identity.is_some()
        || world
            .query::<&api::status::StatusItems>()
            .iter(world)
            .any(|items| !items.0.is_empty());
    world
        .resource_mut::<bedtime_workbench::WorkbenchViewport>()
        .bottom_inset = if has_status {
        bedtime_workbench::status_bar_height(ui)
    } else {
        0.0
    };
    bedtime_workbench::render_ui(world, ui);
    if has_status {
        let identity = identity.unwrap_or_default();
        bedtime_workbench::render_status_bar(
            world,
            ui,
            bedtime_workbench::StatusBarHost {
                icon: &identity.icon,
                summary: &identity.summary,
                details: &identity.details,
            },
            |world, key| {
                world
                    .get_non_send::<bedtime_ui::icons::Icons>()
                    .and_then(|icons| icons.get(key))
            },
        );
    }
    bedtime_workbench::render_overlays(world, ui, api::workbench::OverlayPhase::AfterChrome);
    finish_workbench_ui(world);
    finish_native_toolbar_drag(ui);
}

/// Observations of native command ingress are independent of command execution.
#[derive(Message, Clone, Debug)]
pub struct NativeCommandDispatched(pub bedtime_desktop::DesktopNativeAction);

pub fn route_native_commands(world: &mut World) {
    let commands = std::mem::take(
        &mut world
            .resource_mut::<bedtime_desktop::DesktopNativeActions>()
            .0,
    );
    for command in commands {
        world.write_message(NativeCommandDispatched(command.clone()));
        // Native menu key equivalents never pass through panel interception.
        // Give editing to its panel owner before replaying ordinary UI chords.
        if let Some(action) = NativeEditAction::from_command(&command.command) {
            route_native_edit(world, action);
            continue;
        }
        if command.keyboard
            && let Some(stroke) = menu_shortcut(world, &command.command)
        {
            queue_native_shortcut(
                &mut world
                    .non_send_mut::<bedtime_desktop::DesktopContext>()
                    .context,
                stroke,
            );
            continue;
        }
        let id = if command.quit {
            api::CommandId::new("bedtime.core.quit")
        } else {
            command.command.into()
        };
        if world
            .resource::<api::workbench::WorkbenchRegistry>()
            .command(&id)
            .is_some()
        {
            let arguments = match world.get_resource::<api::menus::MenuLayout>().map_or_else(
                || Ok(serde_json::Value::Null),
                |layout| layout.arguments(&id),
            ) {
                Ok(arguments) => arguments,
                Err(message) => {
                    world.write_message(CoreNotice {
                        source: None,
                        message,
                    });
                    continue;
                }
            };
            let mut invocation = bedtime_workbench::capture_command(world, id, arguments);
            if command.quit {
                invocation.source = None;
            }
            let action = api::workbench::WorkbenchAction::Invoke(invocation);
            if command.quit {
                world
                    .resource_mut::<bedtime_workbench::WorkbenchRequests>()
                    .push_unscoped(action);
            } else {
                world
                    .resource_mut::<bedtime_workbench::WorkbenchRequests>()
                    .push(action);
            }
        }
    }
}
fn after_ui(world: &mut World) {
    for error in world
        .resource_mut::<bedtime_workbench::WorkbenchRequests>()
        .take_errors()
    {
        world.write_message(CoreNotice {
            source: None,
            message: error,
        });
    }
    for error in std::mem::take(
        &mut world
            .resource_mut::<bedtime_desktop::DesktopCanvasErrors>()
            .0,
    ) {
        world.write_message(CoreNotice {
            source: error.entity,
            message: error.message,
        });
    }
    for error in std::mem::take(
        &mut world
            .resource_mut::<bedtime_desktop::DesktopThemeErrors>()
            .0,
    ) {
        world.write_message(CoreNotice {
            source: None,
            message: error,
        });
    }
    for error in std::mem::take(
        &mut world
            .resource_mut::<bedtime_desktop::DesktopNativeErrors>()
            .0,
    ) {
        world.write_message(CoreNotice {
            source: None,
            message: error,
        });
    }
    if let Some(error) = world
        .resource::<bedtime_desktop::DesktopRenderStatus>()
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
    let closing = world
        .resource_mut::<Messages<bevy::window::WindowCloseRequested>>()
        .drain()
        .next()
        .is_some();
    if closing {
        submit_quit(world);
    }
}
