//! The `ui` feature's plugin.
use crate::*;
use bedtime_ui::UiPlugin;
use bevy::prelude::*;

/// The unsaved-changes prompt in bedtime-ui's modal layer. Layouts and their
/// drawing are `LayoutsPlugin` (installed by `WorkbenchCorePlugins`).
pub struct WorkbenchUiPlugin;
impl Plugin for WorkbenchUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_modal(crate::dialogs::draw_close_prompt);
    }
    fn finish(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<UiPlugin>(),
            "WorkbenchUiPlugin requires UiPlugin (either registration order)"
        );
        assert!(
            app.is_plugin_added::<DocumentsPlugin>(),
            "WorkbenchUiPlugin requires DocumentsPlugin (part of WorkbenchCorePlugins)"
        );
    }
}
