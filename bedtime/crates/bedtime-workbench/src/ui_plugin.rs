//! The `ui` feature's plugin.
use crate::*;
use bedtime_ui::UiPlugin;
use bevy::prelude::*;

/// The status bar's state. Layouts and their drawing are `LayoutsPlugin`
/// (installed by `AppCorePlugins`); the unsaved-changes prompt is
/// `DocumentsPlugin`'s.
pub struct WorkbenchUiPlugin;
impl Plugin for WorkbenchUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<crate::status::StatusActivity>()
            .init_resource::<crate::status::GlobalStatusItems>()
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
            app.is_plugin_added::<SessionPlugin>(),
            "WorkbenchUiPlugin requires SessionPlugin (part of AppCorePlugins)"
        );
    }
}
