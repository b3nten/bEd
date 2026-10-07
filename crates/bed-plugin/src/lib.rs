//! Native plugin contracts. The application owns documents and processes typed requests.
//! Plugins are explicitly linked Rust crates; this is not a dynamic-library ABI.
pub use bed_core::editor_state::DocumentKind;
use bed_core::{buffer::text_buffer::Snapshot, identity::DocumentId};
use dear_imgui_rs::{TextureId, Ui};
use serde_json::Value;
use std::{
    any::Any,
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

pub type Revision = (u64, u64);

#[derive(Clone, Debug)]
pub struct PluginDocument {
    pub id: DocumentId,
    pub path: String,
    pub kind: DocumentKind,
    pub language_id: String,
    pub revision: Revision,
    pub dirty: bool,
    pub bytes: Arc<[u8]>,
    /// Cheap immutable rope snapshot for source-processing workers.
    pub text: Option<Snapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureHandle(pub u64);
impl TextureHandle {
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionContext {
    pub document: DocumentId,
    pub revision: Revision,
    pub ranges: Vec<std::ops::Range<usize>>,
    pub text: String,
}

/// Captured at the originating UI surface, never reinterpreted using later focus.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandContext {
    pub path: Option<String>,
    pub document: Option<DocumentId>,
    pub revision: Option<Revision>,
    pub selection: Option<SelectionContext>,
}

pub struct HostContext<'a> {
    pub documents: &'a [PluginDocument],
    pub active_document: Option<DocumentId>,
    pub settings: &'a Value,
    pub textures: &'a HashMap<TextureHandle, TextureId>,
    pub animations: bool,
    pub workspace: u64,
    /// Read-only LSP data grouped by document path. No client/process handles escape.
    pub diagnostics: &'a Value,
}
impl HostContext<'_> {
    pub fn document(&self, id: DocumentId) -> Option<&PluginDocument> {
        self.documents.iter().find(|document| document.id == id)
    }
    pub fn active(&self) -> Option<&PluginDocument> {
        self.document(self.active_document?)
    }
    pub fn settings_for(&self, plugin: &str) -> &Value {
        &self.settings[plugin]
    }
    pub fn texture(&self, handle: TextureHandle) -> Option<TextureId> {
        self.textures.get(&handle).copied()
    }
}

#[derive(Debug)]
pub enum HostRequest {
    OpenPanel {
        panel_type: String,
        document: Option<DocumentId>,
        state: Value,
    },
    OpenFile {
        path: String,
        viewer: Option<String>,
    },
    OpenFileDialog {
        viewer: Option<String>,
    },
    Navigate {
        document: DocumentId,
        byte_offset: usize,
        revision: Revision,
    },
    ApplyEdits {
        document: DocumentId,
        revision: Revision,
        edits: Vec<bed_session::editor_session::ByteEdit>,
    },
    Save {
        document: DocumentId,
    },
    Undo {
        document: DocumentId,
    },
    Redo {
        document: DocumentId,
    },
    SetSetting {
        plugin: String,
        key: String,
        value: Value,
    },
    UploadTexture {
        handle: TextureHandle,
        size: [u32; 2],
        rgba: Arc<[u8]>,
    },
    ReleaseTexture {
        handle: TextureHandle,
    },
    Notify {
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuSlot {
    File,
    Folder,
    TreeBackground,
    TextSelection,
    Application,
}

#[derive(Clone, Debug)]
pub struct CommandDescriptor {
    pub id: &'static str,
    pub plugin: &'static str,
    pub label: &'static str,
    pub icon: Option<&'static str>,
}
#[derive(Clone, Debug)]
pub struct MenuContribution {
    pub slot: MenuSlot,
    pub command: &'static str,
}
#[derive(Clone, Debug)]
pub struct ToolbarContribution {
    pub command: &'static str,
}
#[derive(Clone, Debug)]
pub struct SettingsSection {
    pub id: &'static str,
    pub plugin: &'static str,
    pub label: &'static str,
}
#[derive(Clone, Debug)]
pub struct PanelType {
    pub id: &'static str,
    pub plugin: &'static str,
    pub label: &'static str,
}
#[derive(Clone, Debug)]
pub struct FileViewer {
    pub id: &'static str,
    pub plugin: &'static str,
    pub label: &'static str,
    pub panel_type: &'static str,
    /// Lowercase extensions without the dot. Matching ignores ASCII case.
    pub extensions: &'static [&'static str],
    pub kind: DocumentKind,
}

#[derive(Default)]
pub struct Registry {
    pub commands: Vec<CommandDescriptor>,
    pub menus: Vec<MenuContribution>,
    pub toolbar: Vec<ToolbarContribution>,
    pub settings: Vec<SettingsSection>,
    pub panels: Vec<PanelType>,
    pub viewers: Vec<FileViewer>,
    plugins: std::collections::HashSet<&'static str>,
}
impl Registry {
    pub fn register(&mut self, plugin: &dyn Plugin) -> Result<(), String> {
        if self.plugins.contains(plugin.id()) {
            return Err(format!("Plugin already registered: {}", plugin.id()));
        }
        let mut pending = Registry::default();
        plugin.register(&mut Registrar {
            owner: plugin.id(),
            registry: &mut pending,
        });
        // Validate the entire contribution before changing the registry.
        let mut ids = std::collections::HashSet::new();
        for id in pending
            .commands
            .iter()
            .map(|x| x.id)
            .chain(pending.panels.iter().map(|x| x.id))
            .chain(pending.viewers.iter().map(|x| x.id))
            .chain(pending.settings.iter().map(|x| x.id))
        {
            if !id.starts_with(&format!("{}.", plugin.id())) || !ids.insert(id) {
                return Err(format!("Invalid or duplicate contribution ID: {id}"));
            }
            if self.commands.iter().any(|x| x.id == id)
                || self.panels.iter().any(|x| x.id == id)
                || self.viewers.iter().any(|x| x.id == id)
                || self.settings.iter().any(|x| x.id == id)
            {
                return Err(format!("Contribution already registered: {id}"));
            }
        }
        for command in pending
            .menus
            .iter()
            .map(|x| x.command)
            .chain(pending.toolbar.iter().map(|x| x.command))
        {
            if !pending.commands.iter().any(|x| x.id == command) {
                return Err(format!("Missing contributed command: {command}"));
            }
        }
        for viewer in &pending.viewers {
            if !pending.panels.iter().any(|x| x.id == viewer.panel_type) {
                return Err(format!("Missing viewer panel: {}", viewer.panel_type));
            }
        }
        self.commands.extend(pending.commands);
        self.menus.extend(pending.menus);
        self.toolbar.extend(pending.toolbar);
        self.settings.extend(pending.settings);
        self.panels.extend(pending.panels);
        self.viewers.extend(pending.viewers);
        self.plugins.insert(plugin.id());
        Ok(())
    }
    pub fn command(&self, id: &str) -> Option<&CommandDescriptor> {
        self.commands.iter().find(|entry| entry.id == id)
    }
    pub fn panel(&self, id: &str) -> Option<&PanelType> {
        self.panels.iter().find(|entry| entry.id == id)
    }
    pub fn viewer(&self, id: &str) -> Option<&FileViewer> {
        self.viewers.iter().find(|entry| entry.id == id)
    }
    /// Registration order resolves conflicts; no generic wildcard masks text fallback.
    pub fn viewer_for_path(&self, path: &str) -> Option<&FileViewer> {
        let filename = path.rsplit(['/', '\\']).next()?;
        let extension = filename.rsplit_once('.')?.1;
        self.viewers.iter().find(|viewer| {
            viewer
                .extensions
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(extension))
        })
    }
}

pub struct Registrar<'a> {
    owner: &'static str,
    registry: &'a mut Registry,
}
impl Registrar<'_> {
    pub fn command(&mut self, id: &'static str, label: &'static str, icon: Option<&'static str>) {
        self.registry.commands.push(CommandDescriptor {
            id,
            plugin: self.owner,
            label,
            icon,
        });
    }
    pub fn menu(&mut self, slot: MenuSlot, command: &'static str) {
        self.registry.menus.push(MenuContribution { slot, command });
    }
    pub fn toolbar(&mut self, command: &'static str) {
        self.registry.toolbar.push(ToolbarContribution { command });
    }
    pub fn settings(&mut self, id: &'static str, label: &'static str) {
        self.registry.settings.push(SettingsSection {
            id,
            plugin: self.owner,
            label,
        });
    }
    pub fn panel(&mut self, id: &'static str, label: &'static str) {
        self.registry.panels.push(PanelType {
            id,
            plugin: self.owner,
            label,
        });
    }
    pub fn viewer(
        &mut self,
        id: &'static str,
        label: &'static str,
        panel_type: &'static str,
        extensions: &'static [&'static str],
        kind: DocumentKind,
    ) {
        self.registry.viewers.push(FileViewer {
            id,
            plugin: self.owner,
            label,
            panel_type,
            extensions,
            kind,
        });
    }
}

pub trait Plugin: Any {
    fn id(&self) -> &'static str;
    fn register(&self, registrar: &mut Registrar<'_>);
    fn tick(&mut self, _host: &HostContext<'_>, _requests: &mut Vec<HostRequest>) {}
    fn command_enabled(
        &self,
        _command: &str,
        _context: &CommandContext,
        _host: &HostContext<'_>,
    ) -> bool {
        true
    }
    fn command(
        &mut self,
        command: &str,
        context: &CommandContext,
        host: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    );
    fn create_panel(
        &mut self,
        panel_type: &str,
        document: Option<DocumentId>,
        state: &Value,
    ) -> Result<Box<dyn PluginPanel>, String>;
    fn draw_settings(&mut self, _section: &str, _ui: &Ui, _settings: &mut Value) -> bool {
        false
    }
    fn draw_popups(&mut self, _ui: &Ui, _host: &HostContext<'_>, _requests: &mut Vec<HostRequest>) {
    }
    fn as_any(&self) -> &dyn Any;
}

pub trait PluginPanel: Any {
    fn title(&self, host: &HostContext<'_>) -> String;
    fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>);
    fn save_state(&self) -> Value {
        Value::Null
    }
    fn attached_document(&self) -> Option<DocumentId> {
        None
    }
    fn close(&mut self, _requests: &mut Vec<HostRequest>) {}
    fn as_any(&self) -> &dyn Any;
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Example {
        id: &'static str,
        panel: &'static str,
        viewer: &'static str,
        command: &'static str,
    }
    impl Plugin for Example {
        fn id(&self) -> &'static str {
            self.id
        }
        fn register(&self, registrar: &mut Registrar<'_>) {
            registrar.panel(self.panel, "Example");
            registrar.viewer(
                self.viewer,
                "Example",
                self.panel,
                &["abc"],
                DocumentKind::Bytes,
            );
            registrar.command(self.command, "Open", None);
            registrar.menu(MenuSlot::File, self.command);
        }
        fn command(
            &mut self,
            _: &str,
            _: &CommandContext,
            _: &HostContext<'_>,
            _: &mut Vec<HostRequest>,
        ) {
        }
        fn create_panel(
            &mut self,
            _: &str,
            _: Option<DocumentId>,
            _: &Value,
        ) -> Result<Box<dyn PluginPanel>, String> {
            Err("Not needed".into())
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }
    #[test]
    fn matching_is_case_insensitive_and_conflicts_keep_registration_order() {
        let mut registry = Registry::default();
        registry
            .register(&Example {
                id: "first",
                panel: "first.panel",
                viewer: "first.viewer",
                command: "first.open",
            })
            .unwrap();
        registry
            .register(&Example {
                id: "second",
                panel: "second.panel",
                viewer: "second.viewer",
                command: "second.open",
            })
            .unwrap();
        assert_eq!(
            registry
                .viewer_for_path("/a/folder.name/file.ABC")
                .unwrap()
                .id,
            "first.viewer"
        );
        assert!(registry.viewer_for_path("/a/folder.abc/file").is_none());
        assert!(registry.viewer_for_path("file.unknown").is_none());
    }
    #[test]
    fn invalid_registration_is_atomic_and_duplicate_ids_are_rejected() {
        let mut registry = Registry::default();
        let valid = Example {
            id: "example",
            panel: "example.panel",
            viewer: "example.viewer",
            command: "example.open",
        };
        registry.register(&valid).unwrap();
        assert!(registry.register(&valid).is_err());
        assert!(
            registry
                .register(&Example {
                    id: "example",
                    panel: "example.other-panel",
                    viewer: "example.other-viewer",
                    command: "example.other-open"
                })
                .is_err()
        );
        assert_eq!(registry.viewers.len(), 1);
        assert!(
            registry
                .register(&Example {
                    id: "broken",
                    panel: "example.stolen",
                    viewer: "broken.viewer",
                    command: "broken.open"
                })
                .is_err()
        );
        assert_eq!(registry.commands.len(), 1);
    }
}
