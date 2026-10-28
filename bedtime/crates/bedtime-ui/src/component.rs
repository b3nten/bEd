//! Ordinary, composable drawing functions. State belongs to egui IDs or ECS data.
use crate::*;
use bevy::prelude::*;
use std::sync::Mutex;

pub trait Component {
    type Props<'a>;
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, props: Self::Props<'_>);
}

/// A stored root renderer. Children can use arbitrary props and normal Rust calls.
pub type UiDraw = fn(&mut egui::Ui, &mut World, &UiContext<'_>);

/// Services and identity independent of the parent's mutable egui allocation.
/// View and surface entity IDs belong to the application drawing World.
#[derive(Clone, Copy)]
pub struct UiContext<'a> {
    pub view: Entity,
    pub surface: Entity,
    pub theme: &'a Theme,
    pub metrics: &'a Metrics,
    pub motion: MotionPolicy,
    pub(crate) icons: &'a Mutex<icon_cache::IconCache>,
}
impl UiContext<'_> {
    pub fn for_view(&self, view: Entity) -> Self {
        Self { view, ..*self }
    }
    /// Optional access to existing icon, clipping and motion helpers.
    /// Ordinary egui layout remains available through the separate Ui argument.
    pub fn frame<'a>(&'a self, ui: &'a mut egui::Ui) -> UiFrame<'a> {
        UiFrame {
            view: self.view,
            surface: self.surface,
            ui,
            theme: self.theme,
            metrics: self.metrics,
            motion: self.motion,
            icons: self.icons,
        }
    }
}
