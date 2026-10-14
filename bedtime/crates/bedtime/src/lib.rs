//! An owned entrypoint for Bedtime desktop applications.
//! Early process handlers run before configuration and Bevy construction.
pub use bedtime_api as api;
pub use bedtime_plugin_automation as automation;
#[path = "automation.rs"]
mod host_automation;
pub use bedtime_api::launch::{
    ApplicationIdentity, ApplicationPaths, LaunchRegistration, ProcessEntrypoint, ProcessSelector,
};
pub use bedtime_core as core;
pub use bedtime_desktop as desktop;
pub use bedtime_ui as ui;
pub use bevy;
use bevy::prelude::*;
use std::{
    collections::HashSet,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

pub struct LaunchBuilder {
    identity: ApplicationIdentity,
    arguments: Vec<OsString>,
    registrations: Vec<LaunchRegistration>,
}

/// Begin launch dispatch before constructing Bevy or reading application settings.
pub fn awaken(id: impl Into<String>, display_name: impl Into<String>) -> io::Result<LaunchBuilder> {
    Ok(LaunchBuilder {
        identity: ApplicationIdentity::new(id, display_name)?,
        arguments: std::env::args_os().collect(),
        registrations: Vec::new(),
    })
}

pub enum LaunchOutcome {
    Handled(i32),
    Desktop(Box<DesktopLaunch>),
}

pub struct DesktopLaunch {
    identity: ApplicationIdentity,
    arguments: Vec<OsString>,
    automation: automation::transport::LaunchOptions,
}

/// Configuration for the desktop path, supplied after early process dispatch.
pub struct DesktopOptions {
    pub config_directory: Option<PathBuf>,
    pub workspace_directory_name: Option<String>,
    pub desktop: bedtime_desktop::DesktopPlugins,
    pub icon_pack: bedtime_ui::icons::IconPack,
    pub theme: Option<bedtime_ui::appearance::UiTheme>,
}

impl Default for DesktopOptions {
    fn default() -> Self {
        let mut desktop = bedtime_desktop::DesktopPlugins {
            close_when_requested: false,
            ..Default::default()
        };
        desktop.window.title.clear();
        Self {
            config_directory: None,
            workspace_directory_name: None,
            desktop,
            icon_pack: Default::default(),
            theme: None,
        }
    }
}

impl LaunchBuilder {
    pub fn with_args(mut self, arguments: impl IntoIterator<Item = OsString>) -> Self {
        self.arguments = arguments.into_iter().collect();
        self
    }

    pub fn with_launchers(mut self, registration: LaunchRegistration) -> Self {
        self.registrations.push(registration);
        self
    }

    pub fn dispatch(self) -> io::Result<LaunchOutcome> {
        let arguments = self.arguments;
        let mut owners = HashSet::new();
        let mut selectors = Vec::new();
        for registration in &self.registrations {
            if registration.owner.is_empty() || !owners.insert(registration.owner) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Duplicate or empty launch owner",
                ));
            }
            for entry in &registration.entrypoints {
                let value = match &entry.selector {
                    ProcessSelector::ExecutablePrefix(value)
                    | ProcessSelector::FirstArgument(value) => value,
                };
                if value.is_empty()
                    || selectors.iter().any(|prior: &&ProcessSelector| {
                        match (*prior, &entry.selector) {
                            (
                                ProcessSelector::ExecutablePrefix(a),
                                ProcessSelector::ExecutablePrefix(b),
                            ) => a.starts_with(b) || b.starts_with(a),
                            (
                                ProcessSelector::FirstArgument(a),
                                ProcessSelector::FirstArgument(b),
                            ) => a == b,
                            _ => false,
                        }
                    })
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Duplicate, overlapping or empty launch selector",
                    ));
                }
                selectors.push(&entry.selector);
            }
        }
        // Executable helpers own their argument tail, even when it resembles a
        // different plugin's private flag.
        for executable_match in [true, false] {
            for registration in &self.registrations {
                for entry in &registration.entrypoints {
                    let matches = match &entry.selector {
                        ProcessSelector::ExecutablePrefix(prefix) if executable_match => arguments
                            .first()
                            .and_then(|arg| Path::new(arg).file_name())
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.starts_with(prefix)),
                        ProcessSelector::FirstArgument(flag) if !executable_match => {
                            arguments.get(1).is_some_and(|arg| arg == flag.as_str())
                        }
                        _ => false,
                    };
                    if matches {
                        if let Some(exit) = (entry.run)(arguments.clone())? {
                            return Ok(LaunchOutcome::Handled(exit));
                        }
                        return Ok(LaunchOutcome::Desktop(Box::new(DesktopLaunch::new(
                            self.identity,
                            arguments,
                        )?)));
                    }
                }
            }
        }
        Ok(LaunchOutcome::Desktop(Box::new(DesktopLaunch::new(
            self.identity,
            arguments,
        )?)))
    }

    pub fn run(
        self,
        options: DesktopOptions,
        configure: impl FnOnce(&mut App) -> io::Result<()>,
    ) -> io::Result<i32> {
        match self.dispatch()? {
            LaunchOutcome::Handled(exit) => Ok(exit),
            LaunchOutcome::Desktop(launch) => launch
                .run(options, configure)
                .map(|exit| if exit.is_error() { 1 } else { 0 }),
        }
    }
}

impl DesktopLaunch {
    fn new(identity: ApplicationIdentity, mut arguments: Vec<OsString>) -> io::Result<Self> {
        let automation = automation::transport::LaunchOptions::extract(&mut arguments)?;
        Ok(Self {
            identity,
            arguments,
            automation,
        })
    }

    pub fn arguments(&self) -> &[OsString] {
        self.arguments.get(1..).unwrap_or_default()
    }

    /// Build a real Bevy application. Normal plugins belong in `configure`.
    pub fn build(
        self,
        mut options: DesktopOptions,
        configure: impl FnOnce(&mut App) -> io::Result<()>,
    ) -> io::Result<App> {
        if options.desktop.window.title.is_empty() {
            options.desktop.window.title = self.identity.display_name.clone();
        }
        let paths = ApplicationPaths::new(
            &self.identity.id,
            options.config_directory,
            options.workspace_directory_name,
        )?;
        let settings_path = paths.config_dir.join("settings.json");
        let mut app = App::new();
        let application_id = self.identity.id.clone();
        app.insert_resource(paths)
            .insert_resource(ApplicationInfo(self.identity));
        let mut icons = bedtime_ui::icons::Icons::default();
        icons.replace_pack(&options.icon_pack)?;
        app.insert_non_send(icons);
        app.add_plugins(bedtime_core::FoundationPlugin {
            desktop: options.desktop,
        });
        bedtime_core::settings::initialize_settings(app.world_mut(), Some(settings_path))?;
        if let Some(theme) = options.theme {
            bedtime_core::settings::set_application_defaults(
                app.world_mut(),
                "bedtime.ui",
                [
                    ("font".into(), serde_json::json!(theme.fonts.family)),
                    ("fontSize".into(), serde_json::json!(theme.fonts.size)),
                    (
                        "background_opacity".into(),
                        serde_json::json!(theme.opacity),
                    ),
                    (
                        "ui_animations".into(),
                        serde_json::json!(theme.motion.enabled),
                    ),
                ],
            )?;
            app.insert_resource(bedtime_desktop::DesktopTheme(theme));
        }
        app.add_plugins(automation::AutomationPlugin).add_plugins(
            automation::transport::TransportPlugin {
                options: self.automation,
                application: application_id,
            },
        );
        host_automation::register(app.world_mut());
        configure(&mut app)?;
        bedtime_core::settings::finalize_settings(app.world_mut())?;
        Ok(app)
    }

    pub fn run(
        self,
        options: DesktopOptions,
        configure: impl FnOnce(&mut App) -> io::Result<()>,
    ) -> io::Result<AppExit> {
        let mut app = self.build(options, configure)?;
        Ok(app.run())
    }
}

#[derive(Resource)]
pub struct ApplicationInfo(pub ApplicationIdentity);

#[cfg(test)]
mod tests {
    use super::*;

    fn handled(args: Vec<OsString>) -> io::Result<Option<i32>> {
        assert_eq!(args[2], "opaque");
        Ok(Some(7))
    }

    fn launcher(owner: &'static str, selector: ProcessSelector) -> LaunchRegistration {
        LaunchRegistration {
            owner,
            entrypoints: vec![ProcessEntrypoint {
                selector,
                run: handled,
            }],
        }
    }

    #[test]
    fn helpers_dispatch_without_constructing_bevy_or_loading_settings() {
        let result = awaken("test", "Test")
            .unwrap()
            .with_args(["test", "--helper", "opaque"].map(OsString::from))
            .with_launchers(launcher(
                "test",
                ProcessSelector::FirstArgument("--helper".into()),
            ))
            .dispatch()
            .unwrap();
        assert!(matches!(result, LaunchOutcome::Handled(7)));
    }

    #[test]
    fn duplicate_owners_are_rejected_before_dispatch() {
        let result = awaken("test", "Test")
            .unwrap()
            .with_args([OsString::from("test")])
            .with_launchers(launcher(
                "test",
                ProcessSelector::FirstArgument("--one".into()),
            ))
            .with_launchers(launcher(
                "test",
                ProcessSelector::FirstArgument("--two".into()),
            ))
            .dispatch();
        assert!(result.is_err());
    }

    #[test]
    fn executable_helper_keeps_its_opaque_argument_tail() {
        let result = awaken("test", "Test")
            .unwrap()
            .with_args(["/tmp/helpers/+custom", "--debugger", "opaque"].map(OsString::from))
            .with_launchers(launcher(
                "test",
                ProcessSelector::ExecutablePrefix("+".into()),
            ))
            .with_launchers(LaunchRegistration {
                owner: "debugger",
                entrypoints: vec![ProcessEntrypoint {
                    selector: ProcessSelector::FirstArgument("--debugger".into()),
                    run: |_| panic!("The helper's argument must not select another handler"),
                }],
            })
            .dispatch()
            .unwrap();
        assert!(matches!(result, LaunchOutcome::Handled(7)));
    }

    #[test]
    fn overlapping_selectors_fail_before_any_process_handler() {
        let result = awaken("test", "Test")
            .unwrap()
            .with_args([OsString::from("+custom")])
            .with_launchers(launcher(
                "test",
                ProcessSelector::ExecutablePrefix("+".into()),
            ))
            .with_launchers(launcher(
                "other",
                ProcessSelector::ExecutablePrefix("+custom".into()),
            ))
            .dispatch();
        assert!(result.is_err());
    }
}
