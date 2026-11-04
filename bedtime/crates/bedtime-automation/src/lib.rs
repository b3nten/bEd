//! Drive a bedtime app from scripts over the Bevy Remote Protocol (JSON-RPC
//! over HTTP on 127.0.0.1). Bevy's own methods read and write reflected
//! components and resources; this crate adds a few generic ones:
//!
//! | Method | Params | Result |
//! | --- | --- | --- |
//! | `bedtime.status` | | frame count, queued input steps, window size and scale, finished screenshots |
//! | `bedtime.input` | `{"steps": [...]}` | queued; one step per frame (see [`Step`]) |
//! | `bedtime.ui` | | every widget: role, label, value, rect (logical pixels), state |
//! | `bedtime.screenshot` | `{"path": "..."}` | queued; listed in `status` once written |
//! | `bedtime.invoke` | `{"command": "settings.open"}` | runs the command as if from its key |
//! | `bedtime.exit` | | exits the app normally |
//!
//! Scripts start the app with `BEDTIME_AUTOMATION_PORT` set. The app should
//! then open its window unfocused ([`AutomationPlugin::active`]), so scripts
//! run without taking over the computer. See `tools/bed.py`.
//!
//! Nothing here is specific to one app or plugin. When a script can't see
//! something, reflect it or label its widget rather than adding a method.
use bevy::prelude::*;

#[cfg(feature = "enabled")]
mod remote;
#[cfg(feature = "enabled")]
pub use remote::Step;

/// The environment variable a script sets to the port to listen on.
pub const PORT_VARIABLE: &str = "BEDTIME_AUTOMATION_PORT";

/// Listens for scripts when the `enabled` feature is on and the app was started
/// with [`PORT_VARIABLE`] set; otherwise does nothing.
pub struct AutomationPlugin {
    port: Option<u16>,
}

impl AutomationPlugin {
    pub fn from_env() -> Self {
        let port = std::env::var(PORT_VARIABLE).ok().and_then(|port| {
            let parsed = port.parse().ok();
            if parsed.is_none() {
                eprintln!("{PORT_VARIABLE} isn't a port: {port}");
            }
            parsed
        });
        Self {
            port: port.filter(|_| cfg!(feature = "enabled")),
        }
    }

    /// A script is driving the app: open the window unfocused.
    pub fn active(&self) -> bool {
        self.port.is_some()
    }
}

impl Plugin for AutomationPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(feature = "enabled")]
        if let Some(port) = self.port {
            remote::install(app, port);
        }
        #[cfg(not(feature = "enabled"))]
        let _ = app;
    }
}
