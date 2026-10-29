//! The `ui` feature's plugin, and the surface's root view.
use crate::*;
use bedtime_ui::{UiPlugin, UiSystems};
use bevy::prelude::*;

pub struct WorkbenchUiPlugin;
impl Plugin for WorkbenchUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            crate::surface::configure.before(UiSystems::Configure),
        );
        #[cfg(feature = "native")]
        app.configure_sets(
            PreUpdate,
            NativeSystems::WindowGeometry.before(UiSystems::Draw),
        );
    }
    fn finish(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<UiPlugin>(),
            "WorkbenchUiPlugin requires UiPlugin (either registration order)"
        );
        assert!(
            app.is_plugin_added::<WorkbenchCorePlugin>(),
            "WorkbenchUiPlugin requires WorkbenchCorePlugin"
        );
    }
}

/// The surface root owns error presentation, not any originating panel. Draw
/// the modal first so its layer and dismissal keys gate the rest of this pass.
pub(crate) fn draw_active(
    ui: &mut bedtime_ui::egui::Ui,
    world: &mut World,
    context: &bedtime_ui::UiContext<'_>,
) {
    // Errors first: a failed save may explain what the close prompt then asks about.
    let modal_this_frame = crate::dialogs::draw(ui.ctx(), world, context.surface)
        || crate::dialogs::draw_close_prompt(ui.ctx(), world, context.surface);
    // Keep the shell's ID scope stable whether or not a dialog is visible.
    let mut shell = bedtime_ui::egui::UiBuilder::new().id_salt("workbench-shell");
    if modal_this_frame {
        // egui's modal hit/focus observations use the previous pass. Explicitly
        // disable this pass's shell, including the first and closing frames.
        shell = shell.disabled();
    }
    ui.scope_builder(shell, |ui| {
        crate::frontend::draw_active(ui, world, context);
    });
}
