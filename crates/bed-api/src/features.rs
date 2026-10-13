//! Plugin host requests, preparation, contributions and persistence facts.
#[cfg(feature = "documents")]
use crate::{
    application::{WorkspaceAction, WorkspaceSpec},
    documents::*,
    settings::SettingsSection,
    workbench::{PanelAction, PanelPlacement, PanelTarget, PanelType, valid_id},
};
#[cfg(feature = "documents")]
use bed_ui::TextureHandle;
#[cfg(feature = "documents")]
use dear_imgui_rs::{TextureId, Ui};
use serde_json::Value;
#[cfg(feature = "documents")]
use std::{collections::HashMap, io, path::PathBuf, sync::Arc};

/// Shared emission order for hosts merging panel commands and document requests.
pub fn next_request_sequence() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_update(
        std::sync::atomic::Ordering::Relaxed,
        std::sync::atomic::Ordering::Relaxed,
        |value| value.checked_add(1),
    )
    .expect("request sequence overflow")
}

/// Opaque feature state supplied by the host's chosen persistence policy.
pub struct FeatureStateRestore {
    pub state: Option<Value>,
}

/// A feature committed a change to the state it contributes to persistence.
/// The host decides whether and where that state should be stored.
#[derive(bevy::prelude::Message, Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeatureStateChanged {
    pub owner: &'static str,
}

/// A feature contributes state without requiring a project or workspace model.
#[derive(bevy::prelude::Component, Clone, Copy)]
pub struct FeatureStateContribution {
    pub owner: &'static str,
    pub save: bevy::ecs::system::SystemId<(), Value>,
    pub restore:
        bevy::ecs::system::SystemId<bevy::prelude::In<FeatureStateRestore>, Result<(), String>>,
}

#[cfg(feature = "documents")]
/// An independent frame intention with its origin and shared emission order.
/// Producers capture these facts immediately; hosts retain and resolve them
/// between UI scopes without consulting a later focus or document session.
/// Command and draft handlers use `HostEffects` for effects that must finish
/// within their enclosing operation before another command executes.
#[derive(bevy::prelude::Message, Clone, Debug)]
pub struct HostRequest {
    session_id: bed_documents::SessionId,
    source: Option<bevy::prelude::Entity>,
    action: HostAction,
    area: u32,
    sequence: u64,
    layout_epoch: u64,
}

#[cfg(feature = "documents")]
impl HostRequest {
    pub fn capture(
        session_id: bed_documents::SessionId,
        source: Option<bevy::prelude::Entity>,
        action: HostAction,
        area: u32,
        layout_epoch: u64,
    ) -> Self {
        Self {
            session_id,
            source,
            action,
            area,
            sequence: crate::features::next_request_sequence(),
            layout_epoch,
        }
    }

    pub fn session_id(&self) -> bed_documents::SessionId {
        self.session_id
    }
    pub fn source(&self) -> Option<bevy::prelude::Entity> {
        self.source
    }
    pub fn action(&self) -> &HostAction {
        &self.action
    }
    pub fn area(&self) -> u32 {
        self.area
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn layout_epoch(&self) -> u64 {
        self.layout_epoch
    }

    /// UI intentions expire when their captured layout is replaced. Document
    /// edits, saves and history operations keep their explicit document target.
    pub fn is_layout_scoped(&self) -> bool {
        self.action.is_layout_scoped()
    }
}

#[cfg(feature = "documents")]
impl HostAction {
    pub fn is_layout_scoped(&self) -> bool {
        match self {
            HostAction::OpenTerminalFiles { .. }
            | HostAction::OpenTerminalLink { .. }
            | HostAction::Command { .. }
            | HostAction::ClosePanels { .. }
            | HostAction::FocusView { .. }
            | HostAction::RevealSource { .. }
            | HostAction::PreviewSource { .. }
            | HostAction::ShowTerminal { .. }
            | HostAction::ShowPanel { .. }
            | HostAction::ShowPanelIn { .. }
            | HostAction::OpenPanel { .. }
            | HostAction::OpenFile { .. }
            | HostAction::OpenFileIn { .. }
            | HostAction::PreviewFileIn { .. }
            | HostAction::DropFile { .. }
            | HostAction::SwitchViewer { .. }
            | HostAction::OpenFileDialog { .. }
            | HostAction::SaveAsDialog { .. }
            | HostAction::Navigate { .. } => true,
            #[cfg(feature = "language-services")]
            HostAction::NavigateLocation { .. } => true,
            HostAction::Workspace(_)
            | HostAction::PromoteWorkspace { .. }
            | HostAction::Invalidate
            | HostAction::PersistWorkspace
            | HostAction::ApplyEdits { .. }
            | HostAction::ApplyEditsWithResult { .. }
            | HostAction::ApplyModuleEditsWithResult { .. }
            | HostAction::Save { .. }
            | HostAction::CommitDocumentsWithResult { .. }
            | HostAction::SaveDocumentsWithResult { .. }
            | HostAction::Undo { .. }
            | HostAction::Redo { .. }
            | HostAction::SetSetting { .. }
            | HostAction::Notify { .. } => false,
        }
    }
}

#[cfg(feature = "documents")]
pub struct HostContext<'a> {
    /// Whether paths and documents belong to an SSH workspace.
    pub remote: bool,
    pub documents: &'a [PluginDocument],
    pub active_document: Option<DocumentId>,
    pub settings: &'a Value,
    pub textures: &'a HashMap<TextureHandle, TextureId>,
    pub animations: bool,
    pub session_id: u64,
    /// Read-only LSP data grouped by document path. No client/process handles escape.
    pub diagnostics: &'a Value,
    /// Choices for the originating tab; absent in module-wide callbacks.
    pub viewer_menu: Option<&'a ViewerMenu>,
    pub default_viewers: &'a Value,
}

#[cfg(feature = "documents")]
/// Immutable host data published before feature systems and ImGui drawing.
/// Workers and game plugins can inspect documents without borrowing the session.
#[derive(Clone, Default, bevy::prelude::Resource)]
pub struct HostFrame {
    pub remote: bool,
    pub project_root: String,
    pub working_directory: String,
    pub recent_workspaces: Vec<WorkspaceSpec>,
    pub remote_connecting: bool,
    pub remote_disconnected: bool,
    pub active_view: Option<bed_documents::session::ViewId>,
    pub documents: Vec<PluginDocument>,
    pub active_document: Option<DocumentId>,
    pub settings: Value,
    pub textures: HashMap<TextureHandle, TextureId>,
    pub animations: bool,
    pub session_id: u64,
    pub diagnostics: Value,
    pub default_viewers: Value,
}

#[cfg(feature = "documents")]
impl HostFrame {
    /// Refresh revision-bound snapshots from the canonical document session.
    pub fn refresh_documents(
        &mut self,
        documents: &bed_documents::session::DocumentSession,
    ) -> io::Result<()> {
        let ids = documents.document_ids();
        self.documents.retain(|doc| ids.contains(&doc.id));
        for id in ids {
            let revision = documents.document_revision(id)?;
            let existing = self.documents.iter().position(|doc| doc.id == id);
            if let Some(index) =
                existing.filter(|&index| self.documents[index].revision == revision)
            {
                documents.with_document(id, |state| {
                    let cached = &mut self.documents[index];
                    cached.path.clone_from(&state.path);
                    cached.language_id.clone_from(&state.language_id);
                    cached.dirty = state.dirty;
                })?;
            } else {
                let doc = documents.snapshot(id)?;
                let text = if doc.kind == DocumentKind::Text {
                    Some(documents.with_document(id, |state| state.snapshot())?)
                } else {
                    None
                };
                let document = PluginDocument {
                    id,
                    path: doc.path,
                    kind: doc.kind,
                    language_id: doc.language_id,
                    revision,
                    dirty: doc.dirty,
                    bytes: Arc::from(doc.bytes),
                    text,
                };
                if let Some(index) = existing {
                    self.documents[index] = document;
                } else {
                    self.documents.push(document);
                }
            }
        }
        Ok(())
    }

    pub fn context(&self) -> HostContext<'_> {
        HostContext {
            remote: self.remote,
            documents: &self.documents,
            active_document: self.active_document,
            settings: &self.settings,
            textures: &self.textures,
            animations: self.animations,
            session_id: self.session_id,
            diagnostics: &self.diagnostics,
            viewer_menu: None,
            default_viewers: &self.default_viewers,
        }
    }
}

/// Feature work is ordered before the frontend; intentions resolve after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, bevy::prelude::SystemSet)]
pub enum FeatureSystems {
    Prepare,
}

#[cfg(feature = "documents")]
#[derive(Debug)]
pub struct HostEffect {
    pub source: Option<bevy::prelude::Entity>,
    pub area: Option<u32>,
    pub tab_index: Option<usize>,
    pub action: HostAction,
}

#[cfg(feature = "documents")]
/// Effects produced synchronously by a command or draft handler. The host
/// commits these within that operation. Independent frame producers emit a
/// captured `HostRequest` so their origin and ordering cannot change.
#[derive(Default, bevy::prelude::Resource)]
pub struct HostEffects {
    pending: Vec<HostEffect>,
}

#[cfg(feature = "documents")]
impl HostEffects {
    pub fn push(&mut self, source: Option<bevy::prelude::Entity>, action: HostAction) {
        self.pending.push(HostEffect {
            source,
            area: None,
            tab_index: None,
            action,
        });
    }

    pub fn push_at(
        &mut self,
        source: Option<bevy::prelude::Entity>,
        area: u32,
        action: HostAction,
    ) {
        self.pending.push(HostEffect {
            source,
            area: Some(area),
            tab_index: None,
            action,
        });
    }

    pub fn take(&mut self) -> Vec<HostEffect> {
        std::mem::take(&mut self.pending)
    }

    pub fn push_effect(&mut self, effect: HostEffect) {
        self.pending.push(effect);
    }
}

#[cfg(feature = "documents")]
impl HostContext<'_> {
    pub fn draw_viewer_menu(&self, ui: &Ui, requests: &mut Vec<HostAction>) {
        if let Some(menu) = self.viewer_menu {
            menu.draw(ui, requests);
        }
    }

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

#[cfg(feature = "documents")]
#[derive(Clone, Debug)]
pub enum HostAction {
    Workspace(WorkspaceAction),
    /// Save and retire the loose window's other panels, attaching its workspace
    /// while preserving the requesting terminal presentation and live process.
    PromoteWorkspace {
        root: PathBuf,
    },
    /// Open shell-requested files using the standard largest-area placement.
    OpenTerminalFiles {
        paths: Vec<PathBuf>,
    },
    /// Open a terminal file link using the standard largest-area placement.
    /// Positions use zero-based lines and byte columns.
    OpenTerminalLink {
        path: PathBuf,
        position: Option<(i32, i32)>,
    },
    Command {
        command: String,
        context: CommandContext,
    },
    ClosePanels {
        panel_type: String,
    },
    FocusView {
        view: bed_documents::session::ViewId,
    },
    RevealSource {
        path: String,
        row: i32,
        column: i32,
        center: bool,
    },
    /// Reveal source in the largest area's preview tab, replacing its previous clean preview.
    /// Positions use zero-based lines and byte columns, as in `RevealSource`.
    PreviewSource {
        path: String,
        row: i32,
        column: i32,
        center: bool,
    },
    ShowTerminal {
        id: u64,
    },
    Invalidate,
    /// Snapshot current layout and feature workspace contributions to persistence.
    PersistWorkspace,

    ShowPanel {
        panel_type: String,
        document: Option<DocumentId>,
        state: Value,
        action: Option<PanelAction>,
    },
    /// Reveal a panel, choosing where newly created content is placed.
    ShowPanelIn {
        panel_type: String,
        document: Option<DocumentId>,
        state: Value,
        action: Option<PanelAction>,
        target: PanelTarget,
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
    /// Open a file, choosing where newly created content is placed.
    OpenFileIn {
        path: String,
        viewer: Option<String>,
        target: PanelTarget,
    },
    /// Preview a file in the target area, replacing its previous clean preview.
    PreviewFileIn {
        path: String,
        viewer: Option<String>,
        target: PanelTarget,
    },
    /// Open a dropped file in the captured area, moving its tab if already open.
    DropFile {
        path: String,
    },
    SwitchViewer {
        tab: u64,
        document: DocumentId,
        viewer: String,
    },
    OpenFileDialog {
        viewer: Option<String>,
    },
    Navigate {
        document: DocumentId,
        byte_offset: usize,
        revision: Revision,
    },
    /// LSP locations use UTF-16 columns and retain their original request identity.
    /// New destinations open as previews in the source editor's area;
    /// existing tabs keep their mode.
    #[cfg(feature = "language-services")]
    NavigateLocation {
        path: String,
        row: i32,
        utf16_column: i32,
        origin: Option<LspRequestOrigin>,
    },
    ApplyEdits {
        document: DocumentId,
        revision: Revision,
        edits: Vec<bed_documents::session::ByteEdit>,
    },
    ApplyEditsWithResult {
        token: EditToken,
        document: DocumentId,
        revision: Revision,
        edits: Vec<bed_documents::session::ByteEdit>,
    },
    /// Flush panel drafts, validate the revision, and acknowledge to a module.
    ApplyModuleEditsWithResult {
        recipient: String,
        token: EditToken,
        document: DocumentId,
        revision: Revision,
        edits: Vec<bed_documents::session::ByteEdit>,
    },
    Save {
        document: DocumentId,
    },
    SaveAsDialog {
        document: DocumentId,
    },
    /// Commit attached drafts and reject documents with pending writes. No disk write occurs.
    CommitDocumentsWithResult {
        recipient: String,
        token: SaveToken,
        documents: Vec<DocumentId>,
    },
    /// Flush attached panel drafts and await every requested disk write.
    SaveDocumentsWithResult {
        recipient: String,
        token: SaveToken,
        documents: Vec<DocumentId>,
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

#[cfg(feature = "documents")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuSlot {
    File,
    Folder,
    TreeBackground,
    TextSelection,
    Application,
}

#[cfg(feature = "documents")]
#[derive(Clone, Debug)]
pub struct CommandDescriptor {
    pub id: &'static str,
    pub plugin: &'static str,
}

#[cfg(feature = "documents")]
#[derive(Clone, Debug)]
pub struct MenuContribution {
    pub slot: MenuSlot,
    pub command: &'static str,
}

#[cfg(feature = "documents")]
#[derive(Clone, Debug)]
pub struct ToolbarContribution {
    pub command: &'static str,
}

#[cfg(feature = "documents")]
#[derive(Clone, Default, bevy::prelude::Resource)]
pub struct FeatureRegistry {
    commands: Vec<CommandDescriptor>,
    menus: Vec<MenuContribution>,
    toolbar: Vec<ToolbarContribution>,
    settings: Vec<SettingsSection>,
    panels: Vec<PanelType>,
    viewers: Vec<FileViewer>,
    plugins: std::collections::HashSet<&'static str>,
}

#[cfg(feature = "documents")]
impl FeatureRegistry {
    pub fn has_owner(&self, owner: &str) -> bool {
        self.plugins.contains(owner)
    }

    pub fn commands(&self) -> &[CommandDescriptor] {
        &self.commands
    }

    pub fn menus(&self) -> &[MenuContribution] {
        &self.menus
    }

    pub fn toolbar(&self) -> &[ToolbarContribution] {
        &self.toolbar
    }

    pub fn settings(&self) -> &[SettingsSection] {
        &self.settings
    }

    pub fn panels(&self) -> &[PanelType] {
        &self.panels
    }

    pub fn viewers(&self) -> &[FileViewer] {
        &self.viewers
    }

    fn contribution_ids(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.commands
            .iter()
            .map(|entry| entry.id)
            .chain(self.panels.iter().map(|entry| entry.id))
            .chain(self.viewers.iter().map(|entry| entry.id))
            .chain(self.settings.iter().map(|entry| entry.id))
    }

    /// Publish one complete contribution batch after validating every entry.
    /// Invalid contributions leave this registry unchanged.
    pub fn register_with(
        &mut self,
        owner: &'static str,
        build: impl FnOnce(&mut FeatureRegistrar<'_>),
    ) -> Result<(), String> {
        if !valid_id(owner) {
            return Err(format!("Invalid module ID: {owner}"));
        }
        if self.plugins.contains(owner) {
            return Err(format!("Module already registered: {owner}"));
        }
        let mut pending = FeatureRegistry::default();
        build(&mut FeatureRegistrar {
            owner,
            registry: &mut pending,
        });
        // Validate the entire contribution before changing the registry.
        let mut ids = std::collections::HashSet::new();
        crate::workbench::validate_contribution_ids(
            owner,
            pending.contribution_ids(),
            self.contribution_ids().chain(
                self.viewers
                    .iter()
                    .flat_map(|viewer| viewer.aliases.iter().copied()),
            ),
        )?;
        ids.extend(pending.contribution_ids());
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
                let kind = viewer
                    .document_kind()
                    .ok_or("Local-file viewers cannot be fallbacks")?;
                if fallback_kinds.contains(&kind) || self.default_viewer(kind).is_some() {
                    return Err(format!("Fallback viewer already registered for {:?}", kind));
                }
                fallback_kinds.push(kind);
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
            .find(|entry| entry.fallback && entry.document_kind() == Some(kind))
    }

    /// Registration order resolves conflicts; no generic wildcard masks text fallback.
    pub fn viewer_for_path(&self, path: &str) -> Option<&FileViewer> {
        self.viewers.iter().find(|viewer| viewer.matches_path(path))
    }

    /// Text defaults to its editor. Binary uses the first matching compatible
    /// format viewer, then hex. Unknown content has no default yet.
    pub fn default_viewer_for_file(
        &self,
        path: &str,
        content_kind: Option<DocumentKind>,
    ) -> Option<&FileViewer> {
        self.viewers
            .iter()
            .find(|viewer| {
                content_kind == Some(DocumentKind::Bytes)
                    && viewer.matches_path(path)
                    && (viewer.is_local_file()
                        || viewer.supports_document_kind(DocumentKind::Bytes))
            })
            .or_else(|| content_kind.and_then(|kind| self.default_viewer(kind)))
    }

    /// Preferences select presentations, while classification owns document storage.
    pub fn preferred_viewer_for_file(
        &self,
        path: &str,
        kind: DocumentKind,
        defaults: &Value,
    ) -> Option<&FileViewer> {
        let extension = path
            .rsplit(['/', '\\'])
            .next()
            .and_then(|name| name.rsplit_once('.'))
            .map(|(_, ext)| ext);
        let configured = extension.and_then(|extension| {
            defaults
                .as_object()?
                .iter()
                .find(|(ext, _)| ext.eq_ignore_ascii_case(extension))?
                .1
                .as_str()
        });
        configured
            .and_then(|id| self.viewer(id))
            .filter(|viewer| {
                (viewer.fallback || viewer.matches_path(path))
                    && (viewer.is_local_file() || viewer.supports_document_kind(kind))
            })
            .or_else(|| self.default_viewer_for_file(path, Some(kind)))
    }

    /// List the compatible default, other matching formats in registration
    /// order, then text and byte fallbacks. Text viewers require known text;
    /// byte fallbacks remain available while classification is pending.
    pub fn compatible_viewers(
        &self,
        path: &str,
        content_kind: Option<DocumentKind>,
    ) -> Vec<&FileViewer> {
        let default = self.default_viewer_for_file(path, content_kind);
        let is_default = |viewer: &FileViewer| default.is_some_and(|entry| entry.id == viewer.id);
        let mut viewers = Vec::new();
        if let Some(default) = default {
            viewers.push(default);
        }
        viewers.extend(self.viewers.iter().filter(|viewer| {
            !viewer.fallback
                && viewer.matches_path(path)
                && viewer.supports_content_kind(content_kind)
                && !is_default(viewer)
        }));
        for kind in [DocumentKind::Text, DocumentKind::Bytes] {
            if let Some(viewer) = self.default_viewer(kind)
                && viewer.supports_content_kind(content_kind)
                && !is_default(viewer)
            {
                viewers.push(viewer);
            }
        }
        viewers
    }
}

#[cfg(feature = "documents")]
pub struct FeatureRegistrar<'a> {
    owner: &'static str,
    registry: &'a mut FeatureRegistry,
}

#[cfg(feature = "documents")]
impl FeatureRegistrar<'_> {
    /// Feature domains refer to commands whose handler and presentation are
    /// contributed once through the neutral workbench registry.
    pub fn command_ref(&mut self, id: &'static str) {
        self.registry.commands.push(CommandDescriptor {
            id,
            plugin: self.owner,
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
            draw: None,
        });
    }

    pub fn settings_ui(
        &mut self,
        id: &'static str,
        label: &'static str,
        draw: fn(&Ui, &mut Value) -> bool,
    ) {
        self.registry.settings.push(SettingsSection {
            id,
            plugin: self.owner,
            label,
            draw: Some(draw),
        });
    }

    pub fn panel_ref(&mut self, id: &'static str) {
        self.registry.panels.push(PanelType {
            empty_input: true,
            singleton: false,
            placement: PanelPlacement::Center,
            open_placement: PanelPlacement::Center,
            legacy_kind: None,
            id,
            plugin: self.owner,
        });
    }

    pub fn panel_options(
        &mut self,
        id: &'static str,
        singleton: bool,
        placement: PanelPlacement,
        legacy_kind: Option<&'static str>,
    ) {
        self.panel_ref(id);
        let panel = self.registry.panels.last_mut().unwrap();
        panel.singleton = singleton;
        panel.placement = placement;
        panel.open_placement = placement;
        panel.legacy_kind = legacy_kind;
    }

    /// Override the legacy runtime placement hint without changing the initial layout.
    pub fn panel_requires_input(&mut self, id: &'static str) {
        self.registry
            .panels
            .iter_mut()
            .find(|panel| panel.id == id)
            .expect("panel metadata is registered before its input policy")
            .empty_input = false;
    }

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
            backing: ViewerBacking::Document {
                kind,
                supported_kinds: match kind {
                    DocumentKind::Text => &[DocumentKind::Text],
                    DocumentKind::Bytes => &[DocumentKind::Bytes],
                },
            },
        });
    }

    pub fn local_file_viewer(
        &mut self,
        id: &'static str,
        label: &'static str,
        panel_type: &'static str,
        extensions: &'static [&'static str],
    ) {
        self.registry.viewers.push(FileViewer {
            fallback: false,
            aliases: &[],
            id,
            plugin: self.owner,
            label,
            panel_type,
            extensions,
            backing: ViewerBacking::LocalFile,
        });
    }

    pub fn viewer_document_kinds(&mut self, id: &str, kinds: &'static [DocumentKind]) {
        let viewer = self
            .registry
            .viewers
            .iter_mut()
            .find(|viewer| viewer.id == id && viewer.plugin == self.owner)
            .expect("register a viewer before declaring its document kinds");
        let ViewerBacking::Document {
            kind,
            supported_kinds,
        } = &mut viewer.backing
        else {
            panic!("Local-file viewers do not consume document kinds");
        };
        assert!(
            kinds.contains(kind),
            "supported kinds must include the original kind"
        );
        *supported_kinds = kinds;
    }
}

#[cfg(all(test, feature = "documents"))]
mod tests {
    impl FeatureRegistry {
        fn register(&mut self, contribution: &dyn TestDeclaration) -> Result<(), String> {
            self.register_with(contribution.id(), |registrar| {
                contribution.register(registrar)
            })
        }
    }
    use super::*;

    struct TestModule {
        id: &'static str,
        contributions: Box<dyn Fn(&mut FeatureRegistrar<'_>)>,
    }

    fn module(
        id: &'static str,
        contributions: impl Fn(&mut FeatureRegistrar<'_>) + 'static,
    ) -> TestModule {
        TestModule {
            id,
            contributions: Box::new(contributions),
        }
    }

    trait TestDeclaration {
        fn id(&self) -> &'static str;

        fn register(&self, registrar: &mut FeatureRegistrar<'_>);
    }

    impl TestDeclaration for TestModule {
        fn id(&self) -> &'static str {
            self.id
        }

        fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
            (self.contributions)(registrar);
        }
    }

    fn assert_rejected_atomically(registry: &mut FeatureRegistry, module: &TestModule) {
        let snapshot = |registry: &FeatureRegistry| {
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
        let mut registry = FeatureRegistry::default();
        registry
            .register(&module("editor", |registrar| {
                registrar.panel_ref("editor.text");
                registrar.panel_ref("editor.hex");
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
                    registrar.command_ref("alternative.open");
                    registrar.menu(MenuSlot::File, "alternative.open");
                    registrar.settings("alternative.settings", "Alternative");
                    registrar.panel_ref("alternative.panel");
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
                registrar.panel_ref("alternative.panel");
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
        let mut registry = FeatureRegistry::default();
        assert_rejected_atomically(
            &mut registry,
            &module("editor", |registrar| {
                registrar.panel_ref("editor.panel");
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
            let mut registry = FeatureRegistry::default();
            assert_rejected_atomically(
                &mut registry,
                &module("owner", move |registrar| {
                    registrar.panel_ref("owner.panel");
                    registrar.command_ref("owner.open");
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
                "panel" => registrar.panel_ref("target.shared"),
                "viewer" => {
                    registrar.panel_ref("target.panel");
                    registrar.viewer(
                        "target.shared",
                        "Target",
                        "target.panel",
                        &["x"],
                        DocumentKind::Bytes,
                    );
                }
                "command" => registrar.command_ref("target.shared"),
                _ => registrar.settings("target.shared", "Target"),
            });
            let alias = module("legacy", |registrar| {
                registrar.panel_ref("legacy.panel");
                registrar.fallback_viewer(
                    "legacy.viewer",
                    "Legacy",
                    "legacy.panel",
                    DocumentKind::Text,
                    &["target.shared"],
                );
            });
            let mut registry = FeatureRegistry::default();
            registry.register(&canonical).unwrap();
            assert_rejected_atomically(&mut registry, &alias);
            let mut registry = FeatureRegistry::default();
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
        let mut registry = FeatureRegistry::default();
        assert_rejected_atomically(
            &mut registry,
            &module("owner", |registrar| {
                registrar.panel_ref("owner.panel");
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
                registrar.panel_ref("owner.panel");
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
                registrar.panel_ref("owner.panel");
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
                registrar.panel_ref("other.panel");
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
        let mut registry = FeatureRegistry::default();
        registry
            .register(&module("original", |registrar| {
                registrar.panel_options(
                    "original.panel",
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
                    false,
                    PanelPlacement::Center,
                    Some("duplicate"),
                );
                registrar.panel_options(
                    "other.second",
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
                    false,
                    PanelPlacement::Center,
                    Some("other.second"),
                );
                registrar.panel_ref("other.second");
            }),
        );
        registry
            .register(&module("earlier", |registrar| {
                registrar.panel_options(
                    "earlier.panel",
                    false,
                    PanelPlacement::Center,
                    Some("later.panel"),
                );
            }))
            .unwrap();
        assert_rejected_atomically(
            &mut registry,
            &module("later", |registrar| {
                registrar.panel_ref("later.panel");
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
            let mut registry = FeatureRegistry::default();
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
            let mut registry = FeatureRegistry::default();
            assert_rejected_atomically(
                &mut registry,
                &module("owner", move |registrar| {
                    registrar.panel_ref(id);
                }),
            );
        }
        let mut registry = FeatureRegistry::default();
        registry
            .register(&module("owner.child", |registrar| {
                registrar.panel_ref("owner.child.valid-panel_1");
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
            let mut registry = FeatureRegistry::default();
            assert_rejected_atomically(
                &mut registry,
                &module("owner", move |registrar| {
                    registrar.panel_ref("owner.panel");
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
            let mut registry = FeatureRegistry::default();
            assert_rejected_atomically(
                &mut registry,
                &module("owner", move |registrar| {
                    registrar.panel_options(
                        "owner.panel",
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
        let mut registry = FeatureRegistry::default();
        registry
            .register(&module("existing", |registrar| {
                registrar.panel_ref("existing.panel");
                registrar.command_ref("existing.open");
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

    impl TestDeclaration for Example {
        fn id(&self) -> &'static str {
            self.id
        }

        fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
            registrar.command_ref(self.command);
            registrar.panel_ref(self.panel);
            registrar.viewer(
                self.viewer,
                "Example",
                self.panel,
                &["abc"],
                DocumentKind::Text,
            );
        }
    }

    #[test]
    fn matching_is_case_insensitive_and_conflicts_keep_registration_order() {
        let mut registry = FeatureRegistry::default();
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

    fn compatibility_registry() -> FeatureRegistry {
        let mut registry = FeatureRegistry::default();
        registry
            .register(&module("views", |registrar| {
                registrar.panel_ref("views.panel");
                registrar.fallback_viewer(
                    "views.text",
                    "Text",
                    "views.panel",
                    DocumentKind::Text,
                    &[],
                );
                registrar.fallback_viewer(
                    "views.hex",
                    "Hex",
                    "views.panel",
                    DocumentKind::Bytes,
                    &[],
                );
                registrar.viewer(
                    "views.table",
                    "Table",
                    "views.panel",
                    &["csv", "tsv", "shared"],
                    DocumentKind::Text,
                );
                registrar.viewer(
                    "views.image",
                    "Image",
                    "views.panel",
                    &["png", "svg", "shared"],
                    DocumentKind::Bytes,
                );
                registrar.viewer(
                    "views.model",
                    "Model",
                    "views.panel",
                    &["gltf", "glb", "stl", "shared"],
                    DocumentKind::Bytes,
                );
                registrar.viewer(
                    "views.alternative-table",
                    "Alternative Table",
                    "views.panel",
                    &["csv", "tsv", "shared"],
                    DocumentKind::Text,
                );
            }))
            .unwrap();
        registry
    }

    #[test]
    fn compatible_viewers_require_text_content_for_text_viewers() {
        let registry = compatibility_registry();
        for (path, content_kind, expected) in [
            (
                "source.rs",
                Some(DocumentKind::Text),
                vec!["views.text", "views.hex"],
            ),
            (
                "README",
                Some(DocumentKind::Text),
                vec!["views.text", "views.hex"],
            ),
            ("unknown.bin", Some(DocumentKind::Bytes), vec!["views.hex"]),
            ("binary.txt", Some(DocumentKind::Bytes), vec!["views.hex"]),
            ("binary.csv", Some(DocumentKind::Bytes), vec!["views.hex"]),
            (
                "photo.png",
                Some(DocumentKind::Bytes),
                vec!["views.image", "views.hex"],
            ),
            (
                "drawing.svg",
                Some(DocumentKind::Text),
                vec!["views.text", "views.image", "views.hex"],
            ),
            (
                "model.gltf",
                Some(DocumentKind::Text),
                vec!["views.text", "views.model", "views.hex"],
            ),
            (
                "ascii.stl",
                Some(DocumentKind::Text),
                vec!["views.text", "views.model", "views.hex"],
            ),
            (
                "binary.stl",
                Some(DocumentKind::Bytes),
                vec!["views.model", "views.hex"],
            ),
            (
                "table.CsV",
                Some(DocumentKind::Text),
                vec![
                    "views.text",
                    "views.table",
                    "views.alternative-table",
                    "views.hex",
                ],
            ),
        ] {
            let viewers: Vec<_> = registry
                .compatible_viewers(path, content_kind)
                .iter()
                .map(|viewer| viewer.id)
                .collect();
            assert_eq!(viewers, expected, "{path}");
            assert_eq!(
                registry
                    .default_viewer_for_file(path, content_kind)
                    .map(|viewer| viewer.id),
                viewers.first().copied(),
                "{path}"
            );
        }
    }

    #[test]
    fn pending_classification_has_no_default_but_lists_byte_formats() {
        let registry = compatibility_registry();
        for path in ["source.rs", "README", "table.csv", "unknown.bin"] {
            assert_eq!(
                registry
                    .compatible_viewers(path, None)
                    .iter()
                    .map(|viewer| viewer.id)
                    .collect::<Vec<_>>(),
                ["views.hex"],
                "{path}"
            );
            assert!(
                registry.default_viewer_for_file(path, None).is_none(),
                "{path}"
            );
        }
        assert_eq!(
            registry
                .compatible_viewers("drawing.SVG", None)
                .iter()
                .map(|viewer| viewer.id)
                .collect::<Vec<_>>(),
            ["views.image", "views.hex"]
        );
        assert_eq!(
            registry
                .default_viewer_for_file("drawing.SVG", None)
                .map(|v| v.id),
            None
        );
    }

    #[test]
    fn compatible_viewers_preserve_format_order_and_use_eligible_defaults() {
        let registry = compatibility_registry();
        assert_eq!(
            registry
                .compatible_viewers("file.shared", Some(DocumentKind::Text))
                .iter()
                .map(|viewer| viewer.id)
                .collect::<Vec<_>>(),
            [
                "views.text",
                "views.table",
                "views.image",
                "views.model",
                "views.alternative-table",
                "views.hex"
            ]
        );
        assert_eq!(
            registry
                .compatible_viewers("file.shared", Some(DocumentKind::Bytes))
                .iter()
                .map(|viewer| viewer.id)
                .collect::<Vec<_>>(),
            ["views.image", "views.model", "views.hex"]
        );
        assert_eq!(
            registry
                .compatible_viewers("file.shared", None)
                .iter()
                .map(|viewer| viewer.id)
                .collect::<Vec<_>>(),
            ["views.image", "views.model", "views.hex"]
        );
        assert!(
            registry
                .default_viewer_for_file("file.shared", None)
                .is_none()
        );
        // The compatibility resolver must not change existing extension routing.
        assert_eq!(
            registry.viewer_for_path("file.shared").unwrap().id,
            "views.table"
        );
    }

    #[test]
    fn viewer_path_matching_excludes_parent_directories_on_both_platforms() {
        let registry = compatibility_registry();
        let image = registry.viewer("views.image").unwrap();
        assert!(image.matches_path("/folder.with.dots/drawing.SvG"));
        assert!(image.matches_path(r"C:\folder.with.dots\drawing.SvG"));
        assert!(!image.matches_path("/folder.svg/README"));
        assert!(!image.matches_path(r"C:\folder.svg\README"));
        assert!(!image.matches_path(""));
        assert!(!image.matches_path("drawing."));
        assert!(!image.matches_path("drawing.png/"));
    }

    #[test]
    fn viewer_menu_queues_a_switch_for_its_originating_tab() {
        let menu = ViewerMenu {
            tab: 37,
            document: DocumentId(9),
            current: "views.table".into(),
            choices: vec![ViewerChoice {
                id: "views.text".into(),
                label: "Text editor".into(),
                text_editor: true,
            }],
        };
        let mut context = dear_imgui_rs::Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut requests = Vec::new();
        let draw = |context: &mut dear_imgui_rs::Context, requests: &mut Vec<HostAction>| {
            context.prepare_frame(dear_imgui_rs::FramePrepareOptions::new(
                [800.0, 600.0],
                1.0 / 60.0,
            ));
            let ui = context.frame();
            let mut point = [0.0; 2];
            ui.window("Viewer menu test")
                .position([0.0; 2], dear_imgui_rs::Condition::Always)
                .size([600.0, 400.0], dear_imgui_rs::Condition::Always)
                .build(|| {
                    ui.open_popup("Switch presentation");
                    if let Some(_popup) = ui.begin_popup("Switch presentation") {
                        menu.draw(ui, requests);
                        let (min, max) = ui.item_rect();
                        point = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
                    }
                });
            drop(context.render_legacy());
            point
        };
        draw(&mut context, &mut requests);
        let point = draw(&mut context, &mut requests);
        context.io_mut().add_mouse_pos_event(point);
        draw(&mut context, &mut requests);
        context
            .io_mut()
            .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, true);
        draw(&mut context, &mut requests);
        context
            .io_mut()
            .add_mouse_button_event(dear_imgui_rs::MouseButton::Left, false);
        draw(&mut context, &mut requests);
        assert!(
            matches!(requests.as_slice(), [HostAction::SwitchViewer{tab:37,document:DocumentId(9),viewer}] if viewer=="views.text")
        );
    }

    #[test]
    fn local_file_viewers_resolve_without_an_editor_document_kind() {
        let mut registry = compatibility_registry();
        registry
            .register(&module("database", |registrar| {
                registrar.panel_ref("database.panel");
                registrar.local_file_viewer(
                    "database.viewer",
                    "Database",
                    "database.panel",
                    &["db", "sqlite"],
                );
            }))
            .unwrap();
        let viewer = registry.viewer("database.viewer").unwrap();
        assert!(viewer.is_local_file());
        assert_eq!(viewer.document_kind(), None);
        assert_eq!(viewer.single_document_kind(), None);
        assert!(!viewer.supports_document_kind(DocumentKind::Text));
        assert!(!viewer.supports_document_kind(DocumentKind::Bytes));
        assert_eq!(
            registry
                .default_viewer_for_file("DATA.SQLITE", Some(DocumentKind::Bytes))
                .unwrap()
                .id,
            "database.viewer"
        );
        assert_eq!(
            registry
                .default_viewer_for_file("notes.db", Some(DocumentKind::Text))
                .unwrap()
                .id,
            "views.text"
        );
        assert_eq!(
            registry
                .compatible_viewers("data.db", None)
                .iter()
                .map(|viewer| viewer.id)
                .collect::<Vec<_>>(),
            ["database.viewer", "views.hex"]
        );
        assert_eq!(
            registry
                .preferred_viewer_for_file(
                    "data.db",
                    DocumentKind::Bytes,
                    &serde_json::json!({"DB":"views.hex"})
                )
                .unwrap()
                .id,
            "views.hex"
        );
        assert_eq!(
            registry
                .preferred_viewer_for_file(
                    "notes.db",
                    DocumentKind::Text,
                    &serde_json::json!({"db":"database.viewer"})
                )
                .unwrap()
                .id,
            "database.viewer"
        );

        assert_rejected_atomically(
            &mut registry,
            &module("invalid", |registrar| {
                registrar.panel_ref("invalid.panel");
                registrar.local_file_viewer("invalid.viewer", "Invalid", "invalid.panel", &["db"]);
                registrar.registry.viewers.last_mut().unwrap().fallback = true;
            }),
        );
    }

    #[test]
    fn preferences_override_baselines_only_for_available_compatible_viewers() {
        let mut registry = compatibility_registry();
        registry
            .viewers
            .iter_mut()
            .find(|v| v.id == "views.image")
            .unwrap()
            .backing = ViewerBacking::Document {
            kind: DocumentKind::Text,
            supported_kinds: &[DocumentKind::Text, DocumentKind::Bytes],
        };
        for (path, kind, preferences, expected) in [
            (
                "drawing.SVG",
                DocumentKind::Text,
                serde_json::json!({}),
                "views.text",
            ),
            (
                "drawing.SVG",
                DocumentKind::Text,
                serde_json::json!({"svg":"views.image"}),
                "views.image",
            ),
            (
                "drawing.svg",
                DocumentKind::Text,
                serde_json::json!({"SVG":"views.text"}),
                "views.text",
            ),
            (
                "data.csv",
                DocumentKind::Text,
                serde_json::json!({"csv":"views.alternative-table"}),
                "views.alternative-table",
            ),
            (
                "data.csv",
                DocumentKind::Text,
                serde_json::json!({"csv":"missing.viewer"}),
                "views.text",
            ),
            (
                "data.csv",
                DocumentKind::Text,
                serde_json::json!({"csv":"views.model"}),
                "views.text",
            ),
            (
                "data.csv",
                DocumentKind::Bytes,
                serde_json::json!({"csv":"views.table"}),
                "views.hex",
            ),
            (
                "image.png",
                DocumentKind::Bytes,
                serde_json::json!({"png":"views.text"}),
                "views.image",
            ),
            (
                "file.shared",
                DocumentKind::Bytes,
                serde_json::json!({}),
                "views.image",
            ),
            (
                "file.shared",
                DocumentKind::Bytes,
                serde_json::json!({"shared":"views.model"}),
                "views.model",
            ),
        ] {
            assert_eq!(
                registry
                    .preferred_viewer_for_file(path, kind, &preferences)
                    .unwrap()
                    .id,
                expected,
                "{path}, {preferences}"
            );
        }
    }

    #[test]
    fn invalid_registration_is_atomic_and_duplicate_ids_are_rejected() {
        let mut registry = FeatureRegistry::default();
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
