//! What a widget needs from its surface, carried by the egui `Context`: a
//! widget is then drawn from a `Ui` alone, in any closure.
use crate::*;
use std::sync::{Arc, OnceLock};

#[derive(Clone)]
pub(crate) struct Services {
    pub theme: Arc<Theme>,
    pub motion: MotionPolicy,
    pub icons: Arc<Mutex<icon_cache::IconCache>>,
}

/// Set before each pass of a surface's context.
pub(crate) fn install(
    ctx: &egui::Context,
    theme: Arc<Theme>,
    motion: MotionPolicy,
    icons: Arc<Mutex<icon_cache::IconCache>>,
) {
    let services = Services {
        theme,
        motion,
        icons,
    };
    ctx.data_mut(|data| data.insert_temp(egui::Id::NULL, services));
}

/// The context's services. A context that isn't a surface's (a test's) has
/// the dark theme, default motion and no icons.
pub(crate) fn services(ctx: &egui::Context) -> Services {
    ctx.data(|data| data.get_temp::<Services>(egui::Id::NULL))
        .unwrap_or_else(|| {
            static BARE: OnceLock<Services> = OnceLock::new();
            BARE.get_or_init(|| Services {
                theme: Arc::new(Theme::dark()),
                motion: MotionPolicy::default(),
                icons: default(),
            })
            .clone()
        })
}

impl Theme {
    /// The theme `ui` is drawn with.
    pub fn of(ui: &egui::Ui) -> Arc<Theme> {
        services(ui.ctx()).theme
    }
}

impl MotionPolicy {
    /// The motion policy `ui` is drawn with.
    pub fn of(ui: &egui::Ui) -> MotionPolicy {
        services(ui.ctx()).motion
    }
}
