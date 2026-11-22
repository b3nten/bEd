//! What bEd's window looks like beyond its panels: the backdrop behind
//! them, the screen effects over them, and bedtime's cat, which walks along
//! the bottom.
pub mod backdrop;
pub mod effects;

use bevy::prelude::*;

pub struct VisualsPlugin;

impl Plugin for VisualsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            effects::EffectsPlugin,
            backdrop::BackdropPlugin,
            bedtime::cat::CatPlugin,
        ));
    }
}
