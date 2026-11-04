//! Opening: showing a document in a viewer panel, with state ("at line 40").
//!
//! A viewer is an ordinary panel that shows a document through
//! `DocumentReference`. `register_viewer::<M, P>` pairs a model with the
//! panel type, which is what makes `P` a viewer; with the `files` feature it
//! also says which files the viewer suits, and `open_path` picks one.
//!
//! Opening is the same call whether the document is new, already shown in a
//! panel (Structure moving its editor to a symbol), or open elsewhere: it
//! focuses a viewer already showing the document, or spawns a new one with
//! `Place` (`Target` says which). The state goes to the panel as `Reveal`,
//! which the viewer reads and removes.
use crate::*;
use crate::{Workbench, WorkbenchRef, WorkbenchWorldExt};
use bedtime_document::{DocumentModel, DocumentSource};
use bevy::prelude::*;
use std::any::TypeId;

/// On a viewer panel: show this state of its document (for a text editor, a
/// position or selection). Opening inserts it, on a new panel or one already
/// showing the document; the viewer reads and removes it, once its content
/// is loaded. Inserting it again replaces it, so the latest wins. Viewers of
/// models with nothing to point at (`OpenState = ()`) never get one.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct Reveal<S: Send + Sync + 'static>(pub S);

/// Which panel an open shows the document in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Target {
    /// A panel already showing the document (with the requested viewer) in
    /// the current layout: the one in the focused area, else the first. With
    /// none, a new panel.
    #[default]
    Existing,
    /// This panel, which must be a viewer showing the document.
    Panel(Entity),
    /// Always a new panel.
    New,
}

pub struct OpenOptions<M: DocumentModel> {
    pub state: M::OpenState,
    pub target: Target,
    /// Where a new panel goes. `None` is where documents go: an area with
    /// this viewer's panels, else one with any viewer's, else the largest
    /// (so a file opened from a file tree's sidebar doesn't land in it).
    pub place: Option<Placement>,
    /// Leave the layout's focus where it is. The panel is still its area's
    /// selected tab, so a panel opened into the focused area has it anyway.
    pub keep_focus: bool,
    /// Which viewer; `None` is the default viewer for the model.
    pub viewer: Option<ViewerId>,
    /// A new panel replaces its area's preview panel, and is the preview
    /// until pinned (by editing, or the user keeping it).
    pub preview: bool,
}

impl<M: DocumentModel> Default for OpenOptions<M> {
    fn default() -> Self {
        Self {
            state: default(),
            target: default(),
            place: None,
            keep_focus: false,
            viewer: None,
            preview: false,
        }
    }
}

impl<M: DocumentModel> OpenOptions<M> {
    pub fn at(state: M::OpenState) -> Self {
        Self { state, ..default() }
    }
}

/// A registered viewer: its panel type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ViewerId(TypeId);

impl ViewerId {
    pub fn of<P: 'static>() -> Self {
        Self(TypeId::of::<P>())
    }
}

struct ErasedViewer {
    id: ViewerId,
    label: &'static str,
    model: TypeId,
    /// Insert `Reveal` of an `M::OpenState` on one of its panels.
    reveal: fn(&mut World, Entity, &dyn std::any::Any),
    /// A new viewer panel, placed with `Place`.
    create: fn(&mut World, DocumentId, Place) -> Result<Entity, String>,
    #[cfg(feature = "files")]
    files: Option<FileViewer>,
}

impl ErasedViewer {
    fn of<M: DocumentModel, P: Panel + Default>(label: &'static str) -> Self {
        Self {
            id: ViewerId::of::<P>(),
            label,
            model: TypeId::of::<M>(),
            reveal: |world, panel, state| {
                if TypeId::of::<M::OpenState>() == TypeId::of::<()>() {
                    return;
                }
                if let Some(state) = state.downcast_ref::<M::OpenState>() {
                    world.entity_mut(panel).insert(Reveal(state.clone()));
                }
            },
            create: |world, document, place| {
                world
                    .workbench()
                    .cast_document::<M>(document)
                    .map_err(|e| e.to_string())?;
                Ok(world
                    .spawn((P::default(), DocumentReference::new(document), place))
                    .id())
            },
            #[cfg(feature = "files")]
            files: None,
        }
    }

    /// Whether `panel` is this viewer: its `Panel` type, not one built from
    /// its parts (a diff panel with an editor in it).
    fn shows(&self, world: &World, panel: Entity) -> bool {
        world.get::<PanelInstance>(panel).and_then(|p| p.kind) == Some(self.id.0)
    }
}

#[cfg(feature = "files")]
struct FileViewer {
    /// The formats it shows (and so any that conform to them).
    formats: Vec<FormatId>,
    open: fn(&mut World, &std::path::Path, ResolvedPathOptions) -> Result<Entity, String>,
    /// Show the file in a restored viewer panel that has lost its document.
    reopen: fn(&mut World, Entity, &std::path::Path) -> Result<(), String>,
}

#[cfg(feature = "files")]
impl FileViewer {
    fn of<M: bedtime_filesystem::FileModel>(formats: Vec<FormatId>) -> Self {
        Self {
            formats,
            open: |world, path, options| {
                let source = bedtime_filesystem::Filesystem::local()
                    .document_source(path)
                    .map_err(|e| e.to_string())?;
                let file = source.path();
                let state = match (options.line, options.select_to) {
                    (Some(from), Some(to)) => M::state_selecting(from, to),
                    (Some((line, column)), None) => M::state_at_line(line, column),
                    (None, _) => M::OpenState::default(),
                };
                let handle = world
                    .workbench_mut()
                    .load_document::<M>(source)
                    .map_err(|e| e.to_string())?;
                // In step with any other model's document of this file.
                if !M::UNREAD {
                    world
                        .workbench_mut()
                        .set_byte_codec(handle)
                        .map_err(|e| e.to_string())?;
                }
                let panel = world.workbench_mut().open_document_handle(
                    handle,
                    OpenOptions {
                        state,
                        target: options.target,
                        place: options.place,
                        keep_focus: options.keep_focus,
                        viewer: Some(options.viewer),
                        preview: options.preview,
                    },
                )?;
                world.entity_mut(panel).insert(ViewerFile(file));
                if let Some(document) = world.get::<DocumentReference>(panel).map(|r| r.document())
                {
                    world
                        .entity_mut(document.entity())
                        .insert(DocumentFormat(options.format));
                }
                Ok(panel)
            },
            reopen: |world, panel, path| {
                let source = bedtime_filesystem::Filesystem::local()
                    .document_source(path)
                    .map_err(|e| e.to_string())?;
                let document = world
                    .workbench_mut()
                    .load_document::<M>(source)
                    .map_err(|e| e.to_string())?;
                if !M::UNREAD {
                    world
                        .workbench_mut()
                        .set_byte_codec(document)
                        .map_err(|e| e.to_string())?;
                }
                world
                    .entity_mut(panel)
                    .insert(DocumentReference::new(document.id()));
                Ok(())
            },
        }
    }
}

/// The file a viewer panel shows, saved with the panel: restoring the panel
/// opens the file again, with the viewer the panel is.
#[cfg(feature = "files")]
#[derive(Component, Reflect, Clone, Debug, PartialEq, Eq)]
#[reflect(Component)]
#[cfg_attr(feature = "persistence", reflect(crate::persistence::Persist))]
pub struct ViewerFile(pub std::path::PathBuf);

/// Give restored viewer panels their files back. One that can't be opened is
/// closed, with a notification.
#[cfg(feature = "files")]
pub(crate) fn reopen_viewers(world: &mut World) {
    let restored: Vec<(Entity, std::path::PathBuf)> = world
        .query_filtered::<(Entity, &ViewerFile), Without<DocumentReference>>()
        .iter(world)
        .map(|(panel, file)| (panel, file.0.clone()))
        .collect();
    for (panel, path) in restored {
        let Some(viewers) = world.get_resource::<Viewers>() else {
            return;
        };
        // Only viewers themselves: a panel built from a viewer's parts keeps
        // its own documents.
        let Some(viewer) = viewers.of_panel(world, panel) else {
            continue;
        };
        let result = match viewer.files.as_ref().map(|files| files.reopen) {
            Some(reopen) => reopen(world, panel, &path),
            None => Err("nothing here opens it".into()),
        };
        if let Err(error) = result {
            world.entity_mut(panel).remove::<ViewerFile>();
            let _ = world.layouts_mut().close_panel(panel);
            let name = path.file_name().unwrap_or(path.as_os_str()).display();
            world.workbench_mut().notify(Notification::warning(format!(
                "Couldn't reopen {name}: {error}"
            )));
        }
    }
}

#[cfg(feature = "files")]
struct ResolvedPathOptions {
    line: Option<(u32, u32)>,
    select_to: Option<(u32, u32)>,
    target: Target,
    place: Option<Placement>,
    keep_focus: bool,
    viewer: ViewerId,
    preview: bool,
    format: FormatId,
}

/// The registered viewers.
#[derive(Resource, Default)]
pub struct Viewers {
    viewers: Vec<ErasedViewer>,
    /// Each model's default, for opens that aren't of a file (a document
    /// handle, a new untitled one): see `register_default_viewer`.
    default: Option<ViewerId>,
}

impl Viewers {
    fn get(&self, id: ViewerId) -> Option<&ErasedViewer> {
        self.viewers.iter().find(|viewer| viewer.id == id)
    }

    /// The viewer for model `model`: `requested`, else the default if it's
    /// for this model, else the first registered for it.
    fn for_model(&self, model: TypeId, requested: Option<ViewerId>) -> Option<&ErasedViewer> {
        if let Some(id) = requested {
            return self.get(id).filter(|viewer| viewer.model == model);
        }
        self.default
            .and_then(|id| self.get(id))
            .filter(|viewer| viewer.model == model)
            .or_else(|| self.viewers.iter().find(|viewer| viewer.model == model))
    }

    /// The viewer `panel` is, if it's one.
    fn of_panel(&self, world: &World, panel: Entity) -> Option<&ErasedViewer> {
        self.viewers
            .iter()
            .find(|viewer| viewer.shows(world, panel))
    }

    /// Whether a panel of type `kind` (`PanelInstance::kind`) is a viewer.
    pub fn is_viewer(&self, kind: Option<TypeId>) -> bool {
        kind.is_some_and(|kind| self.viewers.iter().any(|viewer| viewer.id.0 == kind))
    }

    pub fn label(&self, id: ViewerId) -> Option<&'static str> {
        self.get(id).map(|viewer| viewer.label)
    }
}

pub trait ViewersAppExt {
    /// Make panel `P` a viewer of model `M` (labelled "Text Editor", say).
    /// New viewers are `P::default()` with a `DocumentReference`; a state to
    /// show arrives as `Reveal<M::OpenState>`. Add `.formats([..])` to open
    /// files with it.
    fn register_viewer<M: DocumentModel, P: Panel + Default>(
        &mut self,
        label: &'static str,
    ) -> ViewerRegistration<'_, M>;
    /// Open documents of `P`'s model with `P` when no file format says
    /// otherwise: a document handle, a new untitled document. Files use
    /// formats (`set_default_viewer`).
    fn register_default_viewer<P: 'static>(&mut self) -> &mut Self;
}

impl ViewersAppExt for App {
    fn register_viewer<M: DocumentModel, P: Panel + Default>(
        &mut self,
        label: &'static str,
    ) -> ViewerRegistration<'_, M> {
        let viewer = ErasedViewer::of::<M, P>(label);
        let id = viewer.id;
        let mut viewers = self.world_mut().get_resource_or_init::<Viewers>();
        assert!(viewers.get(id).is_none(), "{label} was registered twice");
        viewers.viewers.push(viewer);
        ViewerRegistration {
            app: self,
            id,
            marker: std::marker::PhantomData,
        }
    }

    fn register_default_viewer<P: 'static>(&mut self) -> &mut Self {
        self.world_mut().get_resource_or_init::<Viewers>().default = Some(ViewerId::of::<P>());
        self
    }
}

/// A viewer just registered: `.formats([..])` says which files it opens. It
/// derefs to the `App`, to keep building it.
pub struct ViewerRegistration<'a, M> {
    app: &'a mut App,
    id: ViewerId,
    marker: std::marker::PhantomData<fn() -> M>,
}

impl<'a, M: DocumentModel> ViewerRegistration<'a, M> {
    /// Open files of these formats with this viewer, and of formats that
    /// conform to them: `[TEXT]` is any text, `[DATA]` any file. The closer a
    /// file's format, the better it suits (see `formats`).
    #[cfg(feature = "files")]
    pub fn formats(self, formats: impl IntoIterator<Item = FormatId>) -> &'a mut App
    where
        M: bedtime_filesystem::FileModel,
    {
        let formats = formats.into_iter().collect();
        let mut viewers = self.app.world_mut().resource_mut::<Viewers>();
        let viewer = viewers
            .viewers
            .iter_mut()
            .find(|viewer| viewer.id == self.id)
            .expect("just registered");
        viewer.files = Some(FileViewer::of::<M>(formats));
        self.app
    }
}

impl<M> std::ops::Deref for ViewerRegistration<'_, M> {
    type Target = App;
    fn deref(&self) -> &App {
        self.app
    }
}

impl<M> std::ops::DerefMut for ViewerRegistration<'_, M> {
    fn deref_mut(&mut self) -> &mut App {
        self.app
    }
}

/// A `DocumentReference` put on a viewer of another model is a mistake (the
/// viewer couldn't read it): say so.
pub(crate) fn check_viewer_model(insert: On<Insert<DocumentReference>>, world: &World) {
    let panel = insert.entity;
    let Some(viewers) = world.get_resource::<Viewers>() else {
        return;
    };
    let Some(viewer) = viewers.of_panel(world, panel) else {
        return;
    };
    let Some(reference) = world.get::<DocumentReference>(panel) else {
        return;
    };
    let model = world
        .get::<Document>(reference.document().entity())
        .map(Document::model_type);
    if model.is_some_and(|model| model != viewer.model) {
        error!(
            "Panel {panel} is a {} viewer, but its document is another model",
            viewer.label
        );
    }
}

impl WorkbenchRef {
    /// The viewers for an open document's model ("Open With").
    pub fn viewers_for_document(&self, document: DocumentId) -> Vec<(ViewerId, &'static str)> {
        let Some(model) = self
            .world
            .get::<Document>(document.entity())
            .map(Document::model_type)
        else {
            return Vec::new();
        };
        self.world
            .get_resource::<Viewers>()
            .map(|viewers| {
                viewers
                    .viewers
                    .iter()
                    .filter(|viewer| viewer.model == model)
                    .map(|viewer| (viewer.id, viewer.label))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Workbench {
    /// Show `source`'s document in a viewer, with `options.state` applied.
    /// Returns the panel: the existing one it focused, or the new one.
    pub fn open_document<M: DocumentModel>(
        &mut self,
        source: impl DocumentSource<M>,
        options: OpenOptions<M>,
    ) -> Result<Entity, String> {
        let handle = self.load_document(source).map_err(|e| e.to_string())?;
        self.open_document_handle(handle, options)
    }

    /// Show an open document, as `open_document` does.
    pub fn open_document_handle<M: DocumentModel>(
        &mut self,
        handle: DocumentHandle<M>,
        options: OpenOptions<M>,
    ) -> Result<Entity, String> {
        let document = handle.id();
        let world = &mut self.world;
        let viewer = world
            .get_resource::<Viewers>()
            .and_then(|viewers| viewers.for_model(TypeId::of::<M>(), options.viewer))
            .map(|viewer| viewer.id)
            .ok_or_else(|| format!("Nothing opens {}", std::any::type_name::<M>()))?;
        let panel = match options.target {
            Target::Panel(panel) => {
                let showing = world
                    .get::<DocumentReference>(panel)
                    .is_some_and(|reference| reference.document() == document);
                if !showing || viewer_of(world, panel, TypeId::of::<M>()).is_none() {
                    return Err("That panel doesn't show this document".into());
                }
                show(world, panel, options.keep_focus)?;
                panel
            }
            Target::Existing => match existing_panel(world, document, options.viewer) {
                Some(panel) => {
                    // Opening for good pins a preview.
                    if !options.preview
                        && let Some(mut instance) = world.get_mut::<PanelInstance>(panel)
                    {
                        instance.preview = false;
                    }
                    show(world, panel, options.keep_focus)?;
                    panel
                }
                None => new_panel(world, document, viewer, &options)?,
            },
            Target::New => new_panel(world, document, viewer, &options)?,
        };
        if let Some(reveal) = viewer_of(world, panel, TypeId::of::<M>()) {
            reveal(world, panel, &options.state);
        }
        Ok(panel)
    }

    /// Open the file at `path` as its format (`options.format`, else the one
    /// detected) with the viewer that suits it: `options.viewer` if it can,
    /// else the format's default viewer, else the closest (the first
    /// registered among equals). Records the format on the document. Returns
    /// the panel.
    #[cfg(feature = "files")]
    pub fn open_path(
        &mut self,
        path: &std::path::Path,
        options: PathOptions,
    ) -> Result<Entity, String> {
        let world = &mut self.world;
        let format = options
            .format
            .unwrap_or_else(|| file_format(world, path, &file_head(path)));
        let viewer = choose_file_viewer(world, format, options.viewer)
            .ok_or("nothing here opens this file")?;
        let open = world
            .resource::<Viewers>()
            .get(viewer)
            .unwrap()
            .files
            .as_ref()
            .unwrap()
            .open;
        open(
            world,
            path,
            ResolvedPathOptions {
                line: options.line,
                select_to: options.select_to,
                target: options.target,
                place: options.place,
                keep_focus: options.keep_focus,
                viewer,
                preview: options.preview,
                format,
            },
        )
    }
}

/// Opening documents from `Commands`.
pub trait DocumentsCommandsExt {
    /// Show `handle`'s document as `Workbench::open_document_handle` does,
    /// once the commands apply. Which panel shows it is only known then, so
    /// nothing is returned; a failure is shown as a notification.
    fn open_document<M: DocumentModel>(
        &mut self,
        handle: DocumentHandle<M>,
        options: OpenOptions<M>,
    );
}

impl DocumentsCommandsExt for Commands<'_, '_> {
    fn open_document<M: DocumentModel>(
        &mut self,
        handle: DocumentHandle<M>,
        options: OpenOptions<M>,
    ) {
        self.queue(move |world: &mut World| {
            if let Err(error) = world.workbench_mut().open_document_handle(handle, options) {
                world
                    .workbench_mut()
                    .notify(Notification::error(format!("Couldn't open it: {error}")));
            }
        });
    }
}

#[cfg(feature = "files")]
impl WorkbenchRef {
    /// The viewers that can open the file at `path`, the one it opens with
    /// first, then the closest ("Open With").
    pub fn viewers_for(&self, path: &std::path::Path) -> Vec<(ViewerId, &'static str)> {
        let format = file_format(&self.world, path, &file_head(path));
        let Some(viewers) = self.world.get_resource::<Viewers>() else {
            return Vec::new();
        };
        let chosen = choose_file_viewer(&self.world, format, None);
        let mut candidates: Vec<_> = viewers
            .viewers
            .iter()
            .filter_map(|viewer| {
                let distance = viewer_distance(&self.world, viewer, format)?;
                Some((Some(viewer.id) != chosen, distance, viewer.id, viewer.label))
            })
            .collect();
        candidates.sort_by_key(|&(not_chosen, distance, ..)| (not_chosen, distance));
        candidates
            .into_iter()
            .map(|(.., id, label)| (id, label))
            .collect()
    }

    /// The format the file at `path` would open as.
    pub fn format_of(&self, path: &std::path::Path) -> FormatId {
        file_format(&self.world, path, &file_head(path))
    }
}

/// What a caller that only has a path can say about opening it.
#[cfg(feature = "files")]
#[derive(Clone, Copy, Debug, Default)]
pub struct PathOptions {
    /// From 0; the model turns it into its state.
    pub line: Option<(u32, u32)>,
    /// With `line`: select from it to this line and column.
    pub select_to: Option<(u32, u32)>,
    pub target: Target,
    /// As `OpenOptions::place`.
    pub place: Option<Placement>,
    /// As `OpenOptions::keep_focus`.
    pub keep_focus: bool,
    pub viewer: Option<ViewerId>,
    pub preview: bool,
    /// Open it as this format ("Open As"), not the one detected.
    pub format: Option<FormatId>,
}

#[cfg(feature = "files")]
fn file_head(path: &std::path::Path) -> Vec<u8> {
    use std::io::Read;
    let mut head = Vec::new();
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.take(4096).read_to_end(&mut head);
    }
    head
}

#[cfg(feature = "files")]
fn file_format(world: &World, path: &std::path::Path, head: &[u8]) -> FormatId {
    match world.get_resource::<Formats>() {
        Some(formats) => formats.detect(path, head),
        None => Formats::default().detect(path, head),
    }
}

/// How close `viewer`'s formats are to `format`, if it shows that at all.
#[cfg(feature = "files")]
fn viewer_distance(world: &World, viewer: &ErasedViewer, format: FormatId) -> Option<u32> {
    let formats = world.get_resource::<Formats>()?;
    viewer
        .files
        .as_ref()?
        .formats
        .iter()
        .filter_map(|&shown| formats.distance(format, shown))
        .min()
}

/// The viewer a file of `format` opens with: `requested` if it can show it,
/// else the format's default viewer, else the closest (the first registered
/// among equals).
#[cfg(feature = "files")]
fn choose_file_viewer(
    world: &World,
    format: FormatId,
    requested: Option<ViewerId>,
) -> Option<ViewerId> {
    let viewers = world.get_resource::<Viewers>()?;
    let distance = |id: ViewerId| {
        viewers
            .get(id)
            .and_then(|viewer| viewer_distance(world, viewer, format))
    };
    if let Some(id) = requested {
        return distance(id).map(|_| id);
    }
    let default = world
        .get_resource::<Formats>()
        .and_then(|formats| formats.default_viewer(format))
        .filter(|&id| distance(id).is_some());
    if default.is_some() {
        return default;
    }
    let mut best: Option<(u32, ViewerId)> = None;
    for viewer in &viewers.viewers {
        if let Some(distance) = distance(viewer.id)
            && best.is_none_or(|(best, _)| distance < best)
        {
            best = Some((distance, viewer.id));
        }
    }
    best.map(|(_, id)| id)
}

/// How to reveal a state on `panel`, if it's a viewer for `model`.
fn viewer_of(
    world: &World,
    panel: Entity,
    model: TypeId,
) -> Option<fn(&mut World, Entity, &dyn std::any::Any)> {
    world
        .get_resource::<Viewers>()?
        .viewers
        .iter()
        .find(|viewer| viewer.model == model && viewer.shows(world, panel))
        .map(|viewer| viewer.reveal)
}

/// A viewer in the current layout showing `document` (with `viewer`, if one
/// was asked for): the one in the focused area, else the first.
fn existing_panel(world: &World, document: DocumentId, viewer: Option<ViewerId>) -> Option<Entity> {
    let layout = world.layouts().active()?;
    let viewers = world.get_resource::<Viewers>()?;
    let candidates: Vec<_> = world
        .layouts()
        .panels(layout)
        .into_iter()
        .filter(|&panel| {
            world
                .get::<DocumentReference>(panel)
                .is_some_and(|reference| reference.document() == document)
                && match viewer.and_then(|id| viewers.get(id)) {
                    Some(viewer) => viewer.shows(world, panel),
                    None => viewers.of_panel(world, panel).is_some(),
                }
        })
        .collect();
    let focused = world.get::<Layout>(layout).and_then(Layout::focused_area);
    candidates
        .iter()
        .copied()
        .find(|&panel| world.get::<InArea>(panel).map(|a| a.0) == focused)
        .or_else(|| candidates.first().copied())
}

/// A new viewer panel, focused: in `area`, else where `Placement::Auto` puts
/// it (an area with this viewer's panels, the focused one first, else the
/// largest). A preview replaces that area's preview panel.
fn new_panel<M: DocumentModel>(
    world: &mut World,
    document: DocumentId,
    viewer: ViewerId,
    options: &OpenOptions<M>,
) -> Result<Entity, String> {
    let at = match options.place {
        Some(placement) => placement,
        None => Placement::In(document_area(world, viewer)?),
    };
    let place = Place::at(at).focus(!options.keep_focus);
    let create = world.resource::<Viewers>().get(viewer).unwrap().create;
    let panel = create(world, document, place)?;
    world.flush();
    let area = world
        .get::<InArea>(panel)
        .ok_or("The viewer couldn't be placed")?
        .0;
    if options.preview {
        let replaced = world
            .layouts()
            .panels_in_area(area)
            .iter()
            .copied()
            .find(|&other| {
                other != panel && world.get::<PanelInstance>(other).is_some_and(|p| p.preview)
            });
        world.get_mut::<PanelInstance>(panel).unwrap().preview = true;
        if let Some(old) = replaced {
            world.layouts_mut().close_panel(old)?;
        }
    }
    // Closing the replaced preview may have selected another tab.
    show(world, panel, options.keep_focus)?;
    Ok(panel)
}

/// Where a new viewer goes by default: an area with panels of this viewer
/// (the focused one, else the most recently focused), else one with any
/// viewer's, else the largest. A sidebar (a file tree) isn't a viewer, so
/// opening from it lands beside it, not in it.
fn document_area(world: &mut World, viewer: ViewerId) -> Result<Entity, String> {
    let layout = world.layouts_mut().ensure_active()?;
    let kind = |world: &World, panel| world.get::<PanelInstance>(panel).and_then(|p| p.kind);
    let layouts = world.layouts();
    let viewers = world.resource::<Viewers>();
    layouts
        .area_holding(
            layout,
            |world, panel| kind(world, panel) == Some(viewer.0),
            None,
        )
        .or_else(|| {
            layouts.area_holding(
                layout,
                |world, panel| viewers.is_viewer(kind(world, panel)),
                None,
            )
        })
        .map_or_else(|| layouts.destination_area(layout), Ok)
}

/// Select `panel`'s tab, and focus it unless `keep_focus`.
fn show(world: &mut World, panel: Entity, keep_focus: bool) -> Result<(), String> {
    if keep_focus {
        world.layouts_mut().select_panel(panel)
    } else {
        world.layouts_mut().focus_panel(panel)
    }
}

/// What a viewer panel shows, for drawing it: its document's model (viewers'
/// models are cheap to clone), handle and revision.
pub struct ViewerDocument<M: DocumentModel> {
    pub handle: DocumentHandle<M>,
    pub model: M,
    pub revision: u64,
}

/// The document `panel` shows, if it's ready. While it loads this draws a
/// spinner; if it couldn't be read, why.
pub fn viewer_document<M: DocumentModel>(
    ui: &mut bedtime_ui::egui::Ui,
    world: &World,
    panel: Entity,
) -> Option<ViewerDocument<M>> {
    use bedtime_ui::egui;
    let document = world.get::<DocumentReference>(panel)?.document();
    let workbench = world.workbench();
    let info = workbench.inspect_document(document).ok()?;
    match info.status {
        DocumentStatus::Loading => {
            ui.centered_and_justified(|ui| ui.spinner());
            None
        }
        DocumentStatus::Failed(error) => {
            egui::Frame::NONE.inner_margin(14).show(ui, |ui| {
                ui.label(format!("This file can't be opened: {error}"));
            });
            None
        }
        DocumentStatus::Ready => {
            let handle = workbench.cast_document::<M>(document).ok()?;
            let model = workbench.read_document(handle).ok()?.clone();
            Some(ViewerDocument {
                handle,
                model,
                revision: info.revision,
            })
        }
    }
}

/// Editing a document pins its preview panels.
pub(crate) fn pin_edited_previews(world: &mut World) {
    let dirty: Vec<Entity> = world
        .query::<(Entity, &PanelInstance, &DocumentReference)>()
        .iter(world)
        .filter(|(_, instance, _)| instance.preview)
        .filter(|(_, _, reference)| {
            world
                .workbench()
                .inspect_document(reference.document())
                .is_ok_and(|info| info.dirty)
        })
        .map(|(panel, _, _)| panel)
        .collect();
    for panel in dirty {
        world.get_mut::<PanelInstance>(panel).unwrap().preview = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::layout;
    use bedtime_document::{MemorySource, TextBuffer, TextOpenState, TextPosition};

    /// A text viewer: what it's asked to show stays as its `Reveal`.
    #[bedtime_ui::panel]
    #[derive(Default)]
    struct TextView;

    impl Panel for TextView {
        const LABEL: &'static str = "Text";
        fn draw(
            _: &mut bedtime_ui::egui::Ui,
            _: &mut World,
            _: &bedtime_ui::UiContext<'_>,
            _: Entity,
        ) {
        }
    }

    #[bedtime_ui::panel]
    #[derive(Default)]
    struct HexView;

    impl Panel for HexView {
        const LABEL: &'static str = "Hex";
        fn draw(
            _: &mut bedtime_ui::egui::Ui,
            _: &mut World,
            _: &bedtime_ui::UiContext<'_>,
            _: Entity,
        ) {
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugins)
            .register_viewer::<TextBuffer, TextView>("Text")
            .register_viewer::<TextBuffer, HexView>("Hex")
            .register_default_viewer::<TextView>();
        layout(app.world_mut());
        app
    }

    fn line(line: u32) -> OpenOptions<TextBuffer> {
        OpenOptions::at(TextOpenState {
            cursor: Some(TextPosition::LineColumn { line, column: 0 }),
            ..default()
        })
    }

    /// The line `panel` was last asked to show.
    fn revealed(world: &World, panel: Entity) -> Option<u32> {
        match world.get::<Reveal<TextOpenState>>(panel)?.0.cursor {
            Some(TextPosition::LineColumn { line, .. }) => Some(line),
            _ => None,
        }
    }

    #[test]
    fn an_open_shows_the_document_once_and_moves_the_existing_viewer() {
        let mut app = app();
        let world = app.world_mut();
        let source = MemorySource::new("notes", TextBuffer::from("a\nb\nc"));
        let first = world
            .workbench_mut()
            .open_document(source.clone(), line(1))
            .unwrap();
        assert_eq!(revealed(world, first), Some(1));
        // Opened again: the same panel, moved.
        let again = world
            .workbench_mut()
            .open_document(source.clone(), line(2))
            .unwrap();
        assert_eq!(first, again);
        assert_eq!(revealed(world, first), Some(2));
        let document = world.get::<DocumentReference>(first).unwrap().document();
        assert_eq!(world.workbench().document_ids(), [document]);

        // A named panel; one showing something else is refused.
        let other = world
            .workbench_mut()
            .open_document(
                MemorySource::new("other", TextBuffer::from("x")),
                OpenOptions {
                    target: Target::New,
                    ..line(0)
                },
            )
            .unwrap();
        assert_ne!(other, first);
        assert!(
            world
                .workbench_mut()
                .open_document(
                    source.clone(),
                    OpenOptions {
                        target: Target::Panel(other),
                        ..line(3)
                    }
                )
                .is_err()
        );
        world
            .workbench_mut()
            .open_document(
                source.clone(),
                OpenOptions {
                    target: Target::Panel(first),
                    ..line(3)
                },
            )
            .unwrap();
        assert_eq!(revealed(world, first), Some(3));

        // Another viewer for the same document is a new panel.
        let hex = world
            .workbench_mut()
            .open_document(
                source,
                OpenOptions {
                    viewer: Some(ViewerId::of::<HexView>()),
                    ..default()
                },
            )
            .unwrap();
        assert!(world.get::<HexView>(hex).is_some());
        assert_eq!(world.workbench().viewers_for_document(document).len(), 2);
    }

    /// A panel built from a viewer's parts (a diff panel with an editor in
    /// it) isn't that viewer.
    #[bedtime_ui::panel]
    #[derive(Default)]
    #[require(TextView)]
    struct Wrapper;
    impl Panel for Wrapper {
        const LABEL: &'static str = "Wrapper";
        fn draw(
            _: &mut bedtime_ui::egui::Ui,
            _: &mut World,
            _: &bedtime_ui::UiContext<'_>,
            _: Entity,
        ) {
        }
    }

    #[test]
    fn a_panel_built_from_a_viewer_isnt_that_viewer() {
        let mut app = app();
        let world = app.world_mut();
        let handle = world
            .workbench_mut()
            .load_document(MemorySource::new("notes", TextBuffer::from("a")))
            .unwrap();
        let wrapper = world
            .spawn((Wrapper, DocumentReference::of(handle), Place::auto()))
            .id();
        world.flush();
        let viewer = world
            .workbench_mut()
            .open_document_handle(handle, line(0))
            .unwrap();
        assert_ne!(viewer, wrapper);
        assert!(world.get::<Reveal<TextOpenState>>(wrapper).is_none());
    }

    #[test]
    fn commands_open_documents_too() {
        let mut app = app();
        let handle = app
            .world_mut()
            .workbench_mut()
            .load_document(MemorySource::new("notes", TextBuffer::from("a\nb")))
            .unwrap();
        app.world_mut().commands().open_document(handle, line(1));
        app.world_mut().flush();
        let world = app.world_mut();
        let layout = world.layouts().active().unwrap();
        let panel = world.layouts().find::<TextView>(layout).unwrap();
        assert_eq!(revealed(world, panel), Some(1));
        assert_eq!(world.layouts().focused_panel(layout), Some(panel));
    }

    #[test]
    fn a_preview_replaces_the_areas_preview() {
        let mut app = app();
        let world = app.world_mut();
        let preview = |name: &str| {
            OpenOptions::<TextBuffer> {
                preview: true,
                target: Target::New,
                ..default()
            }
            .with_source(name)
        };
        let (source, options) = preview("one");
        let first = world
            .workbench_mut()
            .open_document(source, options)
            .unwrap();
        assert!(world.get::<PanelInstance>(first).unwrap().preview);
        let (source, options) = preview("two");
        let second = world
            .workbench_mut()
            .open_document(source, options)
            .unwrap();
        assert!(
            world.get_entity(first).is_err(),
            "the old preview is replaced"
        );
        assert!(world.get::<PanelInstance>(second).unwrap().preview);
        // A pinned panel stays.
        world.get_mut::<PanelInstance>(second).unwrap().preview = false;
        let (source, options) = preview("three");
        let third = world
            .workbench_mut()
            .open_document(source.clone(), options)
            .unwrap();
        assert!(world.get_entity(second).is_ok());
        // Opening it for good pins it.
        world
            .workbench_mut()
            .open_document(source, default())
            .unwrap();
        assert!(!world.get::<PanelInstance>(third).unwrap().preview);
    }

    /// A panel that isn't a viewer, like a file tree.
    #[bedtime_ui::panel]
    struct Sidebar;
    impl Panel for Sidebar {
        const LABEL: &'static str = "Sidebar";
        fn draw(
            _: &mut bedtime_ui::egui::Ui,
            _: &mut World,
            _: &bedtime_ui::UiContext<'_>,
            _: Entity,
        ) {
        }
    }

    #[test]
    fn new_viewers_go_where_viewers_are_not_into_a_focused_sidebar() {
        let mut app = app();
        let world = app.world_mut();
        let layout = world.layouts().active().unwrap();
        let main = world.layouts().destination_area(layout).unwrap();
        let side = world
            .layouts_mut()
            .split_area(main, SplitAxis::Horizontal, 2500, [100, 100])
            .unwrap();
        let (big, small) = if world.layouts().destination_area(layout).unwrap() == main {
            (main, side)
        } else {
            (side, main)
        };
        let sidebar = world.spawn((Sidebar, Place::in_area(small))).id();
        world.flush();
        world.layouts_mut().focus_panel(sidebar).unwrap();
        let (source, options) = OpenOptions::<TextBuffer>::default().with_source("one");
        let panel = world
            .workbench_mut()
            .open_document(source, options)
            .unwrap();
        assert_eq!(world.get::<InArea>(panel).unwrap().0, big);
    }

    #[test]
    fn new_viewers_go_where_other_viewers_are_before_the_largest_area() {
        let mut app = app();
        let world = app.world_mut();
        let layout = world.layouts().active().unwrap();
        let main = world.layouts().destination_area(layout).unwrap();
        let side = world
            .layouts_mut()
            .split_area(main, SplitAxis::Horizontal, 2500, [100, 100])
            .unwrap();
        let small = if world.layouts().destination_area(layout).unwrap() == main {
            side
        } else {
            main
        };
        // A hex viewer in the small area: a text viewer joins it, since no
        // text viewer is open, rather than taking the empty large area.
        let (source, options) = OpenOptions::<TextBuffer> {
            target: Target::New,
            place: Some(Placement::In(small)),
            viewer: Some(ViewerId::of::<HexView>()),
            ..default()
        }
        .with_source("one");
        world
            .workbench_mut()
            .open_document(source, options)
            .unwrap();
        let (source, options) = OpenOptions::<TextBuffer>::default().with_source("two");
        let panel = world
            .workbench_mut()
            .open_document(source, options)
            .unwrap();
        assert!(world.get::<TextView>(panel).is_some());
        assert_eq!(world.get::<InArea>(panel).unwrap().0, small);
    }

    #[test]
    fn keep_focus_opens_beside_without_moving_focus() {
        let mut app = app();
        let world = app.world_mut();
        let layout = world.layouts().active().unwrap();
        let (source, options) = OpenOptions::<TextBuffer>::default().with_source("one");
        let editor = world
            .workbench_mut()
            .open_document(source, options)
            .unwrap();
        let options = OpenOptions::<TextBuffer> {
            place: Some(Placement::Beside(editor)),
            keep_focus: true,
            viewer: Some(ViewerId::of::<HexView>()),
            ..default()
        };
        let (source, options) = options.with_source("two");
        let hex = world
            .workbench_mut()
            .open_document(source, options)
            .unwrap();
        assert_ne!(
            world.get::<InArea>(hex).unwrap().0,
            world.get::<InArea>(editor).unwrap().0,
            "the editor's area is split"
        );
        assert_eq!(world.layouts().focused_panel(layout), Some(editor));
    }

    #[test]
    fn new_viewers_go_beside_their_kind_most_recently_focused_first() {
        let mut app = app();
        let world = app.world_mut();
        let layout = world.layouts().active().unwrap();
        let main = world.layouts().destination_area(layout).unwrap();
        let side = world
            .layouts_mut()
            .split_area(main, SplitAxis::Horizontal, 2500, [100, 100])
            .unwrap();
        let small = if world.layouts().destination_area(layout).unwrap() == main {
            side
        } else {
            main
        };
        // A text viewer in the small area: the next goes there too, even
        // with the sidebar focused.
        let (source, options) = OpenOptions::<TextBuffer> {
            target: Target::New,
            place: Some(Placement::In(small)),
            ..default()
        }
        .with_source("one");
        world
            .workbench_mut()
            .open_document(source, options)
            .unwrap();
        let sidebar = world.spawn((Sidebar, Place::in_area(small))).id();
        world.flush();
        world.layouts_mut().focus_panel(sidebar).unwrap();
        let (source, options) = OpenOptions::<TextBuffer>::default().with_source("two");
        let panel = world
            .workbench_mut()
            .open_document(source, options)
            .unwrap();
        assert_eq!(world.get::<InArea>(panel).unwrap().0, small);
    }

    #[cfg(feature = "files")]
    #[test]
    fn untitled_documents_are_saved_where_the_app_says() {
        use crate::documents::tests::{type_text, update_until};
        #[derive(Resource, Default)]
        struct Asked(Vec<DocumentId>);
        let mut app = app();
        app.init_resource::<Asked>();
        let world = app.world_mut();
        world
            .workbench_mut()
            .set_save_as(|world, id| world.resource_mut::<Asked>().0.push(id));
        let panel = world
            .workbench_mut()
            .new_untitled::<TextBuffer>(OpenOptions::default())
            .unwrap();
        let second = world
            .workbench_mut()
            .new_untitled::<TextBuffer>(OpenOptions {
                target: Target::New,
                ..default()
            })
            .unwrap();
        let document = world.get::<DocumentReference>(panel).unwrap().document();
        let other = world.get::<DocumentReference>(second).unwrap().document();
        assert_eq!(world.get::<Untitled>(document.0).unwrap().name, "Untitled");
        assert_eq!(world.get::<Untitled>(other.0).unwrap().name, "Untitled 2");
        update_until(&mut app, |world| {
            world
                .workbench()
                .inspect_document(document)
                .is_ok_and(|info| info.status == DocumentStatus::Ready)
        });
        let world = app.world_mut();
        let handle = world
            .workbench()
            .cast_document::<TextBuffer>(document)
            .unwrap();
        type_text(world, handle, 0, "hello");
        // Saving asks where, once; meanwhile it counts as saving.
        world.workbench_mut().save_document(document).unwrap();
        world.workbench_mut().save_document(document).unwrap();
        assert_eq!(world.resource::<Asked>().0, [document]);
        assert!(world.workbench().inspect_document(document).unwrap().saving);
        let folder = std::env::temp_dir().join(format!("bedtime-untitled-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("notes.md");
        let _ = std::fs::remove_file(&path);
        world
            .workbench_mut()
            .save_untitled_to(document, &path)
            .unwrap();
        assert!(world.get::<Untitled>(document.0).is_none());
        update_until(&mut app, |world| {
            world
                .workbench()
                .inspect_document(document)
                .is_ok_and(|info| !info.dirty && !info.saving)
        });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        let world = app.world();
        assert_eq!(
            world.workbench().inspect_document(document).unwrap().title,
            "notes.md"
        );
        assert!(world.get::<ViewerFile>(panel).is_some());
        std::fs::remove_dir_all(folder).unwrap();
    }

    impl OpenOptions<TextBuffer> {
        fn with_source(self, name: &str) -> (MemorySource<TextBuffer>, Self) {
            (MemorySource::new(name, TextBuffer::from(name)), self)
        }
    }

    #[cfg(feature = "files")]
    #[test]
    fn paths_open_with_the_viewer_of_the_closest_format() {
        const NOTES: FormatId = FormatId("notes");
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugins)
            .register_format(Format::new(NOTES).extensions(["notes"]).conforms_to([TEXT]))
            .register_viewer::<TextBuffer, TextView>("Text")
            .formats([TEXT])
            .register_viewer::<TextBuffer, HexView>("Hex")
            .formats([DATA]);
        layout(app.world_mut());
        let directory = std::env::temp_dir().join(format!("bedtime-open-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let notes = directory.join("notes.txt");
        std::fs::write(&notes, "a\nb\nc").unwrap();
        let world = app.world_mut();
        let labels: Vec<_> = world
            .workbench()
            .viewers_for(&notes)
            .into_iter()
            .map(|(_, label)| label)
            .collect();
        assert_eq!(labels, ["Text", "Hex"]);
        let panel = world
            .workbench_mut()
            .open_path(
                &notes,
                PathOptions {
                    line: Some((2, 0)),
                    ..default()
                },
            )
            .unwrap();
        assert_eq!(revealed(world, panel), Some(2));
        let data = directory.join("data.bin");
        std::fs::write(&data, [0u8, 1]).unwrap();
        let hex = world.workbench_mut().open_path(&data, default()).unwrap();
        assert!(world.get::<HexView>(hex).is_some());
        // The text viewer doesn't show data: an error.
        let unopened = world.workbench_mut().open_path(
            &data,
            PathOptions {
                viewer: Some(ViewerId::of::<TextView>()),
                ..default()
            },
        );
        assert!(unopened.is_err());
        // The document knows its format.
        let document = world.get::<DocumentReference>(panel).unwrap().document();
        assert_eq!(world.workbench().document_format(document), Some(TEXT));

        // A format's default viewer wins over a closer one, and "Open As"
        // asks for another format.
        let notes_file = directory.join("todo.notes");
        std::fs::write(&notes_file, "x").unwrap();
        assert_eq!(world.workbench().format_of(&notes_file), NOTES);
        app.set_default_viewer::<HexView>(NOTES);
        let world = app.world_mut();
        let opened = world
            .workbench_mut()
            .open_path(&notes_file, default())
            .unwrap();
        assert!(world.get::<HexView>(opened).is_some());
        let as_text = world
            .workbench_mut()
            .open_path(
                &notes_file,
                PathOptions {
                    format: Some(TEXT),
                    ..default()
                },
            )
            .unwrap();
        assert!(world.get::<TextView>(as_text).is_some());
        let document = world.get::<DocumentReference>(as_text).unwrap().document();
        assert_eq!(world.workbench().document_format(document), Some(TEXT));

        // A restored viewer has its file but not its document: it gets it back.
        assert_eq!(
            world.get::<ViewerFile>(panel),
            Some(&ViewerFile(notes.canonicalize().unwrap()))
        );
        let area = world.get::<InArea>(panel).unwrap().0;
        let restored = world
            .spawn((TextView, ViewerFile(notes.clone()), InArea(area)))
            .id();
        app.update();
        let world = app.world_mut();
        let reference = world.get::<DocumentReference>(restored).unwrap();
        assert_eq!(
            Some(reference.document()),
            world.get::<DocumentReference>(panel).map(|r| r.document())
        );
        // One whose file is gone is closed, and the user told.
        let gone = world
            .spawn((
                TextView,
                ViewerFile(directory.join("gone.txt")),
                InArea(area),
            ))
            .id();
        app.update();
        assert!(app.world().get_entity(gone).is_err());
        let world = app.world_mut();
        let told = &world.resource::<Notifications>().latest().unwrap().text;
        assert!(told.starts_with("Couldn't reopen gone.txt"), "{told}");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
