//! Requested runtime configuration is ordinary ECS data. Consumers prepare later.
use crate::{CoreNotice, CorePresentationMode};
use bevy::prelude::*;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CoreConfigurationSystems {
    Settings,
}

#[cfg(feature = "language-services")]
#[derive(Resource, Default, Clone, PartialEq)]
pub struct SyntaxAppearance(pub bed_highlight::tree_sitter::ThemeColors);

pub(crate) fn configure(world: &mut World) {
    if *world.resource::<CorePresentationMode>() != CorePresentationMode::Standard {
        return;
    }
    crate::settings::refresh_settings(world);
    let values = world
        .resource::<bed_api::settings::SettingsSnapshot>()
        .for_owner("bed.ui")
        .clone();
    if let Some(mut theme) = world.get_resource_mut::<bed_desktop::DesktopTheme>() {
        theme.0.fonts.family = values["font"]
            .as_str()
            .expect("Registered font family")
            .into();
        theme.0.fonts.size = values["fontSize"].as_f64().expect("Validated font size") as f32;
        theme.0.opacity = values["background_opacity"]
            .as_f64()
            .expect("Validated opacity") as f32;
        theme.0.motion.enabled = values["ui_animations"]
            .as_bool()
            .expect("Registered motion preference");
    }
    if let Err(error) = crate::settings::configure_document_settings(world) {
        world.write_message(CoreNotice {
            source: None,
            message: error.to_string(),
        });
    }
}

#[cfg(feature = "language-services")]
pub(crate) fn configure_syntax(world: &mut World) {
    let Some(palette) = world
        .get_resource::<SyntaxAppearance>()
        .map(|value| value.0.clone())
    else {
        return;
    };
    if let Some(mut session) = world.get_non_send_mut::<bed_documents::DocumentSession>() {
        session.set_highlight_theme(palette);
    }
}
