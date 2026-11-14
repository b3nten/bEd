//! bEd's caret motion, expressed in content coordinates so scrolling and pane
//! movement do not animate. Logical selections are always immediate.
use bedtime_ui::egui::{self, Color32, Pos2, Rect, Vec2};

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
            from + (self.target - from) * bedtime_ui::ease((elapsed / seconds) as f32)
        };
        (
            at(self.leading, 0.035),
            at(self.trailing, 0.11),
            (1.0 - elapsed / 0.11).max(0.0) as f32,
        )
    }
}

#[derive(Default)]
pub(crate) struct CaretAnimation {
    motion: Option<Motion>,
    last_frame: Option<u64>,
    height: f32,
    interacted: f64,
}

impl CaretAnimation {
    /// Whether the caret shows now: steady just after moving, then blinking.
    pub(crate) fn blink_on(&self, now: f64) -> bool {
        let elapsed = now - self.interacted;
        elapsed < 0.7 || (elapsed % 1.0) < 0.5
    }
    pub(crate) fn paint(
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
