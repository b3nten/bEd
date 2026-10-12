//! Translated from ned editor/views/caret_view.{h,cpp}; see LICENSE and NOTICE.
use crate::views::view_layout::{ViewLayout, line_column_x, rainbow_color};
use bed_editing::{editor_state::EditorState, editor_view_state::EditorViewState};
use dear_imgui_rs::{StyleColor, Ui, sys};

pub struct CaretView;

const LEADING_DURATION: f64 = 0.035;
const TRAILING_DURATION: f64 = 0.11;

#[derive(Clone, Copy)]
struct CaretMotion {
    head: (i32, i32),
    target: [f32; 2],
    from_leading: [f32; 2],
    from_trailing: [f32; 2],
    started: f64,
}

impl CaretMotion {
    fn stationary(head: (i32, i32), target: [f32; 2], now: f64) -> Self {
        Self {
            head,
            target,
            from_leading: target,
            from_trailing: target,
            started: now,
        }
    }

    fn sample(self, now: f64) -> ([f32; 2], [f32; 2], f32) {
        let elapsed = (now - self.started).max(0.0);
        let position = |from: [f32; 2], duration: f64| {
            let t = (elapsed / duration).min(1.0) as f32;
            let eased = 1.0 - (1.0 - t).powi(3);
            std::array::from_fn(|axis| from[axis] + (self.target[axis] - from[axis]) * eased)
        };
        (
            position(self.from_leading, LEADING_DURATION),
            position(self.from_trailing, TRAILING_DURATION),
            (1.0 - elapsed / TRAILING_DURATION).max(0.0) as f32,
        )
    }

    fn retarget(&mut self, head: (i32, i32), target: [f32; 2], now: f64, line_height: f32) {
        // Reflow and folding can move an unchanged document position. Those
        // changes belong to the layout, rather than to caret movement.
        if self.head == head && self.target == target {
            return;
        }
        if self.head == head {
            *self = Self::stationary(head, target, now);
            return;
        }
        let (leading, trailing, _) = self.sample(now);
        // Long lines and wrap boundaries are still caret movement. Bound the
        // visible stretch instead of turning those moves into instant jumps.
        let bounded = |position: [f32; 2]| {
            let delta = [position[0] - target[0], position[1] - target[1]];
            let length = delta[0].hypot(delta[1]);
            let scale = (line_height * 8.0 / length.max(1.0)).min(1.0);
            std::array::from_fn(|axis| target[axis] + delta[axis] * scale)
        };
        *self = Self {
            head,
            target,
            from_leading: bounded(leading),
            from_trailing: bounded(trailing),
            started: now,
        };
    }
}

/// Each editor frame owns its visual history; document selections stay immediate.
#[derive(Default)]
pub(crate) struct CaretAnimation {
    carets: Vec<Option<CaretMotion>>,
    last_frame: Option<usize>,
    primary_index: usize,
    geometry: Option<(usize, u32, u32)>,
}

impl CaretAnimation {
    pub(crate) fn clear(&mut self) {
        self.carets.clear();
        self.last_frame = None;
    }
}

impl CaretView {
    pub fn draw(ui: &Ui, state: &EditorState, view: &EditorViewState, layout: &ViewLayout) {
        Self::draw_projected(ui, state, view, layout, None);
    }
    pub(crate) fn draw_projected(
        ui: &Ui,
        state: &EditorState,
        view: &EditorViewState,
        layout: &ViewLayout,
        projection: Option<&crate::diff::RowProjection>,
    ) {
        Self::draw_carets(ui, state, view, layout, projection, None);
    }

    pub(crate) fn draw_animated_projected(
        ui: &Ui,
        state: &EditorState,
        view: &EditorViewState,
        layout: &ViewLayout,
        projection: &crate::diff::RowProjection,
        animation: &mut CaretAnimation,
    ) {
        Self::draw_carets(ui, state, view, layout, Some(projection), Some(animation));
    }

    fn draw_carets(
        ui: &Ui,
        state: &EditorState,
        view: &EditorViewState,
        layout: &ViewLayout,
        projection: Option<&crate::diff::RowProjection>,
        mut animation: Option<&mut CaretAnimation>,
    ) {
        if view.block_input {
            if let Some(animation) = animation {
                animation.clear();
            }
            return;
        }
        if let Some(animation) = &mut animation {
            let geometry = (
                ui.with_bound_context(|| unsafe { sys::igGetFont() as usize }),
                ui.current_font_size().to_bits(),
                layout.size[0].to_bits(),
            );
            if animation.last_frame != Some(ui.frame_count() - 1)
                || animation.primary_index != view.primary_index
                || animation.carets.len() != view.selections.len()
                || animation.geometry != Some(geometry)
            {
                animation.carets = vec![None; view.selections.len()];
            }
            animation.last_frame = Some(ui.frame_count());
            animation.primary_index = view.primary_index;
            animation.geometry = Some(geometry);
        }
        let draw = ui.get_window_draw_list();
        let blink_alpha = ((view.cursor_blink_time * 4.0).sin() + 1.0) * 0.5;
        let ink = bed_ui::presentation::readable_color(
            ui,
            if layout.rainbow_mode {
                rainbow_color(ui.time() as f32)
            } else {
                ui.style_color(StyleColor::Text)
            },
        );
        let color = ui.with_bound_context(|| unsafe { sys::igColorConvertFloat4ToU32(ink.into()) });
        let (primary_color, secondary_color) = if layout.rainbow_mode {
            (color, color)
        } else {
            // Source IM_COL32 truncates blink alpha explicitly, while rainbow
            // uses ColorConvertFloat4ToU32's rounded channels.
            (
                (color & 0x00ff_ffff) | (((blink_alpha * 255.0) as u32) << 24),
                (color & 0x00ff_ffff) | 0xa000_0000,
            )
        };
        for (index, sel) in view.selections.iter().enumerate() {
            if projection.is_some_and(|p| !p.is_document_visible(sel.head_row)) {
                if let Some(animation) = &mut animation {
                    animation.carets[index] = None;
                }
                continue;
            }
            let content_x = projection.map_or_else(
                || line_column_x(ui, &state.line(sel.head_row), sel.head_column, 0.0),
                |p| p.position_x(ui, state, sel.head_row, sel.head_column, 0.0),
            );
            let x = (layout.text_pos[0] + content_x).floor();
            let visual = projection.map_or(sel.head_row as usize, |p| {
                p.visual_position(sel.head_row, sel.head_column)
            });
            let top = projection.map_or(visual as f32, |p| p.row_top(visual));
            let height = projection.map_or(1.0, |p| p.row_height(visual)) * layout.line_height;
            if height <= 0.0 {
                if let Some(animation) = &mut animation {
                    animation.carets[index] = None;
                }
                continue;
            }
            let y = (layout.text_pos[1] + top * layout.line_height).floor();
            let height = height.min(layout.line_height - 1.0);
            let mut color = if index == view.primary_index {
                primary_color
            } else {
                secondary_color
            };
            let mut position = [x, y];
            if let Some(animation) = &mut animation {
                let head = (sel.head_row, sel.head_column);
                // Content coordinates keep scrolling and pane movement from
                // becoming animation input, even while a smear is in flight.
                let target = [content_x, top * layout.line_height + height * 0.5];
                let motion = animation.carets[index]
                    .get_or_insert_with(|| CaretMotion::stationary(head, target, ui.time()));
                motion.retarget(head, target, ui.time(), layout.line_height);
                let (leading, trailing, fade) = motion.sample(ui.time());
                let delta = [leading[0] - trailing[0], leading[1] - trailing[1]];
                let length = delta[0].hypot(delta[1]);
                if fade > 0.0
                    && (motion.from_leading != motion.target
                        || motion.from_trailing != motion.target)
                {
                    // Keep the primary caret visible throughout movement,
                    // including when its idle blink happens to be transparent.
                    if index == view.primary_index {
                        color = (color & 0x00ff_ffff) | 0xff00_0000;
                    }
                    if length > 0.5 {
                        let normal = [-delta[1] / length, delta[0] / length];
                        let half_width = normal[0].abs() + normal[1].abs() * height * 0.5;
                        let edge = |center: [f32; 2], width: f32| {
                            std::array::from_fn(|axis| {
                                layout.text_pos[axis] + center[axis] + normal[axis] * width
                            })
                        };
                        let alpha = ((color >> 24) as f32 * 0.3 * fade) as u32;
                        draw.add_quad_filled(
                            edge(trailing, -half_width * 0.25),
                            edge(leading, -half_width),
                            edge(leading, half_width),
                            edge(trailing, half_width * 0.25),
                            (color & 0x00ff_ffff) | (alpha << 24),
                        );
                    }
                }
                position = [
                    (layout.text_pos[0] + leading[0]).floor(),
                    (layout.text_pos[1] + leading[1] - height * 0.5).floor(),
                ];
            }
            draw.add_rect(
                [position[0] - 1.0, position[1]],
                [position[0] + 1.0, position[1] + height],
                color,
            )
            .filled(true)
            .build();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_document_session::editor::Editor;
    use bed_editing::{editor_commands::CursorReveal, folding::FoldRange};
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions};

    #[derive(Clone, Debug, PartialEq)]
    struct Vertex {
        position: [f32; 2],
        color: u32,
    }

    fn paint(ui: &Ui, draw: impl FnOnce()) -> Vec<Vertex> {
        let start =
            ui.with_bound_context(|| unsafe { (*sys::igGetWindowDrawList()).VtxBuffer.Size });
        draw();
        ui.with_bound_context(|| unsafe {
            let vertices = &(*sys::igGetWindowDrawList()).VtxBuffer;
            (start..vertices.Size)
                .map(|index| {
                    let vertex = &*vertices.Data.add(index as usize);
                    Vertex {
                        position: [vertex.pos.x, vertex.pos.y],
                        color: vertex.col,
                    }
                })
                .collect()
        })
    }

    struct Fixture {
        context: Context,
        editor: Editor,
        animation: CaretAnimation,
        origin: [f32; 2],
        folds: Vec<FoldRange>,
        wrap_width: Option<f32>,
    }

    impl Fixture {
        fn new() -> Self {
            let mut context = Context::create();
            context
                .set_ini_filename(None::<std::path::PathBuf>)
                .unwrap();
            context
                .font_atlas()
                .try_claim_legacy_renderer()
                .unwrap()
                .build();
            let mut editor = Editor::new();
            editor
                .set_content("\talpha hé🙂 omega\nsecond line\nthird line\nfourth line".as_bytes());
            editor.view.cursor_blink_time = std::f32::consts::FRAC_PI_8;
            Self {
                context,
                editor,
                animation: CaretAnimation::default(),
                origin: [60.25, 60.5],
                folds: Vec::new(),
                wrap_width: None,
            }
        }

        fn move_to(&mut self, row: i32, column: i32) {
            self.editor
                .commands()
                .set_cursor(row, column, false, CursorReveal::Ensure);
        }

        fn render(&mut self, delta: f32) -> (Vec<Vertex>, Vec<Vertex>) {
            self.context
                .prepare_frame(FramePrepareOptions::new([640.0, 480.0], delta));
            let ui = self.context.frame();
            let before = self.editor.view.clone();
            let mut result = (Vec::new(), Vec::new());
            ui.window("Caret paint fixture")
                .position([0.0; 2], Condition::Always)
                .size([640.0, 480.0], Condition::Always)
                .build(|| {
                    let layout = ViewLayout {
                        text_pos: self.origin,
                        size: [600.0, 400.0],
                        line_height: ui.text_line_height(),
                        ..Default::default()
                    };
                    let mut projection =
                        crate::diff::RowProjection::build(&self.editor.state, None, None);
                    projection.fold(&self.folds);
                    if let Some(width) = self.wrap_width {
                        projection.wrap(ui, &self.editor.state, width);
                    }
                    result.0 = paint(ui, || {
                        CaretView::draw_animated_projected(
                            ui,
                            &self.editor.state,
                            &self.editor.view,
                            &layout,
                            &projection,
                            &mut self.animation,
                        )
                    });
                    result.1 = paint(ui, || {
                        CaretView::draw_projected(
                            ui,
                            &self.editor.state,
                            &self.editor.view,
                            &layout,
                            Some(&projection),
                        )
                    });
                });
            drop(self.context.render_legacy());
            assert_eq!(
                self.editor.view, before,
                "painting never moves a document selection"
            );
            result
        }
    }

    fn has_trail(vertices: &[Vertex]) -> bool {
        vertices
            .iter()
            .any(|vertex| (1..100).contains(&(vertex.color >> 24)))
    }

    #[test]
    fn movement_stretches_and_fades_in_both_axes_then_matches_the_real_caret() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut fixture = Fixture::new();
        let initial = fixture.render(1.0 / 60.0);
        assert_eq!(initial.0, initial.1);
        for (row, column) in [(0, 7), (1, 7), (2, 2), (0, 0)] {
            fixture.move_to(row, column);
            assert_eq!(
                (fixture.editor.view.row, fixture.editor.view.column),
                (row, column)
            );
            fixture.render(1.0 / 60.0);
            let moving = fixture.render(1.0 / 60.0);
            assert_ne!(moving.0, moving.1);
            assert!(has_trail(&moving.0), "moving caret has a faint smear");
            let settled = fixture.render(0.12);
            assert_eq!(
                settled.0, settled.1,
                "settled paint matches the current selection"
            );
        }
    }

    #[test]
    fn scrolling_reflow_focus_and_selection_changes_discard_only_stale_history() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut fixture = Fixture::new();
        fixture.render(1.0 / 60.0);
        fixture.move_to(1, 5);
        fixture.render(1.0 / 60.0);
        fixture.origin = [49.75, 40.125];
        assert!(
            has_trail(&fixture.render(1.0 / 60.0).0),
            "scrolling preserves an ongoing movement"
        );
        let settled = fixture.render(0.12);
        assert_eq!(settled.0, settled.1);
        fixture.origin = [43.25, 35.75];
        let scrolled = fixture.render(1.0 / 60.0);
        assert_eq!(
            scrolled.0, scrolled.1,
            "scrolling alone never starts a smear"
        );

        fixture.wrap_width = Some(35.0);
        let wrapped = fixture.render(1.0 / 60.0);
        assert_eq!(
            wrapped.0, wrapped.1,
            "reflow snaps an unchanged document position"
        );
        fixture.move_to(2, 5);
        fixture.render(1.0 / 60.0);
        let moving = fixture.render(1.0 / 60.0);
        assert!(has_trail(&moving.0));
        fixture.editor.view.block_input = true;
        assert!(fixture.render(1.0 / 60.0).0.is_empty());
        fixture.editor.view.block_input = false;
        let focused = fixture.render(1.0 / 60.0);
        assert_eq!(
            focused.0, focused.1,
            "focus restoration never replays old movement"
        );

        fixture.wrap_width = None;
        fixture.folds = vec![FoldRange {
            start_line: 0,
            end_line: 1,
        }];
        let folded = fixture.render(1.0 / 60.0);
        assert_eq!(folded.0, folded.1);
        fixture.editor.commands().add_cursor_at(3, 2);
        let added = fixture.render(1.0 / 60.0);
        assert_eq!(added.0, added.1, "new selections have no inherited trail");
        fixture.editor.view.primary_index = 0;
        let primary_changed = fixture.render(1.0 / 60.0);
        assert_eq!(primary_changed.0, primary_changed.1);
    }

    #[test]
    fn rapid_reversals_start_from_current_geometry_and_settle_at_every_frame_rate() {
        for delta in [1.0 / 30.0, 1.0 / 60.0, 1.0 / 144.0] {
            let mut motion = CaretMotion::stationary((0, 0), [0.0; 2], 0.0);
            motion.retarget((0, 4), [40.0, 0.0], 0.0, 16.0);
            let (leading, trailing, _) = motion.sample(delta);
            assert!(leading[0] > trailing[0], "leading edge moves faster");
            motion.retarget((1, 0), [0.0, 16.0], delta, 16.0);
            let reversed = motion.sample(delta);
            assert_eq!(
                (reversed.0, reversed.1),
                (leading, trailing),
                "retargeting is continuous"
            );
            let settled = motion.sample(delta + 0.12);
            assert_eq!(
                (settled.0, settled.1, settled.2),
                ([0.0, 16.0], [0.0, 16.0], 0.0)
            );
            motion.retarget((50, 0), [0.0, 800.0], delta + 0.13, 16.0);
            let jumped = motion.sample(delta + 0.13);
            assert!(
                jumped.0[1] < 800.0 && jumped.1[1] < 800.0,
                "large moves remain animated"
            );
            assert!(
                800.0 - jumped.1[1] <= 128.0,
                "large moves keep a bounded trail"
            );
            let moving = motion.sample(delta + 0.15);
            assert!(moving.0[1] > moving.1[1]);
            let settled = motion.sample(delta + 0.25);
            assert_eq!((settled.0, settled.1), ([0.0, 800.0], [0.0, 800.0]));
        }
    }

    #[test]
    fn long_columns_and_wrap_boundaries_remain_animated() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut fixture = Fixture::new();
        fixture.editor.set_content(&vec![b'x'; 120]);
        fixture.render(1.0 / 60.0);
        fixture.move_to(0, 80);
        fixture.render(1.0 / 60.0);
        assert!(
            has_trail(&fixture.render(1.0 / 60.0).0),
            "long column moves still smear"
        );
        fixture.render(0.12);
        fixture.wrap_width = Some(200.0);
        fixture.move_to(0, 27);
        fixture.render(0.12);
        fixture.render(0.12);
        let before_wrap = fixture.render(1.0 / 60.0).1;
        fixture.move_to(0, 28);
        fixture.render(1.0 / 60.0);
        let after_wrap = fixture.render(1.0 / 60.0);
        assert!(
            after_wrap.1[0].position[1] > before_wrap[0].position[1],
            "movement crosses a visual row"
        );
        assert!(
            has_trail(&after_wrap.0),
            "wrapping still smears despite a large horizontal change"
        );
        let settled = fixture.render(0.12);
        assert_eq!(settled.0, settled.1);
    }
}
