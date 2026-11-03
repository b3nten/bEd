use super::*;
use crate::editor::tests::{focus, frame, key};
use crate::syntax::{AnalysisSession, AnalysisStatus};
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
const CODE: &str = "fn one() {\n    let cat = 1;\n    println!(\"{}\", cat);\n}\nfn two() {\n    let dog = 2;\n}\n";
fn observation(doc: &MemoryTextSession) -> Arc<crate::syntax::Observation> {
    let mut analysis = AnalysisSession::new();
    syntax::update_analysis(&mut analysis, doc, "rust").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while analysis.status() == &AnalysisStatus::Pending {
        analysis.poll();
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    analysis.observation().unwrap().clone()
}
#[test]
fn sibling_folds_are_independent_and_hidden_rows_share_hit_and_keyboard_geometry() {
    let mut doc = MemoryTextSession::new(CODE);
    let snapshot = doc.snapshot().unwrap();
    let syntax = observation(&doc);
    let ctx = egui::Context::default();
    let id = egui::Id::new("folded");
    let mut a = EditorView::default();
    let mut b = EditorView::default();
    a.set_syntax(Some(syntax.clone()));
    b.set_syntax(Some(syntax));
    a.settings.line_numbers = false;
    focus(&ctx, &mut a, &mut doc, id);
    a.toggle_fold(&snapshot, 0).unwrap();
    frame(&ctx, &mut a, &mut doc, id, vec![]);
    for _ in 0..12 {
        frame(&ctx, &mut a, &mut doc, id, vec![]);
    }
    let folded = frame(&ctx, &mut a, &mut doc, id, vec![]);
    let full = frame(
        &egui::Context::default(),
        &mut b,
        &mut doc,
        egui::Id::new("sibling"),
        vec![],
    );
    assert!(folded.rendered_lines < full.rendered_lines);
    assert!(b.collapsed_regions().is_empty());
    assert_eq!(doc.snapshot().unwrap().revision, 0);
    frame(
        &ctx,
        &mut a,
        &mut doc,
        id,
        vec![key(egui::Key::ArrowDown, egui::Modifiers::NONE)],
    );
    assert_eq!(doc.text().line_of(a.selection.head()), 4);
    let out = frame(&ctx, &mut a, &mut doc, id, vec![]);
    let click = out.caret_rect.center();
    frame(
        &ctx,
        &mut a,
        &mut doc,
        id,
        vec![
            egui::Event::PointerMoved(click),
            egui::Event::PointerButton {
                pos: click,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    assert_eq!(doc.text().line_of(a.selection.head()), 4);
    assert_eq!(a.collapsed_regions().len(), 1);
}
#[test]
fn navigation_and_find_unfold_targets_and_reveal_only_keeps_caret() {
    let mut doc = MemoryTextSession::new(CODE);
    let snapshot = doc.snapshot().unwrap();
    let mut view = EditorView::default();
    view.set_syntax(Some(observation(&doc)));
    let ctx = egui::Context::default();
    let id = egui::Id::new("reveal-fold");
    focus(&ctx, &mut view, &mut doc, id);
    view.toggle_fold(&snapshot, 0).unwrap();
    let before = view.selection();
    view.scroll_to_line(&snapshot, 2, Reveal::Center, false)
        .unwrap();
    frame(&ctx, &mut view, &mut doc, id, vec![]);
    assert!(view.collapsed_regions().is_empty());
    assert_eq!(view.selection(), before);
    view.toggle_fold(&snapshot, 0).unwrap();
    let at = CODE.find("cat").unwrap();
    view.request_navigation(
        &snapshot,
        Navigation {
            revision: 0,
            range: at..at + 3,
            selection: Some(TextSelection::new(&snapshot.text, at, at + 3).unwrap()),
            reveal: Reveal::Minimal,
            focus: true,
        },
    )
    .unwrap();
    frame(&ctx, &mut view, &mut doc, id, vec![]);
    assert!(view.collapsed_regions().is_empty());
    assert_eq!(view.selection().range(), at..at + 3);
}
#[test]
fn edits_before_a_fold_map_it_and_edits_inside_expand_it_without_history_entries() {
    let mut doc = MemoryTextSession::new(CODE);
    let mut view = EditorView::default();
    view.set_syntax(Some(observation(&doc)));
    let ctx = egui::Context::default();
    let id = egui::Id::new("fold-edits");
    frame(&ctx, &mut view, &mut doc, id, vec![]);
    view.toggle_fold(&doc.snapshot().unwrap(), 4).unwrap();
    doc.edit(
        0,
        TextEdit {
            range: 0..0,
            replacement: "// hi\n".into(),
        },
    )
    .unwrap();
    frame(&ctx, &mut view, &mut doc, id, vec![]);
    assert_eq!(view.collapsed_regions().len(), 1);
    let start = view.collapsed_regions()[0].start;
    assert_eq!(doc.text().line_of(start), 5);
    // Repainting the same revision must not apply its journal a second time.
    frame(&ctx, &mut view, &mut doc, id, vec![]);
    assert_eq!(view.collapsed_regions()[0].start, start);
    doc.end_undo_group();
    let inside = doc.text().line_start(6);
    doc.edit(
        1,
        TextEdit {
            range: inside..inside,
            replacement: "    // edit\n".into(),
        },
    )
    .unwrap();
    frame(&ctx, &mut view, &mut doc, id, vec![]);
    assert!(view.collapsed_regions().is_empty());
    assert_eq!(doc.snapshot().unwrap().revision, 2);
    doc.history(HistoryAction::Undo).unwrap();
    assert_eq!(doc.snapshot().unwrap().revision, 3);
}
#[test]
fn wrapping_folding_theme_and_composition_preserve_geometry_and_canonical_text() {
    let mut doc = MemoryTextSession::new(CODE.replace(
        "fn one() {",
        &format!("fn one() {{ // {}", "comment ".repeat(40)),
    ));
    let mut view = EditorView::default();
    view.settings.wrap = true;
    view.set_syntax(Some(observation(&doc)));
    let ctx = egui::Context::default();
    let id = egui::Id::new("fold-wrap");
    focus(&ctx, &mut view, &mut doc, id);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !view.wrapping_ready() {
        frame(&ctx, &mut view, &mut doc, id, vec![]);
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    view.toggle_fold(&doc.snapshot().unwrap(), 0).unwrap();
    // Let the fold finish closing: only the theme changes between these frames.
    for _ in 0..60 {
        frame(&ctx, &mut view, &mut doc, id, vec![]);
    }
    let first = frame(&ctx, &mut view, &mut doc, id, vec![]);
    let syntax = view.syntax().unwrap().clone();
    view.syntax_palette.keyword = egui::Color32::MAGENTA;
    let second = frame(&ctx, &mut view, &mut doc, id, vec![]);
    assert_eq!(first.rendered_lines, second.rendered_lines);
    assert_eq!(first.caret_rect, second.caret_rect);
    assert!(Arc::ptr_eq(view.syntax().unwrap(), &syntax));
    view.unfold_all();
    frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![egui::Event::Ime(egui::ImeEvent::Preedit {
            text: "猫".into(),
            active_range_chars: None,
        })],
    );
    assert!(view.is_composing());
    let snapshot = doc.snapshot().unwrap();
    assert!(view.toggle_fold(&snapshot, 0).is_err());
    assert!(
        view.scroll_to_line(&snapshot, 1, Reveal::Center, true)
            .is_err()
    );
    assert!(!doc.text().range_text(0..doc.text().len()).contains('猫'));
}
#[test]
fn sparse_mapping_handles_nested_and_adjacent_collapsed_regions() {
    let text = TextBuffer::from("0\n1\n2\n3\n4\n5\n6\n7\n8");
    let mut folds = folding::Folding::default();
    folds.collapsed = vec![
        0..text.line_start(4),
        text.line_start(1)..text.line_start(3),
        text.line_start(4)..text.line_start(7),
    ];
    folds.prepare(&text, None, true);
    assert_eq!(folds.map.rows(9), 4);
    for (visible, source) in [0, 4, 7, 8].into_iter().enumerate() {
        assert_eq!(folds.map.source_row(visible), source);
        assert_eq!(folds.map.visible_row(source), visible);
    }
}

#[test]
fn analysis_limits_keep_long_documents_editable_without_whole_line_layout() {
    let mut doc = MemoryTextSession::new("x".repeat(crate::syntax::MAX_SOURCE_BYTES + 1));
    let mut analysis = AnalysisSession::new();
    let start = Instant::now();
    syntax::update_analysis(&mut analysis, &doc, "rust").unwrap();
    assert!(matches!(analysis.status(), AnalysisStatus::Limited(_)));
    assert!(analysis.observation().is_none());
    let mut view = EditorView::default();
    view.set_syntax(analysis.observation().cloned());
    let ctx = egui::Context::default();
    let id = egui::Id::new("limited-long-line");
    focus(&ctx, &mut view, &mut doc, id);
    let output = frame(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![egui::Event::Text("!".into())],
    );
    assert_eq!(doc.text().byte(0), Some(b'!'));
    assert!(output.laid_out_bytes < 32 * 1024);
    assert!(output.rendered_lines < 32);
    eprintln!(
        "Limited analysis + editable long line: {:?}",
        start.elapsed()
    );
}

#[test]
fn analysis_journals_cover_grouped_edits_undo_redo_and_expiry() {
    let mut doc = MemoryTextSession::new(CODE);
    let mut analysis = AnalysisSession::new();
    fn settle(analysis: &mut AnalysisSession, doc: &MemoryTextSession) {
        syntax::update_analysis(analysis, doc, "rust").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while analysis.status() == &AnalysisStatus::Pending {
            analysis.poll();
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(analysis.status(), &AnalysisStatus::Ready);
    }
    settle(&mut analysis, &doc);
    for text in ["// first\n", "// second\n"] {
        doc.edit(
            doc.snapshot().unwrap().revision,
            TextEdit {
                range: 0..0,
                replacement: text.into(),
            },
        )
        .unwrap();
    }
    settle(&mut analysis, &doc);
    assert!(analysis.observation().unwrap().incremental);
    doc.history(HistoryAction::Undo).unwrap();
    settle(&mut analysis, &doc);
    assert!(analysis.observation().unwrap().incremental);
    doc.history(HistoryAction::Redo).unwrap();
    settle(&mut analysis, &doc);
    assert!(analysis.observation().unwrap().incremental);
    for _ in 0..130 {
        doc.edit(
            doc.snapshot().unwrap().revision,
            TextEdit {
                range: 0..0,
                replacement: " ".into(),
            },
        )
        .unwrap();
    }
    settle(&mut analysis, &doc);
    assert!(!analysis.observation().unwrap().incremental);
    let fresh = observation(&doc);
    assert_eq!(analysis.observation().unwrap().outline, fresh.outline);
    assert_eq!(analysis.observation().unwrap().highlights, fresh.highlights);
}

#[test]
fn find_navigation_expands_a_fold_containing_the_match() {
    let mut doc = MemoryTextSession::new(CODE);
    let mut view = EditorView::default();
    view.set_syntax(Some(observation(&doc)));
    let ctx = egui::Context::default();
    let id = egui::Id::new("find-fold");
    focus(&ctx, &mut view, &mut doc, id);
    view.toggle_fold(&doc.snapshot().unwrap(), 0).unwrap();
    view.open_search(false);
    view.search.query.text = "println".into();
    view.search.navigation = Some(true);
    let deadline = Instant::now() + Duration::from_secs(5);
    while view.selection().range().is_empty() {
        frame(&ctx, &mut view, &mut doc, id, vec![]);
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(doc.text().range_text(view.selection().range()), "println");
    assert!(view.collapsed_regions().is_empty());
    assert_eq!(doc.snapshot().unwrap().revision, 0);
}

#[test]
fn fold_animation_preserves_gutter_width_and_moves_following_rows_continuously() {
    let mut doc = MemoryTextSession::new(CODE);
    let snapshot = doc.snapshot().unwrap();
    let ctx = egui::Context::default();
    let id = egui::Id::new("animated-fold");
    let mut view = EditorView::default();
    // Reserve the gutter before a worker supplies fold ranges.
    let plain = frame(&ctx, &mut view, &mut doc, id, vec![]);
    view.set_syntax(Some(observation(&doc)));
    view.selection = TextSelection::caret(&snapshot.text, snapshot.text.line_start(4)).unwrap();
    let full = frame(&ctx, &mut view, &mut doc, id, vec![]);
    assert_eq!(plain.text_rect.left(), full.text_rect.left());
    view.toggle_fold(&snapshot, 0).unwrap();
    let start = frame(&ctx, &mut view, &mut doc, id, vec![]);
    assert_eq!(start.caret_rect.top(), full.caret_rect.top());
    for _ in 0..4 {
        frame(&ctx, &mut view, &mut doc, id, vec![]);
    }
    let middle = frame(&ctx, &mut view, &mut doc, id, vec![]);
    for _ in 0..12 {
        frame(&ctx, &mut view, &mut doc, id, vec![]);
    }
    let closed = frame(&ctx, &mut view, &mut doc, id, vec![]);
    assert!(closed.caret_rect.top() < middle.caret_rect.top());
    assert!(middle.caret_rect.top() < full.caret_rect.top());
    assert_eq!(closed.text_rect.left(), full.text_rect.left());
    assert_eq!(doc.snapshot().unwrap().revision, 0);
    view.toggle_fold(&snapshot, 0).unwrap();
    frame(&ctx, &mut view, &mut doc, id, vec![]);
    for _ in 0..12 {
        frame(&ctx, &mut view, &mut doc, id, vec![]);
    }
    let opened = frame(&ctx, &mut view, &mut doc, id, vec![]);
    assert_eq!(opened.caret_rect.top(), full.caret_rect.top());
}

#[test]
fn reversing_a_fold_keeps_the_displayed_height_and_nested_folds() {
    let text = TextBuffer::from("outer\ninner\na\nb\nc\nd\nend\nafter\n");
    let outer = 0..text.line_start(7);
    let inner = text.line_start(1)..text.line_start(5);
    let mut folds = folding::Folding::default();
    let ctx = egui::Context::default();
    folds.collapsed = vec![inner.clone()];
    folds.animate(&text, None, true, 0.0, false, &ctx);
    let nested_height = folds.display_map.height(text.line_count());
    folds.collapsed = vec![outer.clone(), inner.clone()];
    folds.animate(&text, None, true, 1.0, true, &ctx);
    folds.animate(&text, None, true, 1.08, true, &ctx);
    let middle = folds.display_map.height(text.line_count());
    assert!(middle < nested_height);
    folds.collapsed = vec![inner];
    folds.animate(&text, None, true, 1.08, true, &ctx);
    assert_eq!(folds.display_map.height(text.line_count()), middle);
    folds.animate(&text, None, true, 2.0, true, &ctx);
    assert_eq!(folds.display_map.height(text.line_count()), nested_height);
    folds.collapsed = vec![outer];
    folds.animate(&text, None, true, 3.0, false, &ctx);
    assert_eq!(folds.display_map.height(text.line_count()), 3.0);
}

// Inspect the actual text galleys so this catches a missing render contribution,
// including the frame that commits an edit before the worker is resubmitted.
fn painted_syntax(
    ctx: &egui::Context,
    view: &mut EditorView,
    doc: &mut MemoryTextSession,
    id: egui::Id,
    events: Vec<egui::Event>,
) -> Vec<(String, egui::Color32)> {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 300.0),
            )),
            focused: true,
            events,
            ..Default::default()
        },
        |ui| {
            view.show(
                ui,
                id,
                doc,
                MotionPolicy {
                    enabled: false,
                    ..Default::default()
                },
            )
            .unwrap();
        },
    );
    output.textures_delta.clear();
    fn collect(shape: &egui::epaint::Shape, result: &mut Vec<(String, egui::Color32)>) {
        match shape {
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, result);
                }
            }
            egui::epaint::Shape::Text(text) => {
                for section in &text.galley.job.sections {
                    result.push((
                        text.galley.job.text[section.byte_range.start.0..section.byte_range.end.0]
                            .into(),
                        section.format.color,
                    ));
                }
            }
            _ => {}
        }
    }
    let mut result = Vec::new();
    for shape in &output.shapes {
        collect(&shape.shape, &mut result);
    }
    result
}

#[test]
fn syntax_colors_survive_edit_frames_pending_analysis_and_shared_history() {
    let mut doc = MemoryTextSession::new(CODE);
    let mut analysis = AnalysisSession::new();
    syntax::update_analysis(&mut analysis, &doc, "rust").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while analysis.status() == &AnalysisStatus::Pending {
        analysis.poll();
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    let mut view = EditorView::default();
    view.set_analysis(&analysis);
    let keyword = view.syntax_palette.keyword;
    let ctx = egui::Context::default();
    let id = egui::Id::new("continuous-syntax");
    ctx.memory_mut(|memory| memory.request_focus(id));
    let has_keyword = |sections: Vec<(String, egui::Color32)>| {
        sections
            .iter()
            .any(|(text, color)| text == "fn" && *color == keyword)
    };
    assert!(has_keyword(painted_syntax(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![]
    )));
    assert!(has_keyword(painted_syntax(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![egui::Event::Text(" ".into())]
    )));
    let snapshot = doc.snapshot().unwrap();
    analysis.request(Arc::new(snapshot.text), snapshot.revision, "rust", Some(0));
    view.set_analysis(&analysis);
    assert!(view.syntax().is_none());
    assert!(view.fold_all(&doc.snapshot().unwrap()).is_err());
    for _ in 0..3 {
        assert!(has_keyword(painted_syntax(
            &ctx,
            &mut view,
            &mut doc,
            id,
            vec![]
        )));
    }
    // A sibling edit moves every old span past a multi-byte insertion.
    doc.edit(
        snapshot.revision,
        TextEdit {
            range: 0..0,
            replacement: "//猫\n".into(),
        },
    )
    .unwrap();
    assert!(has_keyword(painted_syntax(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![]
    )));
    doc.history(HistoryAction::Undo).unwrap();
    assert!(has_keyword(painted_syntax(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![]
    )));
    doc.history(HistoryAction::Redo).unwrap();
    assert!(has_keyword(painted_syntax(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![]
    )));
    syntax::update_analysis(&mut analysis, &doc, "rust").unwrap();
    while analysis.status() == &AnalysisStatus::Pending {
        analysis.poll();
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    view.set_analysis(&analysis);
    assert_eq!(
        view.syntax().unwrap().key.revision,
        doc.snapshot().unwrap().revision
    );
    assert!(has_keyword(painted_syntax(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![]
    )));
    let snapshot = doc.snapshot().unwrap();
    analysis.request(Arc::new(snapshot.text), snapshot.revision, "python", None);
    view.set_analysis(&analysis);
    assert!(!has_keyword(painted_syntax(
        &ctx,
        &mut view,
        &mut doc,
        id,
        vec![]
    )));
}

#[test]
fn retained_colors_clear_on_journal_gaps_and_explicit_detachment() {
    let mut doc = MemoryTextSession::new(CODE);
    let mut view = EditorView::default();
    view.set_syntax(Some(observation(&doc)));
    for _ in 0..130 {
        doc.edit(
            doc.snapshot().unwrap().revision,
            TextEdit {
                range: 0..0,
                replacement: " ".into(),
            },
        )
        .unwrap();
    }
    let ctx = egui::Context::default();
    let id = egui::Id::new("syntax-journal-gap");
    let keyword = view.syntax_palette.keyword;
    let colors = painted_syntax(&ctx, &mut view, &mut doc, id, vec![]);
    assert!(!colors.iter().any(|(_, color)| *color == keyword));
    view.set_syntax(Some(observation(&doc)));
    assert!(
        painted_syntax(&ctx, &mut view, &mut doc, id, vec![])
            .iter()
            .any(|(_, color)| *color == keyword)
    );
    view.set_syntax(None);
    assert!(
        !painted_syntax(&ctx, &mut view, &mut doc, id, vec![])
            .iter()
            .any(|(_, color)| *color == keyword)
    );
}

#[test]
fn provisional_colors_cover_insertions_inside_tokens_and_survive_deleted_lines() {
    let source = "fn one() { let message = \"abc\"; }\nfn other() {}\n";
    let mut doc = MemoryTextSession::new(source);
    let mut view = EditorView::default();
    view.set_syntax(Some(observation(&doc)));
    let ctx = egui::Context::default();
    let id = egui::Id::new("mapped-token-colors");
    let at = source.find("abc").unwrap() + 1;
    doc.edit(
        0,
        TextEdit {
            range: at..at,
            replacement: "猫".into(),
        },
    )
    .unwrap();
    let colors = painted_syntax(&ctx, &mut view, &mut doc, id, vec![]);
    assert!(
        colors
            .iter()
            .any(|(text, color)| text.contains("a猫bc") && *color == view.syntax_palette.string)
    );
    let end = doc.text().line_start(1);
    doc.edit(
        1,
        TextEdit {
            range: 0..end,
            replacement: String::new(),
        },
    )
    .unwrap();
    let colors = painted_syntax(&ctx, &mut view, &mut doc, id, vec![]);
    assert!(
        colors
            .iter()
            .any(|(text, color)| text == "fn" && *color == view.syntax_palette.keyword)
    );
    assert!(
        colors
            .iter()
            .any(|(text, color)| text == "other" && *color == view.syntax_palette.function)
    );
}
