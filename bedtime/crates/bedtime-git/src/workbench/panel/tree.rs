//! Git owns its prepared hierarchy; UI owns the Files tree's artwork and timing.
use super::*;
use bedtime_ui::tree::{self, RowMotion};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct TreeView {
    view: Option<Arc<RepositoryView>>,
    motions: HashMap<String, (RowMotion, u64)>,
    last_rows: Vec<usize>,
    retiring: Vec<RetiringRow>,
    scroll: f32,
    pending_scroll: Option<f32>,
    anchor: Option<(String, f32)>,
    animate_new: bool,
    animating: bool,
}
struct RetiringRow {
    view: Arc<RepositoryView>,
    index: usize,
    key: String,
    motion: RowMotion,
}

// Only an explicitly collapsed ancestor can retire a row or replace a lost
// scroll anchor. A rename/status refresh must never pull the viewport to root.
fn collapsed_parent(old: &RepositoryView, new: &RepositoryView, row: &Row) -> Option<usize> {
    let (group, mut path) = match row {
        Row::Directory { key, .. } => (key.group, key.path.as_str()),
        Row::File { index, group, .. } => (*group, old.status.entries[*index].path.as_str()),
        Row::Header(..) => return None,
    };
    while let Some((parent, _)) = path.rsplit_once('/') {
        let key = DirectoryKey {
            group,
            path: parent.to_owned(),
        };
        if new.collapsed.contains(&key) && !old.collapsed.contains(&key) {
            let key = format!("directory:{group:?}:{parent}");
            if let Some(index) = new.row_index.get(&key) {
                return Some(*index);
            }
        }
        path = parent;
    }
    None
}

impl TreeView {
    fn changed(
        &mut self,
        view: &Arc<RepositoryView>,
        height: f32,
        now: f64,
        policy: bedtime_ui::MotionPolicy,
    ) {
        if self.view.as_ref().is_some_and(|old| Arc::ptr_eq(old, view)) {
            return;
        }
        let old = self.view.take();
        if let Some(old) = old {
            if old.options != view.options || old.status.root != view.status.root {
                *self = Self::default();
                self.pending_scroll = Some(0.0);
            } else {
                self.animate_new = true;
                let delta = self
                    .anchor
                    .as_ref()
                    .and_then(|(key, offset)| {
                        view.row_index
                            .get(key)
                            .copied()
                            .or_else(|| {
                                old.row_index.get(key).and_then(|index| {
                                    collapsed_parent(&old, view, &old.rows[*index])
                                })
                            })
                            .map(|index| index as f32 * height + offset - self.scroll)
                    })
                    .unwrap_or(0.0);
                if delta != 0.0 {
                    self.pending_scroll = Some((self.scroll + delta).max(0.0));
                    for (motion, _) in self.motions.values_mut() {
                        motion.shift(delta);
                    }
                    for row in &mut self.retiring {
                        row.motion.shift(delta);
                    }
                }
                for &index in &self.last_rows {
                    let key = old.rows[index].key(&old.status);
                    if !view.row_index.contains_key(&key)
                        && let Some(parent) = collapsed_parent(&old, view, &old.rows[index])
                        && let Some((mut motion, _)) = self.motions.remove(&key)
                    {
                        motion.retarget(parent as f32 * height, 0.0, now, policy);
                        self.retiring.push(RetiringRow {
                            view: old.clone(),
                            index,
                            key,
                            motion,
                        });
                    }
                }
                // Rapid collapse/reopen reversals start at the displayed state.
                self.retiring.retain(|row| {
                    if let Some(index) = view.row_index.get(&row.key) {
                        let mut motion = row.motion.clone();
                        motion.retarget(*index as f32 * height, 1.0, now, policy);
                        self.motions.insert(row.key.clone(), (motion, 0));
                        false
                    } else {
                        true
                    }
                });
                let cap = self.last_rows.len().saturating_mul(2).max(64);
                if self.retiring.len() > cap {
                    self.retiring.drain(..self.retiring.len() - cap);
                }
            }
        }
        self.view = Some(view.clone());
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel_id: Entity,
    view: &Arc<RepositoryView>,
    collapsed: &BTreeSet<DirectoryKey>,
    panel: &mut GitPanel,
    enabled: bool,
    action: &mut Option<Action>,
) {
    let height = tree::row_height(context);
    let now = ui.input(|i| i.time);
    let frame = ui.ctx().cumulative_frame_nr();
    // Keep presentation state per panel, independent of shared Git operations.
    let mut state = std::mem::take(&mut panel.tree);
    state.changed(view, height, now, context.motion);
    state.animating = false;
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(("git-changes", panel_id))
        .auto_shrink([false; 2]);
    if let Some(offset) = state.pending_scroll.take() {
        scroll = scroll.vertical_scroll_offset(offset);
    }
    let output = scroll.show_viewport(ui, |ui, viewport| {
        let width = ui.available_width();
        let top = ui.max_rect().top();
        let left = ui.max_rect().left() + 3.0;
        ui.set_min_height((view.rows.len() as f32 * height + tree::INSET).max(viewport.height()));
        ui.set_min_width(width);
        let min = ((viewport.min.y / height).floor().max(0.0) as usize).saturating_sub(2);
        let max = ((viewport.max.y / height).ceil().max(0.0) as usize + 3).min(view.rows.len());
        let mut candidates: HashSet<usize> = (min.min(max)..max).collect();
        for (key, (motion, _)) in &state.motions {
            let (y, _) = motion.sample(now, context.motion);
            if y + height >= viewport.min.y
                && y <= viewport.max.y
                && let Some(index) = view.row_index.get(key)
            {
                candidates.insert(*index);
            }
        }
        let mut candidates: Vec<_> = candidates.into_iter().collect();
        candidates.sort_unstable();
        state.last_rows.clear();
        for index in candidates {
            let row = &view.rows[index];
            let key = row.key(&view.status);
            let target = index as f32 * height;
            let (motion, seen) = state.motions.entry(key.clone()).or_insert_with(|| {
                (
                    RowMotion::new(
                        target + 5.0,
                        target,
                        if state.animate_new { 0.0 } else { 1.0 },
                        now,
                    ),
                    frame,
                )
            });
            motion.retarget(target, 1.0, now, context.motion);
            *seen = frame;
            let (y, alpha) = motion.sample(now, context.motion);
            state.animating |= alpha < 0.999 || (y - target).abs() > 0.01;
            let rect = egui::Rect::from_min_size(
                egui::pos2(left, top + y),
                egui::vec2(width - 6.0, height),
            );
            if !rect.intersects(ui.clip_rect()) {
                continue;
            }
            state.last_rows.push(index);
            panel.rendered_rows += 1;
            let mut row = row.clone();
            if let Row::Directory { key, expanded, .. } = &mut row {
                *expanded = !collapsed.contains(key);
            }
            ui.scope_builder(
                egui::UiBuilder::new()
                    .id_salt((&view.status.root, key))
                    .max_rect(rect),
                |ui| {
                    ui.set_opacity(alpha);
                    // Rows use explicit positions; spacing must not change their pitch.
                    ui.spacing_mut().item_spacing.y = 0.0;
                    draw_row(ui, context, view, &row, panel, enabled, action);
                },
            );
        }
        state
            .retiring
            .retain(|row| row.motion.sample(now, context.motion).1 > 0.001);
        for row in &state.retiring {
            let (y, alpha) = row.motion.sample(now, context.motion);
            let rect = egui::Rect::from_min_size(
                egui::pos2(left, top + y),
                egui::vec2(
                    (width
                        - 6.0
                        - 16.0
                        - ui.spacing().interact_size.y
                        - ui.spacing().item_spacing.x)
                        .max(1.0),
                    height,
                ),
            );
            if rect.intersects(ui.clip_rect()) {
                ui.push_id(&row.key, |ui| {
                    paint_entry(
                        ui,
                        context,
                        &row.view,
                        &row.view.rows[row.index],
                        rect,
                        None,
                        panel.selected.as_deref(),
                        None,
                        alpha,
                    )
                });
            }
        }
        state.animating |= !state.retiring.is_empty();
        state.motions.retain(|_, (_, seen)| *seen + 2 >= frame);
        state.anchor = view
            .rows
            .get((viewport.min.y / height).floor().max(0.0) as usize)
            .map(|row| {
                let key = row.key(&view.status);
                let index = view.row_index[&key];
                (key, viewport.min.y - index as f32 * height)
            });
    });
    state.scroll = output.state.offset.y;
    if state.animating {
        ui.ctx().request_repaint();
    }
    panel.tree = state;
}

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_entry(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    view: &RepositoryView,
    row: &Row,
    rect: egui::Rect,
    response: Option<&egui::Response>,
    selected: Option<&str>,
    expanded: Option<bool>,
    alpha: f32,
) {
    let (path, depth, icon, expanded, selected, icon_ink) = match row {
        Row::Directory {
            key,
            depth,
            expanded: open,
            ..
        } => {
            let open = expanded.unwrap_or(*open);
            (
                key.path.as_str(),
                *depth,
                if open {
                    BuiltinIcon::FolderOpen
                } else {
                    BuiltinIcon::Folder
                },
                Some(open),
                selected == Some(key.path.as_str()),
                context.theme.palette.text,
            )
        }
        Row::File {
            index,
            group,
            depth,
        } => {
            let entry = &view.status.entries[*index];
            let (_, ink) = status_ink(context, entry, *group);
            (
                entry.path.as_str(),
                *depth,
                bedtime_filetree::file_icon(Path::new(&entry.path)),
                None,
                selected == Some(entry.path.as_str()),
                ink,
            )
        }
        Row::Header(..) => return,
    };
    // Named for accessibility (and scripts) by its file or folder name.
    if let Some(response) = response {
        let name = path.rsplit('/').next().unwrap_or(path).to_owned();
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, &name)
        });
    }
    tree::paint_row(
        ui,
        context,
        tree::Row {
            label: Some(path.rsplit('/').next().unwrap()),
            depth,
            icon: Some(icon),
            expanded,
            selected,
            ink: bedtime_ui::theme::color(context.theme.palette.text),
            icon_ink,
        },
        rect,
        response,
        alpha,
        tree::INDENT,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Status;
    use bedtime_ui::MotionPolicy;

    fn view(status: Arc<Status>, collapsed: BTreeSet<DirectoryKey>) -> Arc<RepositoryView> {
        Arc::new(RepositoryView::prepare(
            status,
            GitPanelOptions {
                view: ViewMode::Tree,
                group_by: GroupBy::StagedUnstaged,
            },
            collapsed,
        ))
    }
    fn status(count: usize) -> Arc<Status> {
        Arc::new(Status {
            root: "/repo".into(),
            entries: (0..count)
                .map(|i| StatusEntry {
                    path: format!("folder/{i:05}.txt"),
                    old_path: None,
                    index_status: 'M',
                    worktree_status: 'M',
                    conflicted: false,
                })
                .collect(),
            ..Default::default()
        })
    }
    fn visible(
        state: &mut TreeView,
        view: &Arc<RepositoryView>,
        indices: impl Iterator<Item = usize>,
    ) {
        state.last_rows.clear();
        for i in indices {
            state.last_rows.push(i);
            state.motions.insert(
                view.rows[i].key(&view.status),
                (
                    RowMotion::new(i as f32 * 24.0, i as f32 * 24.0, 1.0, 0.0),
                    1,
                ),
            );
        }
    }

    #[test]
    fn collapse_animates_only_the_viewport_and_its_group_and_reopening_reverses_continuously() {
        let policy = MotionPolicy::default();
        let expanded = view(status(10_000), BTreeSet::new());
        let mut state = TreeView::default();
        state.changed(&expanded, 24.0, 0.0, policy);
        // Include copies from both groups in the previously drawn viewport.
        let indices: Vec<_> = expanded
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| matches!(row, Row::File { index: 0..=11, .. }).then_some(i))
            .collect();
        visible(&mut state, &expanded, indices.into_iter());
        let folder = DirectoryKey {
            group: Some(Group::Staged),
            path: "folder".into(),
        };
        let collapsed = view(expanded.status.clone(), BTreeSet::from([folder]));
        state.changed(&collapsed, 24.0, 1.0, policy);
        assert_eq!(
            state.retiring.len(),
            12,
            "10k descendants must retain only the visible staged copies"
        );
        for row in &state.retiring {
            assert!(matches!(
                row.view.rows[row.index],
                Row::File {
                    group: Some(Group::Staged),
                    ..
                }
            ));
            let halfway = row.motion.sample(1.08, policy);
            assert!(halfway.1 > 0.0 && halfway.1 < 1.0);
            assert_eq!(row.motion.sample(1.2, policy), (24.0, 0.0));
        }
        let key = state.retiring[0].key.clone();
        let midpoint = state.retiring[0].motion.sample(1.08, policy);
        state.last_rows.clear();
        state.changed(&expanded, 24.0, 1.08, policy);
        assert!(state.retiring.is_empty());
        let motion = &state.motions[&key].0;
        assert_eq!(motion.sample(1.08, policy), midpoint);
        assert_eq!(motion.sample(1.3, policy).1, 1.0);
    }

    #[test]
    fn collapse_can_anchor_to_parent_but_disappearing_paths_and_new_repositories_cannot() {
        let policy = MotionPolicy::default();
        let expanded = view(status(10), BTreeSet::new());
        let child = expanded.rows[5].key(&expanded.status);
        let mut state = TreeView::default();
        state.changed(&expanded, 24.0, 0.0, policy);
        state.scroll = 5.0 * 24.0;
        state.anchor = Some((child.clone(), 0.0));
        let collapsed = view(
            expanded.status.clone(),
            BTreeSet::from([DirectoryKey {
                group: Some(Group::Staged),
                path: "folder".into(),
            }]),
        );
        state.changed(&collapsed, 24.0, 1.0, policy);
        assert_eq!(state.pending_scroll.take(), Some(24.0));
        state.changed(&expanded, 24.0, 2.0, policy);
        state.pending_scroll = None;
        let mut renamed = (*expanded.status).clone();
        renamed.entries[3].path = "elsewhere.txt".into();
        state.changed(&view(Arc::new(renamed), BTreeSet::new()), 24.0, 3.0, policy);
        assert!(
            state.pending_scroll.is_none(),
            "Rename must not scroll to an arbitrary ancestor"
        );
        assert!(
            state.retiring.is_empty(),
            "Status removals are not folder collapses"
        );
        let mut other = (*expanded.status).clone();
        other.root = "/different-repository".into();
        state.changed(&view(Arc::new(other), BTreeSet::new()), 24.0, 4.0, policy);
        assert!(state.motions.is_empty());
        assert!(state.anchor.is_none());
        assert_eq!(state.pending_scroll, Some(0.0));
    }
}
