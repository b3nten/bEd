//! Short, interruptible transitions for selection and changing documentation.
use dear_imgui_rs::{Ui, sys};

use crate::appearance::UiMotion;

#[derive(Default)]
pub struct Motion {
    transition: Option<(f32, f32, f64)>,
}

impl Motion {
    pub fn sample(&mut self, target: f32, now: f64, enabled: bool, motion: &UiMotion) -> f32 {
        let Some((from, to, began)) = self.transition else {
            self.transition = Some((target, target, now));
            return target;
        };
        let t = ((now - began) / motion.transition_seconds).clamp(0.0, 1.0) as f32;
        let value = from + (to - from) * (1.0 - (1.0 - t).powi(3));
        if !enabled || !motion.enabled {
            self.transition = Some((target, target, now));
            target
        } else if target != to {
            self.transition = Some((value, target, now));
            value
        } else {
            value
        }
    }
}

#[derive(Default)]
pub struct ContentTransition<T> {
    current: Option<T>,
    outgoing: Option<T>,
    began: f64,
}

impl<T: Clone + PartialEq> ContentTransition<T> {
    /// Crossfade both documents at the same origin. Layout and scrolling belong
    /// to the new content; fading the actual vertices also covers syntax painters.
    pub fn draw(&mut self, ui: &Ui, content: &T, enabled: bool, draw: impl Fn(&Ui, &T)) {
        let context = crate::UiFrameContext::for_ui(ui);
        let motion = &context.theme().motion;
        let now = ui.time();
        if self.current.as_ref() != Some(content) {
            let previous = self.current.replace(content.clone());
            // Rapid keyboard repeats and late resolve replies update the incoming
            // document without repeatedly restarting its opacity at zero.
            if self.outgoing.is_none() || now - self.began >= motion.transition_seconds {
                self.outgoing = previous;
                self.began = now;
            }
        }
        let t = if enabled && motion.enabled {
            ((now - self.began) / motion.transition_seconds).clamp(0.0, 1.0) as f32
        } else {
            1.0
        };
        if t >= 1.0 {
            self.outgoing = None;
        }
        if let Some(outgoing) = &self.outgoing {
            let origin = ui.cursor_pos();
            let maximum =
                ui.with_bound_context(|| unsafe { (*sys::igGetCurrentWindow()).DC.CursorMaxPos });
            faded(ui, 1.0 - t, || ui.group(|| draw(ui, outgoing)));
            ui.set_cursor_pos(origin);
            ui.with_bound_context(|| unsafe {
                (*sys::igGetCurrentWindow()).DC.CursorMaxPos = maximum;
            });
            faded(ui, t, || ui.group(|| draw(ui, content)));
        } else {
            draw(ui, content);
        }
    }
}

fn faded(ui: &Ui, alpha: f32, draw: impl FnOnce()) {
    let first = ui.with_bound_context(|| unsafe { (*sys::igGetWindowDrawList()).VtxBuffer.Size });
    draw();
    ui.with_bound_context(|| unsafe {
        let vertices = &mut (*sys::igGetWindowDrawList()).VtxBuffer;
        for index in first..vertices.Size {
            let vertex = &mut *vertices.Data.add(index as usize);
            let opacity = ((vertex.col >> 24) as f32 * alpha).round() as u32;
            vertex.col = (vertex.col & 0x00ff_ffff) | (opacity << 24);
        }
    });
}

/// Animated disclosure body. Its clipped height moves following sections and
/// keeps the native scroll extent in step with the visible content.
#[derive(Default)]
pub struct Disclosure {
    motion: Motion,
}

impl Disclosure {
    pub fn draw(&mut self, ui: &Ui, open: bool, enabled: bool, draw: impl FnOnce()) {
        let amount = self.motion.sample(
            if open { 1.0 } else { 0.0 },
            ui.time(),
            enabled,
            &crate::UiFrameContext::for_ui(ui).theme().motion,
        );
        if amount <= 0.0 {
            return;
        }
        let origin = ui.cursor_pos();
        let screen = ui.cursor_screen_pos();
        let (first_index, maximum) = ui.with_bound_context(|| unsafe {
            let window = &*sys::igGetCurrentWindow();
            ((*window.DrawList).IdxBuffer.Size, window.DC.CursorMaxPos)
        });
        // A separate draw command lets the final measured height constrain its
        // clip rectangle, without changing the panel's remaining commands.
        let clip = ui.push_clip_rect(
            [screen[0], screen[1]],
            [screen[0] + ui.content_region_avail()[0], f32::MAX],
            true,
        );
        faded(ui, amount, || ui.group(draw));
        let height = (ui.cursor_pos()[1] - origin[1]).max(0.0) * amount;
        drop(clip);
        ui.with_bound_context(|| unsafe {
            let window = &mut *sys::igGetCurrentWindow();
            let list = &mut *window.DrawList;
            for index in 0..list.CmdBuffer.Size {
                let command = &mut *list.CmdBuffer.Data.add(index as usize);
                if command.IdxOffset >= first_index as u32 && command.ElemCount > 0 {
                    command.ClipRect.w = command.ClipRect.w.min(screen[1] + height);
                }
            }
            window.DC.CursorMaxPos = maximum;
        });
        ui.set_cursor_pos(origin);
        ui.dummy([0.0, height]);
        ui.set_cursor_pos([origin[0], origin[1] + height]);
        ui.with_bound_context(|| unsafe {
            (*sys::igGetCurrentWindow()).DC.CursorMaxPos.y = maximum.y.max(screen[1] + height);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions};

    fn context() -> Context {
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context
    }

    fn frame(context: &mut Context, delta: f32, draw: impl FnOnce(&Ui)) {
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], delta));
        let ui = context.frame();
        ui.window("transition fixture")
            .position([0.0; 2], Condition::Always)
            .size([400.0, 300.0], Condition::Always)
            .build(|| draw(ui));
        drop(context.render_legacy());
    }

    #[test]
    fn selection_retargets_from_its_visible_position_and_disabled_motion_snaps() {
        let mut motion = Motion::default();
        assert_eq!(motion.sample(0.0, 0.0, true, &UiMotion::default()), 0.0);
        assert_eq!(motion.sample(1.0, 1.0, true, &UiMotion::default()), 0.0);
        let halfway = motion.sample(1.0, 1.06, true, &UiMotion::default());
        assert!(halfway > 0.0 && halfway < 1.0);
        assert_eq!(
            motion.sample(2.0, 1.06, true, &UiMotion::default()),
            halfway
        );
        assert!(motion.sample(2.0, 1.12, true, &UiMotion::default()) > halfway);
        assert_eq!(motion.sample(3.0, 1.12, false, &UiMotion::default()), 3.0);
    }

    #[test]
    fn content_crossfade_preserves_visible_ink_and_uses_incoming_layout() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut transition = ContentTransition::default();
        let draw = |ui: &Ui, &(color, height): &(u32, u32)| {
            let pos = ui.cursor_screen_pos();
            ui.get_window_draw_list()
                .add_rect(pos, [pos[0] + 20.0, pos[1] + 20.0], color)
                .filled(true)
                .build();
            ui.dummy([20.0, height as f32]);
        };
        let old = (0xff00aa11, 600);
        let next = (0xffaa0011, 40);
        frame(&mut context, 0.01, |ui| {
            transition.draw(ui, &old, true, draw)
        });
        let colors = |ui: &Ui| {
            ui.with_bound_context(|| unsafe {
                let vertices = &(*sys::igGetWindowDrawList()).VtxBuffer;
                (0..vertices.Size)
                    .map(|index| (*vertices.Data.add(index as usize)).col)
                    .collect::<Vec<_>>()
            })
        };
        frame(&mut context, 0.01, |ui| {
            transition.draw(ui, &next, true, draw);
            assert!(
                colors(ui).contains(&old.0),
                "outgoing content stays visible on selection"
            );
            ui.with_bound_context(|| unsafe {
                let window = &*sys::igGetCurrentWindow();
                assert!(
                    window.DC.CursorMaxPos.y < window.Pos.y + 100.0,
                    "old content must not retain its scroll extent"
                );
            });
        });
        frame(&mut context, 0.06, |ui| {
            transition.draw(ui, &next, true, draw);
            let colors = colors(ui);
            for color in [old.0, next.0] {
                assert!(
                    colors
                        .iter()
                        .any(|actual| actual & 0xffffff == color & 0xffffff
                            && actual >> 24 >= 120
                            && actual >> 24 <= 135)
                );
            }
        });
        let newest = (0xff11aa00, 40);
        frame(&mut context, 0.02, |ui| {
            transition.draw(ui, &newest, true, draw);
            assert!(
                colors(ui)
                    .iter()
                    .any(|actual| actual & 0xffffff == newest.0 & 0xffffff && actual >> 24 > 150),
                "rapid changes don't restart the incoming fade"
            );
        });
        frame(&mut context, 0.01, |ui| {
            transition.draw(ui, &next, false, draw);
            let colors = colors(ui);
            assert!(colors.contains(&next.0));
            assert!(
                !colors
                    .iter()
                    .any(|actual| actual & 0xffffff == old.0 & 0xffffff)
            );
        });
    }

    #[test]
    fn disclosure_moves_following_content_continuously_and_reverses() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut disclosure = Disclosure::default();
        let end = std::cell::Cell::new(0.0);
        let mut draw = |ui: &Ui, open, enabled| {
            disclosure.draw(ui, open, enabled, || ui.dummy([40.0, 100.0]));
            end.set(ui.cursor_pos()[1]);
            ui.text("Following section");
        };
        frame(&mut context, 0.01, |ui| draw(ui, true, true));
        let full = end.get();
        frame(&mut context, 0.01, |ui| draw(ui, false, true));
        assert!((end.get() - full).abs() < 1.0);
        frame(&mut context, 0.06, |ui| draw(ui, false, true));
        assert!(end.get() > 0.0 && end.get() < full * 0.5);
        let middle = end.get();
        frame(&mut context, 0.001, |ui| draw(ui, true, true));
        assert!(
            (end.get() - middle).abs() < 3.0,
            "reversal begins at the visible height"
        );
        frame(&mut context, 0.01, |ui| draw(ui, true, false));
        assert!((end.get() - full).abs() < 1.0);
    }
}
