//! Settings panels edit the shared settings service through a scoped borrow.
use crate::{SettingsUi, SettingsWindowState};
use bed_document_session::{DocumentId, editor::Editor};
use bed_settings::Settings;
use bed_ui::icons::Icons;
use bed_workbench_api::{
    CommandContext, HostContext, HostRequest, Module, ModulePanel, ModuleServices, PanelPlacement,
    Registrar,
};
use dear_imgui_rs::Ui;
use serde_json::Value;
use std::{any::Any, io};

pub const MODULE_ID: &str = "bed.settings";
pub const PANEL_ID: &str = "bed.settings.panel";
pub const NEW_COMMAND: &str = "bed.settings.new";

pub struct SettingsModule;
impl Module for SettingsModule {
    fn id(&self) -> &'static str {
        MODULE_ID
    }
    fn register(&self, registrar: &mut Registrar<'_>) {
        registrar.panel_options(
            PANEL_ID,
            "Settings",
            false,
            PanelPlacement::Center,
            Some("settings"),
        );
        registrar.command(NEW_COMMAND, "New Settings", Some("settings"));
    }
    fn command(
        &mut self,
        command: &str,
        _: &CommandContext,
        _: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        if command == NEW_COMMAND {
            requests.push(HostRequest::OpenPanel {
                panel_type: PANEL_ID.into(),
                document: None,
                state: Value::Null,
            });
        }
    }
    fn create_panel(
        &mut self,
        panel_type: &str,
        document: Option<DocumentId>,
        _: &Value,
    ) -> Result<Box<dyn ModulePanel>, String> {
        if panel_type != PANEL_ID || document.is_some() {
            return Err("Settings requires its own panel without a document".into());
        }
        Ok(Box::new(SettingsPanel {
            scratch: Editor::new(),
            ui_state: SettingsWindowState::default(),
        }))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub struct SettingsPanel {
    scratch: Editor,
    ui_state: SettingsWindowState,
}
impl ModulePanel for SettingsPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        "Settings".into()
    }
    fn draw(&mut self, ui: &Ui, _: &HostContext<'_>, _: &mut Vec<HostRequest>) {
        ui.text_disabled("Settings panel requires the application settings service");
    }
    fn draw_with_services(
        &mut self,
        ui: &Ui,
        _: &HostContext<'_>,
        services: &mut ModuleServices<'_>,
        _: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        let settings = services
            .resources
            .take::<Settings>()
            .ok_or_else(|| io::Error::other("Application settings service is unavailable"))?;
        let icons = services.resources.take::<Icons>();
        let mut contributions = services.settings_ui.as_deref_mut();
        settings.draw_tab_with_state(
            ui,
            &mut self.ui_state,
            &mut self.scratch,
            icons.as_deref(),
            &mut |ui, value| {
                contributions
                    .as_deref_mut()
                    .is_some_and(|provider| provider.draw(ui, value))
            },
        );
        Ok(())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_workbench_api::{
        FileDialogService, Registry, ScopedServices, SettingsContributions, TerminalLaunch,
        TerminalService,
    };
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
    };

    struct Terminals;
    impl TerminalService for Terminals {
        fn spawn(&mut self, _: TerminalLaunch) -> io::Result<(u64, u32)> {
            Err(io::Error::other("unused"))
        }
        fn stop(&mut self, _: u64) {}
        fn release(&mut self, _: u64) {}
    }
    struct Dialogs;
    impl FileDialogService for Dialogs {
        fn pick_file(&mut self, _: &Path, _: &[&str]) -> Option<PathBuf> {
            None
        }
    }
    struct Sections {
        calls: usize,
    }
    impl SettingsContributions for Sections {
        fn draw(&mut self, _: &Ui, settings: &mut Value) -> bool {
            self.calls += 1;
            settings["plugins"]["fixture"] = serde_json::json!({ "enabled": true });
            true
        }
    }

    #[test]
    fn settings_panels_register_without_attaching_an_editor_document() {
        let mut module = SettingsModule;
        let mut registry = Registry::default();
        registry.register(&module).unwrap();
        let descriptor = registry.panel(PANEL_ID).unwrap();
        assert_eq!(descriptor.legacy_kind, Some("settings"));
        assert!(!descriptor.singleton);
        let panel = module.create_panel(PANEL_ID, None, &Value::Null).unwrap();
        assert_eq!(panel.attached_document(), None);
        assert_eq!(panel.view_id(), None);
        assert!(
            module
                .create_panel(PANEL_ID, Some(DocumentId::next()), &Value::Null)
                .is_err()
        );
    }

    #[test]
    fn settings_panel_edits_and_persists_the_live_service_with_contributed_sections() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let directory = crate::test_support::TempDir::new();
        let mut settings = Settings::with_paths(
            directory.path("config"),
            PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        )
        .unwrap();
        settings.settings["plugins"] = serde_json::json!({});
        let mut panel = SettingsModule
            .create_panel(PANEL_ID, None, &Value::Null)
            .unwrap();
        panel
            .as_any_mut()
            .downcast_mut::<SettingsPanel>()
            .unwrap()
            .ui_state
            .category = crate::SettingsCategory::Extensions;
        let mut context = dear_imgui_rs::Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
            [1200.0, 1000.0],
            1.0 / 60.0,
        ));
        let ui = context.frame();
        let mut documents = bed_document_session::EditorSession::new();
        let mut terminals = Terminals;
        let mut dialogs = Dialogs;
        let mut sections = Sections { calls: 0 };
        let empty = Value::Null;
        let textures = HashMap::new();
        let host = HostContext {
            documents: &[],
            active_document: None,
            settings: &empty,
            textures: &textures,
            animations: false,
            workspace: documents.workspace_id().0,
            diagnostics: &empty,
        };
        {
            let mut resources = ScopedServices::default();
            resources.insert(&mut settings);
            let mut services = ModuleServices {
                documents: &mut documents,
                terminals: &mut terminals,
                dialogs: &mut dialogs,
                project_root: "",
                active_view: None,
                resources,
                settings_ui: Some(&mut sections),
            };
            ui.window("Registered settings panel")
                .size([1100.0, 900.0], dear_imgui_rs::Condition::Always)
                .build(|| {
                    panel
                        .draw_with_services(ui, &host, &mut services, &mut Vec::new())
                        .unwrap()
                });
        }
        drop(context.render_legacy());
        assert_eq!(sections.calls, 1);
        assert_eq!(settings.settings["plugins"]["fixture"]["enabled"], true);
        let persisted = bed_settings::read_json(&settings.settings_path).unwrap();
        assert_eq!(persisted["plugins"]["fixture"]["enabled"], true);
    }
}
