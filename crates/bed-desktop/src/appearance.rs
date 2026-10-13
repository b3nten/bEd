//! Requested appearance is ordinary ECS data; font handles stay on the UI thread.
use crate::{DesktopContext, DesktopSystems};
use bed_ui::appearance::UiAppearance;
pub use bed_ui::font::UiFonts as DesktopFonts;
use bevy::prelude::*;

#[derive(Resource, Clone, Default, PartialEq)]
pub struct DesktopAppearance(pub UiAppearance);

#[derive(Resource, Default)]
pub struct DesktopAppearanceErrors(pub Vec<String>);

pub struct DesktopAppearancePlugin;

impl Plugin for DesktopAppearancePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DesktopAppearance>()
            .init_non_send::<DesktopFonts>()
            .init_resource::<DesktopAppearanceErrors>()
            .add_systems(PreUpdate, prepare.in_set(DesktopSystems::Prepare));
    }
}

fn prepare(
    appearance: Res<DesktopAppearance>,
    mut fonts: NonSendMut<DesktopFonts>,
    mut desktop: NonSendMut<DesktopContext>,
    mut errors: ResMut<DesktopAppearanceErrors>,
) {
    if let Err(error) = fonts.apply(&mut desktop.context, &appearance.0.fonts, false) {
        let error = error.to_string();
        if errors.0.last() != Some(&error) {
            errors.0.push(error);
        }
        return;
    }
    bed_ui::appearance::apply_style(&mut desktop.context, &appearance.0, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_font_size_preserves_live_style_and_atlas() {
        let _guard = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = dear_imgui_rs::Context::create();
        let platform = crate::DesktopPlatform::new(&mut context);
        let mut app = App::new();
        app.insert_non_send(DesktopContext {
            context,
            platform,
            snapshot_context: None,
        })
        .add_plugins(DesktopAppearancePlugin);
        app.update();
        let generation = app.world().non_send::<DesktopFonts>().generation;
        let padding = app
            .world()
            .non_send::<DesktopContext>()
            .context
            .style()
            .frame_padding();
        app.world_mut()
            .resource_mut::<DesktopAppearance>()
            .0
            .fonts
            .size = f32::NAN;
        app.update();
        assert_eq!(
            app.world().non_send::<DesktopFonts>().generation,
            generation
        );
        assert_eq!(
            app.world()
                .non_send::<DesktopContext>()
                .context
                .style()
                .frame_padding(),
            padding
        );
        assert_eq!(app.world().resource::<DesktopAppearanceErrors>().0.len(), 1);
    }
}
