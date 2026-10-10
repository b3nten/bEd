use crate::{
    model::{self, Kind, Tree},
    worker::{Job, Worker},
};
use bed_editing::identity::DocumentId;
use bed_plugin::{HostContext, HostRequest, PanelAction, PluginPanel, Revision};
use dear_imgui_rs::{Condition, ListClipper, StyleColor, Ui, WindowFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    any::Any,
    collections::{BTreeSet, HashSet},
    sync::Arc,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct ViewState {
    expanded: BTreeSet<String>,
    scroll: [f32; 2],
}
impl Default for ViewState {
    fn default() -> Self {
        Self {
            expanded: BTreeSet::from([String::new()]),
            scroll: [0.0; 2],
        }
    }
}

pub struct JsonPanel {
    document: DocumentId,
    worker: Option<Worker>,
    state: ViewState,
    revision: Option<Revision>,
    jsonc: bool,
    serial: u64,
    submitted: bool,
    updating: bool,
    tree: Option<Tree>,
    visible: Vec<usize>,
    error: Option<String>,
    restore_scroll: bool,
}
impl JsonPanel {
    pub fn new(document: DocumentId, state: &Value) -> Self {
        let mut state: ViewState = serde_json::from_value(state.clone()).unwrap_or_default();
        for scroll in &mut state.scroll {
            *scroll = if scroll.is_finite() {
                scroll.clamp(0.0, 1_000_000_000.0)
            } else {
                0.0
            };
        }
        let (worker, error) = match Worker::new() {
            Ok(worker) => (Some(worker), None),
            Err(error) => (None, Some(error)),
        };
        Self {
            document,
            worker,
            state,
            revision: None,
            jsonc: false,
            serial: 0,
            submitted: false,
            updating: false,
            tree: None,
            visible: Vec::new(),
            error,
            restore_scroll: true,
        }
    }
    /// Whether the structure represents the current immutable host snapshot.
    pub fn is_ready(&self) -> bool {
        self.tree.is_some() && !self.updating && self.error.is_none()
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    fn sync(&mut self, host: &HostContext<'_>) {
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        let Some(document) = host.document(self.document) else {
            worker.invalidate();
            self.revision = None;
            self.tree = None;
            self.visible.clear();
            self.updating = false;
            self.error = Some("This document is no longer open.".into());
            return;
        };
        let jsonc = model::jsonc_path(&document.path);
        if self.revision != Some(document.revision) || self.jsonc != jsonc {
            self.revision = Some(document.revision);
            self.jsonc = jsonc;
            self.serial = worker.invalidate();
            self.submitted = false;
            self.updating = true;
            // Do not show a previous revision while new source is being parsed.
            self.tree = None;
            self.visible.clear();
            self.error = None;
            self.restore_scroll = true;
        }
        if !self.submitted {
            self.submitted = worker.submit(Job {
                serial: self.serial,
                revision: document.revision,
                bytes: Arc::clone(&document.bytes),
                jsonc,
            });
        }
        let outputs: Vec<_> = worker.poll().collect();
        for output in outputs {
            if output.serial != self.serial || Some(output.revision) != self.revision {
                continue;
            }
            self.updating = false;
            match output.result {
                Ok(tree) => {
                    let paths: HashSet<_> = tree
                        .nodes
                        .iter()
                        .filter_map(|node| node.path.as_deref())
                        .collect();
                    self.state
                        .expanded
                        .retain(|path| path.is_empty() || paths.contains(path.as_str()));
                    self.tree = Some(tree);
                    self.error = None;
                    self.rebuild_visible();
                }
                Err(error) => {
                    self.tree = None;
                    self.error = Some(error);
                }
            }
        }
    }
    fn rebuild_visible(&mut self) {
        self.visible.clear();
        let Some(tree) = &self.tree else {
            return;
        };
        let mut index = 0;
        while index < tree.nodes.len() {
            self.visible.push(index);
            let node = &tree.nodes[index];
            index = if node
                .path
                .as_ref()
                .is_some_and(|path| !self.state.expanded.contains(path))
            {
                node.end
            } else {
                index + 1
            };
        }
    }
    fn edit_as_text(&self, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        if let Some(menu) = host.viewer_menu
            && let Some(viewer) = menu.choices.iter().find(|viewer| viewer.text_editor)
        {
            requests.push(HostRequest::SwitchViewer {
                tab: menu.tab,
                document: self.document,
                viewer: viewer.id.clone(),
            });
        } else if let Some(document) = host.document(self.document) {
            requests.push(HostRequest::OpenFile {
                path: document.path.clone(),
                viewer: Some("bed.text".into()),
            });
        }
    }
    fn draw_tree(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        let Some(tree) = &self.tree else {
            return;
        };
        let mut toggled = Vec::new();
        ui.child_window("json-tree")
            .flags(WindowFlags::HORIZONTAL_SCROLLBAR)
            .build(ui, || {
                if self.restore_scroll {
                    ui.set_scroll_x(self.state.scroll[0]);
                    ui.set_scroll_y(self.state.scroll[1]);
                    self.restore_scroll = false;
                }
                let row_height = ui.text_line_height_with_spacing();
                for row in ListClipper::new(self.visible.len())
                    .items_height(row_height)
                    .begin(ui)
                    .iter()
                {
                    let index = self.visible[row];
                    let node = &tree.nodes[index];
                    let can_expand =
                        matches!(node.kind, Kind::Object(count) | Kind::Array(count) if count > 0);
                    let opened = node
                        .path
                        .as_ref()
                        .is_some_and(|path| self.state.expanded.contains(path));
                    ui.set_cursor_pos_x(
                        node.depth as f32 * ui.current_font_size() * 1.25
                            + ui.clone_style().window_padding()[0],
                    );
                    let token = ui
                        .tree_node_config(format!("json-node-{index}"))
                        .label("")
                        .leaf(!can_expand)
                        .no_tree_push_on_open(true)
                        .span_avail_width(true)
                        .opened(opened, Condition::Always)
                        .push();
                    if can_expand && token.is_some() != opened {
                        toggled.push(node.path.as_ref().unwrap().clone());
                    }
                    if let Some(_popup) =
                        ui.begin_popup_context_item_with_label(Some("json-row-viewers"))
                    {
                        host.draw_viewer_menu(ui, requests);
                    }
                    ui.same_line();
                    ui.text(format!("{}:", node.label));
                    ui.same_line();
                    let color = match node.kind {
                        Kind::String => StyleColor::CheckMark,
                        Kind::Number => StyleColor::PlotHistogram,
                        Kind::Boolean => StyleColor::PlotLines,
                        Kind::Null => StyleColor::TextDisabled,
                        Kind::Object(_) | Kind::Array(_) => StyleColor::TextDisabled,
                    };
                    ui.text_colored(ui.style_color(color), &node.summary);
                    if ui.is_item_hovered() && !node.kind.container() {
                        let raw = std::str::from_utf8(&tree.bytes[node.range.clone()])
                            .expect("JSON nodes reference valid UTF-8 source");
                        ui.tooltip_text(model::preview(raw));
                    }
                }
                if let Some(_popup) = ui.begin_popup_context_window() {
                    host.draw_viewer_menu(ui, requests);
                }
                self.state.scroll = [ui.scroll_x(), ui.scroll_y()];
            });
        let changed = !toggled.is_empty();
        for path in toggled {
            if !self.state.expanded.remove(&path) {
                self.state.expanded.insert(path);
            }
        }
        if changed {
            self.rebuild_visible();
        }
    }
}

impl PluginPanel for JsonPanel {
    fn title(&self, host: &HostContext<'_>) -> String {
        host.document(self.document).map_or_else(
            || "JSON Tree".into(),
            |document| {
                document
                    .path
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or("JSON Tree")
                    .to_owned()
            },
        )
    }
    fn attached_document(&self) -> Option<DocumentId> {
        Some(self.document)
    }
    fn save_state(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn action(
        &mut self,
        action: PanelAction,
        _: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) -> Result<bool, String> {
        match action {
            PanelAction::Undo => requests.push(HostRequest::Undo {
                document: self.document,
            }),
            PanelAction::Redo => requests.push(HostRequest::Redo {
                document: self.document,
            }),
            PanelAction::CommitEdit => {}
            _ => return Ok(false),
        }
        Ok(true)
    }
    fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, requests: &mut Vec<HostRequest>) {
        self.sync(host);
        ui.text(if self.jsonc {
            "JSONC Tree"
        } else {
            "JSON Tree"
        });
        ui.same_line();
        ui.text_disabled("Read-only");
        ui.separator();
        if let Some(error) = &self.error {
            ui.text_wrapped(error);
            if ui.button("Edit as Text") {
                self.edit_as_text(host, requests);
            }
        } else if self.updating {
            ui.text_disabled("Loading JSON structure…");
            requests.push(HostRequest::Invalidate);
        } else {
            self.draw_tree(ui, host, requests);
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_plugin::{DocumentKind, PluginDocument};
    use serde_json::json;
    use std::{
        collections::HashMap,
        sync::Mutex,
        thread,
        time::{Duration, Instant},
    };

    static IMGUI_LOCK: Mutex<()> = Mutex::new(());

    fn document(source: &str, revision: Revision) -> PluginDocument {
        PluginDocument {
            id: DocumentId(17),
            path: "fixture.json".into(),
            kind: DocumentKind::Text,
            language_id: "json".into(),
            revision,
            dirty: true,
            bytes: Arc::from(source.as_bytes()),
            text: None,
        }
    }
    fn sync(panel: &mut JsonPanel, documents: &[PluginDocument]) {
        panel.sync(&HostContext {
            remote: false,
            default_viewers: &Value::Null,
            viewer_menu: None,
            documents,
            active_document: Some(panel.document),
            settings: &Value::Null,
            textures: &HashMap::new(),
            animations: false,
            workspace: 1,
            diagnostics: &Value::Null,
        });
    }
    fn wait(panel: &mut JsonPanel, documents: &[PluginDocument]) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            sync(panel, documents);
            if !panel.updating {
                return;
            }
            assert!(Instant::now() < deadline, "JSON worker did not finish");
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn shared_unsaved_revisions_replace_tree_and_keep_surviving_expansion_paths() {
        let mut panel = JsonPanel::new(DocumentId(17), &Value::Null);
        let mut documents = [document(r#"{"keep":{"leaf":1},"removed":[]}"#, (1, 1))];
        wait(&mut panel, &documents);
        assert!(panel.is_ready());
        assert_eq!(panel.visible, vec![0, 1, 3]);
        panel.state.expanded.insert("/p:keep:0".into());
        panel.state.expanded.insert("/p:removed:0".into());
        panel.rebuild_visible();
        assert_eq!(panel.visible, vec![0, 1, 2, 3]);
        panel.state.scroll = [12.0, 100.0];
        documents[0] = document(r#"{"added":0,"keep":{"leaf":2}}"#, (1, 2));
        sync(&mut panel, &documents);
        assert!(!panel.is_ready());
        assert!(panel.tree.is_none());
        wait(&mut panel, &documents);
        assert_eq!(
            panel.tree.as_ref().unwrap().nodes.last().unwrap().summary,
            "2"
        );
        assert!(panel.state.expanded.contains("/p:keep:0"));
        assert!(!panel.state.expanded.contains("/p:removed:0"));
        let restored = JsonPanel::new(DocumentId(17), &panel.save_state());
        assert_eq!(restored.state.expanded, panel.state.expanded);
        assert_eq!(restored.state.scroll, [12.0, 100.0]);
    }

    #[test]
    fn syntax_mode_tracks_path_changes_without_a_text_revision_and_invalid_source_hides_tree() {
        let mut panel = JsonPanel::new(DocumentId(17), &Value::Null);
        let mut documents = [document("[1,]", (0, 0))];
        wait(&mut panel, &documents);
        assert!(panel.error().is_some());
        documents[0].path = "fixture.JSONC".into();
        wait(&mut panel, &documents);
        assert!(panel.is_ready());
        documents[0] = document("{", (0, 1));
        wait(&mut panel, &documents);
        assert!(panel.error().unwrap().contains("line 1"));
        assert!(panel.tree.is_none());
        sync(&mut panel, &[]);
        assert!(!panel.is_ready());
        assert!(panel.error().unwrap().contains("no longer open"));
    }

    #[test]
    fn rapid_revisions_never_publish_an_obsolete_snapshot_as_current() {
        let mut panel = JsonPanel::new(DocumentId(17), &Value::Null);
        let mut documents = [document("[0]", (2, 0))];
        for revision in 0..100 {
            documents[0] = document(&format!("[{revision}]"), (2, revision));
            sync(&mut panel, &documents);
            if panel.is_ready() {
                assert_eq!(
                    panel.tree.as_ref().unwrap().nodes[1].summary,
                    revision.to_string()
                );
            }
        }
        wait(&mut panel, &documents);
        assert!(panel.is_ready());
        assert_eq!(panel.tree.as_ref().unwrap().nodes[1].summary, "99");
    }

    #[test]
    fn sanitizes_saved_scroll_and_distinguishes_duplicate_container_paths() {
        let mut panel = JsonPanel::new(
            DocumentId(17),
            &json!({"scroll": [-30.0, 1e30], "expanded": ["", "/p:x:1"]}),
        );
        assert_eq!(panel.state.scroll, [0.0, 1_000_000_000.0]);
        wait(
            &mut panel,
            &[document(r#"{"x":{"a":0}, "x":{"b":1}}"#, (0, 0))],
        );
        assert_eq!(panel.visible, vec![0, 1, 3, 4]);
        assert!(panel.state.expanded.contains("/p:x:1"));
    }

    #[test]
    fn renders_only_visible_tree_rows_in_native_imgui() {
        use dear_imgui_rs::{Context, FramePrepareOptions};
        let _lock = IMGUI_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let source = format!("[{}0]", "12345678901234567890,".repeat(10_000));
        let documents = [document(&source, (0, 0))];
        let mut panel = JsonPanel::new(documents[0].id, &Value::Null);
        wait(&mut panel, &documents);
        assert_eq!(panel.visible.len(), 10_002);
        for _ in 0..2 {
            context.prepare_frame(FramePrepareOptions::new([800.0, 600.0], 1.0 / 60.0));
            let ui = context.frame();
            ui.window("JSON fixture")
                .position([0.0, 0.0], Condition::Always)
                .size([800.0, 600.0], Condition::Always)
                .build(|| {
                    panel.draw(
                        ui,
                        &HostContext {
                            remote: false,
                            default_viewers: &Value::Null,
                            viewer_menu: None,
                            documents: &documents,
                            active_document: Some(documents[0].id),
                            settings: &Value::Null,
                            textures: &HashMap::new(),
                            animations: false,
                            workspace: 1,
                            diagnostics: &Value::Null,
                        },
                        &mut Vec::new(),
                    )
                });
            let vertices = context.render_legacy().total_vtx_count();
            assert!(vertices > 0);
            assert!(
                vertices < 20_000,
                "offscreen rows should not generate vertices: {vertices}"
            );
        }
    }

    #[test]
    fn native_tree_clicks_expand_collapse_and_restore_the_saved_presentation() {
        use dear_imgui_rs::{Context, FramePrepareOptions, MouseButton};
        let _lock = IMGUI_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let documents = [document(r#"{"nested":{"leaf":42}}"#, (0, 0))];
        let mut panel = JsonPanel::new(documents[0].id, &Value::Null);
        wait(&mut panel, &documents);
        fn frame(
            context: &mut Context,
            panel: &mut JsonPanel,
            documents: &[PluginDocument],
        ) -> ([f32; 2], f32) {
            context.prepare_frame(FramePrepareOptions::new([800.0, 600.0], 1.0 / 60.0));
            let ui = context.frame();
            let mut child_min = [0.0; 2];
            let row_height = ui.text_line_height_with_spacing();
            ui.window("JSON click fixture")
                .position([0.0, 0.0], Condition::Always)
                .size([800.0, 600.0], Condition::Always)
                .build(|| {
                    panel.draw(
                        ui,
                        &HostContext {
                            remote: false,
                            default_viewers: &Value::Null,
                            viewer_menu: None,
                            documents,
                            active_document: Some(documents[0].id),
                            settings: &Value::Null,
                            textures: &HashMap::new(),
                            animations: false,
                            workspace: 1,
                            diagnostics: &Value::Null,
                        },
                        &mut Vec::new(),
                    );
                    child_min = ui.item_rect_min();
                });
            drop(context.render_legacy());
            (child_min, row_height)
        }
        fn click(
            context: &mut Context,
            panel: &mut JsonPanel,
            documents: &[PluginDocument],
            position: [f32; 2],
        ) {
            context.io_mut().add_mouse_pos_event(position);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, true);
            frame(context, panel, documents);
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, false);
            frame(context, panel, documents);
        }
        frame(&mut context, &mut panel, &documents);
        let (child_min, row_height) = frame(&mut context, &mut panel, &documents);
        let root_arrow = [child_min[0] + 12.0, child_min[1] + row_height * 0.4];
        assert_eq!(panel.visible, vec![0, 1]);
        click(&mut context, &mut panel, &documents, root_arrow);
        assert_eq!(panel.visible, vec![0], "root click must collapse the tree");
        click(&mut context, &mut panel, &documents, root_arrow);
        assert_eq!(
            panel.visible,
            vec![0, 1],
            "second click must expand the root"
        );
        let nested_arrow = [root_arrow[0] + 16.0, root_arrow[1] + row_height];
        click(&mut context, &mut panel, &documents, nested_arrow);
        assert_eq!(
            panel.visible,
            vec![0, 1, 2],
            "nested click must reveal its leaf"
        );
        let mut restored = JsonPanel::new(documents[0].id, &panel.save_state());
        wait(&mut restored, &documents);
        assert_eq!(restored.visible, vec![0, 1, 2]);
    }
}
