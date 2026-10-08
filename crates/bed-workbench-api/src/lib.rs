//! Workbench module and panel contracts, shared by built-in features and plugins.
//! Plugins are explicitly linked Rust crates; this is not a dynamic-library ABI.
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod workspace;
pub use bed_editing::editor_state::DocumentKind;
use bed_editing::{buffer::text_buffer::Snapshot, identity::DocumentId};
use dear_imgui_rs::{TextureId, Ui};
use serde_json::Value;
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

pub type Revision = (u64, u64);

/// Correlates a queued edit with its acknowledgement without exposing host panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EditToken(pub u64);
impl EditToken {
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

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

/// Scoped native services. Document ownership and mutations remain in the session.
/// Feature-specific extension contracts live in the feature that owns them.
pub struct ModuleServices<'a> {
    pub documents: &'a mut bed_document_session::editor_session::EditorSession,
    pub terminals: &'a mut dyn TerminalService,
    pub dialogs: &'a mut dyn FileDialogService,
    pub project_root: &'a str,
    pub active_view: Option<bed_document_session::editor_session::ViewId>,
    pub resources: ScopedServices<'a>,
    pub settings_ui: Option<&'a mut dyn SettingsContributions>,
}

/// Optional concrete native services, borrowed for one callback. Taking a service
/// removes it from the scope, so callers can safely borrow distinct services
/// together. This is dependency injection, not a module messaging protocol.
#[derive(Default)]
pub struct ScopedServices<'a> {
    entries: HashMap<TypeId, &'a mut dyn Any>,
}
impl<'a> ScopedServices<'a> {
    pub fn insert<T: Any>(&mut self, service: &'a mut T) {
        self.entries.insert(TypeId::of::<T>(), service);
    }
    pub fn take<T: Any>(&mut self) -> Option<&'a mut T> {
        self.entries.remove(&TypeId::of::<T>())?.downcast_mut()
    }
}

/// Registered settings sections draw in the settings feature's UI scope.
pub trait SettingsContributions {
    fn draw(&mut self, ui: &Ui, values: &mut Value) -> bool;
}

#[derive(Clone, Debug)]
pub struct TerminalLaunch {
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, Option<String>>,
    pub title: String,
}

/// Terminal execution is a workspace service; panels are separate views of sessions.
pub trait TerminalService {
    fn new_shell(&mut self, _cwd: Option<&Path>) -> io::Result<u64> {
        Err(io::Error::other("Shell service unavailable"))
    }
    fn render(&mut self, _ui: &Ui, _id: u64) -> io::Result<()> {
        Err(io::Error::other("Terminal presentation unavailable"))
    }
    fn title(&self, _id: u64) -> Option<String> {
        None
    }
    fn working_directory(&self, _id: u64) -> Option<PathBuf> {
        None
    }
    fn is_command(&self, _id: u64) -> bool {
        false
    }
    fn focus(&mut self, _id: u64) {}
    fn close_panel(&mut self, id: u64) {
        self.release(id);
    }
    fn spawn(&mut self, launch: TerminalLaunch) -> io::Result<(u64, u32)>;
    fn stop(&mut self, id: u64);
    fn release(&mut self, id: u64);
}

/// Native file picking stays with the application host, not feature panels.
pub trait FileDialogService {
    fn pick_file(&mut self, directory: &Path, extensions: &[&str]) -> Option<PathBuf>;
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
    FocusView {
        view: bed_document_session::editor_session::ViewId,
    },
    RevealSource {
        path: String,
        row: i32,
        column: i32,
        center: bool,
    },
    ShowTerminal {
        id: u64,
    },
    Invalidate,

    ShowPanel {
        panel_type: String,
        document: Option<DocumentId>,
        state: Value,
        action: Option<PanelAction>,
    },
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
        edits: Vec<bed_document_session::editor_session::ByteEdit>,
    },
    ApplyEditsWithResult {
        token: EditToken,
        document: DocumentId,
        revision: Revision,
        edits: Vec<bed_document_session::editor_session::ByteEdit>,
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PanelPlacement {
    #[default]
    Center,
    Sidebar,
    Bottom,
}

#[derive(Clone, Debug)]
pub struct PanelType {
    pub singleton: bool,
    /// Placement when constructing the initial workspace layout.
    pub placement: PanelPlacement,
    /// Placement for a panel opened into an existing layout. Center uses the
    /// largest content group, preserving user-created splits.
    pub open_placement: PanelPlacement,
    /// An old persisted panel kind, unique across panel types. New workspace
    /// state should use the canonical `id` instead.
    pub legacy_kind: Option<&'static str>,
    pub id: &'static str,
    pub plugin: &'static str,
    pub label: &'static str,
}
#[derive(Clone, Debug)]
pub struct FileViewer {
    /// The default for this document kind when no file extension matches. At
    /// most one module can provide a fallback for each kind.
    pub fallback: bool,
    /// Explicit persisted viewer names accepted alongside `id`. These may be
    /// outside the module namespace, but cannot shadow any contribution ID.
    pub aliases: &'static [&'static str],
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
    fn contribution_ids(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.commands
            .iter()
            .map(|entry| entry.id)
            .chain(self.panels.iter().map(|entry| entry.id))
            .chain(self.viewers.iter().map(|entry| entry.id))
            .chain(self.settings.iter().map(|entry| entry.id))
    }

    /// Register one complete module. Invalid contributions leave the registry
    /// untouched, including the module ID, so corrected registration can retry.
    pub fn register(&mut self, module: &dyn Module) -> Result<(), String> {
        let owner = module.id();
        if !valid_id(owner) {
            return Err(format!("Invalid module ID: {owner}"));
        }
        if self.plugins.contains(owner) {
            return Err(format!("Module already registered: {owner}"));
        }
        let mut pending = Registry::default();
        module.register(&mut Registrar {
            owner,
            registry: &mut pending,
        });
        // Validate the entire contribution before changing the registry.
        let mut ids = std::collections::HashSet::new();
        let namespace = format!("{owner}.");
        for id in pending.contribution_ids() {
            if !valid_id(id) || !id.starts_with(&namespace) || !ids.insert(id) {
                return Err(format!("Invalid or duplicate contribution ID: {id}"));
            }
            if self.contribution_ids().any(|existing| existing == id)
                || self
                    .viewers
                    .iter()
                    .any(|viewer| viewer.aliases.contains(&id))
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
        let mut aliases = std::collections::HashSet::new();
        let mut fallback_kinds = Vec::new();
        for viewer in &pending.viewers {
            if viewer.fallback {
                if fallback_kinds.contains(&viewer.kind)
                    || self.default_viewer(viewer.kind).is_some()
                {
                    return Err(format!(
                        "Fallback viewer already registered for {:?}",
                        viewer.kind
                    ));
                }
                fallback_kinds.push(viewer.kind);
            }
            for &alias in viewer.aliases {
                if !valid_id(alias)
                    || ids.contains(alias)
                    || !aliases.insert(alias)
                    || self.contribution_ids().any(|existing| existing == alias)
                    || self.viewer(alias).is_some()
                {
                    return Err(format!("Invalid or duplicate viewer alias: {alias}"));
                }
            }
        }
        let mut legacy_kinds = std::collections::HashSet::new();
        for panel in &pending.panels {
            if self
                .panels
                .iter()
                .any(|existing| existing.legacy_kind == Some(panel.id))
            {
                return Err(format!(
                    "Panel ID conflicts with a legacy panel kind: {}",
                    panel.id
                ));
            }
            if let Some(legacy_kind) = panel.legacy_kind
                && (!valid_id(legacy_kind)
                    || !legacy_kinds.insert(legacy_kind)
                    || pending.panels.iter().any(|other| other.id == legacy_kind)
                    || self.panels.iter().any(|other| {
                        other.id == legacy_kind || other.legacy_kind == Some(legacy_kind)
                    }))
            {
                return Err(format!(
                    "Invalid or duplicate legacy panel kind: {legacy_kind}"
                ));
            }
        }
        self.commands.extend(pending.commands);
        self.menus.extend(pending.menus);
        self.toolbar.extend(pending.toolbar);
        self.settings.extend(pending.settings);
        self.panels.extend(pending.panels);
        self.viewers.extend(pending.viewers);
        self.plugins.insert(owner);
        Ok(())
    }
    pub fn command(&self, id: &str) -> Option<&CommandDescriptor> {
        self.commands.iter().find(|entry| entry.id == id)
    }
    pub fn panel(&self, id: &str) -> Option<&PanelType> {
        self.panels.iter().find(|entry| entry.id == id)
    }
    pub fn viewer(&self, id: &str) -> Option<&FileViewer> {
        self.viewers
            .iter()
            .find(|entry| entry.id == id || entry.aliases.contains(&id))
    }
    pub fn default_viewer(&self, kind: DocumentKind) -> Option<&FileViewer> {
        self.viewers
            .iter()
            .find(|entry| entry.fallback && entry.kind == kind)
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

fn valid_id(id: &str) -> bool {
    id.split('.').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
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
            singleton: false,
            placement: PanelPlacement::Center,
            open_placement: PanelPlacement::Center,
            legacy_kind: None,
            id,
            plugin: self.owner,
            label,
        });
    }
    pub fn panel_options(
        &mut self,
        id: &'static str,
        label: &'static str,
        singleton: bool,
        placement: PanelPlacement,
        legacy_kind: Option<&'static str>,
    ) {
        self.panel(id, label);
        let panel = self.registry.panels.last_mut().unwrap();
        panel.singleton = singleton;
        panel.placement = placement;
        panel.open_placement = placement;
        panel.legacy_kind = legacy_kind;
    }
    /// Override placement for new instances without changing the initial layout.
    pub fn panel_open_placement(&mut self, id: &'static str, placement: PanelPlacement) {
        let panel = self
            .registry
            .panels
            .iter_mut()
            .find(|panel| panel.id == id && panel.plugin == self.owner)
            .expect("register a panel before setting its open placement");
        panel.open_placement = placement;
    }
    pub fn fallback_viewer(
        &mut self,
        id: &'static str,
        label: &'static str,
        panel_type: &'static str,
        kind: DocumentKind,
        aliases: &'static [&'static str],
    ) {
        self.viewer(id, label, panel_type, &[], kind);
        let viewer = self.registry.viewers.last_mut().unwrap();
        viewer.fallback = true;
        viewer.aliases = aliases;
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
            fallback: false,
            aliases: &[],
            id,
            plugin: self.owner,
            label,
            panel_type,
            extensions,
            kind,
        });
    }
}

/// Compatibility name for modules implemented by existing Rust plugins.
pub use Module as Plugin;
/// Compatibility name for panels implemented by existing Rust plugins.
pub use ModulePanel as PluginPanel;

/// Workspace behavior with an independent lifetime from its visible panels.
/// Native modules and linked plugins share hosting and services; feature-specific
/// extension contracts remain owned by the corresponding feature.
pub trait Module: Any {
    fn id(&self) -> &'static str;
    fn register(&self, registrar: &mut Registrar<'_>);
    fn shortcuts(
        &mut self,
        _ui: &Ui,
        _host: &HostContext<'_>,
        _services: &mut ModuleServices<'_>,
        _requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        Ok(())
    }
    fn tick_with_services(
        &mut self,
        host: &HostContext<'_>,
        _services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        self.tick(host, requests);
        Ok(())
    }
    fn document_events(
        &mut self,
        _session: &bed_document_session::editor_session::EditorSession,
        _events: &[bed_document_session::editor_session::SessionEvent],
    ) -> io::Result<()> {
        Ok(())
    }
    fn restore_workspace(&mut self, _state: Option<&Value>, _project_root: &str) {}
    fn save_workspace(&self) -> Value {
        Value::Null
    }
    fn shutdown(&mut self, _services: &mut ModuleServices<'_>) {}
    fn retains_terminal(&self, _id: u64) -> bool {
        false
    }
    fn command_with_services(
        &mut self,
        command: &str,
        context: &CommandContext,
        host: &HostContext<'_>,
        _services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        self.command(command, context, host, requests);
        Ok(())
    }
    fn create_panel_with_services(
        &mut self,
        panel_type: &str,
        document: Option<DocumentId>,
        state: &Value,
        _services: &mut ModuleServices<'_>,
    ) -> Result<Box<dyn ModulePanel>, String> {
        self.create_panel(panel_type, document, state)
    }
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
    ) -> Result<Box<dyn ModulePanel>, String>;
    fn draw_settings(&mut self, _section: &str, _ui: &Ui, _settings: &mut Value) -> bool {
        false
    }
    fn draw_popups(&mut self, _ui: &Ui, _host: &HostContext<'_>, _requests: &mut Vec<HostRequest>) {
    }
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// Actions sent to the originating panel by native menus and document lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelAction {
    Find,
    SelectAll,
    GoToLine,
    Undo,
    Redo,
    /// Validate and finish any local edit before saving or closing. Implementations
    /// may queue only edit requests; the host applies them before continuing.
    /// An error must retain the draft and prevents the save or close.
    CommitEdit,
}

/// A visible module surface hosted by the workbench. Panels retain local view
/// state while documents, edits, and history remain in shared services.
pub trait ModulePanel: Any {
    fn persist(&self) -> bool {
        true
    }
    fn focus_with_services(&mut self, _services: &mut ModuleServices<'_>) -> io::Result<()> {
        Ok(())
    }
    fn view_id(&self) -> Option<bed_document_session::editor_session::ViewId> {
        None
    }
    fn window_padding(&self) -> Option<[f32; 2]> {
        None
    }
    fn draw_with_services(
        &mut self,
        ui: &Ui,
        host: &HostContext<'_>,
        _services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        self.draw(ui, host, requests);
        Ok(())
    }
    fn action_with_services(
        &mut self,
        action: PanelAction,
        host: &HostContext<'_>,
        _services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> Result<bool, String> {
        self.action(action, host, requests)
    }
    fn close_with_services(
        &mut self,
        _services: &mut ModuleServices<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> io::Result<()> {
        self.close(requests);
        Ok(())
    }

    fn title(&self, host: &HostContext<'_>) -> String;
    fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>);
    fn action(
        &mut self,
        _action: PanelAction,
        _host: &HostContext<'_>,
        _requests: &mut Vec<HostRequest>,
    ) -> Result<bool, String> {
        Ok(false)
    }
    /// Called after a token-bearing edit is accepted or rejected by the session.
    /// Acknowledgements allow a panel to retain local input until it is applied.
    fn edit_result(&mut self, _token: EditToken, _result: Result<Revision, String>) {}
    /// The host owns this output's allocation and Dear ImGui registration.
    /// Return None while loading, on failure, or when no output is needed.
    #[cfg(feature = "gpu")]
    fn render_output(&self) -> Option<gpu::RenderOutput> {
        None
    }
    /// Called after UI/input and before any viewport is submitted. The host calls
    /// this only after a revision change, resize, or device recreation.
    #[cfg(feature = "gpu")]
    fn render(
        &mut self,
        _gpu: &mut gpu::GpuContext<'_>,
        _target: &gpu::RenderTarget,
    ) -> Result<(), String> {
        Ok(())
    }
    fn save_state_with_services(
        &mut self,
        _services: &mut ModuleServices<'_>,
    ) -> io::Result<Value> {
        Ok(self.save_state())
    }
    fn save_state(&self) -> Value {
        Value::Null
    }
    fn attached_document(&self) -> Option<DocumentId> {
        None
    }
    fn close(&mut self, _requests: &mut Vec<HostRequest>) {}
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_services_lend_distinct_authorities_once_per_callback() {
        struct Settings {
            theme: String,
        }
        struct Terminals {
            focused: Option<u64>,
        }
        let mut settings = Settings {
            theme: "dark".into(),
        };
        let mut terminals = Terminals { focused: None };
        {
            let mut scope = ScopedServices::default();
            scope.insert(&mut settings);
            scope.insert(&mut terminals);
            assert!(
                scope.take::<String>().is_none(),
                "an unrelated service must not consume a registered type"
            );
            let settings = scope.take::<Settings>().unwrap();
            assert!(
                scope.take::<Settings>().is_none(),
                "the same mutable service cannot be lent twice"
            );
            let terminals = scope.take::<Terminals>().unwrap();
            settings.theme = "light".into();
            terminals.focused = Some(42);
        }
        assert_eq!(settings.theme, "light");
        assert_eq!(terminals.focused, Some(42));
    }

    #[test]
    fn a_fresh_callback_scope_observes_previous_service_changes() {
        let mut authority = String::from("initial");
        {
            let mut first = ScopedServices::default();
            first.insert(&mut authority);
            first.take::<String>().unwrap().push_str(" first");
        }
        {
            let mut second = ScopedServices::default();
            second.insert(&mut authority);
            let service = second.take::<String>().unwrap();
            assert_eq!(service, "initial first");
            service.push_str(" second");
        }
        assert_eq!(authority, "initial first second");
    }

    struct TestModule {
        id: &'static str,
        contributions: Box<dyn Fn(&mut Registrar<'_>)>,
    }

    fn module(
        id: &'static str,
        contributions: impl Fn(&mut Registrar<'_>) + 'static,
    ) -> TestModule {
        TestModule {
            id,
            contributions: Box::new(contributions),
        }
    }

    impl Module for TestModule {
        fn id(&self) -> &'static str {
            self.id
        }
        fn register(&self, registrar: &mut Registrar<'_>) {
            (self.contributions)(registrar);
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
        ) -> Result<Box<dyn ModulePanel>, String> {
            Err("Not needed".into())
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }

    fn assert_rejected_atomically(registry: &mut Registry, module: &dyn Module) {
        let snapshot = |registry: &Registry| {
            (
                format!(
                    "{:?}",
                    (
                        &registry.commands,
                        &registry.menus,
                        &registry.toolbar,
                        &registry.settings,
                        &registry.panels,
                        &registry.viewers,
                    )
                ),
                registry.plugins.clone(),
            )
        };
        let before = snapshot(registry);
        assert!(
            registry.register(module).is_err(),
            "invalid module {} registered",
            module.id()
        );
        assert_eq!(
            snapshot(registry),
            before,
            "failed registration changed the registry"
        );
    }

    #[test]
    fn fallbacks_are_unique_per_kind_and_failed_registration_can_retry() {
        let mut registry = Registry::default();
        registry
            .register(&module("editor", |registrar| {
                registrar.panel("editor.text", "Text");
                registrar.panel("editor.hex", "Hex");
                registrar.fallback_viewer(
                    "editor.text-viewer",
                    "Text",
                    "editor.text",
                    DocumentKind::Text,
                    &["bed.text"],
                );
                registrar.fallback_viewer(
                    "editor.hex-viewer",
                    "Hex",
                    "editor.hex",
                    DocumentKind::Bytes,
                    &["bed.hex"],
                );
            }))
            .unwrap();
        assert_eq!(
            registry.default_viewer(DocumentKind::Text).unwrap().id,
            "editor.text-viewer"
        );
        assert_eq!(
            registry.default_viewer(DocumentKind::Bytes).unwrap().id,
            "editor.hex-viewer"
        );
        assert_eq!(
            registry.viewer("bed.text").unwrap().id,
            "editor.text-viewer"
        );
        assert_eq!(registry.viewer("bed.hex").unwrap().id, "editor.hex-viewer");
        assert!(registry.viewer_for_path("unknown.ext").is_none());
        for kind in [DocumentKind::Text, DocumentKind::Bytes] {
            assert_rejected_atomically(
                &mut registry,
                &module("alternative", move |registrar| {
                    registrar.command("alternative.open", "Open", None);
                    registrar.menu(MenuSlot::File, "alternative.open");
                    registrar.settings("alternative.settings", "Alternative");
                    registrar.panel("alternative.panel", "Alternative");
                    registrar.fallback_viewer(
                        "alternative.viewer",
                        "Alternative",
                        "alternative.panel",
                        kind,
                        &[],
                    );
                }),
            );
        }
        registry
            .register(&module("alternative", |registrar| {
                registrar.panel("alternative.panel", "Alternative");
                registrar.viewer(
                    "alternative.viewer",
                    "Alternative",
                    "alternative.panel",
                    &["ext"],
                    DocumentKind::Text,
                );
            }))
            .unwrap();
        assert_eq!(
            registry.viewer_for_path("unknown.ext").unwrap().id,
            "alternative.viewer"
        );
        assert_eq!(
            registry.default_viewer(DocumentKind::Text).unwrap().id,
            "editor.text-viewer"
        );
    }

    #[test]
    fn two_fallbacks_for_one_kind_in_the_same_module_are_rejected() {
        let mut registry = Registry::default();
        assert_rejected_atomically(
            &mut registry,
            &module("editor", |registrar| {
                registrar.panel("editor.panel", "Editor");
                registrar.fallback_viewer(
                    "editor.first",
                    "First",
                    "editor.panel",
                    DocumentKind::Text,
                    &[],
                );
                registrar.fallback_viewer(
                    "editor.second",
                    "Second",
                    "editor.panel",
                    DocumentKind::Text,
                    &[],
                );
            }),
        );
    }

    #[test]
    fn viewer_aliases_cannot_shadow_contributions_in_the_same_registration() {
        for alias in [
            "owner.panel",
            "owner.viewer",
            "owner.open",
            "owner.settings",
        ] {
            let mut registry = Registry::default();
            assert_rejected_atomically(
                &mut registry,
                &module("owner", move |registrar| {
                    registrar.panel("owner.panel", "Owner");
                    registrar.command("owner.open", "Open", None);
                    registrar.settings("owner.settings", "Owner");
                    // Test aliases are static; each slice is stored by the descriptor.
                    let aliases: &'static [&'static str] = match alias {
                        "owner.panel" => &["owner.panel"],
                        "owner.viewer" => &["owner.viewer"],
                        "owner.open" => &["owner.open"],
                        _ => &["owner.settings"],
                    };
                    registrar.fallback_viewer(
                        "owner.viewer",
                        "Owner",
                        "owner.panel",
                        DocumentKind::Text,
                        aliases,
                    );
                }),
            );
        }
    }

    #[test]
    fn viewer_alias_collisions_are_rejected_in_either_registration_order() {
        for contribution in ["panel", "viewer", "command", "settings"] {
            let canonical = module("target", move |registrar| match contribution {
                "panel" => registrar.panel("target.shared", "Target"),
                "viewer" => {
                    registrar.panel("target.panel", "Target");
                    registrar.viewer(
                        "target.shared",
                        "Target",
                        "target.panel",
                        &["x"],
                        DocumentKind::Bytes,
                    );
                }
                "command" => registrar.command("target.shared", "Target", None),
                _ => registrar.settings("target.shared", "Target"),
            });
            let alias = module("legacy", |registrar| {
                registrar.panel("legacy.panel", "Legacy");
                registrar.fallback_viewer(
                    "legacy.viewer",
                    "Legacy",
                    "legacy.panel",
                    DocumentKind::Text,
                    &["target.shared"],
                );
            });
            let mut registry = Registry::default();
            registry.register(&canonical).unwrap();
            assert_rejected_atomically(&mut registry, &alias);
            let mut registry = Registry::default();
            registry.register(&alias).unwrap();
            assert_rejected_atomically(&mut registry, &canonical);
            assert_eq!(
                registry.viewer("target.shared").unwrap().id,
                "legacy.viewer"
            );
        }
    }

    #[test]
    fn viewer_aliases_are_unique_within_and_between_modules() {
        let mut registry = Registry::default();
        assert_rejected_atomically(
            &mut registry,
            &module("owner", |registrar| {
                registrar.panel("owner.panel", "Owner");
                registrar.fallback_viewer(
                    "owner.viewer",
                    "Owner",
                    "owner.panel",
                    DocumentKind::Text,
                    &["old.viewer", "old.viewer"],
                );
            }),
        );
        assert_rejected_atomically(
            &mut registry,
            &module("owner", |registrar| {
                registrar.panel("owner.panel", "Owner");
                registrar.fallback_viewer(
                    "owner.first",
                    "First",
                    "owner.panel",
                    DocumentKind::Text,
                    &["old.viewer"],
                );
                registrar.fallback_viewer(
                    "owner.second",
                    "Second",
                    "owner.panel",
                    DocumentKind::Bytes,
                    &["old.viewer"],
                );
            }),
        );
        registry
            .register(&module("owner", |registrar| {
                registrar.panel("owner.panel", "Owner");
                registrar.fallback_viewer(
                    "owner.viewer",
                    "Owner",
                    "owner.panel",
                    DocumentKind::Text,
                    &["old.viewer"],
                );
            }))
            .unwrap();
        assert_rejected_atomically(
            &mut registry,
            &module("other", |registrar| {
                registrar.panel("other.panel", "Other");
                registrar.fallback_viewer(
                    "other.viewer",
                    "Other",
                    "other.panel",
                    DocumentKind::Bytes,
                    &["old.viewer"],
                );
            }),
        );
    }

    #[test]
    fn legacy_panel_kinds_are_unique_and_cannot_shadow_panel_ids() {
        let mut registry = Registry::default();
        registry
            .register(&module("original", |registrar| {
                registrar.panel_options(
                    "original.panel",
                    "Original",
                    true,
                    PanelPlacement::Bottom,
                    Some("legacy"),
                );
            }))
            .unwrap();
        assert_rejected_atomically(
            &mut registry,
            &module("other", |registrar| {
                registrar.panel_options(
                    "other.panel",
                    "Other",
                    true,
                    PanelPlacement::Sidebar,
                    Some("legacy"),
                );
            }),
        );
        assert_rejected_atomically(
            &mut registry,
            &module("other", |registrar| {
                registrar.panel_options(
                    "other.panel",
                    "Other",
                    false,
                    PanelPlacement::Center,
                    Some("original.panel"),
                );
            }),
        );
        assert_rejected_atomically(
            &mut registry,
            &module("other", |registrar| {
                registrar.panel_options(
                    "other.first",
                    "First",
                    false,
                    PanelPlacement::Center,
                    Some("duplicate"),
                );
                registrar.panel_options(
                    "other.second",
                    "Second",
                    false,
                    PanelPlacement::Center,
                    Some("duplicate"),
                );
            }),
        );
        assert_rejected_atomically(
            &mut registry,
            &module("other", |registrar| {
                registrar.panel_options(
                    "other.first",
                    "First",
                    false,
                    PanelPlacement::Center,
                    Some("other.second"),
                );
                registrar.panel("other.second", "Second");
            }),
        );
        registry
            .register(&module("earlier", |registrar| {
                registrar.panel_options(
                    "earlier.panel",
                    "Earlier",
                    false,
                    PanelPlacement::Center,
                    Some("later.panel"),
                );
            }))
            .unwrap();
        assert_rejected_atomically(
            &mut registry,
            &module("later", |registrar| {
                registrar.panel("later.panel", "Later");
            }),
        );
    }

    #[test]
    fn registration_requires_well_formed_ids_in_the_exact_module_namespace() {
        for id in [
            "",
            ".owner",
            "owner.",
            "owner..child",
            "owner/child",
            "owner child",
        ] {
            let mut registry = Registry::default();
            assert_rejected_atomically(&mut registry, &module(id, |_| {}));
        }
        for id in [
            "owner.",
            "owner..panel",
            "owner.other/panel",
            "owner.other panel",
            "owner/../other.panel",
            "ownerish.panel",
            "other.panel",
        ] {
            let mut registry = Registry::default();
            assert_rejected_atomically(
                &mut registry,
                &module("owner", move |registrar| {
                    registrar.panel(id, "Invalid");
                }),
            );
        }
        let mut registry = Registry::default();
        registry
            .register(&module("owner.child", |registrar| {
                registrar.panel("owner.child.valid-panel_1", "Valid");
            }))
            .unwrap();
    }

    #[test]
    fn malformed_aliases_and_legacy_kinds_are_rejected() {
        for aliases in [
            &[""][..],
            &["legacy..viewer"][..],
            &["legacy/viewer"][..],
            &["legacy viewer"][..],
        ] {
            let mut registry = Registry::default();
            assert_rejected_atomically(
                &mut registry,
                &module("owner", move |registrar| {
                    registrar.panel("owner.panel", "Owner");
                    registrar.fallback_viewer(
                        "owner.viewer",
                        "Owner",
                        "owner.panel",
                        DocumentKind::Text,
                        aliases,
                    );
                }),
            );
        }
        for kind in [
            "",
            ".legacy",
            "legacy.",
            "legacy..panel",
            "legacy/panel",
            "legacy panel",
        ] {
            let mut registry = Registry::default();
            assert_rejected_atomically(
                &mut registry,
                &module("owner", move |registrar| {
                    registrar.panel_options(
                        "owner.panel",
                        "Owner",
                        true,
                        PanelPlacement::Center,
                        Some(kind),
                    );
                }),
            );
        }
    }

    #[test]
    fn references_must_resolve_inside_the_contributing_module() {
        let mut registry = Registry::default();
        registry
            .register(&module("existing", |registrar| {
                registrar.panel("existing.panel", "Existing");
                registrar.command("existing.open", "Open", None);
            }))
            .unwrap();
        assert_rejected_atomically(
            &mut registry,
            &module("other", |registrar| {
                registrar.viewer(
                    "other.viewer",
                    "Other",
                    "existing.panel",
                    &["x"],
                    DocumentKind::Text,
                );
            }),
        );
        assert_rejected_atomically(
            &mut registry,
            &module("other", |registrar| {
                registrar.menu(MenuSlot::File, "existing.open");
            }),
        );
        assert_rejected_atomically(
            &mut registry,
            &module("other", |registrar| {
                registrar.toolbar("existing.open");
            }),
        );
    }

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
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
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
