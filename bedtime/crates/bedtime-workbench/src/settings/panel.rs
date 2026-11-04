//! The settings panel (`ui` feature): every registered setting, laid out by
//! the app's `settings_panel!` or by the sections settings were declared in,
//! for whichever layer is chosen, then the keyboard shortcuts. Add
//! `SettingsPanelPlugin` to have it, and its `OpenSettings` action.
use crate::settings::{SettingsWorldExt, ui};
use crate::*;
use bedtime_persistence::{StoreId, Stores};
use bedtime_ui::{UiContext, egui};
use bevy::prelude::*;

/// Open the settings panel: the one in the active layout, else a new one.
pub struct OpenSettings;
impl Action for OpenSettings {
    const KEY: &'static str = "settings.open";
    const LABEL: &'static str = "Open Settings";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::Comma)];
}

/// Where a layer is saved, shown under the layer tabs: set by whoever adds
/// the layers (workspaces does). Nothing's shown without it.
#[derive(Resource, Clone, Copy)]
pub struct SettingsLocation(pub fn(&World, StoreId) -> Option<String>);

/// The settings panel and the action that opens it.
pub struct SettingsPanelPlugin;

impl Plugin for SettingsPanelPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<SettingsPanel>()
            .add_action_handler::<OpenSettings>(Handler::new(show_panel::<SettingsPanel>));
    }
}

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, crate::persistence::Persist)]
pub struct SettingsPanel;

impl Panel for SettingsPanel {
    const LABEL: &'static str = "Settings";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, _: Entity) {
        egui::Frame::NONE.inner_margin(14.0).show(ui, |ui| {
            let Some(layer) = ui::layer_tabs(ui, world, |_, label| label.to_owned()) else {
                return;
            };
            let location = world
                .get_resource::<SettingsLocation>()
                .and_then(|location| (location.0)(world, layer));
            if let Some(location) = location {
                ui.add(egui::Label::new(egui::RichText::new(location).small().weak()).truncate());
            }
            let errors = [
                world.settings().error(layer),
                world
                    .get_resource::<Stores>()
                    .and_then(|stores| stores.error_in(layer))
                    .map(str::to_owned),
            ];
            for error in errors.into_iter().flatten() {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.add_space(4.0);
            ui.separator();

            // Both ways: rows have fixed columns, so a narrow panel scrolls
            // sideways rather than clipping controls.
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui::sections(ui, world, context, layer);
                    ui::heading(ui, "Keyboard Shortcuts");
                    crate::ui::keybindings_section(ui, world, context, layer);
                });
        });
    }
}
