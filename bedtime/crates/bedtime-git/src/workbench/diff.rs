//! Diff panels: one file's working (unstaged) or staged changes, as a unified
//! diff with old and new line numbers. Each change (hunk) can be staged,
//! unstaged or discarded from its header; Discard asks first. The diff is
//! read again after every Git scan, so it follows edits and operations.
use super::*;
use crate::{Diff, LineKind};
use bedtime_ui::{BuiltinIcon, UiContext, egui, theme};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;

/// Which file, and which side, a diff panel shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Descriptor {
    pub root: PathBuf,
    pub path: String,
    pub side: DiffSide,
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct DiffPanel {
    pub root: PathBuf,
    /// Relative to the repository.
    pub path: String,
    /// The staged changes, else the working ones.
    pub staged: bool,
    #[reflect(ignore)]
    loaded: Option<Result<Arc<Diff>, String>>,
    #[reflect(ignore)]
    task: Option<(u64, ManagedTask<Result<Diff, String>>)>,
    #[reflect(ignore)]
    scan: u64,
    #[reflect(ignore)]
    confirm: Option<usize>,
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

    fn title(&self) -> String {
        let name = self.path.rsplit('/').next().unwrap_or(&self.path);
        if self.staged {
            format!("{name} (Staged)")
        } else {
            format!("{name} (Changes)")
        }
    }

    /// The diff shown, once read.
    pub fn diff(&self) -> Option<&Diff> {
        self.loaded
            .as_ref()?
            .as_ref()
            .ok()
            .map(|diff| diff.as_ref())
    }
}

/// Show a file's diff: the panel already showing it, else the preview diff
/// (replaced), else a new one (`diff_area`). `pin` keeps it (not a preview)
/// and focuses it; a preview leaves the Git panel focused.
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
    });
    let panel = match preview {
        Some(panel) => {
            world.entity_mut(panel).insert(DiffPanel::new(descriptor));
            panel
        }
        None => {
            let area = diff_area(world, layout, origin)?;
            let panel = world.spawn((DiffPanel::new(descriptor), InArea(area))).id();
            world.flush();
            panel
        }
    };
    let title = world.get::<DiffPanel>(panel).unwrap().title();
    if let Some(mut instance) = world.get_mut::<PanelInstance>(panel) {
        instance.title = title;
        instance.preview = !pin;
    }
    if pin {
        world.layouts_mut().focus_panel(panel)?;
    } else {
        world.layouts_mut().select_panel(panel)?;
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
    if let Some(area) = areas.iter().copied().find(|&area| {
        layouts
            .panels_in_area(area)
            .iter()
            .any(|&p| beside(world, p))
    }) {
        return layouts.area_for(layout, beside).or(Ok(area));
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

/// Read each diff panel's diff when there's been a new scan; take finished reads.
pub(super) fn update(world: &mut World, session: &mut GitSession) {
    let panels: Vec<Entity> = world
        .query_filtered::<Entity, With<DiffPanel>>()
        .iter(world)
        .collect();
    for panel in panels {
        let title = world.get::<DiffPanel>(panel).unwrap().title();
        if let Some(mut instance) = world.get_mut::<PanelInstance>(panel)
            && instance.title != title
        {
            instance.title = title;
        }
        let mut diff = world.get_mut::<DiffPanel>(panel).unwrap();
        if let Some((scan, task)) = diff.task.take() {
            match task.try_recv() {
                Ok(result) => {
                    diff.loaded = Some(result.map(Arc::new));
                    diff.scan = scan;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => diff.task = Some((scan, task)),
                Err(_) => diff.loaded = Some(Err("Reading the diff stopped".into())),
            }
        }
        let stale = diff.loaded.is_none() || diff.scan != session.scans;
        if !stale || diff.task.is_some() || session.scan.is_some() {
            continue;
        }
        let descriptor = diff.descriptor();
        let scan = session.scans;
        let task = world.resource_mut::<ManagedWork>().spawn_task(
            Some(panel),
            "Read Git diff",
            move |cancel| match git_job(
                descriptor.root,
                Operation::Diff {
                    path: descriptor.path,
                    side: descriptor.side,
                },
                &cancel,
                &AtomicBool::new(false),
            )? {
                Output::Diff(diff) => Ok(diff),
                _ => Err("Git answered with something else".into()),
            },
        );
        let mut diff = world.get_mut::<DiffPanel>(panel).unwrap();
        match task {
            Ok(task) => diff.task = Some((scan, task)),
            Err(error) => diff.loaded = Some(Err(error)),
        }
    }
}

/// One row of the unified diff.
enum Line<'a> {
    Hunk(usize, &'a crate::Hunk),
    Text {
        kind: LineKind,
        old: Option<usize>,
        new: Option<usize>,
        text: &'a str,
    },
}

fn lines(diff: &Diff) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    for (index, hunk) in diff.hunks.iter().enumerate() {
        lines.push(Line::Hunk(index, hunk));
        let (mut old, mut new) = (hunk.old_start, hunk.new_start);
        for line in &hunk.lines {
            let (o, n) = match line.kind {
                LineKind::Context => {
                    old += 1;
                    new += 1;
                    (Some(old), Some(new))
                }
                LineKind::Removed => {
                    old += 1;
                    (Some(old), None)
                }
                LineKind::Added => {
                    new += 1;
                    (None, Some(new))
                }
            };
            lines.push(Line::Text {
                kind: line.kind,
                old: o,
                new: n,
                text: line.text.trim_end_matches(['\n', '\r']),
            });
        }
    }
    lines
}

enum Asked {
    Operation(Operation),
    Side(bool),
    Open,
}

impl Panel for DiffPanel {
    const LABEL: &'static str = "Diff";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let writable = world
            .get_resource::<GitSession>()
            .is_some_and(|session| session.writable());
        let Some(mut state) = world.entity_mut(panel).take::<DiffPanel>() else {
            return;
        };
        let mut asked = None;
        draw(ui, context, &mut state, writable, &mut asked);
        let root = state.root.clone();
        let path = state.path.clone();
        world.entity_mut(panel).insert(state);
        let result = match asked {
            None => Ok(()),
            Some(Asked::Side(staged)) => {
                let mut diff = world.get_mut::<DiffPanel>(panel).unwrap();
                diff.staged = staged;
                diff.loaded = None;
                diff.task = None;
                Ok(())
            }
            Some(Asked::Open) => world
                .workbench_mut()
                .open_path(&root.join(&path), PathOptions::default())
                .map(|_| ()),
            Some(Asked::Operation(operation)) => match world.remove_resource::<GitSession>() {
                Some(mut session) => {
                    let result = begin(world, &mut session, operation);
                    world.insert_resource(session);
                    result
                }
                None => Err("Git isn't ready yet".into()),
            },
        };
        if let Err(error) = result {
            world
                .workbench_mut()
                .notify(Notification::error(format!("Git: {error}")));
        }
    }
}

fn draw(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    state: &mut DiffPanel,
    writable: bool,
    asked: &mut Option<Asked>,
) {
    let p = context.theme.palette.clone();
    let muted = theme::color(p.muted);
    // The bar: the file, which side, open it.
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&state.path).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut frame = context.frame(ui);
                    let open = frame.icon_button(BuiltinIcon::NewFile);
                    frame.tooltip(&open, "Open File");
                    if open.clicked() {
                        *asked = Some(Asked::Open);
                    }
                    let ui = frame.ui;
                    if ui.selectable_label(state.staged, "Staged").clicked() && !state.staged {
                        *asked = Some(Asked::Side(true));
                    }
                    if ui.selectable_label(!state.staged, "Changes").clicked() && state.staged {
                        *asked = Some(Asked::Side(false));
                    }
                });
            });
        });
    ui.separator();
    let diff = match &state.loaded {
        None => {
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        }
        Some(Err(error)) => {
            ui.colored_label(theme::color(p.error), error);
            return;
        }
        Some(Ok(diff)) => diff.clone(),
    };
    if diff.binary {
        ui.weak("Binary file: no lines to compare.");
        return;
    }
    if diff.hunks.is_empty() {
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            ui.weak(if state.staged {
                "Nothing staged in this file."
            } else {
                "No changes in this file."
            });
        });
        return;
    }
    let rows = lines(&diff);
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let row_height = (font.size * 1.5).round();
    let digits = rows
        .iter()
        .filter_map(|row| match row {
            Line::Text { old, new, .. } => old.max(new).map(|n| n.to_string().len()),
            Line::Hunk(..) => None,
        })
        .max()
        .unwrap_or(1) as f32;
    let char_width = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let number_width = (digits + 1.0) * char_width;
    let added = theme::color(theme::mix(p.success, p.panel_background, 0.82));
    let removed = theme::color(theme::mix(p.error, p.panel_background, 0.82));
    let hunk_fill = theme::color(theme::mix(p.accent, p.panel_background, 0.88));
    let ink = theme::color(p.text);
    egui::ScrollArea::both().auto_shrink(false).show_rows(
        ui,
        row_height,
        rows.len(),
        |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in &rows[range] {
                let width = ui.available_width().max(400.0);
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(width, row_height), egui::Sense::hover());
                let painter = ui.painter();
                match row {
                    Line::Hunk(index, hunk) => {
                        painter.rect_filled(rect, 0.0, hunk_fill);
                        painter.text(
                            rect.left_center() + egui::vec2(8.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            &hunk.header,
                            font.clone(),
                            muted,
                        );
                        hunk_buttons(ui, rect, *index, &diff, state, writable, asked);
                    }
                    Line::Text {
                        kind,
                        old,
                        new,
                        text,
                    } => {
                        let (fill, mark) = match kind {
                            LineKind::Added => (Some(added), "+"),
                            LineKind::Removed => (Some(removed), "-"),
                            LineKind::Context => (None, " "),
                        };
                        if let Some(fill) = fill {
                            painter.rect_filled(rect, 0.0, fill);
                        }
                        let mut x = rect.left() + 8.0;
                        for number in [old, new] {
                            if let Some(number) = number {
                                painter.text(
                                    egui::pos2(x + number_width - char_width, rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    number.to_string(),
                                    font.clone(),
                                    muted,
                                );
                            }
                            x += number_width;
                        }
                        painter.text(
                            egui::pos2(x + 4.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            format!("{mark} {text}"),
                            font.clone(),
                            ink,
                        );
                    }
                }
            }
        },
    );
}

/// A change's buttons, at the right of its header: Stage or Unstage, and
/// (for working changes) Discard, which asks first.
fn hunk_buttons(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    index: usize,
    diff: &Diff,
    state: &mut DiffPanel,
    writable: bool,
    asked: &mut Option<Asked>,
) {
    let enabled = writable && diff.can_apply_hunks;
    let mut buttons = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(8.0, 1.0)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    let path = diff.path.clone();
    let snapshot = diff.snapshot.clone();
    if state.confirm == Some(index) {
        if buttons.small_button("Keep").clicked() {
            state.confirm = None;
        }
        if buttons
            .add_enabled(enabled, egui::Button::new("Discard").small())
            .clicked()
        {
            state.confirm = None;
            *asked = Some(Asked::Operation(Operation::DiscardHunk {
                path,
                snapshot,
                hunk: index,
            }));
        }
        buttons.weak("Discard this change?");
        return;
    }
    if state.staged {
        if buttons
            .add_enabled(enabled, egui::Button::new("Unstage").small())
            .clicked()
        {
            *asked = Some(Asked::Operation(Operation::UnstageHunk {
                path,
                snapshot,
                hunk: index,
            }));
        }
    } else {
        if buttons
            .add_enabled(enabled, egui::Button::new("Discard…").small())
            .clicked()
        {
            state.confirm = Some(index);
        }
        if buttons
            .add_enabled(enabled, egui::Button::new("Stage").small())
            .clicked()
        {
            *asked = Some(Asked::Operation(Operation::StageHunk {
                path,
                snapshot,
                hunk: index,
            }));
        }
    }
}
