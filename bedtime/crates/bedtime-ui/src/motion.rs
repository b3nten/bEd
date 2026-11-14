use bevy::prelude::*;

/// How sharply `ease` approaches: 2^-7 (under 1%) is left just before the end.
const SHARPNESS: f32 = 7.0;

/// bEd's easing, as the RAD Debugger moves things: each moment covers the
/// same fraction of what's left (`1 - 2^(-k·t)`), so motion starts quickly
/// and settles gently. Scaled to land exactly on 1 at `t = 1`; `t` is the
/// fraction of the animation's time gone.
pub fn ease(t: f32) -> f32 {
    // Finished, or no time to take (0/0).
    if t.is_nan() || t >= 1.0 {
        return 1.0;
    }
    if t <= 0.0 {
        return 0.0;
    }
    (1.0 - (-SHARPNESS * t).exp2()) / (1.0 - (-SHARPNESS).exp2())
}

/// Surface policy, independent of its color theme. Disable to settle immediately.
#[derive(Component, Clone, Copy, Debug)]
pub struct MotionPolicy {
    pub enabled: bool,
    pub seconds: f32,
    /// Cycle the caret's and selection's ink through colours (`rainbow_ink`),
    /// repainting all the while; off, they're the theme's selection colour.
    pub rainbow: bool,
}

impl Default for MotionPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            seconds: 0.16,
            rainbow: true,
        }
    }
}

/// A persistent scalar transition. Retargeting starts at the currently displayed value.
#[derive(Clone, Debug, Default)]
pub(crate) struct Transition {
    value: f32,
    target: f32,
    from: f32,
    elapsed: f32,
}

impl Transition {
    pub fn settled(value: f32) -> Self {
        assert!(value.is_finite(), "Animation values must be finite");
        Self {
            value,
            target: value,
            from: value,
            elapsed: 0.0,
        }
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn is_active(&self) -> bool {
        self.value != self.target
    }

    pub fn retarget(&mut self, target: f32) {
        assert!(target.is_finite(), "Animation targets must be finite");
        if self.target != target {
            self.from = self.value;
            self.target = target;
            self.elapsed = 0.0;
        }
    }

    pub(crate) fn advance(&mut self, delta: f32, policy: MotionPolicy) {
        assert!(
            policy.seconds.is_finite() && policy.seconds > 0.0,
            "Motion duration must be positive and finite"
        );
        if !policy.enabled {
            self.value = self.target;
            self.from = self.target;
            self.elapsed = policy.seconds;
            return;
        }
        self.elapsed = (self.elapsed + delta).min(policy.seconds);
        let t = self.elapsed / policy.seconds;
        self.value = self.from + (self.target - self.from) * ease(t);
    }
}

#[derive(Clone)]
struct Scalar {
    transition: Transition,
    last_frame: u64,
    last_time: f64,
}

pub(crate) fn scalar(
    ctx: &egui::Context,
    id: egui::Id,
    initial: f32,
    target: f32,
    policy: MotionPolicy,
) -> f32 {
    let frame = ctx.cumulative_frame_nr();
    let now = ctx.input(|i| i.time);
    let (value, active) = ctx.data_mut(|d| {
        let mut state = d.get_temp::<Scalar>(id).unwrap_or(Scalar {
            transition: Transition::settled(initial),
            last_frame: frame,
            last_time: now,
        });
        if state.last_frame + 1 < frame {
            state.transition = Transition::settled(initial);
        }
        state
            .transition
            .advance((now - state.last_time).max(0.0) as f32, policy);
        state.transition.retarget(target);
        if !policy.enabled {
            state.transition.advance(0.0, policy);
        }
        state.last_frame = frame;
        state.last_time = now;
        let result = (state.transition.value(), state.transition.is_active());
        d.insert_temp(id, state);
        result
    });
    if active {
        ctx.request_repaint();
        track(ctx);
    }
    value
}

#[derive(Clone, Default)]
struct Active {
    frame: u64,
    count: usize,
}

fn track(ctx: &egui::Context) {
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| {
        let state = d.get_temp_mut_or_default::<Active>(egui::Id::new("bedtime.motion"));
        if state.frame != frame {
            *state = Active { frame, count: 0 };
        }
        state.count += 1;
    });
}

pub(crate) fn active(ctx: &egui::Context) -> usize {
    ctx.data(|d| d.get_temp::<Active>(egui::Id::new("bedtime.motion")))
        .filter(|state| state.frame == ctx.cumulative_frame_nr())
        .map_or(0, |state| state.count)
}

#[cfg(test)]
mod tests {
    use super::ease;

    #[test]
    fn ease_starts_fast_settles_gently_and_ends_exactly() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert_eq!(ease(f32::NAN), 1.0);
        assert_eq!(ease(2.0), 1.0);
        // Half the time covers most of the way: faster than a cubic ease out.
        assert!(ease(0.5) > 1.0 - 0.5_f32.powi(3));
        let steps: Vec<f32> = (0..=20).map(|i| ease(i as f32 / 20.0)).collect();
        assert!(steps.windows(2).all(|pair| pair[1] >= pair[0]));
        // Each step covers less than the last.
        let gains: Vec<f32> = steps.windows(2).map(|pair| pair[1] - pair[0]).collect();
        assert!(gains.windows(2).all(|pair| pair[1] <= pair[0]));
    }
}
