//! Concrete Git comparison panels. Repository authority stays in GitSession;
//! the working text remains a normal canonical file document.
use super::*;
use bedtime_editor2::{
    EditorView, TextBuffer, TextDocument, TextSession, TextSnapshot,
    diff::{ChangeRange, Comparison, DiffAction, DiffRequest},
};
use bedtime_ui2::{UiContext, egui};
use std::collections::HashMap;

const STAGE: u64 = 1;
const UNSTAGE: u64 = 2;
const DISCARD: u64 = 3;
type Key = (String, DiffSide);
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Descriptor {
    pub root: PathBuf,
    pub path: String,
    pub side: DiffSide,
}
impl Descriptor {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !self.root.is_absolute()
            || self.root.as_os_str().len() > 16384
            || self.path.is_empty()
            || self.path.len() > 16384
            || self.path.contains('\0')
            || Path::new(&self.path)
                .components()
                .any(|p| !matches!(p, std::path::Component::Normal(_)))
        {
            return Err("Invalid Git comparison descriptor".into());
        }
        Ok(())
    }
}
struct Loaded {
    identity: u64,
    git: bedtime_git::Diff,
    comparison: Option<Comparison>,
    current: Option<TextBuffer>,
    exists: bool,
}
struct Entry {
    task: Option<(
        u64,
        ManagedTask<Result<(Arc<Loaded>, Vec<crate::file_editor::SourceObservation>), String>>,
    )>,
    loaded: Option<Arc<Loaded>>,
    generation: u64,
    scan: u64,
    error: Option<String>,
}
#[derive(Default)]
pub(super) struct Comparisons(HashMap<Key, Entry>);
#[derive(Component)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub(crate) struct DiffPanel {
    pub descriptor: Descriptor,
    attached: u64,
    snapshot: Option<TextDocument>,
    view: EditorView,
    confirm: Option<DiffRequest>,
    error: Option<String>,
    rendered_lines: usize,
    laid_out_bytes: usize,
    controls: Vec<(u64, usize, egui::Rect)>,
    text_rect: Option<egui::Rect>,
}
impl DiffPanel {
    pub(crate) fn new(descriptor: Descriptor) -> Self {
        Self {
            descriptor,
            attached: 0,
            snapshot: None,
            view: EditorView::default(),
            confirm: None,
            error: None,
            rendered_lines: 0,
            laid_out_bytes: 0,
            controls: Vec::new(),
            text_rect: None,
        }
    }
}

pub(super) fn open(
    world: &mut World,
    origin: Entity,
    descriptor: Descriptor,
    pin: bool,
) -> Result<Entity, String> {
    descriptor.validate()?;
    let layout = Workbench::for_entity(world, origin)?.panel_layout(origin)?;
    let mut w = Workbench::for_entity(world, layout)?;
    let panels = w.panels(layout);
    let same = panels.iter().copied().find(|&p| {
        w.world()
            .get::<DiffPanel>(p)
            .is_some_and(|d| d.descriptor == descriptor)
    });
    let panel = if let Some(p) = same {
        p
    } else {
        let replace = panels.iter().copied().find(|&p| {
            w.world().get::<DiffPanel>(p).is_some()
                && w.world().get::<PanelInstance>(p).is_some_and(|i| i.preview)
                && crate::file_editor::can_replace(w.world(), p)
        });
        if let Some(panel) = replace {
            bedtime_editor2::workbench::unbind_text_editor(w.world_mut(), panel)
                .map_err(|e| e.to_string())?;
            w.world_mut()
                .entity_mut(panel)
                .insert(DiffPanel::new(descriptor));
            panel
        } else {
            let area = panels
                .iter()
                .find_map(|&p| {
                    (w.world().get::<crate::files::FilePreviewPanel>(p).is_some()
                        || w.world().get::<DiffPanel>(p).is_some())
                    .then(|| w.world().get::<InArea>(p).map(|a| a.0))
                    .flatten()
                })
                .or_else(|| {
                    w.world()
                        .get::<LayoutAreas>(layout)?
                        .entities()
                        .iter()
                        .copied()
                        .filter(|&a| {
                            panels_in_area(w.world(), a).iter().all(|&p| {
                                w.world().get::<GitPanel>(p).is_none()
                                    && w.world()
                                        .get::<bedtime_filetree2::FileTreePanel>(p)
                                        .is_none()
                            })
                        })
                        .max_by_key(|&a| {
                            let r = w.world().get::<Area>(a).unwrap().rect;
                            (r.max[0] - r.min[0]) * (r.max[1] - r.min[1])
                        })
                });
            let area = if let Some(a) = area {
                a
            } else {
                let source = w.world().get::<InArea>(origin).ok_or("Missing Git area")?.0;
                let r = w.world().get::<Area>(source).unwrap().rect;
                w.split_area(
                    source,
                    SplitAxis::Horizontal,
                    r.min[0] + (r.max[0] - r.min[0]) * 2 / 5,
                    [1, 1],
                )?
            };
            let panel = w
                .world_mut()
                .spawn((DiffPanel::new(descriptor), InArea(area)))
                .id();
            w.world_mut()
                .get_mut::<PanelInstance>(panel)
                .unwrap()
                .preview = true;
            panel
        }
    };
    if pin {
        w.world_mut()
            .get_mut::<PanelInstance>(panel)
            .unwrap()
            .preview = false;
    }
    w.select_panel(panel)?;
    if pin {
        w.focus_panel(panel)?;
    }
    Ok(panel)
}

fn load(
    root: PathBuf,
    key: Key,
    identity: u64,
    cancel: &Cancellation,
) -> Result<Arc<Loaded>, String> {
    let Output::Diff(git) = git_job(
        root.clone(),
        Operation::Diff {
            path: key.0.clone(),
            side: key.1,
        },
        cancel,
        &AtomicBool::new(false),
    )?
    else {
        return Err("Unexpected Git diff response".into());
    };
    let exists =
        std::fs::symlink_metadata(root.join(&key.0)).is_ok_and(|m| m.file_type().is_file());
    let (comparison, current) = if !git.binary {
        match (
            std::str::from_utf8(&git.old_bytes),
            std::str::from_utf8(&git.new_bytes),
        ) {
            (Ok(old), Ok(new)) => {
                let baseline = TextBuffer::from(old);
                let current = TextBuffer::from(new);
                // Git ranges use LF rows. Bare CR documents use Editor2's CR
                // lines, so compare in editor coordinates and keep hunk actions off.
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
        }
    } else {
        (None, None)
    };
    Ok(Arc::new(Loaded {
        identity,
        git,
        comparison,
        current,
        exists,
    }))
}

pub(super) fn update(world: &mut World, session: &mut GitSession) {
    let panels: Vec<_> = world
        .query::<(Entity, &DiffPanel)>()
        .iter(world)
        .map(|(p, d)| (p, d.descriptor.clone()))
        .collect();
    session.comparisons.0.retain(|key, _| {
        panels
            .iter()
            .any(|(_, d)| d.path == key.0 && d.side == key.1)
    });
    for (_, descriptor) in &panels {
        let key = (descriptor.path.clone(), descriptor.side);
        let entry = session.comparisons.0.entry(key.clone()).or_insert(Entry {
            task: None,
            loaded: None,
            generation: 0,
            scan: 0,
            error: None,
        });
        if let Some((epoch, task)) = entry.task.take() {
            match task.try_recv() {
                Ok(result) if epoch == session.epoch => {
                    match result {
                        Ok((loaded, observations)) => {
                            entry.error = None;
                            for observation in observations {
                                if let Err(e) =
                                    crate::file_editor::apply_source_observation(world, observation)
                                {
                                    entry.error = Some(e);
                                }
                            }
                            if entry.loaded.as_ref().is_none_or(|old| {
                                old.git != loaded.git || old.exists != loaded.exists
                            }) {
                                entry.loaded = Some(loaded);
                                entry.generation += 1;
                            }
                        }
                        Err(error) => entry.error = Some(error),
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => entry.task = Some((epoch, task)),
                _ => {}
            }
        }
        if entry.task.is_none()
            && session.pending.is_none()
            && session.view_epoch == session.epoch
            && !session.refresh
            && session.scan.is_none()
            && (entry.scan != session.scans || (entry.loaded.is_none() && entry.error.is_none()))
        {
            // Scan sequence is also a monotonic identity for this cached key.
            let identity = session.scans.max(1);
            entry.scan = session.scans;
            let root = descriptor.root.clone();
            if session
                .view
                .as_ref()
                .is_none_or(|v| Path::new(&v.status.root) != root)
            {
                entry.error = Some(
                    "This comparison belongs to a different repository. Reopen it from Git.".into(),
                );
                continue;
            }
            let sources = if descriptor.side == DiffSide::Unstaged {
                crate::file_editor::capture_sources(world, &root.join(&descriptor.path))
            } else {
                Vec::new()
            };
            match Workbench::new(world).work().spawn_task(
                None,
                "Read Git comparison",
                move |cancel| {
                    let loaded = load(root, key, identity, &cancel)?;
                    let observations = sources
                        .into_iter()
                        .take_while(|_| !cancel.is_cancelled())
                        .map(crate::file_editor::read_source_observation)
                        .collect();
                    Ok((loaded, observations))
                },
            ) {
                Ok(task) => entry.task = Some((session.epoch, task)),
                Err(e) => entry.error = Some(e),
            }
        }
    }
    for (panel, descriptor) in panels {
        let entry = &session.comparisons.0[&(descriptor.path.clone(), descriptor.side)];
        if world.get::<DiffPanel>(panel).unwrap().attached == entry.generation {
            continue;
        }
        let Some(loaded) = &entry.loaded else {
            continue;
        };
        let live = descriptor.side == DiffSide::Unstaged
            && loaded.current.is_some()
            && (loaded.exists
                || world.get::<DocumentView>(panel).is_some_and(|v| {
                    inspect_document(world, v.document()).is_ok_and(|i| i.dirty || i.has_drafts)
                }));
        let result = if live {
            let path = descriptor.root.join(&descriptor.path);
            // Reuse already bound canonical text; source observation is owned by
            // the file backend, never by a Git baseline refresh.
            if crate::file_editor::same_source(world, panel, &path) {
                Ok(())
            } else {
                crate::file_editor::bind_loaded(
                    world,
                    panel,
                    &path,
                    loaded.current.as_ref().unwrap(),
                    None,
                )
            }
        } else {
            bedtime_editor2::workbench::unbind_text_editor(world, panel)
                .map(|_| ())
                .map_err(|e| e.to_string())
        };
        let mut state = world.get_mut::<DiffPanel>(panel).unwrap();
        if let Err(error) = result {
            state.error = Some(error);
            continue;
        }
        state.attached = entry.generation;
        if !live {
            state.snapshot = loaded.current.clone().map(TextDocument::new);
            state.view.settings.read_only = true;
        } else {
            state.snapshot = None;
        }
        world.get_mut::<PanelInstance>(panel).unwrap().title = format!(
            "{} ({})",
            descriptor.path.rsplit('/').next().unwrap(),
            if descriptor.side == DiffSide::Staged {
                "staged"
            } else {
                "changes"
            }
        );
    }
}

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

pub(super) struct StageGuard {
    baseline: Vec<u8>,
    current: TextBuffer,
}
impl StageGuard {
    pub(super) fn validate(&self, fresh: &bedtime_git::Diff) -> Result<(), String> {
        if !fresh.can_apply_hunks
            || fresh.old_bytes != self.baseline
            || std::str::from_utf8(&fresh.new_bytes)
                .ok()
                .map(TextBuffer::from)
                .as_ref()
                != Some(&self.current)
        {
            return Err(
                "The comparison changed while saving. Review the refreshed changes and retry."
                    .into(),
            );
        }
        Ok(())
    }
}
fn request(
    world: &mut World,
    session: &mut GitSession,
    panel: Entity,
    loaded: &Loaded,
    request: DiffRequest,
) -> Result<(), String> {
    let state = world.get::<DiffPanel>(panel).ok_or("Comparison closed")?;
    let descriptor = state.descriptor.clone();
    if session
        .view
        .as_ref()
        .is_none_or(|v| Path::new(&v.status.root) != descriptor.root)
    {
        return Err("Comparison repository changed".into());
    }
    if loaded
        .comparison
        .as_ref()
        .is_none_or(|_| request.comparison != state.attached_identity(session))
    {
        return Err("The comparison changed. Review the refreshed changes and retry.".into());
    }
    let document = world.get::<DocumentView>(panel).map(DocumentView::document);
    let current = if let Some(id) = document {
        let info = inspect_document(world, id).map_err(|e| e.to_string())?;
        if info.revision != request.revision
            || info.has_drafts
            || info.conflict
            || info.requires_source_refresh
        {
            return Err(
                "The document changed or needs source reconciliation. Review and retry.".into(),
            );
        }
        if request.action == DISCARD && (info.dirty || info.saving) {
            return Err("Save and refresh before discarding a chunk.".into());
        }
        read_document(
            world,
            cast_document::<TextBuffer>(world, id).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?
        .clone()
    } else {
        let snapshot = state
            .snapshot
            .as_ref()
            .ok_or("No comparison text")?
            .snapshot()
            .map_err(|e| e.to_string())?;
        if snapshot.revision != request.revision {
            return Err("Stale comparison".into());
        }
        snapshot.text
    };
    if !loaded.git.can_apply_hunks
        || bare_cr(&loaded.git.old_bytes)
        || bare_cr(&loaded.git.new_bytes)
    {
        return Err("This change requires whole-file actions.".into());
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
        _ => return Err("Action is unavailable for this comparison".into()),
    };
    begin(world, session, operation)?;
    if request.action == STAGE {
        if let Some(Pending::Saving { guard, .. }) = &mut session.pending {
            *guard = Some(StageGuard {
                baseline: loaded.git.old_bytes.clone(),
                current,
            });
        }
    }
    Ok(())
}
impl DiffPanel {
    fn attached_identity(&self, session: &GitSession) -> u64 {
        session
            .comparisons
            .0
            .get(&(self.descriptor.path.clone(), self.descriptor.side))
            .and_then(|e| e.loaded.as_ref())
            .map_or(0, |l| l.identity)
    }
}
impl Panel for DiffPanel {
    const KEY: &'static str = "bed.git.diff";
    const LABEL: &'static str = "Git diff";
    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let descriptor = world.get::<Self>(panel).unwrap().descriptor.clone();
        let Some(mut session) = world.remove_resource::<GitSession>() else {
            ui.spinner();
            return;
        };
        let (loaded, error, loading) = session
            .comparisons
            .0
            .get(&(descriptor.path.clone(), descriptor.side))
            .map_or((None, None, true), |e| {
                (e.loaded.clone(), e.error.clone(), e.task.is_some())
            });
        if let Some(e) = error.as_ref().or(session.error.as_ref()) {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        if loading {
            ui.spinner();
        }
        let mut request_action = None;
        let mut whole = None;
        let mut navigation = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(if descriptor.side == DiffSide::Staged {
                "HEAD → Index"
            } else {
                "Index → Working file"
            });
            if ui.small_button("Previous change").clicked() {
                navigation = Some(false);
            }
            if ui.small_button("Next change").clicked() {
                navigation = Some(true);
            }
            if ui.small_button("Open file").clicked() {
                whole = Some(0);
            }
            if let Some(loaded) = &loaded {
                if ui
                    .add_enabled(
                        session.writable(),
                        egui::Button::new(if descriptor.side == DiffSide::Staged {
                            "Unstage file"
                        } else {
                            "Stage file"
                        }),
                    )
                    .clicked()
                {
                    whole = Some(1);
                }
                if descriptor.side == DiffSide::Unstaged
                    && ui
                        .add_enabled(
                            session.writable() && !loaded.git.new_bytes.is_empty(),
                            egui::Button::new("Discard file…"),
                        )
                        .clicked()
                {
                    whole = Some(2);
                }
            }
        });
        if let Some(loaded) = loaded {
            let info = world
                .get::<DocumentView>(panel)
                .and_then(|v| inspect_document(world, v.document()).ok());
            let clean = info.as_ref().is_none_or(|i| {
                !i.dirty && !i.saving && !i.has_drafts && !i.conflict && !i.requires_source_refresh
            });
            if info
                .as_ref()
                .is_some_and(|i| i.conflict || i.requires_source_refresh)
            {
                ui.weak("Source conflict: open the file to resolve it before changing Git state.");
            }
            if let Some(error) = info.as_ref().and_then(|i| i.last_save_error.as_ref()) {
                ui.colored_label(ui.visuals().error_fg_color, error.to_string());
            }
            let enabled =
                session.writable() && !loading && crate::close::accepts_user_actions(world);
            let buttons = actions(&loaded, enabled, clean);
            if let Some(comparison) = loaded
                .comparison
                .as_ref()
                .filter(|_| world.get::<Self>(panel).unwrap().attached > 0)
            {
                let output = if world.get::<DocumentView>(panel).is_some() {
                    if let Some(forward) = navigation {
                        let id = world.get::<DocumentView>(panel).unwrap().document();
                        if let (Ok(info), Ok(handle)) = (
                            inspect_document(world, id),
                            cast_document::<TextBuffer>(world, id),
                        ) {
                            if let Ok(text) = read_document(world, handle) {
                                let snap = TextSnapshot {
                                    text: text.clone(),
                                    revision: info.revision,
                                };
                                if let Some(mut editor) =
                                    world.get_mut::<bedtime_editor2::workbench::WorkbenchEditor>(
                                        panel,
                                    )
                                {
                                    let _ = editor.view.navigate_change(&snap, forward);
                                }
                            }
                        }
                    }
                    world
                        .get_mut::<bedtime_editor2::workbench::WorkbenchEditor>(panel)
                        .unwrap()
                        .view
                        .set_comparison(Some(comparison.clone()), buttons);
                    bedtime_editor2::workbench::draw_editor(ui, world, context, panel)
                } else {
                    let mut state = world.entity_mut(panel).take::<Self>().unwrap();
                    state.view.set_comparison(Some(comparison.clone()), buttons);
                    let output = if let Some(mut snapshot) = state.snapshot.take() {
                        if let Some(forward) = navigation {
                            let _ = state
                                .view
                                .navigate_change(&snapshot.snapshot().unwrap(), forward);
                        }
                        let output = state.view.show(
                            ui,
                            egui::Id::new(("git-snapshot", panel, state.attached)),
                            &mut snapshot,
                            context.motion,
                        );
                        state.snapshot = Some(snapshot);
                        output
                    } else {
                        Err(bedtime_editor2::EditorError(
                            "Loading comparison text…".into(),
                        ))
                    };
                    world.entity_mut(panel).insert(state);
                    output
                };
                match output {
                    Ok(output) => {
                        let mut state = world.get_mut::<Self>(panel).unwrap();
                        state.rendered_lines = output.rendered_lines;
                        state.laid_out_bytes = output.laid_out_bytes;
                        state.controls = output.diff_controls;
                        state.text_rect = Some(output.text_rect);
                        request_action = output.diff_request;
                        if info.as_ref().is_some_and(|i| i.dirty || i.has_drafts)
                            || info.as_ref().is_some_and(|i| i.revision != output.revision)
                        {
                            world.get_mut::<PanelInstance>(panel).unwrap().preview = false;
                        }
                    }
                    Err(e) => {
                        ui.colored_label(ui.visuals().error_fg_color, e.to_string());
                    }
                }
            } else {
                ui.weak("Binary or unsupported content. Use whole-file actions.");
            }
            if let Some(action) = request_action {
                if action.action == DISCARD {
                    world.get_mut::<Self>(panel).unwrap().confirm = Some(action);
                } else if let Err(e) = request(world, &mut session, panel, &loaded, action) {
                    session.error = Some(e);
                }
            }
            let confirm = world.get::<Self>(panel).unwrap().confirm.clone();
            if let Some(confirmed) = confirm {
                egui::Window::new("Discard chunk?")
                    .id(egui::Id::new(("discard-chunk", panel)))
                    .collapsible(false)
                    .resizable(false)
                    .show(ui.ctx(), |ui| {
                        ui.label("Discard this working-file chunk? This changes the saved file.");
                        ui.horizontal(|ui| {
                            if ui.button("Discard chunk").clicked() {
                                world.get_mut::<Self>(panel).unwrap().confirm = None;
                                if let Err(e) =
                                    request(world, &mut session, panel, &loaded, confirmed.clone())
                                {
                                    session.error = Some(e);
                                }
                            }
                            if ui.button("Keep changes").clicked() {
                                world.get_mut::<Self>(panel).unwrap().confirm = None;
                            }
                        });
                    });
            }
        }
        let result = match whole {
            Some(0) => Workbench::for_entity(world, panel)
                .and_then(|w| w.panel_layout(panel))
                .and_then(|layout| {
                    crate::opening::open_file(
                        world,
                        layout,
                        descriptor.root.join(&descriptor.path),
                        crate::opening::FileOpenMode::Focus,
                    )
                    .map(|_| ())
                }),
            Some(1) => begin(
                world,
                &mut session,
                if descriptor.side == DiffSide::Staged {
                    Operation::Unstage {
                        paths: vec![descriptor.path],
                    }
                } else {
                    Operation::Stage {
                        paths: vec![descriptor.path],
                    }
                },
            ),
            Some(2) => request_discard(world, &mut session, descriptor.path),
            _ => Ok(()),
        };
        if let Err(e) = result {
            session.error = Some(e);
        }
        // Whole-file confirmation remains usable with the Git panel closed.
        if let Some(op) = session.confirmation.clone() {
            egui::Window::new("Confirm Git change")
                .id(egui::Id::new(("git-confirm", panel)))
                .show(ui.ctx(), |ui| {
                    ui.label("Discard the reviewed working-file changes?");
                    if ui.button("Discard changes").clicked() {
                        session.confirmation = None;
                        if let Err(e) = begin(world, &mut session, op) {
                            session.error = Some(e);
                        }
                    }
                    if ui.button("Keep changes").clicked() {
                        session.confirmation = None;
                    }
                });
        }
        if let Some(error) = world.get::<Self>(panel).unwrap().error.as_ref() {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        world.insert_resource(session);
    }
}

#[cfg(feature = "automation")]
pub(super) fn inspect(world: &mut World) -> Vec<serde_json::Value> {
    world.query::<(Entity,&DiffPanel)>().iter(world).map(|(p,d)|serde_json::json!({"entity":p.to_bits().to_string(),"descriptor":d.descriptor,"attached":d.attached,"loading":world.get_resource::<GitSession>().and_then(|s|s.comparisons.0.get(&(d.descriptor.path.clone(),d.descriptor.side))).is_none_or(|e|e.task.is_some()),"document":world.get::<DocumentView>(p).map(|v|v.document().entity().to_bits().to_string()),"revision":world.get::<DocumentView>(p).and_then(|v|inspect_document(world,v.document()).ok()).map(|i|i.revision),"error":d.error,"rendered_lines":d.rendered_lines,"laid_out_bytes":d.laid_out_bytes,"text_rect":d.text_rect.map(|r|serde_json::json!({"min":[r.min.x,r.min.y],"max":[r.max.x,r.max.y]})),"controls":d.controls.iter().map(|(a,c,r)|serde_json::json!({"action":a,"chunk":c,"min":[r.min.x,r.min.y],"max":[r.max.x,r.max.y]})).collect::<Vec<_>>() })).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::{Repo, edit, host, settle, wait};
    fn ready(app: &mut App, panel: Entity) {
        wait(app, |w| {
            w.get::<DiffPanel>(panel).is_some_and(|p| p.attached > 0)
        });
    }
    fn request_for(
        app: &mut App,
        panel: Entity,
        action: u64,
        range: ChangeRange,
    ) -> Result<(), String> {
        let mut session = app.world_mut().remove_resource::<GitSession>().unwrap();
        let state = app.world().get::<DiffPanel>(panel).unwrap();
        let loaded = session.comparisons.0[&(state.descriptor.path.clone(), state.descriptor.side)]
            .loaded
            .clone()
            .unwrap();
        let revision = app
            .world()
            .get::<DocumentView>(panel)
            .map(|v| {
                inspect_document(app.world(), v.document())
                    .unwrap()
                    .revision
            })
            .unwrap_or(0);
        let result = request(
            app.world_mut(),
            &mut session,
            panel,
            &loaded,
            DiffRequest {
                action,
                comparison: loaded.identity,
                revision,
                range,
            },
        );
        app.world_mut().insert_resource(session);
        result
    }
    fn open_side(app: &mut App, origin: Entity, repo: &Repo, side: DiffSide) -> Entity {
        let p = open(
            app.world_mut(),
            origin,
            Descriptor {
                root: repo.0.clone(),
                path: "a.txt".into(),
                side,
            },
            false,
        )
        .unwrap();
        ready(app, p);
        p
    }
    #[test]
    fn live_chunk_stage_saves_shared_text_then_unstage_and_discard_preserve_other_changes() {
        let repo = Repo::new();
        repo.write("a.txt", "one\ntwo\nthree\nfour\nfive\n");
        repo.git(&["add", "a.txt"]);
        repo.git(&["commit", "-m", "Initial"]);
        repo.write("a.txt", "ONE\ntwo\nthree\nfour\nFIVE\n");
        let (mut app, _workspace, origin) = host(&repo.0);
        let panel = open_side(&mut app, origin, &repo, DiffSide::Unstaged);
        let document = cast_document::<TextBuffer>(
            app.world(),
            app.world().get::<DocumentView>(panel).unwrap().document(),
        )
        .unwrap();
        edit(&mut app, document, "EDITED\ntwo\nthree\nfour\nFIVE\n");
        request_for(
            &mut app,
            panel,
            STAGE,
            ChangeRange {
                old: 0..1,
                new: 0..1,
            },
        )
        .unwrap();
        settle(&mut app);
        assert_eq!(
            repo.git(&["show", ":a.txt"]),
            "EDITED\ntwo\nthree\nfour\nfive\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.0.join("a.txt")).unwrap(),
            "EDITED\ntwo\nthree\nfour\nFIVE\n"
        );
        assert!(!inspect_document(app.world(), document.id()).unwrap().dirty);
        app.world_mut()
            .get_mut::<PanelInstance>(panel)
            .unwrap()
            .preview = false;
        let staged = open_side(&mut app, origin, &repo, DiffSide::Staged);
        assert!(app.world().get::<DocumentView>(staged).is_none());
        request_for(
            &mut app,
            staged,
            UNSTAGE,
            ChangeRange {
                old: 0..1,
                new: 0..1,
            },
        )
        .unwrap();
        settle(&mut app);
        assert_eq!(
            repo.git(&["show", ":a.txt"]),
            "one\ntwo\nthree\nfour\nfive\n"
        );
        // Wait for the refreshed working comparison, not merely the status scan.
        wait(&mut app, |w| {
            w.get_resource::<GitSession>().unwrap().comparisons.0
                [&("a.txt".into(), DiffSide::Unstaged)]
                .loaded
                .as_ref()
                .is_some_and(|l| l.git.old_bytes.starts_with(b"one\n"))
        });
        request_for(
            &mut app,
            panel,
            DISCARD,
            ChangeRange {
                old: 4..5,
                new: 4..5,
            },
        )
        .unwrap();
        settle(&mut app);
        assert_eq!(
            read_document(app.world(), document).unwrap(),
            "EDITED\ntwo\nthree\nfour\nfive\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.0.join("a.txt")).unwrap(),
            "EDITED\ntwo\nthree\nfour\nfive\n"
        );
    }
    #[test]
    fn preview_identity_pinning_shared_code_and_session_descriptors_are_independent() {
        let repo = Repo::new();
        repo.initial();
        repo.write("a.txt", "changed\n");
        let (mut app, _workspace, origin) = host(&repo.0);
        let panel = open_side(&mut app, origin, &repo, DiffSide::Unstaged);
        assert!(app.world().get::<PanelInstance>(panel).unwrap().preview);
        let pinned = open(
            app.world_mut(),
            origin,
            Descriptor {
                root: repo.0.clone(),
                path: "a.txt".into(),
                side: DiffSide::Unstaged,
            },
            true,
        )
        .unwrap();
        assert_eq!(panel, pinned);
        assert!(!app.world().get::<PanelInstance>(panel).unwrap().preview);
        let area = app.world().get::<InArea>(panel).unwrap().0;
        let code = app
            .world_mut()
            .spawn((crate::files::FilePreviewPanel::default(), InArea(area)))
            .id();
        crate::file_editor::bind_loaded(
            app.world_mut(),
            code,
            &repo.0.join("a.txt"),
            &TextBuffer::from("changed\n"),
            None,
        )
        .unwrap();
        assert_eq!(
            app.world().get::<DocumentView>(code).unwrap().document(),
            app.world().get::<DocumentView>(panel).unwrap().document()
        );
        let mut captured = Workbench::capture_world::<
            bedtime_workbench2::persistence::ReflectWorkbenchPersist,
        >(app.world_mut())
        .unwrap();
        project_persistence(app.world_mut(), &mut captured);
        let saved = captured.component::<PersistedDiff>(panel).unwrap();
        assert_eq!(saved.0.side, DiffSide::Unstaged);
        assert!(app.world().get::<PersistedDiff>(panel).is_none());
        assert!(
            Descriptor {
                root: repo.0.clone(),
                path: "../outside".into(),
                side: DiffSide::Staged
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn stale_baseline_edits_after_capture_and_dirty_discard_leave_index_and_disk_safe() {
        let repo = Repo::new();
        repo.initial();
        repo.write("a.txt", "working\n");
        let (mut app, _workspace, origin) = host(&repo.0);
        let panel = open_side(&mut app, origin, &repo, DiffSide::Unstaged);
        let document = cast_document::<TextBuffer>(
            app.world(),
            app.world().get::<DocumentView>(panel).unwrap().document(),
        )
        .unwrap();
        edit(&mut app, document, "unsaved\n");
        assert!(
            request_for(
                &mut app,
                panel,
                DISCARD,
                ChangeRange {
                    old: 0..1,
                    new: 0..1
                }
            )
            .is_err()
        );
        request_for(
            &mut app,
            panel,
            STAGE,
            ChangeRange {
                old: 0..1,
                new: 0..1,
            },
        )
        .unwrap();
        edit(&mut app, document, "newer\n");
        settle(&mut app);
        assert_eq!(repo.git(&["show", ":a.txt"]), "original\n");
        assert_eq!(read_document(app.world(), document).unwrap(), "newer\n");
        assert!(
            app.world()
                .get_resource::<GitSession>()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .contains("changed")
        );
    }
    #[test]
    fn stage_guard_rejects_external_index_changes_and_snapshot_refresh_keeps_dirty_text() {
        let repo = Repo::new();
        repo.initial();
        repo.write("a.txt", "working\n");
        let (mut app, _workspace, origin) = host(&repo.0);
        let panel = open_side(&mut app, origin, &repo, DiffSide::Unstaged);
        let document = cast_document::<TextBuffer>(
            app.world(),
            app.world().get::<DocumentView>(panel).unwrap().document(),
        )
        .unwrap();
        edit(&mut app, document, "unsaved\n");
        request_for(
            &mut app,
            panel,
            STAGE,
            ChangeRange {
                old: 0..1,
                new: 0..1,
            },
        )
        .unwrap();
        repo.git(&["add", "a.txt"]);
        let external_index = repo.git(&["show", ":a.txt"]);
        settle(&mut app);
        assert_eq!(repo.git(&["show", ":a.txt"]), external_index);
        assert!(
            app.world()
                .get_resource::<GitSession>()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .contains("comparison changed")
        );
        assert_eq!(read_document(app.world(), document).unwrap(), "unsaved\n");
    }
}
