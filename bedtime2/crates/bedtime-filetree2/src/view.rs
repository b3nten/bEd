//! Viewport-only painting and interaction. Neither rendering nor navigation
//! performs filesystem work; even the flattened row projection comes from work.
use super::*;
use bedtime_ui2::{BuiltinIcon, theme::color};
use bedtime_workbench2::{PanelFactoryPayload, PanelPresentation};
use std::collections::HashSet;

use bedtime_ui2::tree::{INSET, RowMotion, animation_fraction};

#[derive(Default)]
pub(crate) struct TreeView {
    pub(crate) row_rects: Vec<(PathBuf, egui::Rect)>,
    pub(crate) reveal: Option<PathBuf>,
    pub(crate) animating: bool,
    pub(crate) drawn_version: u64,
    pub(crate) collapsing: Option<PathBuf>,
    pub(crate) drop_target: Option<(PathBuf, egui::Rect)>,
    row_height: f32,
    scroll: f32,
    pending_scroll: Option<f32>,
    anchor: Option<(PathBuf, f32)>,
    motions: HashMap<PathBuf, (RowMotion, u64)>,
    retiring: Vec<RetiringRow>,
    last_rows: Vec<(FileTreeRow, f32)>,
    time: f64,
    type_prefix: String,
    type_time: f64,
    hover_expand: Option<(PathBuf, f64)>,
}
struct RetiringRow {
    row: FileTreeRow,
    from: f32,
    target: f32,
    started: f64,
}

impl TreeView {
    pub(crate) fn snapshot_changed(&mut self, old: &FileTreeSnapshot, new: &FileTreeSnapshot) {
        let collapsing = self.collapsing.take();
        let height = self.row_height;
        if height <= 0.0 {
            return;
        }
        let delta = self
            .anchor
            .as_ref()
            .and_then(|(path, offset)| {
                // Losing a renamed/moved anchor is not a request to scroll to root.
                // Only an explicit collapse may anchor to a surviving ancestor.
                new.row_index(path)
                    .or_else(|| {
                        collapsing
                            .as_ref()
                            .filter(|folder| path.starts_with(folder))
                            .and_then(|folder| new.row_index(folder))
                    })
                    .map(|index| (index as f32 * height + offset) - self.scroll)
            })
            .unwrap_or(0.0);
        if delta != 0.0 {
            self.pending_scroll = Some((self.scroll + delta).max(0.0));
            for (motion, _) in self.motions.values_mut() {
                motion.shift(delta);
            }
            for retiring in &mut self.retiring {
                retiring.from += delta;
                retiring.target += delta;
            }
        }
        // At most the previous viewport is retained. Collapsing 100k descendants
        // never creates 100k animation records or calls into their widgets.
        for (row, y) in &self.last_rows {
            if new.row_index(&row.path).is_none()
                && old.generation == new.generation
                && collapsing
                    .as_ref()
                    .is_some_and(|folder| row.path != *folder && row.path.starts_with(folder))
            {
                let target = row
                    .path
                    .ancestors()
                    .skip(1)
                    .find_map(|parent| new.row_index(parent))
                    .map_or(*y + delta, |index| index as f32 * height);
                self.retiring.push(RetiringRow {
                    row: row.clone(),
                    from: *y + delta,
                    target,
                    started: self.time,
                });
            }
        }
        if self.retiring.len() > self.last_rows.len().saturating_mul(2).max(64) {
            self.retiring.drain(..self.retiring.len() / 2);
        }
    }
}

#[derive(Clone)]
struct FileDrag {
    panel: Entity,
    workspace: Entity,
    generation: u64,
    root: PathBuf,
    paths: Vec<PathBuf>,
}

pub(crate) fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
    let focused = world
        .get::<PanelPresentation>(panel)
        .is_some_and(|p| p.focused);
    let window_focused = world
        .get::<Window>(context.surface)
        .is_none_or(|w| w.focused);
    let configured = world.get::<FileTreeRoot>(panel).is_some();
    let settings = world
        .get::<FileTreeSettings>(panel)
        .copied()
        .unwrap_or_default();
    let indent = settings.effective_indent();
    let Some(mut state) = world.entity_mut(panel).take::<FileTreePanel>() else {
        return;
    };
    let now = ui.input(|input| input.time);
    state.view.time = now;
    state.view.row_rects.clear();
    state.view.animating = false;
    state.view.drop_target = None;
    if !configured {
        egui::Frame::new().inner_margin(14).show(ui, |ui| {
            ui.weak("No directory selected.");
            ui.label("The application supplies a FileTreeRoot on this panel.");
        });
        world.entity_mut(panel).insert(state);
        return;
    }
    let snapshot = state.snapshot.clone();
    let content_rect = ui.available_rect_before_wrap();
    let text_focused = ui.ctx().text_edit_focused();
    let keyboard = ui.is_enabled()
        && focused
        && window_focused
        && !text_focused
        && state.editing.is_none()
        && !ui.ctx().any_popup_open()
        && ui.ctx().memory(|m| m.top_modal_layer().is_none());
    if keyboard {
        navigate(ui, &mut state);
    }
    let has_status = (snapshot.rows.is_empty() && snapshot.error.is_none())
        || state.is_busy()
        || state.progress.is_some()
        || state.show_hidden
        || state
            .editing
            .as_ref()
            .is_some_and(|e| e.kind != EditKind::Rename);
    if has_status {
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                if snapshot.rows.is_empty() && snapshot.error.is_none() {
                    ui.weak("Loading directory…");
                }
                if state.is_busy() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        if ui.small_button("Cancel").clicked() {
                            state.queue(Action::CancelOperation);
                        }
                    });
                }
                if let Some(progress) = &state.progress {
                    ui.weak(format!(
                        "{} items · {} bytes",
                        progress.completed_items, progress.bytes
                    ));
                }
                if state.show_hidden {
                    ui.weak("Showing hidden items temporarily");
                }
                if state
                    .editing
                    .as_ref()
                    .is_some_and(|edit| edit.kind != EditKind::Rename)
                {
                    let mut edit = state.editing.take().unwrap();
                    ui.horizontal(|ui| {
                        ui.label(if edit.kind == EditKind::File {
                            "New file:"
                        } else {
                            "New folder:"
                        });
                        edit_widget(ui, context, &mut edit, panel, &mut state);
                    });
                    // Escape explicitly clears editing rather than acting like focus loss.
                    if !ui.ctx().data(|d| {
                        d.get_temp::<bool>(egui::Id::new((panel, "cancel-edit")))
                            .unwrap_or(false)
                    }) {
                        state.editing = Some(edit);
                    }
                    ui.ctx()
                        .data_mut(|d| d.remove::<bool>(egui::Id::new((panel, "cancel-edit"))));
                }
            });
    }
    let height = bedtime_ui2::tree::row_height(context);
    state.view.row_height = height;
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt((panel, "files-scroll"))
        .auto_shrink([false, false]);
    if let Some(offset) = state.view.pending_scroll.take() {
        scroll = scroll.vertical_scroll_offset(offset);
    }
    if let Some(path) = state.view.reveal.take() {
        if let Some(index) = snapshot.row_index(&path) {
            let y = index as f32 * height;
            let view_height = ui.available_height();
            let offset = if y < state.view.scroll {
                y
            } else if y + height > state.view.scroll + view_height {
                y + height - view_height
            } else {
                state.view.scroll
            };
            scroll = scroll.vertical_scroll_offset(offset.max(0.0));
        } else if snapshot.loading || snapshot.revision != state.revision {
            state.view.reveal = Some(path);
        }
    }
    let payload = egui::DragAndDrop::payload::<FileDrag>(ui.ctx()).or_else(|| {
        egui::DragAndDrop::payload::<PanelFactoryPayload>(ui.ctx())?.content::<FileDrag>()
    });
    let source_valid = payload.as_ref().is_some_and(|payload| {
        let current = if payload.panel == panel {
            payload.generation == state.generation && payload.root == snapshot.root
        } else {
            world
                .get::<FileTreePanel>(payload.panel)
                .is_some_and(|source| {
                    source.generation == payload.generation && source.snapshot.root == payload.root
                })
        };
        current
            && state.workspace == Some(payload.workspace)
            && !payload.paths.iter().any(|path| path == &payload.root)
    });
    let copy_drag = ui.input(|i| i.modifiers.alt || i.modifiers.command)
        || payload.as_ref().is_some_and(|payload| {
            payload.root != snapshot.root
                || world
                    .get::<FileTreeSettings>(payload.panel)
                    .is_some_and(|settings| settings.read_only)
        });
    let native_sources: Vec<PathBuf> = ui.input(|i| {
        if i.raw.dropped_files.is_empty() {
            i.raw
                .hovered_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        } else {
            i.raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_owned())
                .collect()
        }
    });
    let dragging = payload.is_some() || !native_sources.is_empty();
    if dragging {
        ui.ctx().request_repaint();
    }
    let writable =
        ui.is_enabled() && !settings.read_only && !state.is_busy() && state.editing.is_none();
    let rounding = world
        .get::<bedtime_workbench2::WorkbenchSurface>(context.surface)
        .map_or(7.0, |surface| surface.rounding);
    let output = scroll.show_viewport(ui, |ui, viewport| {
        let width = ui.available_width();
        let top = ui.max_rect().top();
        ui.set_min_height((snapshot.rows.len() as f32 * height + INSET).max(viewport.height()));
        ui.set_min_width(width);
        let background = ui.interact(
            ui.max_rect(),
            egui::Id::new((panel, "file-background")),
            egui::Sense::CLICK,
        );
        if background.secondary_clicked() {
            state.menu_target = Some(MenuTarget {
                path: snapshot.root.clone(),
                directory: snapshot.root.clone(),
                paths: Vec::new(),
                generation: state.generation,
            });
        }
        let min = ((viewport.min.y / height).floor().max(0.0) as usize).saturating_sub(2);
        let max = ((viewport.max.y / height).ceil().max(0.0) as usize + 3).min(snapshot.rows.len());
        let frame = ui.ctx().cumulative_frame_nr();
        let mut candidates: Vec<usize> = (min.min(max)..max).collect();
        let mut seen: HashSet<usize> = candidates.iter().copied().collect();
        for (path, (motion, _)) in &state.view.motions {
            let (y, _) = motion.sample(now, context.motion);
            if y + height >= viewport.min.y
                && y <= viewport.max.y
                && let Some(index) = snapshot.row_index(path)
                && seen.insert(index)
            {
                candidates.push(index);
            }
        }
        candidates.sort_unstable();
        state.view.last_rows.clear();
        for index in candidates {
            let row = &snapshot.rows[index];
            let target = index as f32 * height;
            let (motion, seen) = state
                .view
                .motions
                .entry(row.path.clone())
                .or_insert_with(|| {
                    (
                        RowMotion::new(
                            target + 5.0,
                            target,
                            if snapshot.version > 1 { 0.0 } else { 1.0 },
                            now,
                        ),
                        frame,
                    )
                });
            motion.retarget(target, 1.0, now, context.motion);
            *seen = frame;
            let (y, alpha) = motion.sample(now, context.motion);
            if alpha < 0.999 || (y - target).abs() > 0.01 {
                state.view.animating = true;
                ui.ctx().request_repaint();
            }
            let rect = egui::Rect::from_min_size(
                egui::pos2(ui.max_rect().left() + 3.0, top + y),
                egui::vec2(width - 6.0, height),
            );
            if !rect.intersects(ui.clip_rect()) {
                continue;
            }
            state.view.last_rows.push((row.clone(), y));
            let response = ui.interact(
                rect,
                egui::Id::new((panel, state.generation, &row.path)),
                // Workbench owns keyboard focus for this canvas. Per-row egui
                // focus targets eat the first arrow as widget-focus navigation
                // before our draw, especially after a very fast snapshot load.
                egui::Sense::CLICK | egui::Sense::DRAG,
            );
            if response.clicked() {
                ui.memory_mut(|memory| {
                    if let Some(id) = memory.focused() {
                        memory.surrender_focus(id);
                    }
                });
            }
            state.view.row_rects.push((row.path.clone(), rect));
            paint_row(
                ui,
                context,
                &state,
                row,
                rect,
                Some(&response),
                alpha,
                indent,
            );
            if row.kind == FileTreeEntryKind::Directory {
                let arrow_rect = egui::Rect::from_min_size(
                    rect.min + egui::vec2(INSET + row.depth as f32 * indent, 0.0),
                    egui::vec2(18.0, height),
                );
                let arrow_clicked = response.clicked()
                    && response
                        .interact_pointer_pos()
                        .is_some_and(|p| arrow_rect.contains(p));
                if response.double_clicked() {
                    // The first click already toggled. A double click is not a
                    // second single-click toggle that immediately undoes it.
                    state.select(&row.path, false, false);
                } else if arrow_clicked {
                    state.toggle(&row.path);
                } else if response.clicked() {
                    let modifiers = ui.input(|i| i.modifiers);
                    state.select(&row.path, modifiers.command, modifiers.shift);
                    state.toggle(&row.path);
                }
            } else if response.double_clicked() {
                state.select(&row.path, false, false);
                state.queue(Action::Open {
                    path: row.path.clone(),
                    activation: FileTreeActivation::DoubleClick,
                });
            } else if response.clicked() {
                let modifiers = ui.input(|i| i.modifiers);
                state.select(&row.path, modifiers.command, modifiers.shift);
                if !modifiers.command && !modifiers.shift {
                    state.queue(Action::Open {
                        path: row.path.clone(),
                        activation: FileTreeActivation::SingleClick,
                    });
                }
            }
            if response.secondary_clicked() {
                if !state.selection.contains(&row.path) {
                    state.select(&row.path, false, false);
                }
                state.menu_target = Some(MenuTarget {
                    path: row.path.clone(),
                    directory: if row.kind == FileTreeEntryKind::Directory {
                        row.path.clone()
                    } else {
                        row.path.parent().unwrap_or(&snapshot.root).to_owned()
                    },
                    paths: state.selection.iter().cloned().collect(),
                    generation: state.generation,
                });
            }
            if state
                .editing
                .as_ref()
                .is_some_and(|edit| edit.kind == EditKind::Rename && edit.path == row.path)
            {
                let mut edit = state.editing.take().unwrap();
                let left = rect.left() + INSET + row.depth as f32 * indent + 38.0;
                let edit_rect = egui::Rect::from_min_max(
                    egui::pos2(left, rect.top()),
                    rect.max - egui::vec2(4.0, 0.0),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(edit_rect), |ui| {
                    edit_widget(ui, context, &mut edit, panel, &mut state);
                });
                let cancelled = ui.ctx().data(|d| {
                    d.get_temp::<bool>(egui::Id::new((panel, "cancel-edit")))
                        .unwrap_or(false)
                });
                ui.ctx()
                    .data_mut(|d| d.remove::<bool>(egui::Id::new((panel, "cancel-edit"))));
                if !cancelled {
                    state.editing = Some(edit);
                }
            }
            if row.path != snapshot.root && response.drag_started() {
                if !state.selection.contains(&row.path) {
                    state.select(&row.path, false, false);
                }
                let drag = FileDrag {
                    panel,
                    workspace: state.workspace.unwrap(),
                    generation: state.generation,
                    root: snapshot.root.clone(),
                    paths: operations::top_level_paths(
                        &state.selection.iter().cloned().collect::<Vec<_>>(),
                    ),
                };
                if drag.paths.len() == 1
                    && row.kind != FileTreeEntryKind::Directory
                    && let Some(factory) = world.get_resource::<FileTreePanelDrag>()
                {
                    response.dnd_set_drag_payload(PanelFactoryPayload::with_content(
                        (factory.0)(row.path.clone()),
                        drag,
                    ));
                } else {
                    response.dnd_set_drag_payload(drag);
                }
            }
            response.context_menu(|ui| {
                menu::draw(
                    ui,
                    &mut state,
                    settings.read_only,
                    world.get_resource::<crate::FileTreeOpenWith>(),
                )
            });
            if let FileTreeDirectoryState::Failed(_) = &row.directory_state {
                context.frame(ui).tooltip(
                    &response,
                    "This folder couldn't be read. Use Refresh to try again.",
                );
            }
        }
        let pointer = ui.input(|i| i.pointer.latest_pos());
        let inside = pointer.is_some_and(|pointer| ui.clip_rect().contains(pointer));
        let destination = if dragging && writable && inside && !ui.ctx().any_popup_open() {
            let hovered = state
                .view
                .row_rects
                .iter()
                .filter(|(_, rect)| rect.contains(pointer.unwrap()))
                .min_by(|(_, a), (_, b)| {
                    (a.center().y - pointer.unwrap().y)
                        .abs()
                        .total_cmp(&(b.center().y - pointer.unwrap().y).abs())
                })
                .map(|(path, _)| path.as_path());
            let directory = enclosing_directory(&snapshot, hovered);
            directory
                .filter(|directory| {
                    if let Some(payload) = &payload {
                        source_valid && transfer_allowed(&payload.paths, directory, copy_drag)
                    } else {
                        transfer_allowed(&native_sources, directory, true)
                    }
                })
                .map(Path::to_owned)
        } else {
            None
        };
        let hovered_directory = destination
            .as_ref()
            .filter(|path| !state.is_expanded(path))
            .cloned();
        if let Some(path) = hover_expansion(&mut state.view, hovered_directory, now) {
            state.toggle(&path);
        }
        if state.view.hover_expand.is_some() {
            ui.ctx().request_repaint();
        }
        if let Some(directory) = &destination {
            if payload.is_some() {
                PanelFactoryPayload::reserve_for_content(ui.ctx());
            }
            if let (Some(first), Some(end)) = (
                snapshot.row_index(directory),
                snapshot.subtree_end_index(directory),
            ) {
                let rect = egui::Rect::from_min_max(
                    egui::pos2(ui.max_rect().left() + 3.0, top + first as f32 * height),
                    egui::pos2(ui.max_rect().right() - 3.0, top + end as f32 * height),
                )
                .intersect(ui.clip_rect());
                state.view.drop_target = Some((directory.clone(), rect));
            }
        }
        // Eligibility and destination are shared by feedback and release. An
        // invalid gesture never consumes a mutation slot or creates an error.
        if payload.is_some()
            && inside
            && ui.input(|i| i.pointer.any_released())
            && (destination.is_some()
                || !egui::DragAndDrop::has_payload_of_type::<PanelFactoryPayload>(ui.ctx()))
        {
            egui::DragAndDrop::clear_payload(ui.ctx());
            if let (Some(payload), Some(directory)) = (&payload, &destination) {
                state.queue(Action::Transfer {
                    sources: payload.paths.clone(),
                    directory: directory.clone(),
                    copy: copy_drag,
                });
            }
        }
        if inside && !ui.ctx().any_popup_open() {
            let files = ui.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
            if !files.is_empty()
                && let Some(directory) = destination
            {
                state.queue(Action::Transfer {
                    sources: files.iter().map(|file| file.path().to_owned()).collect(),
                    directory,
                    copy: true,
                });
            }
        }
        for retiring in &state.view.retiring {
            let t = animation_fraction(now, retiring.started, context.motion);
            let y = retiring.from + (retiring.target - retiring.from) * t;
            let rect = egui::Rect::from_min_size(
                egui::pos2(ui.max_rect().left() + 3.0, top + y),
                egui::vec2(width - 6.0, height),
            );
            if rect.intersects(ui.clip_rect()) {
                paint_row(
                    ui,
                    context,
                    &state,
                    &retiring.row,
                    rect,
                    None,
                    1.0 - t,
                    indent,
                );
            }
        }
        state
            .view
            .retiring
            .retain(|retiring| animation_fraction(now, retiring.started, context.motion) < 1.0);
        if !state.view.retiring.is_empty() {
            state.view.animating = true;
            ui.ctx().request_repaint();
        }
        state.view.motions.retain(|_, (_, seen)| *seen + 2 >= frame);
        if let Some(row) = snapshot
            .rows
            .get((viewport.min.y / height).floor().max(0.0) as usize)
        {
            let index = snapshot.row_index(&row.path).unwrap();
            state.view.anchor = Some((row.path.clone(), viewport.min.y - index as f32 * height));
        }
        if let Some((_, rect)) = &state.view.drop_target {
            bedtime_ui2::paint_drop_overlay(ui.painter(), &context.theme.palette, *rect, rounding);
        }
        if dragging
            && writable
            && let Some(pointer) = ui.input(|i| i.pointer.latest_pos())
        {
            let delta = drag_scroll_delta(pointer, ui.clip_rect(), ui.input(|i| i.stable_dt));
            if delta != egui::Vec2::ZERO {
                ui.scroll_with_delta(delta);
                ui.ctx().request_repaint();
            }
        }
        background.context_menu(|ui| {
            menu::draw(
                ui,
                &mut state,
                settings.read_only,
                world.get_resource::<crate::FileTreeOpenWith>(),
            )
        });
    });
    state.view.scroll = output.state.offset.y;
    if !content_rect.contains(
        ui.input(|i| i.pointer.latest_pos())
            .unwrap_or(egui::Pos2::ZERO),
    ) {
        state.view.hover_expand = None;
    }
    state.view.drawn_version = snapshot.version;
    world.entity_mut(panel).insert(state);
}

fn drag_scroll_delta(pointer: egui::Pos2, viewport: egui::Rect, dt: f32) -> egui::Vec2 {
    if !viewport.contains(pointer) {
        return egui::Vec2::ZERO;
    }
    let speed = if pointer.y < viewport.top() + 28.0 {
        280.0
    } else if pointer.y > viewport.bottom() - 28.0 {
        -280.0
    } else {
        0.0
    };
    egui::vec2(0.0, speed * dt.min(0.1))
}

fn hover_expansion(view: &mut TreeView, path: Option<PathBuf>, now: f64) -> Option<PathBuf> {
    let Some(path) = path else {
        view.hover_expand = None;
        return None;
    };
    if let Some((previous, since)) = &view.hover_expand
        && previous == &path
    {
        if now - since >= 0.65 {
            view.hover_expand = None;
            return Some(path);
        }
    } else {
        view.hover_expand = Some((path, now));
    }
    None
}

/// A file row is never a destination: its enclosing visible directory is.
fn enclosing_directory<'a>(
    snapshot: &'a FileTreeSnapshot,
    hovered: Option<&Path>,
) -> Option<&'a Path> {
    let path = hovered.unwrap_or(&snapshot.root);
    let row = &snapshot.rows[snapshot.row_index(path)?];
    let directory = if row.kind == FileTreeEntryKind::Directory {
        row.path.as_path()
    } else {
        row.path.parent()?
    };
    let row = &snapshot.rows[snapshot.row_index(directory)?];
    (row.kind == FileTreeEntryKind::Directory
        && !matches!(row.directory_state, FileTreeDirectoryState::Failed(_)))
    .then_some(row.path.as_path())
}

/// Obvious invalid/no-op gestures are rejected before feedback or admission.
/// Filesystem races and permissions remain the worker's authoritative checks.
pub(crate) fn transfer_allowed(sources: &[PathBuf], directory: &Path, copy: bool) -> bool {
    !sources.is_empty()
        && !directory.as_os_str().is_empty()
        && !sources
            .iter()
            .any(|source| directory == source || directory.starts_with(source))
        && (copy
            || sources
                .iter()
                .any(|source| source.parent() != Some(directory)))
}

fn paint_row(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    state: &FileTreePanel,
    row: &FileTreeRow,
    rect: egui::Rect,
    response: Option<&egui::Response>,
    alpha: f32,
    indent: f32,
) {
    let expanded = (row.kind == FileTreeEntryKind::Directory).then(|| state.is_expanded(&row.path));
    let icon = match row.kind {
        FileTreeEntryKind::Directory if expanded == Some(true) => BuiltinIcon::FolderOpen,
        FileTreeEntryKind::Directory => BuiltinIcon::Folder,
        FileTreeEntryKind::Symlink => BuiltinIcon::Extension,
        _ => file_icon(&row.path),
    };
    let ink = color(
        if matches!(row.directory_state, FileTreeDirectoryState::Failed(_)) {
            context.theme.palette.error
        } else if row.hidden {
            context.theme.palette.muted
        } else {
            context.theme.palette.text
        },
    );
    let editing = state
        .editing
        .as_ref()
        .is_some_and(|edit| edit.kind == EditKind::Rename && edit.path == row.path);
    ui.push_id(&row.path, |ui| {
        bedtime_ui2::tree::paint_row(
            ui,
            context,
            bedtime_ui2::tree::Row {
                label: (!editing).then_some(row.label.as_ref()),
                depth: row.depth,
                icon,
                expanded,
                selected: state.selection.contains(&row.path),
                ink,
                icon_ink: context.theme.palette.text,
            },
            rect,
            response,
            alpha,
            indent,
        );
    });
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    use crate::tests::{Fixture, eventually, finish, host, panel, ready, tree};

    #[test]
    fn hovering_child_files_targets_enclosing_folders_with_complete_visible_extents() {
        let fixture = Fixture::new();
        let folder = fixture.directory("folder");
        let child = fixture.file("folder/child.txt", b"child");
        let root_file = fixture.file("root.txt", b"root");
        let (mut app, workspace) = host();
        let panel = panel(&mut app, workspace, Some(&fixture.0));
        ready(&mut app, panel, &fixture.0);
        app.world_mut()
            .get_mut::<FileTreePanel>(panel)
            .unwrap()
            .toggle(&folder);
        eventually(&mut app, |app| {
            tree(app, panel).snapshot.row_index(&child).is_some()
        });
        let snapshot = tree(&app, panel).snapshot();
        assert_eq!(
            enclosing_directory(snapshot, Some(&child)),
            Some(folder.as_path())
        );
        assert_eq!(
            enclosing_directory(snapshot, Some(&root_file)),
            Some(fixture.0.as_path())
        );
        assert_eq!(
            enclosing_directory(snapshot, Some(&folder)),
            Some(folder.as_path())
        );
        assert_eq!(
            enclosing_directory(snapshot, None),
            Some(fixture.0.as_path())
        );
        assert_eq!(
            snapshot.subtree_end_index(&folder).unwrap(),
            snapshot.row_index(&child).unwrap() + 1
        );
        assert_eq!(
            snapshot.subtree_end_index(&fixture.0),
            Some(snapshot.rows.len())
        );
        finish(&mut app);
    }

    #[test]
    fn a_renamed_row_neither_retires_toward_root_nor_reanchors_scroll_to_root_but_collapse_still_animates()
     {
        let fixture = Fixture::new();
        let folder = fixture.directory("folder");
        let child = fixture.file("folder/child.txt", b"child");
        fixture.file("folder/other.txt", b"other");
        let (mut app, workspace) = host();
        let panel = panel(&mut app, workspace, Some(&fixture.0));
        ready(&mut app, panel, &fixture.0);
        app.world_mut()
            .get_mut::<FileTreePanel>(panel)
            .unwrap()
            .toggle(&folder);
        eventually(&mut app, |app| {
            tree(app, panel).snapshot.row_index(&child).is_some()
        });
        let before = tree(&app, panel).snapshot.clone();
        let renamed = folder.join("z-renamed.txt");
        std::fs::rename(&child, &renamed).unwrap();
        app.world_mut()
            .get_mut::<FileTreePanel>(panel)
            .unwrap()
            .refresh();
        eventually(&mut app, |app| {
            tree(app, panel).snapshot.row_index(&renamed).is_some()
                && tree(app, panel).snapshot.row_index(&child).is_none()
        });
        let after = tree(&app, panel).snapshot.clone();
        let mut view = TreeView {
            row_height: 24.0,
            scroll: before.row_index(&child).unwrap() as f32 * 24.0,
            anchor: Some((child, 0.0)),
            ..Default::default()
        };
        view.last_rows = before
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| (row.clone(), index as f32 * 24.0))
            .collect();
        view.snapshot_changed(&before, &after);
        assert!(
            view.retiring.is_empty(),
            "Rename is not a collapse-to-ancestor animation"
        );
        assert!(
            view.pending_scroll.is_none(),
            "Losing a path anchor is not a request to jump to root"
        );
        app.world_mut()
            .get_mut::<FileTreePanel>(panel)
            .unwrap()
            .toggle(&folder);
        eventually(&mut app, |app| {
            tree(app, panel).snapshot.row_index(&renamed).is_none()
        });
        let collapsed = tree(&app, panel).snapshot.clone();
        view.last_rows = after
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| (row.clone(), index as f32 * 24.0))
            .collect();
        view.collapsing = Some(folder.clone());
        view.snapshot_changed(&after, &collapsed);
        assert!(!view.retiring.is_empty());
        assert!(
            view.retiring
                .iter()
                .all(|row| row.target == collapsed.row_index(&folder).unwrap() as f32 * 24.0)
        );
        finish(&mut app);
    }

    #[test]
    fn indentation_configuration_cannot_make_nonfinite_or_negative_layout() {
        for (indent_size, expected) in [
            (0.0, 0.0),
            (5.0, 5.0),
            (-2.0, 0.0),
            (100.0, 64.0),
            (f32::NAN, 16.0),
            (f32::INFINITY, 16.0),
        ] {
            assert_eq!(
                FileTreeSettings {
                    indent_size,
                    ..Default::default()
                }
                .effective_indent(),
                expected
            );
        }
    }

    #[test]
    fn a_drag_scrolls_only_the_hovered_viewport_in_a_split_layout() {
        let left = egui::Rect::from_min_max(egui::pos2(0.0, 80.0), egui::pos2(300.0, 600.0));
        let right = left.translate(egui::vec2(320.0, 0.0));
        let pointer = egui::pos2(500.0, 590.0);
        assert_eq!(drag_scroll_delta(pointer, left, 0.016), egui::Vec2::ZERO);
        assert!(drag_scroll_delta(pointer, right, 0.016).y < 0.0);
        for pointer in [
            egui::pos2(500.0, 79.0),
            egui::pos2(500.0, 601.0),
            egui::pos2(310.0, 590.0),
        ] {
            assert_eq!(drag_scroll_delta(pointer, left, 0.016), egui::Vec2::ZERO);
            assert_eq!(drag_scroll_delta(pointer, right, 0.016), egui::Vec2::ZERO);
        }
        assert!(drag_scroll_delta(egui::pos2(10.0, 81.0), left, 0.016).y > 0.0);
    }

    #[test]
    fn hover_expansion_requires_continuous_dwell_and_resets_on_drag_end() {
        let mut view = TreeView::default();
        let folder = PathBuf::from("folder");
        assert!(hover_expansion(&mut view, Some(folder.clone()), 1.0).is_none());
        assert!(hover_expansion(&mut view, Some(folder.clone()), 1.5).is_none());
        assert!(hover_expansion(&mut view, None, 1.6).is_none());
        assert!(hover_expansion(&mut view, Some(folder.clone()), 2.0).is_none());
        assert!(hover_expansion(&mut view, Some(folder.clone()), 2.4).is_none());
        assert_eq!(
            hover_expansion(&mut view, Some(folder.clone()), 2.7),
            Some(folder)
        );
        assert!(view.hover_expand.is_none());
    }
}

/// Bundled filename artwork, without filesystem I/O. Shared by file listings
/// such as the Files panel and application-owned changed-file trees.
pub fn file_icon(path: &Path) -> BuiltinIcon {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
    {
        "rs" => BuiltinIcon::Rust,
        "py" => BuiltinIcon::Python,
        "js" | "mjs" => BuiltinIcon::Javascript,
        "ts" => BuiltinIcon::Typescript,
        "json" => BuiltinIcon::Json,
        "toml" | "ini" => BuiltinIcon::Config,
        "md" => BuiltinIcon::Markdown,
        "png" | "jpg" | "jpeg" | "svg" | "webp" => BuiltinIcon::Image,
        "sh" => BuiltinIcon::Bash,
        "txt" => BuiltinIcon::Text,
        _ => BuiltinIcon::Default,
    }
}

fn edit_widget(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    edit: &mut InlineEdit,
    panel: Entity,
    state: &mut FileTreePanel,
) {
    let id = egui::Id::new((panel, state.generation, "inline-name"));
    let mut output = ui
        .add_enabled_ui(edit.pending.is_none(), |ui| {
            let width = ui.available_width();
            context.frame(ui).text_edit(
                egui::TextEdit::singleline(&mut edit.text)
                    .id(id)
                    .desired_width(width),
            )
        })
        .inner;
    if edit.focus && ui.is_enabled() && ui.ctx().memory(|memory| memory.top_modal_layer().is_none())
    {
        output.response.request_focus();
        let count = if edit.kind == EditKind::Rename {
            Path::new(&edit.text)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&edit.text)
                .chars()
                .count()
        } else {
            edit.text.chars().count()
        };
        output
            .state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(count),
            )));
        output.state.store(ui.ctx(), id);
        edit.focus = false;
    }
    if (output.response.has_focus() || output.response.lost_focus())
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        && edit.pending.is_none()
    {
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new((panel, "cancel-edit")), true));
        output.response.surrender_focus();
    } else if (output.response.has_focus() || output.response.lost_focus())
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
        && edit.pending.is_none()
    {
        state.queue(Action::SubmitEdit);
    }
}

fn navigate(ui: &mut egui::Ui, state: &mut FileTreePanel) {
    let snapshot = state.snapshot.clone();
    if snapshot.rows.is_empty() {
        return;
    }
    let current = state
        .cursor
        .as_deref()
        .and_then(|path| snapshot.row_index(path))
        .unwrap_or(0);
    let modifiers = ui.input(|i| i.modifiers);
    if modifiers.command || modifiers.ctrl || modifiers.alt {
        return;
    }
    let consume = |ui: &mut egui::Ui, key| ui.input_mut(|i| i.consume_key(modifiers, key));
    let page = (ui.available_height() / state.view.row_height.max(24.0))
        .floor()
        .max(1.0) as usize;
    let index = if consume(ui, egui::Key::ArrowDown) {
        Some((current + 1).min(snapshot.rows.len() - 1))
    } else if consume(ui, egui::Key::ArrowUp) {
        Some(current.saturating_sub(1))
    } else if consume(ui, egui::Key::Home) {
        Some(0)
    } else if consume(ui, egui::Key::End) {
        Some(snapshot.rows.len() - 1)
    } else if consume(ui, egui::Key::PageDown) {
        Some((current + page).min(snapshot.rows.len() - 1))
    } else if consume(ui, egui::Key::PageUp) {
        Some(current.saturating_sub(page))
    } else {
        None
    };
    if let Some(index) = index {
        let path = &snapshot.rows[index].path;
        state.select(path, modifiers.command, modifiers.shift);
        state.view.reveal = Some(path.clone());
    } else if consume(ui, egui::Key::ArrowRight) {
        let row = &snapshot.rows[current];
        if row.kind == FileTreeEntryKind::Directory {
            if !state.is_expanded(&row.path) {
                state.toggle(&row.path);
            } else if let Some(child) = snapshot.rows.get(current + 1)
                && child.depth > row.depth
            {
                state.select(&child.path, false, false);
                state.view.reveal = Some(child.path.clone());
            }
        }
    } else if consume(ui, egui::Key::ArrowLeft) {
        let row = &snapshot.rows[current];
        if row.kind == FileTreeEntryKind::Directory && state.is_expanded(&row.path) {
            state.toggle(&row.path);
        } else if let Some(parent) = row.path.parent()
            && snapshot.row_index(parent).is_some()
        {
            state.select(parent, false, false);
            state.view.reveal = Some(parent.to_owned());
        }
    } else if consume(ui, egui::Key::Space) {
        let row = &snapshot.rows[current];
        if row.kind == FileTreeEntryKind::Directory {
            state.toggle(&row.path);
        }
    } else if consume(ui, egui::Key::Enter) {
        let row = &snapshot.rows[current];
        state.queue(Action::Open {
            path: row.path.clone(),
            activation: FileTreeActivation::Keyboard,
        });
    }
    if !modifiers.command && !modifiers.alt {
        let text: String = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect()
        });
        if !text.is_empty() && text.chars().all(|c| !c.is_control()) {
            let now = ui.input(|i| i.time);
            if now - state.view.type_time > 0.8 {
                state.view.type_prefix.clear();
            }
            state.view.type_time = now;
            state.view.type_prefix.push_str(&text.to_lowercase());
            if let Some(index) = (1..=snapshot.rows.len())
                .map(|offset| (current + offset) % snapshot.rows.len())
                .find(|&index| {
                    snapshot.rows[index]
                        .label
                        .to_lowercase()
                        .starts_with(&state.view.type_prefix)
                })
            {
                let path = &snapshot.rows[index].path;
                state.select(path, false, false);
                state.view.reveal = Some(path.clone());
            }
            ui.input_mut(|i| {
                i.events
                    .retain(|event| !matches!(event, egui::Event::Text(_)))
            });
        }
    }
}
