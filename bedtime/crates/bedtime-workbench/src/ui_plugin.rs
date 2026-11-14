//! The `ui` feature's plugin.
use crate::*;
use bedtime_ui::UiPlugin;
use bevy::prelude::*;

/// The unsaved-changes prompt in bedtime-ui's modal layer. Layouts and their
/// drawing are `LayoutsPlugin` (installed by `WorkbenchCorePlugins`).
pub struct WorkbenchUiPlugin;
impl Plugin for WorkbenchUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_modal(crate::dialogs::draw_close_prompt)
            .init_resource::<crate::status::StatusActivity>()
            .init_resource::<crate::status::StatusItems>()
            .init_resource::<crate::status::PanelStatusItems>()
            .init_resource::<crate::status::StatusSource>()
            .add_systems(Update, crate::status::follow_focus);
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
