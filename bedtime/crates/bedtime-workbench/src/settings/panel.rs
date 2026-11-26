//! The settings panel (`ui` feature): every registered setting, in the
//! section it was declared in, for whichever layer is chosen, then the
//! keyboard shortcuts. Add
//! `SettingsPanelPlugin` to have it, and `Show<SettingsPanel>`, which opens
//! it.
use crate::settings::ui;
use crate::*;
use bedtime_persistence::{StoreId, Stores};
use bedtime_ui::{Separator, Text, Tone, UiContext, egui};
use bevy::prelude::*;

impl crate::ShowPanel for SettingsPanel {
    const SHOW: &'static str = "settings.show";
    const SHOW_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::Comma)];
}

/// Where a layer is saved, shown under the layer tabs: set by whoever adds
/// the layers (workspaces does). Nothing's shown without it.
#[derive(Resource, Clone, Copy)]
pub struct SettingsLocation(pub fn(&World, StoreId) -> Option<String>);

/// The settings panel and the action that opens it.
pub struct SettingsPanelPlugin;

impl Plugin for SettingsPanelPlugin {
    fn build(&self, app: &mut App) {
        app.add_panel::<SettingsPanel>()
            .add_menu_item_labelled::<Show<SettingsPanel>>(
                StandardMenu::App,
                "settings",
                "Settings…",
            );
    }
}

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, crate::persistence::Persist)]
pub struct SettingsPanel;

impl Panel for SettingsPanel {
    const LABEL: &'static str = "Settings";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        bedtime_ui::draw::with_system(ui, world, context, panel, settings_panel);
    }
}

/// The panel, drawn: a system that reads the settings. What a row changes
/// is queued by the row, and made when the draw is done.
fn settings_panel(
    bedtime_ui::PanelUi { ui, context, .. }: bedtime_ui::PanelUi<SettingsPanel>,
    world: &World,
) {
    let context = &context;
    {
        egui::Frame::NONE
            .inner_margin(bedtime_ui::Space::Lg.px(ui))
            .show(ui, |ui| {
                let Some(layer) = ui::layer_tabs(ui, world, |_, label| label.to_owned()) else {
                    return;
                };
                let location = world
                    .get_resource::<SettingsLocation>()
                    .and_then(|location| (location.0)(world, layer));
                if let Some(location) = location {
                    ui.add(Text::caption(location).muted().truncate());
                }
                let errors = [
                    world.bedtime().settings().error(layer),
                    world
                        .get_resource::<Stores>()
                        .and_then(|stores| stores.error_in(layer))
                        .map(str::to_owned),
                ];
                for error in errors.into_iter().flatten() {
                    ui.add(Text::new(error).tone(Tone::Error));
                }
                ui.add(Separator::new());

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
