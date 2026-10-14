//! Plugin host requests, preparation, contributions and persistence facts.
#[cfg(feature = "documents")]
use crate::documents::*;
#[cfg(feature = "documents")]
use crate::workbench::{PanelAction, PanelTarget};
use crate::{
    settings::SettingsSection,
    workbench::{PanelPlacement, PanelType, valid_id},
};
#[cfg(feature = "documents")]
use bedtime_ui::TextureHandle;
#[cfg(feature = "documents")]
use dear_imgui_rs::TextureId;
use dear_imgui_rs::Ui;
use serde_json::Value;
#[cfg(feature = "documents")]
use std::{collections::HashMap, path::PathBuf};

/// Identifies a held operation without coupling shared editing to its owner's policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OperationBarrierToken(pub u64);

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

#[cfg(feature = "documents")]
/// An independent frame intention with its origin and shared emission order.
/// Producers capture these facts immediately; hosts retain and resolve them
/// between UI scopes without consulting a later focus or document session.
/// Command and draft handlers use `HostEffects` for effects that must finish
/// within their enclosing operation before another command executes.
#[derive(bevy::prelude::Message, Clone, Debug)]
pub struct HostRequest {
    session_id: bedtime_documents::SessionId,
    source: Option<bevy::prelude::Entity>,
    action: HostAction,
    area: u32,
    sequence: u64,
    layout_epoch: u64,
    tab_index: Option<usize>,
}

#[cfg(feature = "documents")]
impl HostRequest {
    pub fn capture(
        session_id: bedtime_documents::SessionId,
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
            tab_index: None,
        }
    }

    pub fn session_id(&self) -> bedtime_documents::SessionId {
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
    pub fn tab_index(&self) -> Option<usize> {
        self.tab_index
    }
    pub fn with_tab_index(mut self, tab_index: Option<usize>) -> Self {
        self.tab_index = tab_index;
        self
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
            HostAction::NewDocument
            | HostAction::DuplicatePanel { .. }
            | HostAction::OpenTerminalFiles { .. }
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
            | HostAction::OpenFileInstance { .. }
            | HostAction::OpenFileIn { .. }
            | HostAction::PreviewFileIn { .. }
            | HostAction::DropFile { .. }
            | HostAction::ExternalFiles { .. }
            | HostAction::PanelAction { .. }
            | HostAction::Clipboard { .. }
            | HostAction::OpenDocumentViewer { .. }
            | HostAction::OpenFileDialog { .. }
            | HostAction::SaveAsDialog { .. }
            | HostAction::Navigate { .. } => true,
            #[cfg(feature = "language-services")]
            HostAction::NavigateLocation { .. } => true,
            HostAction::OperationBarrier { .. }
            | HostAction::OpenDirectory { .. }
            | HostAction::ChooseDirectory
            | HostAction::Invalidate
            | HostAction::ApplyEdits { .. }
            | HostAction::ApplyEditsWithResult { .. }
            | HostAction::ApplyModuleEditsWithResult { .. }
            | HostAction::Save { .. }
            | HostAction::SaveDocuments { .. }
            | HostAction::SaveAs { .. }
            | HostAction::CommitDocumentsWithResult { .. }
            | HostAction::SaveDocumentsWithResult { .. }
            | HostAction::Undo { .. }
            | HostAction::Redo { .. }
            | HostAction::Notify { .. } => false,
        }
    }
}

#[cfg(feature = "documents")]
pub struct HostContext<'a> {
    /// Whether paths and documents belong to an SSH file provider.
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
/// A frontend-owned snapshot assembled at a callback boundary. It is not a
/// resource and does not make document consumers depend on application state.
#[derive(Clone, Default)]
pub struct FrontendSnapshot {
    pub remote: bool,
    pub active_view: Option<bedtime_documents::ViewId>,
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
impl FrontendSnapshot {
    pub fn capture(world: &bevy::prelude::World) -> Self {
        let documents = world
            .get_resource::<crate::documents::DocumentFrame>()
            .cloned()
            .unwrap_or_default();
        let settings = world
            .get_resource::<crate::settings::SettingsSnapshot>()
            .cloned()
            .unwrap_or_default();
        let presentation = world
            .get_resource::<crate::presentation::PresentationFrame>()
            .cloned()
            .unwrap_or_default();
        let animations = world
            .get_non_send::<bedtime_ui::UiFrameContext>()
            .map_or_else(
                || {
                    settings
                        .get("bedtime.ui", "ui_animations")
                        .and_then(Value::as_bool)
                        .unwrap_or_else(|| {
                            bedtime_ui::appearance::UiTheme::default().motion.enabled
                        })
                },
                |context| context.theme().motion.enabled,
            );
        Self {
            remote: world
                .get_resource::<bedtime_files::FileContext>()
                .is_some_and(|context| {
                    matches!(context.provider, bedtime_files::FileProviderId::Ssh { .. })
                }),
            active_view: documents.active_view,
            documents: documents.documents,
            active_document: documents.active_document,
            session_id: documents.session_id,
            diagnostics: documents.diagnostics,
            settings: settings.settings,
            default_viewers: settings.default_viewers,
            textures: presentation.textures,
            animations,
        }
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
    NewDocument,
    DuplicatePanel {
        panel: bevy::prelude::Entity,
    },
    OperationBarrier {
        token: OperationBarrierToken,
    },
    OpenDirectory {
        location: bedtime_files::FileLocation,
    },
    ChooseDirectory,
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
        view: bedtime_documents::session::ViewId,
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
    OpenFileInstance {
        path: String,
        viewer: Option<String>,
        additional: bool,
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
    /// Native file drag facts, resolved by the same owner as other panel requests.
    ExternalFiles {
        panel: Option<bevy::prelude::Entity>,
        event: crate::workbench::ExternalFileDrag,
    },
    PanelAction {
        panel: bevy::prelude::Entity,
        action: PanelAction,
    },
    Clipboard {
        panel: bevy::prelude::Entity,
        action: crate::workbench::ClipboardAction,
    },
    /// Open another viewer tab for the same document, retaining the originating tab.
    OpenDocumentViewer {
        /// The originating tab identity; its tab group receives the new viewer.
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
        edits: Vec<bedtime_documents::session::ByteEdit>,
    },
    ApplyEditsWithResult {
        token: EditToken,
        document: DocumentId,
        revision: Revision,
        edits: Vec<bedtime_documents::session::ByteEdit>,
    },
    /// Flush panel drafts, validate the revision, and acknowledge to a module.
    ApplyModuleEditsWithResult {
        recipient: String,
        token: EditToken,
        document: DocumentId,
        revision: Revision,
        edits: Vec<bedtime_documents::session::ByteEdit>,
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
    SaveDocuments {
        documents: Vec<DocumentId>,
    },
    SaveAs {
        document: DocumentId,
        path: PathBuf,
    },
    Undo {
        document: DocumentId,
    },
    Redo {
        document: DocumentId,
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

#[derive(Clone, Default, bevy::prelude::Resource)]
pub struct FeatureRegistry {
    commands: Vec<CommandDescriptor>,
    menus: Vec<MenuContribution>,
    toolbar: Vec<ToolbarContribution>,
    settings: Vec<SettingsSection>,
    panels: Vec<PanelType>,
    #[cfg(feature = "documents")]
    viewers: Vec<FileViewer>,
    plugins: std::collections::HashSet<&'static str>,
}

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

    #[cfg(feature = "documents")]
    pub fn viewers(&self) -> &[FileViewer] {
        &self.viewers
    }

    fn contribution_ids(&self) -> impl Iterator<Item = &'static str> + '_ {
        let ids = self
            .commands
            .iter()
            .map(|entry| entry.id)
            .chain(self.panels.iter().map(|entry| entry.id))
            .chain(self.settings.iter().map(|entry| entry.id));
        #[cfg(feature = "documents")]
        let ids = ids.chain(self.viewers.iter().map(|entry| entry.id));
        ids
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
        crate::workbench::validate_contribution_ids(
            owner,
            pending.contribution_ids(),
            self.contribution_ids(),
        )?;
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
        #[cfg(feature = "documents")]
        {
            for viewer in &pending.viewers {
                if !pending.panels.iter().any(|x| x.id == viewer.panel_type) {
                    return Err(format!("Missing viewer panel: {}", viewer.panel_type));
                }
            }
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
            }
        }
        self.commands.extend(pending.commands);
        self.menus.extend(pending.menus);
        self.toolbar.extend(pending.toolbar);
        self.settings.extend(pending.settings);
        self.panels.extend(pending.panels);
        #[cfg(feature = "documents")]
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

    #[cfg(feature = "documents")]
    pub fn viewer(&self, id: &str) -> Option<&FileViewer> {
        self.viewers.iter().find(|entry| entry.id == id)
    }

    #[cfg(feature = "documents")]
    pub fn default_viewer(&self, kind: DocumentKind) -> Option<&FileViewer> {
        self.viewers
            .iter()
            .find(|entry| entry.fallback && entry.document_kind() == Some(kind))
    }

    /// Registration order resolves conflicts; no generic wildcard masks text fallback.
    #[cfg(feature = "documents")]
    pub fn viewer_for_path(&self, path: &str) -> Option<&FileViewer> {
        self.viewers.iter().find(|viewer| viewer.matches_path(path))
    }

    /// Text prefers its fallback editor, then a matching compatible format.
    /// Binary prefers a matching format, then hex. Unknown content has no default.
    #[cfg(feature = "documents")]
    pub fn default_viewer_for_file(
        &self,
        path: &str,
        content_kind: Option<DocumentKind>,
    ) -> Option<&FileViewer> {
        let kind = content_kind?;
        let matching = self.viewers.iter().find(|viewer| {
            viewer.matches_path(path)
                && (viewer.is_local_file() || viewer.supports_document_kind(kind))
        });
        let fallback = self.default_viewer(kind);
        match kind {
            DocumentKind::Text => fallback.or(matching),
            DocumentKind::Bytes => matching.or(fallback),
        }
    }

    /// Preferences select presentations, while classification owns document storage.
    #[cfg(feature = "documents")]
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
    #[cfg(feature = "documents")]
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

pub struct FeatureRegistrar<'a> {
    owner: &'static str,
    registry: &'a mut FeatureRegistry,
}

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
            draw_page: None,
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
            draw_page: None,
        });
    }

    pub fn settings_page(
        &mut self,
        id: &'static str,
        label: &'static str,
        draw: crate::settings::SettingsPageDraw,
    ) {
        self.registry.settings.push(SettingsSection {
            id,
            plugin: self.owner,
            label,
            draw: None,
            draw_page: Some(draw),
        });
    }

    pub fn panel_ref(&mut self, id: &'static str) {
        self.registry.panels.push(PanelType {
            empty_input: true,
            singleton: false,
            placement: PanelPlacement::Center,
            open_placement: PanelPlacement::Center,
            id,
            plugin: self.owner,
        });
    }

    pub fn panel_options(&mut self, id: &'static str, singleton: bool, placement: PanelPlacement) {
        self.panel_ref(id);
        let panel = self.registry.panels.last_mut().unwrap();
        panel.singleton = singleton;
        panel.placement = placement;
        panel.open_placement = placement;
    }

    /// Require document or file input before creating this panel.
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

    #[cfg(feature = "documents")]
    pub fn fallback_viewer(
        &mut self,
        id: &'static str,
        label: &'static str,
        panel_type: &'static str,
        kind: DocumentKind,
    ) {
        self.viewer(id, label, panel_type, &[], kind);
        let viewer = self.registry.viewers.last_mut().unwrap();
        viewer.fallback = true;
    }

    #[cfg(feature = "documents")]
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

    #[cfg(feature = "documents")]
    pub fn local_file_viewer(
        &mut self,
        id: &'static str,
        label: &'static str,
        panel_type: &'static str,
        extensions: &'static [&'static str],
    ) {
        self.registry.viewers.push(FileViewer {
            fallback: false,
            id,
            plugin: self.owner,
            label,
            panel_type,
            extensions,
            backing: ViewerBacking::LocalFile,
        });
    }

    #[cfg(feature = "documents")]
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
                );
                registrar.fallback_viewer(
                    "editor.hex-viewer",
                    "Hex",
                    "editor.hex",
                    DocumentKind::Bytes,
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
        assert!(registry.viewer("bedtime.text").is_none());
        assert!(registry.viewer("bedtime.hex").is_none());
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
                );
                registrar.fallback_viewer(
                    "editor.second",
                    "Second",
                    "editor.panel",
                    DocumentKind::Text,
                );
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
                registrar.fallback_viewer("views.text", "Text", "views.panel", DocumentKind::Text);
                registrar.fallback_viewer("views.hex", "Hex", "views.panel", DocumentKind::Bytes);
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
    fn matching_text_viewers_work_without_a_fallback_editor() {
        let mut registry = FeatureRegistry::default();
        registry
            .register(&module("standalone", |registrar| {
                registrar.panel_ref("standalone.panel");
                registrar.viewer(
                    "standalone.table",
                    "Table",
                    "standalone.panel",
                    &["csv"],
                    DocumentKind::Text,
                );
                registrar.viewer(
                    "standalone.image",
                    "Image",
                    "standalone.panel",
                    &["svg", "png"],
                    DocumentKind::Bytes,
                );
                registrar.viewer_document_kinds(
                    "standalone.image",
                    &[DocumentKind::Text, DocumentKind::Bytes],
                );
            }))
            .unwrap();
        for (path, kind, expected) in [
            (
                "results.csv",
                Some(DocumentKind::Text),
                Some("standalone.table"),
            ),
            (
                "chart.svg",
                Some(DocumentKind::Text),
                Some("standalone.image"),
            ),
            (
                "chart.png",
                Some(DocumentKind::Bytes),
                Some("standalone.image"),
            ),
            ("binary.csv", Some(DocumentKind::Bytes), None),
            ("README", Some(DocumentKind::Text), None),
            ("results.csv", None, None),
        ] {
            assert_eq!(
                registry
                    .default_viewer_for_file(path, kind)
                    .map(|viewer| viewer.id),
                expected,
                "{path}"
            );
        }
        assert_eq!(
            registry
                .preferred_viewer_for_file(
                    "results.csv",
                    DocumentKind::Text,
                    &serde_json::json!({"csv":"unavailable.editor"}),
                )
                .map(|viewer| viewer.id),
            Some("standalone.table")
        );
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
    fn viewer_menu_queues_another_viewer_from_its_originating_tab() {
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
                    ui.open_popup("Open another viewer");
                    if let Some(_popup) = ui.begin_popup("Open another viewer") {
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
            matches!(requests.as_slice(), [HostAction::OpenDocumentViewer{tab:37,document:DocumentId(9),viewer}] if viewer=="views.text")
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
