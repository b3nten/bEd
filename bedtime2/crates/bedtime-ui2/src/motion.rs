use bevy::prelude::*;

/// Surface policy, independent of its color theme. Disable to settle immediately.
#[derive(Component, Clone, Copy, Debug)]
pub struct MotionPolicy {
    pub enabled: bool,
    pub seconds: f32,
    pub popup_initial_scale: f32,
}
impl Default for MotionPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            seconds: 0.16,
            popup_initial_scale: 0.96,
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

    pub fn target(&self) -> f32 {
        self.target
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
        let ease = 1.0 - (1.0 - t).powi(3);
        self.value = self.from + (self.target - self.from) * ease;
    }
}

/// Private opening state for one transient presentation.
#[derive(Default, Debug)]
pub(crate) struct PopupMotion {
    transition: Transition,
    last_frame: Option<usize>,
    source: u32,
    opening: bool,
    anchor: [f32; 2],
    scale_settled: bool,
    pub(crate) presentation: Option<PopupEffect>,
}

impl PopupMotion {
    pub fn progress(&self) -> f32 {
        self.transition.value()
    }

    pub fn is_active(&self) -> bool {
        self.transition.is_active()
    }

    pub(crate) fn advance(&mut self, delta: f32, policy: MotionPolicy) {
        self.transition.advance(delta, policy);
    }

    pub(crate) fn reveal(&mut self, frame: usize, source: u32, policy: MotionPolicy) -> f32 {
        if self.source != source
            || !self
                .last_frame
                .is_some_and(|last| last == frame || last + 1 == frame)
        {
            self.transition = Transition::default();
            self.transition.retarget(1.0);
            self.opening = true;
            self.scale_settled = false;
        }
        self.source = source;
        self.last_frame = Some(frame);
        if !policy.enabled {
            self.transition.advance(0.0, policy);
        }
        self.transition.value()
    }

    pub(crate) fn present(&mut self, ui: &dear_imgui_rs::Ui, anchor: [f32; 2], initial_scale: f32) {
        assert!(
            initial_scale.is_finite() && (0.0..=1.0).contains(&initial_scale),
            "Popup initial scale must be in 0..=1"
        );
        if self.opening {
            self.anchor = if ui.is_mouse_pos_valid_at(anchor) {
                anchor
            } else {
                ui.window_pos()
            };
            self.opening = false;
        }
        if ui.is_any_item_active() || (ui.is_window_hovered() && ui.io().mouse_delta() != [0.0; 2])
        {
            self.scale_settled = true;
        }
        let alpha = self.progress();
        let scale = if self.scale_settled {
            1.0
        } else {
            initial_scale + (1.0 - initial_scale) * alpha
        };
        // Retain only an opaque identity and owned scalar data. Native
        // window/draw-list pointers are resolved inside the current frame owner.
        let window =
            ui.with_bound_context(|| unsafe { (*dear_imgui_rs::sys::igGetCurrentWindowRead()).ID });
        self.presentation = Some(PopupEffect {
            frame: ui.frame_count(),
            window,
            alpha,
            scale,
            anchor: self.anchor,
        });
    }
}

/// Private, context-owned interpolation state. Entries are scoped to the drawing
/// view and ImGui ID, so reordering correctly identified items preserves motion.
#[derive(Default)]
pub(crate) struct Animations {
    frame: usize,
    now: f64,
    scalars: std::collections::HashMap<(Entity, u32, ScalarKind), ScalarEntry>,
    popups: std::collections::HashMap<(Entity, u32, PopupKind), PopupEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ScalarKind {
    Hover,
    Toggle,
    Disclosure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PopupKind {
    Tooltip,
    Menu,
    Dialog,
}

struct ScalarEntry {
    transition: Transition,
    last_frame: usize,
    last_time: f64,
}

struct PopupEntry {
    motion: PopupMotion,
    last_time: f64,
}

impl Animations {
    pub fn begin_frame(&mut self, frame: usize, now: f64) {
        self.frame = frame;
        self.now = now;
    }

    pub fn scalar(
        &mut self,
        view: Entity,
        id: u32,
        kind: ScalarKind,
        initial: f32,
        target: f32,
        policy: MotionPolicy,
    ) -> f32 {
        let entry = self
            .scalars
            .entry((view, id, kind))
            .or_insert_with(|| ScalarEntry {
                transition: Transition::settled(initial),
                last_frame: self.frame,
                last_time: self.now,
            });
        if entry.last_frame + 1 < self.frame {
            entry.transition = Transition::settled(initial);
        }
        entry
            .transition
            .advance((self.now - entry.last_time).max(0.0) as f32, policy);
        entry.transition.retarget(target);
        if !policy.enabled {
            entry.transition.advance(0.0, policy);
        }
        entry.last_frame = self.frame;
        entry.last_time = self.now;
        entry.transition.value()
    }

    pub fn hover(&mut self, view: Entity, id: u32, policy: MotionPolicy) -> f32 {
        let target = self
            .scalars
            .get(&(view, id, ScalarKind::Hover))
            .map_or(0.0, |entry| entry.transition.target());
        self.scalar(view, id, ScalarKind::Hover, 0.0, target, policy)
    }

    pub fn retarget_hover(&mut self, view: Entity, id: u32, hovered: bool, policy: MotionPolicy) {
        let entry = self
            .scalars
            .get_mut(&(view, id, ScalarKind::Hover))
            .expect("Hover was sampled before painting");
        entry.transition.retarget(if hovered { 1.0 } else { 0.0 });
        if !policy.enabled {
            entry.transition.advance(0.0, policy);
        }
    }

    pub fn popup(
        &mut self,
        view: Entity,
        id: u32,
        kind: PopupKind,
        ui: &dear_imgui_rs::Ui,
        anchor: [f32; 2],
        policy: MotionPolicy,
    ) {
        let entry = self
            .popups
            .entry((view, id, kind))
            .or_insert_with(|| PopupEntry {
                motion: PopupMotion::default(),
                last_time: self.now,
            });
        entry
            .motion
            .advance((self.now - entry.last_time).max(0.0) as f32, policy);
        entry.motion.reveal(self.frame, id, policy);
        entry.last_time = self.now;
        entry.motion.present(
            ui,
            anchor,
            if kind == PopupKind::Dialog {
                1.0
            } else {
                policy.popup_initial_scale
            },
        );
    }

    /// Keep only current/recently submitted controls belonging to live views.
    /// Hidden/clipped controls never keep a reactive event loop awake.
    pub fn finish_frame(
        &mut self,
        mut view_alive: impl FnMut(Entity) -> bool,
    ) -> (Vec<PopupEffect>, usize) {
        let oldest = self.frame.saturating_sub(1);
        self.scalars
            .retain(|(view, _, _), entry| entry.last_frame >= oldest && view_alive(*view));
        self.popups.retain(|(view, _, _), entry| {
            entry.motion.last_frame.is_some_and(|frame| frame >= oldest) && view_alive(*view)
        });
        let active = self
            .scalars
            .values()
            .filter(|entry| entry.last_frame == self.frame && entry.transition.is_active())
            .count()
            + self
                .popups
                .values()
                .filter(|entry| {
                    entry.motion.last_frame == Some(self.frame) && entry.motion.is_active()
                })
                .count();
        let effects = self
            .popups
            .values()
            .filter_map(|entry| entry.motion.presentation)
            .filter(|effect| effect.frame == self.frame)
            .collect();
        (effects, active)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PopupEffect {
    pub frame: usize,
    pub window: u32,
    pub alpha: f32,
    pub scale: f32,
    pub anchor: [f32; 2],
}

/// One explicit final presentation pass, before ImGui captures the frame. Native
/// layout, scrolling and hit testing retain their final, untransformed geometry.
pub(crate) fn apply_popups(ui: &dear_imgui_rs::Ui, effects: &[PopupEffect]) {
    use dear_imgui_rs::sys;
    if effects.is_empty() {
        return;
    }
    ui.with_bound_context(|| unsafe {
        // SAFETY: all pointers are borrowed from this bound, live context and
        // consumed synchronously. No draw scope or native reference escapes.
        let context = &*sys::igGetCurrentContext();
        let mut transformed = std::collections::HashSet::new();
        for index in 0..context.Windows.Size {
            let window = &**context.Windows.Data.add(index as usize);
            if !window.Active || window.Hidden || window.DrawList.is_null() {
                continue;
            }
            let mut ancestor = window as *const sys::ImGuiWindow;
            let mut effect = None;
            for _ in 0..context.Windows.Size {
                if ancestor.is_null() {
                    break;
                }
                let node = &*ancestor;
                if let Some(found) = effects
                    .iter()
                    .find(|e| e.window == node.ID && e.frame == ui.frame_count())
                {
                    effect = Some(*found);
                    break;
                }
                ancestor = node.ParentWindow;
            }
            let Some(effect) = effect else {
                continue;
            };
            if effect.alpha == 1.0 && effect.scale == 1.0 {
                continue;
            }
            if !transformed.insert(window.DrawList as usize) {
                continue;
            }
            sys::ImDrawList_ChannelsMerge(window.DrawList);
            let list = &mut *window.DrawList;
            for i in 0..list.VtxBuffer.Size {
                let vertex = &mut *list.VtxBuffer.Data.add(i as usize);
                vertex.col = (vertex.col & 0x00ff_ffff)
                    | ((((vertex.col >> 24) as f32 * effect.alpha).round() as u32) << 24);
                if effect.scale != 1.0 {
                    vertex.pos.x =
                        effect.anchor[0] + (vertex.pos.x - effect.anchor[0]) * effect.scale;
                    vertex.pos.y =
                        effect.anchor[1] + (vertex.pos.y - effect.anchor[1]) * effect.scale;
                }
            }
            if effect.scale != 1.0 {
                for i in 0..list.CmdBuffer.Size {
                    let clip = &mut (*list.CmdBuffer.Data.add(i as usize)).ClipRect;
                    clip.x = effect.anchor[0] + (clip.x - effect.anchor[0]) * effect.scale;
                    clip.y = effect.anchor[1] + (clip.y - effect.anchor[1]) * effect.scale;
                    clip.z = effect.anchor[0] + (clip.z - effect.anchor[0]) * effect.scale;
                    clip.w = effect.anchor[1] + (clip.w - effect.anchor[1]) * effect.scale;
                }
            }
        }
    });
}
