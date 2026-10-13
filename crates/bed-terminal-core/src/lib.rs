//! Bed terminal components. Attribution: workspace LICENSE and NOTICE.
#[cfg(feature = "ui")]
#[path = "bed_terminal.rs"]
pub mod bed_terminal_core;
pub mod process_cwd;
mod process_title;
pub mod shell_bridge;
pub mod terminal;
#[cfg(feature = "ui")]
pub mod terminal_font;
pub mod terminal_input;
pub mod terminal_links;
pub mod terminal_pty;
#[cfg(feature = "ui")]
pub mod terminal_renderer;
#[cfg(feature = "ui")]
pub mod terminal_view;

#[cfg(all(test, feature = "ui"))]
pub(crate) static IMGUI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Process sessions are shared by terminal presentations and debugger launches.
#[derive(bevy::prelude::Resource, Default)]
pub struct TerminalRetentions(pub std::collections::HashSet<u64>);

#[derive(bevy::prelude::Component, Clone, Copy, Debug)]
pub struct TerminalAttachment {
    pub session: u64,
    pub command: bool,
}

#[cfg(feature = "ui")]
pub struct TerminalCorePlugin;

#[cfg(feature = "ui")]
impl bevy::app::Plugin for TerminalCorePlugin {
    fn build(&self, app: &mut bevy::app::App) {
        if !app
            .world()
            .contains_non_send::<bed_terminal_core::BedTerminal>()
        {
            app.insert_non_send(bed_terminal_core::BedTerminal::new_empty());
        }
        app.init_non_send::<terminal_font::TerminalFonts>()
            .init_resource::<TerminalRetentions>()
            .init_resource::<TerminalActivity>()
            .add_systems(
                bevy::prelude::PreUpdate,
                bevy::prelude::IntoScheduleConfigs::in_set(poll, TerminalCoreSystems::Poll),
            )
            .add_systems(
                bevy::prelude::Last,
                bevy::prelude::IntoScheduleConfigs::after(shutdown, bevy::app::OnAppExitSystems),
            );
    }
}

#[cfg(feature = "ui")]
fn shutdown(
    mut exit: bevy::prelude::MessageReader<bevy::app::AppExit>,
    mut terminal: bevy::prelude::NonSendMut<bed_terminal_core::BedTerminal>,
) {
    if exit.read().next().is_some() {
        terminal.shutdown();
    }
}

#[derive(bevy::prelude::SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TerminalCoreSystems {
    Poll,
}

#[derive(bevy::prelude::Resource, Default)]
pub struct TerminalActivity {
    pub changed: bool,
    pub error: Option<String>,
}

#[cfg(feature = "ui")]
fn poll(
    mut terminal: bevy::prelude::NonSendMut<bed_terminal_core::BedTerminal>,
    mut activity: bevy::prelude::ResMut<TerminalActivity>,
) {
    match terminal.poll() {
        Ok(changed) => activity.changed = changed,
        Err(error) => {
            activity.error = Some(error.to_string());
            activity.changed = true;
        }
    }
}
