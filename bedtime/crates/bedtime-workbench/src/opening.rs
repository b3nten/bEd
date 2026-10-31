//! Opening: showing a document in a viewer panel, with state ("at line 40").
//!
//! It's the same call whether the document is new, already shown in a panel
//! (Structure moving its editor to a symbol), or open elsewhere. Every open
//! ends with a viewer panel and `Viewer::apply` of the state; `Target` says
//! how the panel is found. Openers pair a model with a viewer; with the
//! `files` feature they say which files they open, and `open_path` picks one.
use crate::*;
use crate::{Workbench, WorkbenchRef, WorkbenchWorldExt};
use bedtime_document::{DocumentModel, DocumentSource};
use bevy::prelude::*;
use std::any::TypeId;

/// A panel that shows documents of model `M`.
pub trait Viewer<M: DocumentModel>: Panel {
    /// A new panel showing `document`.
    fn new(document: DocumentHandle<M>) -> Self;
    /// Move to `state`, on a fresh panel or an existing one. A viewer whose
    /// content isn't loaded yet keeps it and applies it once it is.
    fn apply(world: &mut World, panel: Entity, state: &M::OpenState);
}

/// Which panel an open shows the document in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Target {
    /// A panel already showing the document (with the requested opener) in
    /// the current layout: the one in the focused area, else the first. With
    /// none, a new panel.
    #[default]
    Existing,
    /// This panel, which must be a viewer showing the document.
    Panel(Entity),
    /// Always a new panel: in `area`, else the focused one.
    New { area: Option<Entity> },
}

pub struct OpenOptions<M: DocumentModel> {
    pub state: M::OpenState,
    pub target: Target,
    /// Which viewer; `None` is the default opener for the model.
    pub opener: Option<OpenerId>,
    /// A new panel replaces its area's preview panel, and is the preview
    /// until pinned (by editing, or the user keeping it).
    pub preview: bool,
}

impl<M: DocumentModel> Default for OpenOptions<M> {
    fn default() -> Self {
        Self {
            state: default(),
            target: default(),
            opener: None,
            preview: false,
        }
    }
}

impl<M: DocumentModel> OpenOptions<M> {
    pub fn at(state: M::OpenState) -> Self {
        Self { state, ..default() }
    }
}

/// An opener: which viewer panel type it creates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OpenerId(TypeId);

impl OpenerId {
    pub fn of<P: 'static>() -> Self {
        Self(TypeId::of::<P>())
    }
}

/// How well an opener suits a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fit {
    No,
    /// It can show it (a hex viewer, anything).
    Can,
    /// It suits it (an editor, text).
    Preferred,
    /// It's made for this kind of file (an image viewer, a PNG).
    Specific,
}

/// A model and a viewer for it, with a label ("Text Editor") and, optionally,
/// the files it opens.
pub struct Opener<M: DocumentModel> {
    erased: ErasedOpener,
    marker: std::marker::PhantomData<fn() -> M>,
}

impl<M: DocumentModel> Opener<M> {
    pub fn new<P: Viewer<M>>(label: &'static str) -> Self {
        Self {
            erased: ErasedOpener {
                id: OpenerId::of::<P>(),
                label,
                model: TypeId::of::<M>(),
                shows: |world, panel| world.get::<P>(panel).is_some(),
                apply: |world, panel, state| {
                    if let Some(state) = state.downcast_ref::<M::OpenState>() {
                        P::apply(world, panel, state);
                    }
                },
                create: |world, document, area| {
                    let handle = world
                        .workbench()
                        .cast_document::<M>(document)
                        .map_err(|e| e.to_string())?;
                    Ok(world
                        .spawn((
                            P::new(handle),
                            DocumentReference::new(document),
                            InArea(area),
                        ))
                        .id())
                },
                #[cfg(feature = "files")]
                files: None,
            },
            marker: std::marker::PhantomData,
        }
    }

    /// Open files with this opener, as well as they `fit` (given the path
    /// and the file's first bytes).
    #[cfg(feature = "files")]
    pub fn files(mut self, fit: fn(&std::path::Path, &[u8]) -> Fit) -> Self
    where
        M: bedtime_filesystem::FileModel,
    {
        self.erased.files = Some(FileOpener {
            fit,
            open: |world, path, options| {
                let source = bedtime_filesystem::Filesystem::local()
                    .document_source(path)
                    .map_err(|e| e.to_string())?;
                let file = source.path();
                let state = options
                    .line
                    .map_or_else(M::OpenState::default, |(line, column)| {
                        M::state_at_line(line, column)
                    });
                let panel = world.workbench_mut().open_document::<M>(
                    source,
                    OpenOptions {
                        state,
                        target: options.target,
                        opener: Some(options.opener),
                        preview: options.preview,
                    },
                )?;
                world.entity_mut(panel).insert(ViewerFile(file));
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
                world
                    .entity_mut(panel)
                    .insert(DocumentReference::new(document.id()));
                Ok(())
            },
        });
        self
    }
}

struct ErasedOpener {
    id: OpenerId,
    label: &'static str,
    model: TypeId,
    /// Whether a panel is this opener's viewer.
    shows: fn(&World, Entity) -> bool,
    /// Apply an `M::OpenState` to one of its panels.
    apply: fn(&mut World, Entity, &dyn std::any::Any),
    /// A new viewer panel in an area.
    create: fn(&mut World, DocumentId, Entity) -> Result<Entity, String>,
    #[cfg(feature = "files")]
    files: Option<FileOpener>,
}

#[cfg(feature = "files")]
struct FileOpener {
    fit: fn(&std::path::Path, &[u8]) -> Fit,
    open: fn(&mut World, &std::path::Path, ResolvedPathOptions) -> Result<Entity, String>,
    /// Show the file in a restored viewer panel that has lost its document.
    reopen: fn(&mut World, Entity, &std::path::Path) -> Result<(), String>,
}

/// The file a viewer panel shows, saved with the panel: restoring the panel
/// opens the file again, with the opener whose viewer the panel is.
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
        let reopen = world.get_resource::<Openers>().and_then(|openers| {
            openers
                .openers
                .iter()
                .find(|opener| (opener.shows)(world, panel))
                .and_then(|opener| Some(opener.files.as_ref()?.reopen))
        });
        let result = match reopen {
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
    target: Target,
    opener: OpenerId,
    preview: bool,
}

/// The registered openers.
#[derive(Resource, Default)]
pub struct Openers {
    openers: Vec<ErasedOpener>,
    /// Preferred where it fits, over other openers' fit.
    default: Option<OpenerId>,
    #[cfg(feature = "files")]
    fallback: Option<fn(&mut World, &std::path::Path, String)>,
}

impl Openers {
    fn get(&self, id: OpenerId) -> Option<&ErasedOpener> {
        self.openers.iter().find(|opener| opener.id == id)
    }

    /// The opener for model `model`: `requested`, else the default if it's
    /// for this model, else the first registered for it.
    fn for_model(&self, model: TypeId, requested: Option<OpenerId>) -> Option<&ErasedOpener> {
        if let Some(id) = requested {
            return self.get(id).filter(|opener| opener.model == model);
        }
        self.default
            .and_then(|id| self.get(id))
            .filter(|opener| opener.model == model)
            .or_else(|| self.openers.iter().find(|opener| opener.model == model))
    }

    pub fn label(&self, id: OpenerId) -> Option<&'static str> {
        self.get(id).map(|opener| opener.label)
    }
}

pub trait OpenersAppExt {
    fn register_opener<M: DocumentModel>(&mut self, opener: Opener<M>) -> &mut Self;
    /// Prefer opener `P` wherever it fits.
    fn register_default_opener<P: 'static>(&mut self) -> &mut Self;
    /// What happens to a file no opener claims (a notification, say).
    #[cfg(feature = "files")]
    fn set_open_fallback(
        &mut self,
        fallback: fn(&mut World, &std::path::Path, String),
    ) -> &mut Self;
}

impl OpenersAppExt for App {
    fn register_opener<M: DocumentModel>(&mut self, opener: Opener<M>) -> &mut Self {
        let mut openers = self.world_mut().get_resource_or_init::<Openers>();
        assert!(
            openers.get(opener.erased.id).is_none(),
            "{} was registered twice",
            opener.erased.label
        );
        openers.openers.push(opener.erased);
        self
    }

    fn register_default_opener<P: 'static>(&mut self) -> &mut Self {
        self.world_mut().get_resource_or_init::<Openers>().default = Some(OpenerId::of::<P>());
        self
    }

    #[cfg(feature = "files")]
    fn set_open_fallback(
        &mut self,
        fallback: fn(&mut World, &std::path::Path, String),
    ) -> &mut Self {
        self.world_mut().get_resource_or_init::<Openers>().fallback = Some(fallback);
        self
    }
}

impl WorkbenchRef {
    /// The openers for an open document's model ("Open With").
    pub fn openers_for_document(&self, document: DocumentId) -> Vec<(OpenerId, &'static str)> {
        let Some(model) = self
            .world
            .get::<Document>(document.entity())
            .map(Document::model_type)
        else {
            return Vec::new();
        };
        self.world
            .get_resource::<Openers>()
            .map(|openers| {
                openers
                    .openers
                    .iter()
                    .filter(|opener| opener.model == model)
                    .map(|opener| (opener.id, opener.label))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Workbench {
    /// Show `source`'s document in a viewer, with `options.state` applied.
    /// Returns the panel.
    pub fn open_document<M: DocumentModel>(
        &mut self,
        source: impl DocumentSource<M>,
        options: OpenOptions<M>,
    ) -> Result<Entity, String> {
        let document = self.load_document(source).map_err(|e| e.to_string())?.id();
        let world = &mut self.world;
        let opener_id = world
            .get_resource::<Openers>()
            .and_then(|openers| openers.for_model(TypeId::of::<M>(), options.opener))
            .map(|opener| opener.id)
            .ok_or_else(|| format!("Nothing opens {}", std::any::type_name::<M>()))?;
        let panel = match options.target {
            Target::Panel(panel) => {
                let showing = world
                    .get::<DocumentReference>(panel)
                    .is_some_and(|reference| reference.document() == document);
                if !showing || viewer_of(world, panel, TypeId::of::<M>()).is_none() {
                    return Err("That panel doesn't show this document".into());
                }
                world.layouts_mut().focus_panel(panel)?;
                panel
            }
            Target::Existing => match existing_panel(world, document, options.opener) {
                Some(panel) => {
                    // Opening for good pins a preview.
                    if !options.preview
                        && let Some(mut instance) = world.get_mut::<PanelInstance>(panel)
                    {
                        instance.preview = false;
                    }
                    world.layouts_mut().focus_panel(panel)?;
                    panel
                }
                None => new_panel(world, document, opener_id, None, options.preview)?,
            },
            Target::New { area } => new_panel(world, document, opener_id, area, options.preview)?,
        };
        if let Some(apply) = viewer_of(world, panel, TypeId::of::<M>()) {
            apply(world, panel, &options.state);
        }
        Ok(panel)
    }

    /// Open the file at `path` with the opener that suits it: `options.opener`
    /// if it can, else the best fit (the default opener among equals).
    /// Returns the panel. A file nothing opens goes to the app's fallback
    /// (`None`), or is an error without one.
    #[cfg(feature = "files")]
    pub fn open_path(
        &mut self,
        path: &std::path::Path,
        options: PathOptions,
    ) -> Result<Option<Entity>, String> {
        let world = &mut self.world;
        let choice = choose_file_opener(world, path, options.opener);
        let Some(opener) = choice else {
            let reason = format!("Nothing here can open {}", path.display());
            let Some(fallback) = world.get_resource::<Openers>().and_then(|o| o.fallback) else {
                return Err(reason);
            };
            fallback(world, path, reason);
            return Ok(None);
        };
        let open = world
            .resource::<Openers>()
            .get(opener)
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
                target: options.target,
                opener,
                preview: options.preview,
            },
        )
        .map(Some)
    }
}

#[cfg(feature = "files")]
impl WorkbenchRef {
    /// The openers that can open the file at `path`, best first ("Open With").
    pub fn openers_for(&self, path: &std::path::Path) -> Vec<(OpenerId, &'static str)> {
        let head = file_head(path);
        let Some(openers) = self.world.get_resource::<Openers>() else {
            return Vec::new();
        };
        let mut fits: Vec<_> = openers
            .openers
            .iter()
            .filter_map(|opener| {
                let fit = (opener.files.as_ref()?.fit)(path, &head);
                (fit != Fit::No).then_some((fit, opener.id, opener.label))
            })
            .collect();
        fits.sort_by_key(|fit| std::cmp::Reverse(fit.0));
        fits.into_iter().map(|(_, id, label)| (id, label)).collect()
    }
}

/// What a caller that only has a path can say about opening it.
#[cfg(feature = "files")]
#[derive(Clone, Copy, Debug, Default)]
pub struct PathOptions {
    /// From 0; the model turns it into its state.
    pub line: Option<(u32, u32)>,
    pub target: Target,
    pub opener: Option<OpenerId>,
    pub preview: bool,
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
fn choose_file_opener(
    world: &World,
    path: &std::path::Path,
    requested: Option<OpenerId>,
) -> Option<OpenerId> {
    let openers = world.get_resource::<Openers>()?;
    let head = file_head(path);
    let fit = |id: OpenerId| {
        openers
            .get(id)
            .and_then(|opener| opener.files.as_ref())
            .map_or(Fit::No, |files| (files.fit)(path, &head))
    };
    if let Some(id) = requested {
        return (fit(id) != Fit::No).then_some(id);
    }
    // The best fit; among equals the default, else the first registered.
    let mut best: Option<(Fit, OpenerId)> = None;
    for opener in &openers.openers {
        let fit = fit(opener.id);
        let better = match best {
            None => fit != Fit::No,
            Some((best, _)) => fit > best || (fit == best && openers.default == Some(opener.id)),
        };
        if better {
            best = Some((fit, opener.id));
        }
    }
    best.map(|(_, id)| id)
}

/// How to apply state to `panel`, if it's a viewer for `model`.
fn viewer_of(
    world: &World,
    panel: Entity,
    model: TypeId,
) -> Option<fn(&mut World, Entity, &dyn std::any::Any)> {
    world
        .get_resource::<Openers>()?
        .openers
        .iter()
        .find(|opener| opener.model == model && (opener.shows)(world, panel))
        .map(|opener| opener.apply)
}

/// A panel in the current layout showing `document` (with `opener`'s viewer,
/// if one was asked for): the one in the focused area, else the first.
fn existing_panel(world: &World, document: DocumentId, opener: Option<OpenerId>) -> Option<Entity> {
    let layout = world.layouts().active()?;
    let shows = opener.and_then(|id| world.resource::<Openers>().get(id).map(|o| o.shows));
    let candidates: Vec<_> = world
        .layouts()
        .panels(layout)
        .into_iter()
        .filter(|&panel| {
            world
                .get::<DocumentReference>(panel)
                .is_some_and(|reference| reference.document() == document)
                && shows.is_none_or(|shows| shows(world, panel))
        })
        .collect();
    let focused = world.get::<Layout>(layout).and_then(Layout::focused_area);
    candidates
        .iter()
        .copied()
        .find(|&panel| world.get::<InArea>(panel).map(|a| a.0) == focused)
        .or_else(|| candidates.first().copied())
}

/// A new viewer panel: in `area`, else the focused area if it shows a viewer,
/// else the largest area (not a file tree's sidebar). A preview replaces that
/// area's preview panel.
fn new_panel(
    world: &mut World,
    document: DocumentId,
    opener: OpenerId,
    area: Option<Entity>,
    preview: bool,
) -> Result<Entity, String> {
    let layout = world.layouts().active().ok_or("No layout is open")?;
    let area = match area {
        Some(area) => area,
        None => match world
            .get::<Layout>(layout)
            .and_then(Layout::focused_area)
            .filter(|&area| shows_viewer(world, area))
        {
            Some(area) => area,
            None => world.layouts().destination_area(layout)?,
        },
    };
    let replaced = if preview {
        world
            .layouts()
            .panels_in_area(area)
            .iter()
            .copied()
            .find(|&panel| world.get::<PanelInstance>(panel).is_some_and(|p| p.preview))
    } else {
        None
    };
    let create = world.resource::<Openers>().get(opener).unwrap().create;
    let panel = create(world, document, area)?;
    world.flush();
    if let Some(mut instance) = world.get_mut::<PanelInstance>(panel) {
        instance.preview = preview;
    }
    if let Some(old) = replaced {
        world.layouts_mut().close_panel(old)?;
    }
    world.layouts_mut().focus_panel(panel)?;
    Ok(panel)
}

/// Whether any panel in `area` is a viewer.
fn shows_viewer(world: &World, area: Entity) -> bool {
    let Some(openers) = world.get_resource::<Openers>() else {
        return false;
    };
    world.layouts().panels_in_area(area).iter().any(|&panel| {
        openers
            .openers
            .iter()
            .any(|opener| (opener.shows)(world, panel))
    })
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

    /// A text viewer that records the states applied to it.
    #[derive(Component, Default)]
    #[require(PanelInstance = PanelInstance::of::<Self>())]
    struct TextView {
        applied: Vec<TextOpenState>,
    }

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

    impl Viewer<TextBuffer> for TextView {
        fn new(_: DocumentHandle<TextBuffer>) -> Self {
            Self {
                applied: Vec::new(),
            }
        }
        fn apply(world: &mut World, panel: Entity, state: &TextOpenState) {
            world
                .get_mut::<TextView>(panel)
                .unwrap()
                .applied
                .push(state.clone());
        }
    }

    #[derive(Component)]
    #[require(PanelInstance = PanelInstance::of::<Self>())]
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

    impl Viewer<TextBuffer> for HexView {
        fn new(_: DocumentHandle<TextBuffer>) -> Self {
            Self
        }
        fn apply(_: &mut World, _: Entity, _: &TextOpenState) {}
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugins)
            .register_opener(Opener::<TextBuffer>::new::<TextView>("Text"))
            .register_opener(Opener::<TextBuffer>::new::<HexView>("Hex"))
            .register_default_opener::<TextView>();
        layout(app.world_mut());
        app
    }

    fn line(line: u32) -> OpenOptions<TextBuffer> {
        OpenOptions::at(TextOpenState {
            cursor: Some(TextPosition::LineColumn { line, column: 0 }),
            selection: None,
        })
    }

    fn applied(world: &World, panel: Entity) -> Vec<u32> {
        world
            .get::<TextView>(panel)
            .unwrap()
            .applied
            .iter()
            .filter_map(|state| match state.cursor {
                Some(TextPosition::LineColumn { line, .. }) => Some(line),
                _ => None,
            })
            .collect()
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
        // Opened again: the same panel, moved.
        let again = world
            .workbench_mut()
            .open_document(source.clone(), line(2))
            .unwrap();
        assert_eq!(first, again);
        assert_eq!(applied(world, first), [1, 2]);
        let document = world.get::<DocumentReference>(first).unwrap().document();
        assert_eq!(world.workbench().document_ids(), [document]);

        // A named panel; one showing something else is refused.
        let other = world
            .workbench_mut()
            .open_document(
                MemorySource::new("other", TextBuffer::from("x")),
                OpenOptions {
                    target: Target::New { area: None },
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
        assert_eq!(applied(world, first), [1, 2, 3]);

        // Another viewer for the same document is a new panel.
        let hex = world
            .workbench_mut()
            .open_document(
                source,
                OpenOptions {
                    opener: Some(OpenerId::of::<HexView>()),
                    ..default()
                },
            )
            .unwrap();
        assert!(world.get::<HexView>(hex).is_some());
        assert_eq!(world.workbench().openers_for_document(document).len(), 2);
    }

    #[test]
    fn a_preview_replaces_the_areas_preview() {
        let mut app = app();
        let world = app.world_mut();
        let preview = |name: &str| {
            OpenOptions::<TextBuffer> {
                preview: true,
                target: Target::New { area: None },
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
    #[derive(Component)]
    #[require(PanelInstance = PanelInstance::of::<Self>())]
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
        let sidebar = world.spawn((Sidebar, InArea(small))).id();
        world.flush();
        world.layouts_mut().focus_panel(sidebar).unwrap();
        let (source, options) = OpenOptions::<TextBuffer>::default().with_source("one");
        let panel = world
            .workbench_mut()
            .open_document(source, options)
            .unwrap();
        assert_eq!(world.get::<InArea>(panel).unwrap().0, big);
    }

    impl OpenOptions<TextBuffer> {
        fn with_source(self, name: &str) -> (MemorySource<TextBuffer>, Self) {
            (MemorySource::new(name, TextBuffer::from(name)), self)
        }
    }

    #[cfg(feature = "files")]
    #[test]
    fn paths_open_with_the_opener_that_fits() {
        #[derive(Resource, Default)]
        struct Unopened(Vec<String>);
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugins)
            .init_resource::<Unopened>()
            .register_opener(
                Opener::<TextBuffer>::new::<TextView>("Text").files(|path, _| {
                    if path.extension().is_some_and(|e| e == "txt") {
                        Fit::Preferred
                    } else {
                        Fit::No
                    }
                }),
            )
            .register_opener(Opener::<TextBuffer>::new::<HexView>("Hex").files(|_, _| Fit::Can))
            .set_open_fallback(|world, _, reason| {
                world.resource_mut::<Unopened>().0.push(reason);
            });
        layout(app.world_mut());
        let directory = std::env::temp_dir().join(format!("bedtime-open-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let notes = directory.join("notes.txt");
        std::fs::write(&notes, "a\nb\nc").unwrap();
        let world = app.world_mut();
        let labels: Vec<_> = world
            .workbench()
            .openers_for(&notes)
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
            .unwrap()
            .unwrap();
        assert_eq!(applied(world, panel), [2]);
        let data = directory.join("data.bin");
        std::fs::write(&data, [0u8, 1]).unwrap();
        let hex = world
            .workbench_mut()
            .open_path(&data, default())
            .unwrap()
            .unwrap();
        assert!(world.get::<HexView>(hex).is_some());
        // Nothing opens a missing file's directory: the fallback hears.
        let unopened = world.workbench_mut().open_path(
            &directory,
            PathOptions {
                opener: Some(OpenerId::of::<TextView>()),
                ..default()
            },
        );
        assert_eq!(unopened, Ok(None));

        // A restored viewer has its file but not its document: it gets it back.
        assert_eq!(
            world.get::<ViewerFile>(panel),
            Some(&ViewerFile(notes.canonicalize().unwrap()))
        );
        let area = world.get::<InArea>(panel).unwrap().0;
        let restored = world
            .spawn((TextView::default(), ViewerFile(notes.clone()), InArea(area)))
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
                TextView::default(),
                ViewerFile(directory.join("gone.txt")),
                InArea(area),
            ))
            .id();
        app.update();
        assert!(app.world().get_entity(gone).is_err());
        let world = app.world_mut();
        let told = &world.resource::<Notifications>().latest().unwrap().text;
        assert!(told.starts_with("Couldn't reopen gone.txt"), "{told}");
        assert_eq!(world.resource::<Unopened>().0.len(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
