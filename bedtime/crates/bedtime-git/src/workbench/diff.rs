//! Diff panels: an editor on one file with its changes inline. The working
//! changes are compared over the file itself (edit it there); the staged
//! changes over a read-only copy of what's staged. Old lines show between
//! the new, and each change has Stage, Unstage or Discard… above it, which
//! apply exactly that change (`StageRange`, …): staging first checks that
//! what's on disk is still what was compared. The comparison is read again
//! after every Git scan.
use super::*;
use bedtime_code::{
    ComparisonRequest, EditorComparison, EditorPanel,
    editor::diff::{ChangeRange, Comparison, DiffAction, DiffRequest},
};
use bedtime_document::{
    DocumentSource, MemorySource, SourceError, SourceId, SourceLoad, SourceVersion, TextBuffer,
};
use bedtime_ui::{BuiltinIcon, UiContext, egui, theme};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;

const STAGE: u64 = 1;
const UNSTAGE: u64 = 2;
const DISCARD: u64 = 3;

/// Which file, and which side, a diff panel shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Descriptor {
    pub root: PathBuf,
    pub path: String,
    pub side: DiffSide,
}

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(EditorPanel)]
pub struct DiffPanel {
    pub root: PathBuf,
    /// Relative to the repository.
    pub path: String,
    /// The staged changes, else the working ones.
    pub staged: bool,
    #[reflect(ignore)]
    loaded: Option<Result<Arc<Loaded>, String>>,
    #[reflect(ignore)]
    task: Option<(u64, ManagedTask<Result<Loaded, String>>)>,
    #[reflect(ignore)]
    scan: u64,
    /// The staged text, for the staged side.
    #[reflect(ignore)]
    staged_text: Option<MemorySource<TextBuffer>>,
    /// The comparison and enabled actions last given to the editor.
    #[reflect(ignore)]
    shown: Option<(u64, Vec<bool>)>,
    /// A Discard… clicked: asked about before it's done.
    #[reflect(ignore)]
    confirm: Option<DiffRequest>,
    #[reflect(ignore)]
    error: Option<String>,
}

pub(super) struct Loaded {
    identity: u64,
    git: crate::Diff,
    comparison: Option<Comparison>,
    current: Option<TextBuffer>,
    exists: bool,
}

impl DiffPanel {
    pub fn new(descriptor: Descriptor) -> Self {
        Self {
            root: descriptor.root,
            path: descriptor.path,
            staged: descriptor.side == DiffSide::Staged,
            ..default()
        }
    }

    fn descriptor(&self) -> Descriptor {
        Descriptor {
            root: self.root.clone(),
            path: self.path.clone(),
            side: self.side(),
        }
    }

    fn side(&self) -> DiffSide {
        if self.staged {
            DiffSide::Staged
        } else {
            DiffSide::Unstaged
        }
    }

    fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    fn title(&self) -> String {
        let side = if self.staged { "Staged" } else { "Changes" };
        format!("{} ({side})", self.name())
    }

    /// The Git diff compared, once read.
    pub fn diff(&self) -> Option<&crate::Diff> {
        self.loaded
            .as_ref()?
            .as_ref()
            .ok()
            .map(|loaded| &loaded.git)
    }
}

/// Text that can't be saved: what's staged, shown read-only.
struct Staged(MemorySource<TextBuffer>);

impl DocumentSource<TextBuffer> for Staged {
    fn id(&self) -> SourceId {
        self.0.id()
    }
    fn title(&self) -> String {
        self.0.title()
    }
    fn load(&self) -> Result<SourceLoad<TextBuffer>, SourceError> {
        self.0.load()
    }
    fn save(
        &self,
        _: TextBuffer,
        _: Option<SourceVersion>,
    ) -> Result<Option<SourceVersion>, SourceError> {
        Err(SourceError::Failed(
            "What's staged isn't edited here".into(),
        ))
    }
    fn writable(&self) -> bool {
        false
    }
}

/// Show a file's diff: the panel already showing it, else in place of the
/// preview diff (unless it has unsaved edits), else a new one
/// (`diff_area`). `pin` keeps it (not a preview) and focuses it; a preview
/// leaves the Git panel focused.
pub(super) fn open(
    world: &mut World,
    origin: Entity,
    descriptor: Descriptor,
    pin: bool,
) -> Result<Entity, String> {
    let layout = world.layouts().active().ok_or("No layout is open")?;
    let panels = world.layouts().panels(layout);
    if let Some(&panel) = panels.iter().find(|&&panel| {
        world
            .get::<DiffPanel>(panel)
            .is_some_and(|diff| diff.descriptor() == descriptor)
    }) {
        if pin && let Some(mut instance) = world.get_mut::<PanelInstance>(panel) {
            instance.preview = false;
        }
        world.layouts_mut().focus_panel(panel)?;
        return Ok(panel);
    }
    let preview = panels.iter().copied().find(|&panel| {
        world.get::<DiffPanel>(panel).is_some()
            && world.get::<PanelInstance>(panel).is_some_and(|p| p.preview)
            && world
                .get::<DocumentReference>(panel)
                .is_none_or(|reference| {
                    world
                        .workbench()
                        .inspect_document(reference.document())
                        .is_ok_and(|info| !info.dirty)
                })
    });
    let area = preview.and_then(|panel| world.get::<InArea>(panel).map(|area| area.0));
    if let Some(old) = preview {
        world.layouts_mut().close_panel(old)?;
    }
    let area = match area {
        Some(area) => area,
        None => diff_area(world, layout, origin)?,
    };
    // A preview leaves the Git panel focused (it's still the selected tab).
    let place = Place::in_area(area).focus(pin);
    let panel = world.spawn((DiffPanel::new(descriptor), place)).id();
    world.flush();
    if world.get::<InArea>(panel).is_none() {
        return Err("The diff couldn't be placed".into());
    }
    let title = world.get::<DiffPanel>(panel).unwrap().title();
    if let Some(mut instance) = world.get_mut::<PanelInstance>(panel) {
        instance.title = title;
        instance.preview = !pin;
    }
    Ok(panel)
}

/// Where a new diff goes: beside editors or diffs; else the largest area
/// without the Git panel or a file tree; else a split of the Git panel's
/// area (the diff on the right).
fn diff_area(world: &mut World, layout: Entity, origin: Entity) -> Result<Entity, String> {
    let layouts = world.layouts();
    let beside = |world: &World, panel: Entity| {
        world.get::<DiffPanel>(panel).is_some() || world.get::<DocumentReference>(panel).is_some()
    };
    let areas = world
        .get::<LayoutAreas>(layout)
        .map(|areas| areas.entities().to_vec())
        .unwrap_or_default();
    if areas.iter().any(|&area| {
        layouts
            .panels_in_area(area)
            .iter()
            .any(|&p| beside(world, p))
    }) {
        return layouts.area_for(layout, beside);
    }
    let free = areas
        .iter()
        .copied()
        .filter(|&area| {
            layouts.panels_in_area(area).iter().all(|&p| {
                world.get::<GitPanel>(p).is_none()
                    && world.get::<bedtime_filetree::FileTree>(p).is_none()
            })
        })
        .max_by_key(|&area| {
            let r = world.get::<Area>(area).unwrap().rect;
            (r.max[0] - r.min[0]) * (r.max[1] - r.min[1])
        });
    if let Some(area) = free {
        return Ok(area);
    }
    let source = world
        .get::<InArea>(origin)
        .ok_or("The Git panel has no area")?
        .0;
    let r = world.get::<Area>(source).unwrap().rect;
    world.layouts_mut().split_area(
        source,
        SplitAxis::Horizontal,
        r.min[0] + (r.max[0] - r.min[0]) * 2 / 5,
        [1, 1],
    )
}

/// Read a diff, as a comparison over what's compared (the working file, or
/// what's staged). Git's hunks are the changes until the text is edited.
fn load(descriptor: Descriptor, identity: u64, cancel: &Cancellation) -> Result<Loaded, String> {
    let Output::Diff(git) = git_job(
        descriptor.root.clone(),
        Operation::Diff {
            path: descriptor.path.clone(),
            side: descriptor.side,
        },
        cancel,
        &AtomicBool::new(false),
    )?
    else {
        return Err("Git answered with something else".into());
    };
    let exists = descriptor.root.join(&descriptor.path).is_file();
    let (comparison, current) = match (
        git.binary,
        std::str::from_utf8(&git.old_bytes),
        std::str::from_utf8(&git.new_bytes),
    ) {
        (false, Ok(old), Ok(new)) => {
            let baseline = TextBuffer::from(old);
            let current = TextBuffer::from(new);
            let ranges = git
                .hunks
                .iter()
                .map(|h| ChangeRange {
                    old: h.old_start..h.old_start + h.old_count,
                    new: h.new_start..h.new_start + h.new_count,
                })
                .collect();
            let comparison = Comparison::new(identity, baseline.clone())
                .with_saved(current.clone(), ranges)
                .unwrap_or_else(|_| Comparison::new(identity, baseline));
            (Some(comparison), Some(current))
        }
        _ => (None, None),
    };
    Ok(Loaded {
        identity,
        git,
        comparison,
        current,
        exists,
    })
}

/// Bare CR line endings: Git's rows and the editor's disagree, so changes
/// can only be applied to the whole file.
fn bare_cr(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .enumerate()
        .any(|(i, b)| *b == b'\r' && bytes.get(i + 1) != Some(&b'\n'))
}

fn actions(loaded: &Loaded, enabled: bool, clean: bool) -> Vec<DiffAction> {
    let supported = loaded.git.can_apply_hunks
        && !bare_cr(&loaded.git.old_bytes)
        && !bare_cr(&loaded.git.new_bytes);
    let ids = if loaded.git.side == DiffSide::Staged {
        vec![(UNSTAGE, "Unstage")]
    } else {
        vec![(STAGE, "Stage"), (DISCARD, "Discard…")]
    };
    ids.into_iter()
        .map(|(id, label)| DiffAction {
            id,
            label: label.into(),
            enabled: enabled && supported && (id != DISCARD || clean),
        })
        .collect()
}

/// Read each diff panel's comparison after every scan, give it its
/// document, and act on its clicks.
pub(super) fn update(world: &mut World, session: &mut GitSession) {
    let panels: Vec<Entity> = world
        .query_filtered::<Entity, With<DiffPanel>>()
        .iter(world)
        .collect();
    for panel in panels {
        // Titled by what it compares.
        let title = world.get::<DiffPanel>(panel).unwrap().title();
        if let Some(mut instance) = world.get_mut::<PanelInstance>(panel)
            && instance.title != title
        {
            instance.title = title;
        }
        // A finished read.
        let mut diff = world.get_mut::<DiffPanel>(panel).unwrap();
        let mut fresh = false;
        if let Some((scan, task)) = diff.task.take() {
            match task.try_recv() {
                Ok(result) => {
                    diff.scan = scan;
                    // The same changes as before keep their comparison: a new
                    // identity would have the editor drop and rebuild its
                    // inline rows, flickering after every scan.
                    let unchanged = matches!(
                        (&diff.loaded, &result),
                        (Some(Ok(old)), Ok(new)) if old.git == new.git && old.exists == new.exists
                    );
                    if !unchanged {
                        diff.loaded = Some(result.map(Arc::new));
                        fresh = true;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => diff.task = Some((scan, task)),
                Err(_) => diff.loaded = Some(Err("Reading the diff stopped".into())),
            }
        }
        // A new scan: read it again.
        let stale = diff.loaded.is_none() || diff.scan != session.scans;
        if stale && diff.task.is_none() && session.scan.is_none() && session.pending.is_none() {
            let descriptor = diff.descriptor();
            let scan = session.scans;
            let identity = scan.max(1) * 2 + u64::from(descriptor.side == DiffSide::Staged);
            let task = world.resource_mut::<ManagedWork>().spawn_task(
                Some(panel),
                "Read Git diff",
                move |cancel| load(descriptor, identity, &cancel),
            );
            let mut diff = world.get_mut::<DiffPanel>(panel).unwrap();
            match task {
                Ok(task) => diff.task = Some((scan, task)),
                Err(error) => diff.loaded = Some(Err(error)),
            }
        }
        if fresh {
            if hand_off(world, panel) {
                continue;
            }
            attach(world, panel);
        }
        show(world, session, panel);
        if let Some(ComparisonRequest(clicked)) =
            world.entity_mut(panel).take::<ComparisonRequest>()
        {
            if clicked.action == DISCARD {
                world.get_mut::<DiffPanel>(panel).unwrap().confirm = Some(clicked);
            } else if let Err(error) = request(world, session, panel, clicked) {
                world.get_mut::<DiffPanel>(panel).unwrap().error = Some(error);
            }
        }
    }
}

/// A file Git can't compare as text (an image, say) opens in whatever views
/// it, in the diff's place. A deleted one, a symlink or a submodule stays,
/// summarized: a viewer must never follow a link's target.
fn hand_off(world: &mut World, panel: Entity) -> bool {
    let diff = world.get::<DiffPanel>(panel).unwrap();
    let Some(Ok(loaded)) = &diff.loaded else {
        return false;
    };
    let file = diff.root.join(&diff.path);
    let regular = std::fs::symlink_metadata(&file).is_ok_and(|m| m.file_type().is_file());
    if loaded.comparison.is_some() || !regular {
        return false;
    }
    let Some(area) = world.get::<InArea>(panel).map(|area| area.0) else {
        return false;
    };
    let preview = world
        .get::<PanelInstance>(panel)
        .is_some_and(|instance| instance.preview);
    // Like a preview diff, a preview viewer leaves the Git panel focused.
    let options = PathOptions {
        target: Target::New,
        place: Some(bedtime_workbench::Placement::In(area)),
        keep_focus: preview,
        preview,
        ..default()
    };
    match world.workbench_mut().open_path(&file, options) {
        Ok(_) => {}
        Err(error) => {
            world.get_mut::<DiffPanel>(panel).unwrap().error = Some(error);
            return false;
        }
    }
    // A preview viewer already replaced a preview diff.
    if world.get::<DiffPanel>(panel).is_some() {
        let _ = world.layouts_mut().close_panel(panel);
    }
    true
}

/// Give the panel its document: the working file, or what's staged.
fn attach(world: &mut World, panel: Entity) {
    let diff = world.get::<DiffPanel>(panel).unwrap();
    let Some(Ok(loaded)) = diff.loaded.clone() else {
        return;
    };
    let (file, staged) = (diff.root.join(&diff.path), diff.staged);
    let name = diff.name().to_owned();
    let existing = diff.staged_text.clone();
    let has_document = world.get::<DocumentReference>(panel).is_some();
    let result: Result<(), String> = if !staged && loaded.exists {
        if has_document && world.get::<ViewerFile>(panel).is_some_and(|f| f.0 == file) {
            Ok(())
        } else {
            open_file(world, panel, &file)
        }
    } else {
        // What's staged (or a deleted file's last text), read-only.
        let text = loaded.current.clone().unwrap_or_default();
        match existing {
            Some(source) if has_document => {
                if source.get().as_ref() != Some(&text) {
                    source.set(text);
                    if let Some(document) =
                        world.get::<DocumentReference>(panel).map(|r| r.document())
                    {
                        let _ = world.workbench_mut().refresh_document(document);
                    }
                }
                Ok(())
            }
            _ => {
                let source = MemorySource::new(name, text);
                world
                    .workbench_mut()
                    .load_document::<TextBuffer>(Staged(source.clone()))
                    .map_err(|e| e.to_string())
                    .map(|document| {
                        world.entity_mut(panel).remove::<ViewerFile>();
                        world
                            .entity_mut(panel)
                            .insert(DocumentReference::new(document.id()));
                        world.get_mut::<DiffPanel>(panel).unwrap().staged_text = Some(source);
                    })
            }
        }
    };
    if let Err(error) = result {
        world.get_mut::<DiffPanel>(panel).unwrap().error = Some(error);
    }
}

/// The working file's document, shared with any editor of it.
fn open_file(world: &mut World, panel: Entity, file: &Path) -> Result<(), String> {
    let source = bedtime_filesystem::Filesystem::local()
        .document_source(file)
        .map_err(|e| e.to_string())?;
    let canonical = source.path();
    let document = world
        .workbench_mut()
        .load_document::<TextBuffer>(source)
        .map_err(|e| e.to_string())?;
    world
        .entity_mut(panel)
        .insert((DocumentReference::new(document.id()), ViewerFile(canonical)));
    Ok(())
}

/// Give the editor the comparison, with its actions enabled as they can be.
fn show(world: &mut World, session: &GitSession, panel: Entity) {
    let diff = world.get::<DiffPanel>(panel).unwrap();
    let comparison = match &diff.loaded {
        Some(Ok(loaded)) => loaded.comparison.clone().map(|c| (loaded.clone(), c)),
        _ => None,
    };
    let Some((loaded, comparison)) = comparison else {
        if world.get::<EditorComparison>(panel).is_some() {
            world.entity_mut(panel).remove::<EditorComparison>();
            world.get_mut::<DiffPanel>(panel).unwrap().shown = None;
        }
        return;
    };
    let clean = world
        .get::<DocumentReference>(panel)
        .and_then(|reference| {
            world
                .workbench()
                .inspect_document(reference.document())
                .ok()
        })
        .is_some_and(|info| !info.dirty && !info.saving);
    let actions = actions(&loaded, session.writable(), clean);
    let key = (loaded.identity, actions.iter().map(|a| a.enabled).collect());
    if world.get::<DiffPanel>(panel).unwrap().shown.as_ref() == Some(&key) {
        return;
    }
    world.get_mut::<DiffPanel>(panel).unwrap().shown = Some(key);
    world.entity_mut(panel).insert(EditorComparison {
        comparison,
        actions,
    });
}

/// Apply the change clicked: stage, unstage or discard exactly it.
fn request(
    world: &mut World,
    session: &mut GitSession,
    panel: Entity,
    request: DiffRequest,
) -> Result<(), String> {
    let diff = world.get::<DiffPanel>(panel).ok_or("The diff closed")?;
    let Some(Ok(loaded)) = diff.loaded.clone() else {
        return Err("The diff is still being read".into());
    };
    let descriptor = diff.descriptor();
    if session
        .view
        .as_ref()
        .is_none_or(|view| Path::new(&view.status.root) != descriptor.root)
    {
        return Err("The repository changed".into());
    }
    if request.comparison != loaded.identity {
        return Err("The changes were read again: look again, and retry".into());
    }
    let document = world
        .get::<DocumentReference>(panel)
        .map(DocumentReference::document)
        .ok_or("The diff has no text yet")?;
    let info = world
        .workbench()
        .inspect_document(document)
        .map_err(|e| e.to_string())?;
    if info.revision != request.revision || info.has_drafts || info.conflict || info.missing {
        return Err("The file changed: look again, and retry".into());
    }
    if request.action == DISCARD && (info.dirty || info.saving) {
        return Err("Save the file before discarding a change".into());
    }
    let handle = world
        .workbench()
        .cast_document::<TextBuffer>(document)
        .map_err(|e| e.to_string())?;
    let current = world
        .workbench()
        .read_document(handle)
        .map_err(|e| e.to_string())?
        .clone();
    if !loaded.git.can_apply_hunks
        || bare_cr(&loaded.git.old_bytes)
        || bare_cr(&loaded.git.new_bytes)
    {
        return Err("This change can only be applied to the whole file".into());
    }
    let path = descriptor.path;
    let snapshot = loaded.git.snapshot.clone();
    let ChangeRange {
        old: old_range,
        new: new_range,
    } = request.range;
    let operation = match (descriptor.side, request.action) {
        (DiffSide::Unstaged, STAGE) => Operation::StageRange {
            path,
            snapshot,
            old_range,
            new_range,
        },
        (DiffSide::Unstaged, DISCARD) => Operation::DiscardRange {
            path,
            snapshot,
            old_range,
            new_range,
        },
        (DiffSide::Staged, UNSTAGE) => Operation::UnstageRange {
            path,
            snapshot,
            old_range,
            new_range,
        },
        _ => return Err("That can't be done here".into()),
    };
    begin(world, session, operation)?;
    if request.action == STAGE
        && let Some(Pending::Saving { guard, .. }) = &mut session.pending
    {
        *guard = Some(StageGuard {
            baseline: loaded.git.old_bytes.clone(),
            current,
        });
    }
    Ok(())
}

/// What staging a change checks just before it's applied: Git still has the
/// baseline that was compared, and the file (saved first) is the text the
/// change was chosen in.
pub(super) struct StageGuard {
    baseline: Vec<u8>,
    current: TextBuffer,
}

impl StageGuard {
    pub(super) fn validate(&self, fresh: &crate::Diff) -> Result<(), String> {
        if !fresh.can_apply_hunks
            || fresh.old_bytes != self.baseline
            || std::str::from_utf8(&fresh.new_bytes)
                .ok()
                .map(TextBuffer::from)
                .as_ref()
                != Some(&self.current)
        {
            return Err("The changes moved on while saving: look again, and retry".into());
        }
        Ok(())
    }
}

enum Asked {
    Side(bool),
    Open,
    Discard(DiffRequest),
    Keep,
    Dismiss,
}

impl Panel for DiffPanel {
    const LABEL: &'static str = "Diff";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(diff) = world.get::<DiffPanel>(panel) else {
            return;
        };
        let p = context.theme.palette.clone();
        let (staged, path, root) = (diff.staged, diff.path.clone(), diff.root.clone());
        let confirm = diff.confirm.clone();
        let error = diff.error.clone();
        let loaded = diff.loaded.clone();
        let mut asked = None;
        // The bar: the file, which side, open it; questions and problems.
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(12, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&path).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut frame = context.frame(ui);
                        let open = frame.icon_button(BuiltinIcon::NewFile);
                        frame.tooltip(&open, "Open File");
                        if open.clicked() {
                            asked = Some(Asked::Open);
                        }
                        let ui = frame.ui;
                        if ui.selectable_label(staged, "Staged").clicked() && !staged {
                            asked = Some(Asked::Side(true));
                        }
                        if ui.selectable_label(!staged, "Changes").clicked() && staged {
                            asked = Some(Asked::Side(false));
                        }
                    });
                });
                if let Some(request) = &confirm {
                    ui.horizontal(|ui| {
                        ui.label("Discard this change? It can't be undone here.");
                        if ui.button("Discard").clicked() {
                            asked = Some(Asked::Discard(request.clone()));
                        }
                        if ui.button("Keep").clicked() {
                            asked = Some(Asked::Keep);
                        }
                    });
                }
                if let Some(error) = &error {
                    ui.horizontal(|ui| {
                        ui.colored_label(theme::color(p.error), error);
                        if ui.small_button("Dismiss").clicked() {
                            asked = Some(Asked::Dismiss);
                        }
                    });
                }
            });
        match &loaded {
            None => {
                ui.centered_and_justified(|ui| ui.spinner());
            }
            Some(Err(error)) => {
                ui.colored_label(theme::color(p.error), error);
            }
            Some(Ok(loaded)) if loaded.git.binary => {
                ui.weak("Binary file: no lines to compare.");
            }
            Some(Ok(_)) => {
                if world.get::<DocumentReference>(panel).is_some() {
                    bedtime_code::draw_editor(ui, world, context, panel);
                } else {
                    ui.centered_and_justified(|ui| ui.spinner());
                }
            }
        }
        match asked {
            None => {}
            Some(Asked::Side(staged)) => {
                let mut diff = world.get_mut::<DiffPanel>(panel).unwrap();
                diff.staged = staged;
                diff.loaded = None;
                diff.task = None;
                diff.shown = None;
                diff.confirm = None;
                world.entity_mut(panel).remove::<EditorComparison>();
            }
            Some(Asked::Open) => {
                if let Err(error) = world
                    .workbench_mut()
                    .open_path(&root.join(&path), PathOptions::default())
                {
                    world.get_mut::<DiffPanel>(panel).unwrap().error = Some(error);
                }
            }
            Some(Asked::Keep) => world.get_mut::<DiffPanel>(panel).unwrap().confirm = None,
            Some(Asked::Dismiss) => world.get_mut::<DiffPanel>(panel).unwrap().error = None,
            Some(Asked::Discard(clicked)) => {
                world.get_mut::<DiffPanel>(panel).unwrap().confirm = None;
                let result = match world.remove_resource::<GitSession>() {
                    Some(mut session) => {
                        let result = request(world, &mut session, panel, clicked);
                        world.insert_resource(session);
                        result
                    }
                    None => Err("Git isn't ready yet".into()),
                };
                if let Err(error) = result {
                    world.get_mut::<DiffPanel>(panel).unwrap().error = Some(error);
                }
            }
        }
    }
}
