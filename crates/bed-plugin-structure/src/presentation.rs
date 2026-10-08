//! Source outline presentation. Workbench owns document targeting and navigation.
use bed_editing::identity::DocumentId;
use bed_highlight::outline::{OutlineKey, OutlineResult, OutlineService, OutlineStatus};
use bed_ui::util::tree_animation::TreeAnimation;
use dear_imgui_rs::{Condition, StyleVar, Ui};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct StructurePanel {
    expanded: HashMap<(DocumentId, u64), bool>,
    selected: Option<(DocumentId, u64)>,
    source: Option<OutlineKey>,
    visible: Vec<u64>,
    node_indices: HashMap<u64, usize>,
    animation: TreeAnimation<u64>,
    visibility_dirty: bool,
    pub rows: Vec<(u64, [f32; 2], [f32; 2])>,
    pub row_paints: Vec<(u64, usize, usize)>,
    pub caret_spacing: f32,
}

pub struct StructureJump {
    pub key: OutlineKey,
    pub offset: usize,
}
impl StructurePanel {
    pub fn draw(
        &mut self,
        ui: &Ui,
        service: &OutlineService,
        animations: bool,
    ) -> Option<StructureJump> {
        let _controls = bed_ui::util::popup_style::controls_style(ui);
        let Some(key) = service.requested() else {
            ui.text_disabled("Select a document to view its structure");
            return None;
        };
        let filename = key
            .path
            .rsplit(['/', '\\'])
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("Untitled");
        ui.text(filename);
        if ui.is_item_hovered() && !key.path.is_empty() {
            ui.tooltip_text(&key.path);
        }
        ui.same_line();
        ui.text_disabled(&key.language_id);
        ui.separator();
        let updating = service.updating();
        if updating {
            ui.text_disabled("Updating…");
        }
        let result = service.result()?;
        match &result.status {
            OutlineStatus::Unsupported => {
                ui.text_disabled("Structure is not available for this language");
                return None;
            }
            OutlineStatus::TooLarge => {
                ui.text_disabled("Structure is unavailable for files over 100 MiB");
                return None;
            }
            OutlineStatus::Failed(error) => {
                ui.text_wrapped(format!("Could not build structure: {error}"));
                return None;
            }
            OutlineStatus::Ready => {}
        }
        if result.nodes.is_empty() {
            ui.text_disabled("No outline entries");
            return None;
        }
        let reset = self
            .source
            .as_ref()
            .is_none_or(|key| key.document != result.key.document);
        let source_changed = self.source.as_ref() != Some(&result.key);
        self.refresh_visible(result);
        if source_changed {
            self.animation
                .retain(|id| self.node_indices.contains_key(id));
        }
        self.animation.update(
            &self.visible,
            ui.time(),
            ui.frame_count(),
            animations,
            true,
            reset,
        );
        let mut jump = None;
        {
            self.rows.clear();
            self.row_paints.clear();
        }
        ui.child_window("structure_tree")
            .size([0.0, 0.0])
            .build(ui, || {
                let _disabled = ui.begin_disabled_with_cond(updating);
                let padding = ui.clone_style().frame_padding();
                // Native tree labels include a fixed arrow column as well as
                // frame padding. This halves the visible arrow-to-label gap.
                let _caret_spacing = ui.push_style_var(StyleVar::FramePadding([
                    ui.current_font_size() * 0.14,
                    padding[1],
                ]));
                {
                    self.caret_spacing = ui.clone_style().frame_padding()[0];
                }
                let document = result.key.document;
                let row_height = ui.text_line_height_with_spacing();
                let layout = self.animation.layout(ui, row_height);
                let base_alpha = ui.clone_style().alpha();
                for row in layout.visible.clone() {
                    let id = self.animation.rows()[row];
                    let index = self.node_indices[&id];
                    let node = &result.nodes[index];
                    let motion = self.animation.sample(&id);
                    ui.set_cursor_pos(layout.position(row));
                    let _clip = layout.row_clip(ui, row);
                    let _alpha = ui.push_style_var(StyleVar::Alpha(base_alpha * motion.alpha));
                    let _closing_alpha = ui.push_style_var(StyleVar::DisabledAlpha(1.0));
                    let _closing = ui.begin_disabled_with_cond(!motion.interactive);
                    let identity = (document, node.id);
                    let has_children = result
                        .nodes
                        .get(index + 1)
                        .is_some_and(|next| next.parent == Some(index));
                    let expanded = self
                        .expanded
                        .get(&identity)
                        .copied()
                        .unwrap_or(node.depth == 0);
                    let indent = node.depth as f32 * ui.text_line_height() * 0.5;
                    if indent > 0.0 {
                        ui.indent_by(indent);
                    }
                    let first_vertex = ui.with_bound_context(|| unsafe {
                        (*dear_imgui_rs::sys::igGetWindowDrawList()).VtxBuffer.Size as usize
                    });
                    let open = ui
                        .tree_node_config(format!("outline_{}", node.id))
                        .label(&node.label)
                        .opened(expanded, Condition::Always)
                        .selected(self.selected == Some(identity))
                        .leaf(!has_children)
                        .open_on_arrow(true)
                        .span_avail_width(true)
                        .no_tree_push_on_open(true)
                        .push()
                        .is_some();
                    self.row_paints.push((
                        id,
                        first_vertex,
                        ui.with_bound_context(|| unsafe {
                            (*dear_imgui_rs::sys::igGetWindowDrawList()).VtxBuffer.Size as usize
                        }),
                    ));
                    if motion.interactive && ui.is_item_toggled_open() && has_children {
                        self.expanded.insert(identity, open);
                        self.visibility_dirty = true;
                    } else if motion.interactive && ui.is_item_clicked() && !updating {
                        self.selected = Some(identity);
                        jump = Some(StructureJump {
                            key: result.key.clone(),
                            offset: node.name_range.start,
                        });
                    }
                    if motion.interactive {
                        self.rows
                            .push((node.id, ui.item_rect_min(), ui.item_rect_max()));
                    }
                    if motion.interactive && ui.is_item_hovered() {
                        ui.tooltip_text(format!("{} · {}", node.kind, node.label));
                    }
                    if indent > 0.0 {
                        ui.unindent_by(indent);
                    }
                }
                layout.finish(ui);
            });
        jump
    }
    fn refresh_visible(&mut self, result: &OutlineResult) {
        if self.source.as_ref() == Some(&result.key) && !self.visibility_dirty {
            return;
        }
        let source_changed = self.source.as_ref() != Some(&result.key);
        if self
            .source
            .as_ref()
            .is_none_or(|key| key.document != result.key.document)
        {
            // Expansion is local to this panel and its current source; bound memory on switches.
            self.expanded.clear();
            self.selected = None;
        } else if source_changed {
            let ids = result
                .nodes
                .iter()
                .map(|node| (result.key.document, node.id))
                .collect::<HashSet<_>>();
            self.expanded.retain(|id, _| ids.contains(id));
        }
        if source_changed {
            self.node_indices = result
                .nodes
                .iter()
                .enumerate()
                .map(|(index, node)| (node.id, index))
                .collect();
        }
        self.source = Some(result.key.clone());
        self.visibility_dirty = false;
        self.visible.clear();
        let mut hidden_below = None;
        for node in &result.nodes {
            if hidden_below.is_some_and(|depth| node.depth > depth) {
                continue;
            }
            hidden_below = None;
            self.visible.push(node.id);
            if !self
                .expanded
                .get(&(result.key.document, node.id))
                .copied()
                .unwrap_or(node.depth == 0)
            {
                hidden_below = Some(node.depth);
            }
        }
    }
}
