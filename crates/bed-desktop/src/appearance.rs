//! Requested appearance is ordinary ECS data; font handles stay on the UI thread.
use crate::{DesktopContext, DesktopSystems};
use bed_ui::appearance::UiTheme;
pub use bed_ui::font::UiFonts as DesktopFonts;
use bevy::prelude::*;

#[derive(Resource, Clone, Default, PartialEq)]
pub struct DesktopTheme(pub UiTheme);

#[derive(Resource, Default)]
pub struct DesktopThemeErrors(pub Vec<String>);

pub struct DesktopThemePlugin;

impl Plugin for DesktopThemePlugin {
    fn build(&self, app: &mut App) {
        let runtime = bed_ui::UiRuntime::new(&app.world().non_send::<DesktopContext>().context);
        let prepared = runtime.frame_context();
        app.insert_non_send(runtime)
            .insert_non_send(prepared)
            .init_resource::<DesktopTheme>()
            .init_non_send::<DesktopFonts>()
            .init_resource::<DesktopThemeErrors>()
            .add_systems(PreUpdate, prepare.in_set(DesktopSystems::Prepare));
    }
}

fn prepare(
    appearance: Res<DesktopTheme>,
    mut fonts: NonSendMut<DesktopFonts>,
    mut desktop: NonSendMut<DesktopContext>,
    mut errors: ResMut<DesktopThemeErrors>,
    mut runtime: NonSendMut<bed_ui::UiRuntime>,
    mut prepared: NonSendMut<bed_ui::UiFrameContext>,
) {
    if let Err(error) = runtime.prepare(&mut desktop.context, &appearance.0, &mut fonts) {
        let error = error.to_string();
        if errors.0.last() != Some(&error) {
            errors.0.push(error);
        }
        return;
    }
    *prepared = runtime.frame_context();
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
        .add_plugins(DesktopThemePlugin);
        app.update();
        let generation = app.world().non_send::<DesktopFonts>().generation;
        let padding = app
            .world()
            .non_send::<DesktopContext>()
            .context
            .style()
            .frame_padding();
        app.world_mut().resource_mut::<DesktopTheme>().0.fonts.size = f32::NAN;
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
        assert_eq!(app.world().resource::<DesktopThemeErrors>().0.len(), 1);
    }
}
