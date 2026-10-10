use bed_editing::identity::DocumentId;
use bed_plugin::{
    DocumentKind, HostContext, HostRequest, PanelAction, PluginDocument, PluginPanel,
};
use bed_plugin_markdown::MarkdownPanel;
use dear_imgui_rs::{Condition, Context, FramePrepareOptions, StyleColor, WindowFlags};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

static IMGUI_LOCK: Mutex<()> = Mutex::new(());
struct Harness {
    context: Context,
    panel: MarkdownPanel,
    documents: Vec<PluginDocument>,
    requests: Vec<HostRequest>,
    size: [f32; 2],
}
impl Harness {
    fn new(source: &str, state: Value) -> Self {
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let id = DocumentId::next();
        Self {
            context,
            panel: MarkdownPanel::new(id, &state).unwrap(),
            documents: vec![PluginDocument {
                id,
                path: "/tmp/readme.md".into(),
                kind: DocumentKind::Text,
                language_id: "markdown".into(),
                revision: (0, 0),
                dirty: false,
                bytes: source.as_bytes().to_vec().into(),
                text: None,
            }],
            requests: Vec::new(),
            size: [900.0, 500.0],
        }
    }
    fn frame(&mut self) -> usize {
        self.context
            .prepare_frame(FramePrepareOptions::new(self.size, 1.0 / 60.0));
        let ui = self.context.frame();
        let host = HostContext {
            remote: false,
            documents: &self.documents,
            active_document: Some(self.documents[0].id),
            settings: &Value::Null,
            textures: &HashMap::new(),
            animations: false,
            workspace: 1,
            diagnostics: &Value::Null,
            viewer_menu: None,
            default_viewers: &Value::Null,
        };
        ui.window("Markdown fixture")
            .position([0.0; 2], Condition::Always)
            .size(self.size, Condition::Always)
            .flags(WindowFlags::NO_TITLE_BAR)
            .build(|| self.panel.draw(ui, &host, &mut self.requests));
        self.context.render_legacy().total_vtx_count()
    }
    fn ready(&mut self) {
        self.frame();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.panel.is_ready() {
            self.frame();
            assert!(self.panel.error().is_none(), "{:?}", self.panel.error());
            assert!(Instant::now() < deadline, "Markdown worker did not finish");
            thread::sleep(Duration::from_millis(1));
        }
        self.frame();
    }
    fn edit(&mut self, source: &str) {
        self.documents[0].revision.0 += 1;
        self.documents[0].bytes = source.as_bytes().to_vec().into();
        self.documents[0].dirty = true;
    }
}

#[test]
fn native_render_supports_markdown_styles_images_themes_and_resize() {
    let _lock = IMGUI_LOCK.lock().unwrap();
    let source = "# A document\n\nA paragraph with **bold**, *italic*, ~~removed~~, `inline code`, [a link](other.md), and wrapping words.\n\n> A quote\n>\n> 1. first\n>    - nested\n> 2. second\n\n- [x] Done\n- [ ] Planned\n\n| Name | Value |\n| :--- | ---: |\n| **answer** | `42` |\n\n```rust\nfn main() { println!(\"hello\"); }\n```\n\n![Alt text](missing.png)\n\n<div>Literal HTML</div>\n\n---\n";
    let mut harness = Harness::new(source, Value::Null);
    harness.ready();
    assert!(harness.frame() > 200);
    assert_eq!(harness.documents[0].bytes.as_ref(), source.as_bytes());
    harness.size = [450.0, 300.0];
    assert!(harness.frame() > 100);
    harness.context.style_mut().set_font_size_base(22.0);
    harness
        .context
        .style_mut()
        .set_color(StyleColor::Text, [0.15, 0.15, 0.15, 1.0]);
    assert!(harness.frame() > 100);
    assert_eq!(
        harness.panel.attached_document(),
        Some(harness.documents[0].id)
    );
    assert!(harness.requests.is_empty());
}

#[test]
fn revisions_replace_models_and_invalid_text_recovers() {
    let _lock = IMGUI_LOCK.lock().unwrap();
    let mut harness = Harness::new("# Original", Value::Null);
    harness.ready();
    harness.edit(&format!("# Obsolete\n\n{}", "line\n\n".repeat(30_000)));
    harness.frame();
    harness.edit("# Current\n\nunsaved");
    harness.frame();
    harness.ready();
    assert_eq!(harness.documents[0].bytes.as_ref(), b"# Current\n\nunsaved");
    harness.documents[0].revision.0 += 1;
    harness.documents[0].bytes = vec![255].into();
    let deadline = Instant::now() + Duration::from_secs(10);
    while harness.panel.error().is_none() {
        harness.frame();
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!harness.panel.is_ready());
    assert!(harness.panel.error().unwrap().contains("UTF-8"));
    harness.edit("# Recovered");
    harness.ready();
    harness.panel.close(&mut harness.requests);
    assert!(harness.panel.render_output().is_none());
}

#[test]
fn code_rendering_clips_to_viewport_and_scroll_survives_revisions() {
    let _lock = IMGUI_LOCK.lock().unwrap();
    let source = format!("```\n{}\n```", "a visible code line\n".repeat(10_000));
    let mut harness = Harness::new(&source, json!({"scroll":[0.0,1200.0]}));
    harness.ready();
    harness.frame();
    let vertices = harness.frame();
    assert!(vertices < 20_000, "viewport produced {vertices} vertices");
    let saved = harness.panel.save_state();
    assert!(saved["scroll"][1].as_f64().unwrap() > 1000.0, "{saved}");
    harness.edit(&source);
    harness.ready();
    harness.frame();
    let saved = harness.panel.save_state();
    assert!(saved["scroll"][1].as_f64().unwrap() > 1000.0, "{saved}");
}

#[test]
fn undo_redo_are_host_owned_and_preview_has_no_edit_actions() {
    let panel = MarkdownPanel::new(DocumentId::next(), &Value::Null).unwrap();
    let id = panel.attached_document().unwrap();
    let mut panel = panel;
    let mut requests = Vec::new();
    let host = HostContext {
        remote: false,
        documents: &[],
        active_document: None,
        settings: &Value::Null,
        textures: &HashMap::new(),
        animations: false,
        workspace: 1,
        diagnostics: &Value::Null,
        viewer_menu: None,
        default_viewers: &Value::Null,
    };
    assert!(
        panel
            .action(PanelAction::Undo, &host, &mut requests)
            .unwrap()
    );
    assert!(
        panel
            .action(PanelAction::Redo, &host, &mut requests)
            .unwrap()
    );
    assert!(
        !panel
            .action(PanelAction::CommitEdit, &host, &mut requests)
            .unwrap()
    );
    assert!(
        matches!(requests.as_slice(),[HostRequest::Undo{document:a},HostRequest::Redo{document:b}] if *a==id && *b==id)
    );
}
