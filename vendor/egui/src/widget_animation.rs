//! Bed's paint-only widget transitions. Layout and interaction use target geometry.
use crate::{Color32, Context, Id, IdMap, Response, Style, style::WidgetVisuals};

#[derive(Clone)]
struct Transition<T> {
    from: T,
    target: T,
    started: f64,
    last_seen: u64,
}

impl<T: Copy + PartialEq> Transition<T> {
    fn settled(target: T, frame: u64) -> Self {
        Self {
            from: target,
            target,
            started: f64::NEG_INFINITY,
            last_seen: frame,
        }
    }

    fn update(
        &mut self,
        target: T,
        now: f64,
        frame: u64,
        seconds: f32,
        lerp: fn(T, T, f32) -> T,
    ) -> (T, bool) {
        if seconds <= 0.0 {
            *self = Self::settled(target, frame);
            return (target, false);
        }
        let progress = ((now - self.started) as f32 / seconds).clamp(0.0, 1.0);
        let current = lerp(
            self.from,
            self.target,
            crate::emath::easing::cubic_out(progress),
        );
        if self.target != target {
            self.from = current;
            self.target = target;
            self.started = now;
        }
        self.last_seen = frame;
        (
            current,
            self.from != self.target && now - self.started < f64::from(seconds),
        )
    }
}

#[derive(Clone, Copy, PartialEq)]
struct WidgetColors([Color32; 4]);

impl WidgetColors {
    fn of(v: &WidgetVisuals) -> Self {
        Self([
            v.bg_fill,
            v.weak_bg_fill,
            v.bg_stroke.color,
            v.fg_stroke.color,
        ])
    }

    fn lerp(a: Self, b: Self, t: f32) -> Self {
        Self(std::array::from_fn(|i| a.0[i].lerp_to_gamma(b.0[i], t)))
    }

    fn apply(self, v: &mut WidgetVisuals) {
        [
            v.bg_fill,
            v.weak_bg_fill,
            v.bg_stroke.color,
            v.fg_stroke.color,
        ] = self.0;
    }
}

#[derive(Clone, Default)]
struct Animations {
    frame: u64,
    widgets: IdMap<Transition<WidgetColors>>,
    colors: IdMap<Transition<Color32>>,
}

impl Animations {
    fn begin_frame(&mut self, frame: u64) {
        if self.frame != frame {
            // Removed widgets must neither retain memory indefinitely nor replay
            // an old transition when their ID becomes visible again.
            self.widgets.retain(|_, v| v.last_seen + 1 >= frame);
            self.colors.retain(|_, v| v.last_seen + 1 >= frame);
            self.frame = frame;
        }
    }
}

fn clock(ctx: &Context) -> (Id, u64, f64) {
    (
        Id::new("egui.widget_animations").with(ctx.viewport_id()),
        ctx.cumulative_frame_nr(),
        ctx.input(|i| i.time),
    )
}

pub(crate) fn clear(ctx: &Context) {
    ctx.data_mut(|d| d.remove_by_type::<Animations>());
}

impl Style {
    /// Like [`Self::interact`], with smooth paint colors using [`Self::animation_time`].
    /// Geometry is unchanged, and a zero animation time settles immediately.
    pub fn interact_animated(&self, response: &Response) -> WidgetVisuals {
        self.animate_widget_visuals(&response.ctx, response.id, *self.interact(response))
    }

    /// Like [`Self::interact_selectable`], with smooth interaction and selection colors.
    pub fn interact_selectable_animated(
        &self,
        response: &Response,
        selected: bool,
    ) -> WidgetVisuals {
        self.animate_widget_visuals(
            &response.ctx,
            response.id,
            self.interact_selectable(response, selected),
        )
    }

    pub(crate) fn animate_widget_visuals(
        &self,
        ctx: &Context,
        id: Id,
        mut target: WidgetVisuals,
    ) -> WidgetVisuals {
        let (store, frame, now) = clock(ctx);
        let (colors, active) = ctx.data_mut(|d| {
            let animations = d.get_temp_mut_or_default::<Animations>(store);
            animations.begin_frame(frame);
            let colors = WidgetColors::of(&target);
            animations
                .widgets
                .entry(id)
                .or_insert_with(|| Transition::settled(colors, frame))
                .update(colors, now, frame, self.animation_time, WidgetColors::lerp)
        });
        colors.apply(&mut target);
        if active {
            ctx.request_repaint();
        }
        target
    }
}

impl Context {
    /// Fade a paint color with cubic ease-out. Retargeting starts at the displayed
    /// color. The initial value is settled; zero duration disables the transition.
    /// Hidden widgets discard their transitions after a frame without a call.
    pub fn animate_color_with_time(&self, id: Id, target: Color32, seconds: f32) -> Color32 {
        let (store, frame, now) = clock(self);
        let (color, active) = self.data_mut(|d| {
            let animations = d.get_temp_mut_or_default::<Animations>(store);
            animations.begin_frame(frame);
            animations
                .colors
                .entry(id)
                .or_insert_with(|| Transition::settled(target, frame))
                .update(target, now, frame, seconds, |a, b, t| a.lerp_to_gamma(b, t))
        });
        if active {
            self.request_repaint();
        }
        color
    }
}

#[cfg(test)]
mod tests;
