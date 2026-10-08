//! Compare Rust paint geometry and measurements with the pinned original C++.
//! Colors follow the host theme and are checked by the theme rendering tests.
//! Generated fixtures: scripts/provenance/update-view-fixtures.sh; LICENSE and NOTICE.
use bed_core::editor_view_state::Selection;
use bed_highlight::tree_sitter::TreeSitter;
use bed_lsp::diagnostics::diagnostics_store::{DiagnosticItem, LspDiagnostics};
use bed_session::editor::Editor;
use bed_ui::views::{
    caret_view::CaretView,
    gutter_view::GutterView,
    hover_tooltip::TooltipArbiter,
    hover_trigger::Info,
    text_view::TextView,
    view_layout::{ViewLayout, column_at_x, glyph_advance_bytes, line_column_x},
};
use dear_imgui_rs::{
    Condition, Context, FontLoader, FramePrepareOptions, StyleVar, Ui, WindowFlags, sys,
};
use git2::{Repository, Signature};
use serde_json::Value;
use std::{fs, path::PathBuf};

struct FixtureRepo(PathBuf);
impl FixtureRepo {
    fn new(name: &str, baseline: &str, current: &[u8]) -> Self {
        let root = std::env::temp_dir().join(format!("bed-view-{}-{name}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let repo = Repository::init(&root).unwrap();
        let blob = repo.blob(baseline.as_bytes()).unwrap();
        let mut builder = repo.treebuilder(None).unwrap();
        builder.insert("fixture.rs", blob, 0o100644).unwrap();
        let tree = repo.find_tree(builder.write().unwrap()).unwrap();
        let signature =
            Signature::new("bed-fixture", "fixture@example.com", &git2::Time::new(1, 0)).unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "fixture", &tree, &[])
            .unwrap();
        fs::write(root.join("fixture.rs"), current).unwrap();
        Self(root)
    }
}
impl Drop for FixtureRepo {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn pair(value: &Value) -> [f32; 2] {
    [
        value[0].as_f64().unwrap() as f32,
        value[1].as_f64().unwrap() as f32,
    ]
}

fn draw_geometry(ui: &Ui, draw: impl FnOnce()) -> (Vec<[f64; 3]>, Vec<i32>) {
    let (vertex_base, index_base) = ui.with_bound_context(|| unsafe {
        let list = sys::igGetWindowDrawList();
        ((*list).VtxBuffer.Size, (*list).IdxBuffer.Size)
    });
    draw();
    ui.with_bound_context(|| {
        // The active window owns these buffers. Copy after all mutations; no
        // native references survive the call or the next draw-list allocation.
        unsafe {
            let list = sys::igGetWindowDrawList();
            let mut vertices = Vec::new();
            for index in vertex_base..(*list).VtxBuffer.Size {
                let v = *(*list).VtxBuffer.Data.add(index as usize);
                vertices.push([f64::from(v.pos.x), f64::from(v.pos.y), f64::from(v.col)]);
            }
            let indices = (index_base..(*list).IdxBuffer.Size)
                .map(|index| i32::from(*(*list).IdxBuffer.Data.add(index as usize)) - vertex_base)
                .collect();
            (vertices, indices)
        }
    })
}

fn compare_geometry(name: &str, actual: (Vec<[f64; 3]>, Vec<i32>), expected: &Value, leaf: &str) {
    let vertices = expected[format!("{leaf}_vertices")].as_array().unwrap();
    assert_eq!(
        actual.0.len(),
        vertices.len(),
        "{name}: {leaf} vertex count"
    );
    for (index, (actual, expected)) in actual.0.iter().zip(vertices).enumerate() {
        for axis in 0..2 {
            let expected = expected[axis].as_f64().unwrap();
            assert!(
                (actual[axis] - expected).abs() < 0.0001,
                "{name}: {leaf} vertex {index}, axis {axis}: {} != {expected}",
                actual[axis]
            );
        }
    }
    let indices: Vec<i32> = expected[format!("{leaf}_indices")]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_i64().unwrap() as i32)
        .collect();
    assert_eq!(actual.1, indices, "{name}: {leaf} triangle order");
}

#[test]
fn paint_geometry_and_byte_measurements_match_upstream() {
    // This integration executable has one GUI test, so it owns the context
    // serially; lib.rs GUI tests run in their own executable/process.
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/views.json"
    )))
    .unwrap();
    assert_eq!(fixture["imgui_version"], "1.92.9 WIP");
    let mut context = Context::create();
    context
        .set_ini_filename(None::<std::path::PathBuf>)
        .unwrap();
    context
        .font_atlas()
        .set_font_loader(FontLoader::stb_truetype())
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let bytes: Vec<u8> = case["bytes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_u64().unwrap() as u8)
            .collect();
        let repo = case["git_baseline"]
            .as_str()
            .map(|baseline| FixtureRepo::new(name, baseline, &bytes));
        let mut editor = Editor::new();
        editor.set_content(&bytes);
        if case["highlight"].as_bool().unwrap()
            || case["diagnostics_enabled"].as_bool().unwrap()
            || case["gutter"].as_bool().unwrap()
        {
            editor.state.path = repo.as_ref().map_or_else(
                || "/fixture.rs".into(),
                |repo| repo.0.join("fixture.rs").to_str().unwrap().into(),
            );
        }
        if let Some(repo) = &repo {
            editor
                .git
                .borrow_mut()
                .init(&editor.state, repo.0.to_str().unwrap(), true);
        }
        assert_eq!(
            editor.git.borrow().current_git_changes,
            case["git_changes"].as_str().unwrap()
        );
        if case["diagnostics_enabled"].as_bool().unwrap() {
            let diagnostics = LspDiagnostics::new();
            diagnostics.replace(
                &editor.state.path,
                case["diagnostics"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|d| DiagnosticItem {
                        start_line: d[0].as_i64().unwrap() as i32,
                        start_character: d[1].as_i64().unwrap() as i32,
                        end_line: d[2].as_i64().unwrap() as i32,
                        end_character: d[3].as_i64().unwrap() as i32,
                        severity: d[4].as_i64().unwrap() as i32,
                        message: "fixture diagnostic".into(),
                        source: "fixture".into(),
                    })
                    .collect(),
                7,
            );
            editor.diagnostics = Some(diagnostics);
        }
        editor.view.selections = case["selections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| Selection {
                head_row: s[0].as_i64().unwrap() as i32,
                head_column: s[1].as_i64().unwrap() as i32,
                anchor_row: s[2].as_i64().unwrap() as i32,
                anchor_column: s[3].as_i64().unwrap() as i32,
                ..Default::default()
            })
            .collect();
        editor.view.primary_index = case["primary"].as_u64().unwrap() as usize;
        editor.view.sync_primary_mirrors();
        editor.view.block_input = case["blocked"].as_bool().unwrap();
        editor.view.cursor_blink_time = 0.37;
        editor.view.scroll_position = [0.0, case["scroll_y"].as_f64().unwrap() as f32];
        if case["highlight"].as_bool().unwrap() {
            editor.state.path = "/fixture.rs".into();
            editor.state.language_id = "rs".into();
            TreeSitter::highlight_snippet("rs", &bytes);
            editor
                .highlight
                .highlight_content(&editor.state, &mut editor.ops);
            editor.highlight.poll(&editor.state, &editor.ops);
            assert!(!editor.highlight.spans_for_line(0).is_empty());
        }
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("View fixture")
            .position([20.0, 20.0], Condition::Always)
            .size(pair(&case["window_size"]), Condition::Always)
            .flags(WindowFlags::NO_TITLE_BAR)
            .build(|| {
                ui.with_bound_context(|| unsafe {
                    let baked = sys::igGetFontBaked();
                    let glyph = sys::ImFontBaked_FindGlyph(baked, b'A'.into());
                    (*glyph).AdvanceX = 7.25;
                    *(*baked).IndexAdvanceX.Data.add(b'A' as usize) = 7.25;
                    (*sys::igGetCurrentWindow()).Scroll.y = editor.view.scroll_position[1];
                });
                let layout = ViewLayout {
                    text_pos: pair(&case["origin"]),
                    line_height: ui.text_line_height(),
                    size: pair(&case["window_size"]),
                    rainbow_mode: case["rainbow"].as_bool().unwrap(),
                    editor_top_margin: case["top_margin"].as_f64().unwrap() as f32,
                    ..Default::default()
                };
                assert_eq!(
                    f64::from(layout.line_height),
                    case["line_height"].as_f64().unwrap()
                );
                let text = draw_geometry(ui, || {
                    TextView::draw_with_highlight(
                        ui,
                        &editor.state,
                        &editor.view,
                        &layout,
                        Some(&editor.highlight),
                    );
                    if let Some(diagnostics) = &editor.diagnostics {
                        TextView::draw_diagnostics(
                            ui,
                            &editor.state,
                            &layout,
                            diagnostics,
                            Info::default(),
                            &TooltipArbiter::default(),
                        );
                    }
                });
                compare_geometry(name, text, case, "text");
                let caret = draw_geometry(ui, || {
                    CaretView::draw(ui, &editor.state, &editor.view, &layout)
                });
                compare_geometry(name, caret, case, "caret");
                if case["gutter"].as_bool().unwrap() {
                    let _group = ui.begin_group();
                    let border = ui.push_style_var(StyleVar::ChildBorderSize(0.0));
                    let compact_width = GutterView::width(ui, &editor.state)
                        + GutterView::diagnostic_column_width(ui, &editor);
                    // The user requested compact document-dependent gutters.
                    // Remove the 6px trimmed from upstream's trailing padding
                    // so its recorded text and marker positions still match;
                    // the native Frame regression covers the compact layout.
                    let width = case["gutter_width"].as_f64().unwrap() as f32 - 6.0;
                    assert!(
                        compact_width <= width + 0.0001,
                        "{name}: compact gutter fits the historical reservation"
                    );
                    let mut pos = [0.0; 2];
                    ui.child_window("LineNumbers")
                        .size([width, ui.content_region_avail()[1]])
                        .flags(WindowFlags::NO_SCROLLBAR)
                        .build(ui, || pos = ui.cursor_screen_pos());
                    drop(border);
                    ui.same_line();
                    let expected_pos = pair(&case["gutter_pos"]);
                    assert!(
                        (pos[0] - expected_pos[0]).abs() < 0.0001,
                        "{name}: gutter x"
                    );
                    assert!(
                        (pos[1] + layout.editor_top_margin - expected_pos[1]).abs() < 0.0001,
                        "{name}: gutter y"
                    );
                    let gutter = draw_geometry(ui, || {
                        GutterView::draw(
                            ui,
                            &ui.get_window_draw_list(),
                            &editor,
                            &layout,
                            pos,
                            width,
                        )
                    });
                    compare_geometry(name, gutter, case, "gutter");
                }
                for (row, expected) in case["measurements"].as_array().unwrap().iter().enumerate() {
                    let line = editor.state.line(row as i32);
                    for (index, expected) in
                        expected["columns"].as_array().unwrap().iter().enumerate()
                    {
                        let actual = line_column_x(ui, &line, index as i32 - 1, layout.text_pos[0]);
                        assert!(
                            (f64::from(actual) - expected.as_f64().unwrap()).abs() < 0.0001,
                            "{name}: byte column {index} row {row}"
                        );
                    }
                    for (x, expected) in [-10.0, 0.0, 1.0, 7.1, 15.0, 22.75, 35.0, 200.0]
                        .into_iter()
                        .zip(expected["hits"].as_array().unwrap())
                    {
                        assert_eq!(
                            i64::from(column_at_x(ui, &line, x)),
                            expected.as_i64().unwrap(),
                            "{name}: hit x={x} row {row}"
                        );
                    }
                    assert!(
                        (f64::from(glyph_advance_bytes(ui, &line))
                            - expected["raw_advance"].as_f64().unwrap())
                        .abs()
                            < 0.0001,
                        "{name}: bounded raw byte advance row {row}"
                    );
                }
            });
        drop(context.render_legacy());
    }
}
