//! bEd's caret motion, expressed in content coordinates so scrolling and pane
//! movement do not animate. Logical selections are always immediate.
//!
//! The editor paints with `CaretAnimation` itself; `TextField` and `TextArea`
//! get the same caret over egui's `TextEdit` through `text_edit`.
use crate::MotionPolicy;
use egui::{Color32, Pos2, Rect, Vec2};

#[derive(Clone, Copy)]
struct Motion {
    head: usize,
    target: Pos2,
    leading: Pos2,
    trailing: Pos2,
    started: f64,
}

impl Motion {
    fn sample(self, now: f64) -> (Pos2, Pos2, f32) {
        let elapsed = (now - self.started).max(0.0);
        let at = |from: Pos2, seconds: f64| {
            from + (self.target - from) * crate::ease((elapsed / seconds) as f32)
        };
        (
            at(self.leading, 0.035),
            at(self.trailing, 0.11),
            (1.0 - elapsed / 0.11).max(0.0) as f32,
        )
    }
}

/// One caret's glide, trail and blink, kept between frames by what draws it.
#[derive(Clone, Default)]
pub struct CaretAnimation {
    motion: Option<Motion>,
    last_frame: Option<u64>,
    height: f32,
    interacted: f64,
}

impl CaretAnimation {
    /// Whether the caret shows now: steady just after moving, then blinking.
    pub fn blink_on(&self, now: f64) -> bool {
        let elapsed = now - self.interacted;
        elapsed < 0.7 || (elapsed % 1.0) < 0.5
    }
    /// Paint the caret at `rect` (content coordinates; `origin` is where the
    /// content is on screen), gliding there when `head` has changed since the
    /// frame before.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        ui: &egui::Ui,
        clip: Rect,
        origin: Pos2,
        focused: bool,
        head: usize,
        rect: Rect,
        animate: bool,
        ink: Color32,
        enabled: bool,
    ) {
        if !enabled || !focused || !ui.input(|i| i.focused) {
            self.motion = None;
            self.last_frame = None;
            return;
        }
        let now = ui.input(|i| i.time);
        let frame = ui.ctx().cumulative_frame_nr();
        let target = rect.center();
        if self.motion.is_none_or(|motion| motion.head != head) {
            self.interacted = now;
        }
        let stationary = Motion {
            head,
            target,
            leading: target,
            trailing: target,
            started: now,
        };
        if !animate
            || self.last_frame.is_none_or(|last| last + 1 != frame)
            || self.height != rect.height()
        {
            self.motion = Some(stationary);
        }
        self.last_frame = Some(frame);
        self.height = rect.height();
        let motion = self.motion.get_or_insert(stationary);
        if motion.head != head {
            self.interacted = now;
            let (leading, trailing, _) = motion.sample(now);
            let bounded = |from: Pos2| {
                let delta = from - target;
                target + delta * (rect.height() * 8.0 / delta.length().max(1.0)).min(1.0)
            };
            *motion = Motion {
                leading: bounded(leading),
                trailing: bounded(trailing),
                ..stationary
            };
        } else if motion.target != target {
            *motion = stationary;
        }
        let (leading, trailing, fade) = motion.sample(now);
        let painter = ui.painter().with_clip_rect(clip.intersect(ui.clip_rect()));
        let origin = origin.to_vec2();
        let delta = leading - trailing;
        if animate && fade > 0.0 && delta.length() > 0.5 {
            let normal = Vec2::new(-delta.y, delta.x) / delta.length();
            let width = normal.x.abs() + normal.y.abs() * rect.height() * 0.5;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    trailing + origin - normal * width * 0.25,
                    leading + origin - normal * width,
                    leading + origin + normal * width,
                    trailing + origin + normal * width * 0.25,
                ],
                ink.gamma_multiply(0.3 * fade),
                egui::Stroke::NONE,
            ));
        }
        let active =
            animate && fade > 0.0 && (motion.leading != target || motion.trailing != target);
        if active || self.blink_on(now) {
            painter.rect_filled(
                Rect::from_center_size(leading + origin, egui::vec2(2.0, rect.height())),
                0.0,
                ink,
            );
        }
        if active {
            ui.ctx().request_repaint();
        } else {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}

/// What a text widget keeps for its caret between frames.
#[derive(Clone, Default)]
struct Kept {
    animation: CaretAnimation,
    /// An input method is composing: its caret and underlines are egui's.
    composing: bool,
}

/// Show an egui `TextEdit` with bEd's caret in place of egui's: `show` adds
/// the edit (with this `id`) to the `Ui` it's given, in which egui's caret is
/// turned off.
pub(crate) fn text_edit(
    ui: &mut egui::Ui,
    id: egui::Id,
    font: &egui::FontId,
    motion: MotionPolicy,
    show: impl FnOnce(&mut egui::Ui) -> egui::text_edit::TextEditOutput,
) -> egui::text_edit::TextEditOutput {
    let key = id.with("caret");
    let mut kept: Kept = ui.data(|data| data.get_temp(key)).unwrap_or_default();
    let focused = ui.memory(|memory| memory.has_focus(id));
    if focused {
        ui.input(|input| {
            for event in &input.events {
                match event {
                    egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                        kept.composing = !text.is_empty();
                    }
                    egui::Event::Ime(egui::ImeEvent::Commit(_)) => kept.composing = false,
                    _ => {}
                }
            }
        });
    } else {
        kept.composing = false;
    }

    let egui_caret = ui.visuals().text_cursor.clone();
    if !kept.composing {
        let caret = &mut ui.visuals_mut().text_cursor;
        caret.stroke = egui::Stroke::NONE;
        caret.blink = false;
    }
    let output = show(ui);
    ui.visuals_mut().text_cursor = egui_caret;

    // Focus may have arrived or left in the edit itself (a click, Escape).
    let focused = ui.memory(|memory| memory.has_focus(id));
    let caret = output
        .cursor_range
        .filter(|_| !kept.composing)
        .map(|range| {
            let mut rect = output.galley.pos_from_cursor(range.primary);
            // An empty galley has no row to take a height from.
            let row = ui.fonts_mut(|fonts| fonts.row_height(font));
            rect.max.y = rect.max.y.max(rect.min.y + row);
            (range.primary.index.0, rect.expand(1.5))
        });
    let (head, rect) = caret.unwrap_or((0, Rect::NOTHING));
    kept.animation.paint(
        ui,
        // Room for a caret at either edge of the text.
        output.text_clip_rect.expand(1.0),
        output.galley_pos - egui::vec2(output.galley.rect.left(), 0.0),
        focused,
        head,
        rect,
        motion.enabled,
        crate::rainbow_ink(ui, motion),
        caret.is_some() && ui.is_enabled(),
    );
    ui.data_mut(|data| data.insert_temp(key, kept));
    output
}
