//! An owned entrypoint for Bed desktop applications.
//! Early process handlers run before configuration and Bevy construction.
pub use bed_api as api;
pub use bed_api::launch::{
    ApplicationIdentity, ApplicationPaths, LaunchRegistration, ProcessEntrypoint, ProcessSelector,
};
pub use bed_core;
pub use bed_desktop;
pub use bed_ui as ui;
pub use bevy;
use bevy::prelude::*;
use std::{
    collections::HashSet,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

pub struct Application {
    identity: ApplicationIdentity,
    registrations: Vec<LaunchRegistration>,
}

pub enum LaunchOutcome {
    Handled(i32),
    Desktop(Box<DesktopLaunch>),
}

pub struct DesktopLaunch {
    identity: ApplicationIdentity,
    arguments: Vec<OsString>,
    registrations: Vec<LaunchRegistration>,
    config_directory: Option<PathBuf>,
    workspace_directory_name: Option<String>,
    desktop: bed_desktop::DesktopPlugins,
    icon_directories: bed_ui::icons::IconDirectories,
    presentation_mode: bed_core::CorePresentationMode,
    theme: Option<bed_ui::appearance::UiTheme>,
}

impl Application {
    pub fn new(id: impl Into<String>, display_name: impl Into<String>) -> io::Result<Self> {
        Ok(Self {
            identity: ApplicationIdentity::new(id, display_name)?,
            registrations: Vec::new(),
        })
    }

    pub fn register(mut self, registration: LaunchRegistration) -> Self {
        self.registrations.push(registration);
        self
    }

    pub fn dispatch(
        self,
        arguments: impl IntoIterator<Item = OsString>,
    ) -> io::Result<LaunchOutcome> {
        let arguments = arguments.into_iter().collect::<Vec<_>>();
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
                            self.registrations,
                        ))));
                    }
                }
            }
        }
        Ok(LaunchOutcome::Desktop(Box::new(DesktopLaunch::new(
            self.identity,
            arguments,
            self.registrations,
        ))))
    }

    pub fn run(self, configure: impl FnOnce(&mut App) -> io::Result<()>) -> io::Result<i32> {
        match self.dispatch(std::env::args_os())? {
            LaunchOutcome::Handled(exit) => Ok(exit),
            LaunchOutcome::Desktop(launch) => launch
                .run(configure)
                .map(|exit| if exit.is_error() { 1 } else { 0 }),
        }
    }
}

impl DesktopLaunch {
    fn new(
        identity: ApplicationIdentity,
        arguments: Vec<OsString>,
        registrations: Vec<LaunchRegistration>,
    ) -> Self {
        let mut desktop = bed_desktop::DesktopPlugins {
            close_when_requested: false,
            ..Default::default()
        };
        desktop.window.title = identity.display_name.clone();
        Self {
            identity,
            arguments,
            registrations,
            config_directory: None,
            workspace_directory_name: None,
            desktop,
            icon_directories: Default::default(),
            presentation_mode: Default::default(),
            theme: None,
        }
    }
    pub fn arguments(&self) -> &[OsString] {
        self.arguments.get(1..).unwrap_or_default()
    }
    pub fn options(mut self, desktop: bed_desktop::DesktopPlugins) -> Self {
        self.desktop = desktop;
        self
    }
    pub fn config_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_directory = Some(path.into());
        self
    }
    pub fn workspace_directory_name(mut self, name: impl Into<String>) -> Self {
        self.workspace_directory_name = Some(name.into());
        self
    }
    pub fn icon_directories(mut self, directories: bed_ui::icons::IconDirectories) -> Self {
        self.icon_directories = directories;
        self
    }
    /// Let an application supply its own chrome and document commands.
    pub fn presentation_mode(mut self, mode: bed_core::CorePresentationMode) -> Self {
        self.presentation_mode = mode;
        self
    }
    /// Supply one theme and application defaults for its configurable UI values.
    pub fn theme(mut self, theme: bed_ui::appearance::UiTheme) -> Self {
        self.theme = Some(theme);
        self
    }

    pub fn build(self, configure: impl FnOnce(&mut App) -> io::Result<()>) -> io::Result<App> {
        let paths = ApplicationPaths::new(
            &self.identity.id,
            self.config_directory,
            self.workspace_directory_name,
        )?;
        let settings_path = paths.config_dir.join("settings.json");
        let mut app = App::new();
        app.insert_resource(paths)
            .insert_resource(ApplicationInfo(self.identity));
        app.insert_non_send(self.icon_directories);
        app.insert_resource(self.presentation_mode);
        app.add_plugins(bed_core::FoundationPlugin {
            desktop: self.desktop,
        });
        bed_core::settings::initialize_settings(app.world_mut(), Some(settings_path))?;
        if let Some(theme) = self.theme {
            bed_core::settings::set_application_defaults(
                app.world_mut(),
                "bed.ui",
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
            app.insert_resource(bed_desktop::DesktopTheme(theme));
        }
        for registration in self.registrations {
            (registration.install)(&mut app);
        }
        configure(&mut app)?;
        bed_core::settings::finalize_settings(app.world_mut())?;
        Ok(app)
    }

    pub fn run(self, configure: impl FnOnce(&mut App) -> io::Result<()>) -> io::Result<AppExit> {
        let mut app = self.build(configure)?;
        Ok(app.run())
    }
}

#[derive(Resource)]
pub struct ApplicationInfo(pub ApplicationIdentity);

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    fn handled(args: Vec<OsString>) -> io::Result<Option<i32>> {
        assert_eq!(args[2], "opaque");
        Ok(Some(7))
    }
    fn feature(selector: ProcessSelector) -> LaunchRegistration {
        LaunchRegistration {
            owner: "test",
            entrypoints: vec![ProcessEntrypoint {
                selector,
                run: handled,
            }],
            install: Box::new(|_| {
                INSTALLED.store(true, Ordering::SeqCst);
            }),
        }
    }
    #[test]
    fn helpers_dispatch_before_any_feature_or_desktop_installation() {
        INSTALLED.store(false, Ordering::SeqCst);
        let result = Application::new("test", "Test")
            .unwrap()
            .register(feature(ProcessSelector::FirstArgument("--helper".into())))
            .dispatch(["test", "--helper", "opaque"].map(OsString::from))
            .unwrap();
        assert!(matches!(result, LaunchOutcome::Handled(7)));
        assert!(!INSTALLED.load(Ordering::SeqCst));
    }
    #[test]
    fn duplicate_owners_are_rejected_before_dispatch() {
        let result = Application::new("test", "Test")
            .unwrap()
            .register(feature(ProcessSelector::FirstArgument("--one".into())))
            .register(feature(ProcessSelector::FirstArgument("--two".into())))
            .dispatch([OsString::from("test")]);
        assert!(result.is_err());
    }
    #[test]
    fn executable_helper_keeps_its_opaque_argument_tail() {
        let application = Application::new("test", "Test")
            .unwrap()
            .register(feature(ProcessSelector::ExecutablePrefix("+".into())))
            .register(LaunchRegistration {
                owner: "debugger",
                entrypoints: vec![ProcessEntrypoint {
                    selector: ProcessSelector::FirstArgument("--debugger".into()),
                    run: |_| panic!("The helper's argument must not select another handler"),
                }],
                install: Box::new(|_| panic!("Early helpers must not install desktop plugins")),
            });
        assert!(matches!(
            application
                .dispatch(["/tmp/helpers/+custom", "--debugger", "opaque"].map(OsString::from))
                .unwrap(),
            LaunchOutcome::Handled(7)
        ));
    }
    #[test]
    fn overlapping_selectors_fail_before_any_process_handler() {
        let application = Application::new("test", "Test")
            .unwrap()
            .register(feature(ProcessSelector::ExecutablePrefix("+".into())))
            .register(LaunchRegistration {
                owner: "other",
                entrypoints: vec![ProcessEntrypoint {
                    selector: ProcessSelector::ExecutablePrefix("+custom".into()),
                    run: |_| panic!("Conflicting entrypoints must not execute"),
                }],
                install: Box::new(|_| {}),
            });
        assert!(application.dispatch([OsString::from("+custom")]).is_err());
    }
}
