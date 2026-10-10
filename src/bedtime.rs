//! Native input owns the idle transition, before events reach ImGui or the editor.
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
use winit::{
    event::{ElementState, Ime, MouseButton, TouchPhase, WindowEvent},
    keyboard::PhysicalKey,
};

pub(crate) fn scene_pixels(
    size: [f32; 2],
    viewport_scale: [f32; 2],
    display_scale: [f32; 2],
) -> [u32; 2] {
    // The main viewport can leave FramebufferScale unset. Its native scale
    // is maintained in IO by the platform backend.
    let pixels: [f32; 2] = std::array::from_fn(|axis| {
        let scale = if viewport_scale[axis].is_finite() && viewport_scale[axis] > 0.0 {
            viewport_scale[axis]
        } else {
            display_scale[axis]
        };
        size[axis] * scale
    });
    let factor = (1920.0 / pixels[0].max(pixels[1]).max(1.0)).min(1.0);
    pixels.map(|dimension| (dimension * factor).max(1.0) as u32)
}

pub(crate) struct IdleController {
    last_activity: Instant,
    viewport: Option<u32>,
    eligible: Option<u32>,
    swallowed_keys: HashSet<PhysicalKey>,
    swallowed_buttons: HashSet<MouseButton>,
    swallowed_touches: HashSet<u64>,
    swallow_ime: bool,
    scroll_until: Option<Instant>,
}
impl IdleController {
    pub fn new(now: Instant) -> Self {
        Self {
            last_activity: now,
            viewport: None,
            eligible: None,
            swallowed_keys: HashSet::new(),
            swallowed_buttons: HashSet::new(),
            swallowed_touches: HashSet::new(),
            swallow_ime: false,
            scroll_until: None,
        }
    }
    pub fn activity(&mut self, now: Instant) -> bool {
        self.last_activity = now;
        self.viewport.take().is_some()
    }
    pub fn update(
        &mut self,
        now: Instant,
        eligible: Option<u32>,
        enabled: bool,
        delay: Duration,
    ) -> Option<u32> {
        let eligible = eligible.filter(|_| enabled);
        if eligible.is_none() || eligible != self.eligible {
            self.activity(now);
        }
        self.eligible = eligible;
        if now.saturating_duration_since(self.last_activity) >= delay {
            self.viewport = eligible;
        }
        self.viewport
    }
    pub fn intercept(&mut self, event: &WindowEvent, now: Instant) -> bool {
        match event {
            WindowEvent::Focused(_) => {
                self.activity(now);
                self.eligible = None;
                // A focus change can prevent the matching releases from ever
                // reaching this window. Do not swallow a future fresh press.
                self.swallowed_keys.clear();
                self.swallowed_buttons.clear();
                self.swallowed_touches.clear();
                self.swallow_ime = false;
                self.scroll_until = None;
                false
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.key(event.physical_key, event.state, now)
            }
            WindowEvent::MouseInput { state, button, .. } => self.button(*button, *state, now),
            WindowEvent::MouseWheel { phase, .. } => self.gesture(*phase, now),
            WindowEvent::Touch(touch) => {
                let consume = self.activity(now) || self.swallowed_touches.contains(&touch.id);
                if consume && !matches!(touch.phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                    self.swallowed_touches.insert(touch.id);
                }
                if matches!(touch.phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                    self.swallowed_touches.remove(&touch.id);
                }
                consume
            }
            WindowEvent::Ime(Ime::Preedit(_, _)) => {
                let consume = self.activity(now) || self.swallow_ime;
                if consume {
                    self.swallow_ime = true;
                }
                consume
            }
            WindowEvent::Ime(Ime::Commit(_)) => {
                let consume = self.activity(now) || self.swallow_ime;
                self.swallow_ime = false;
                consume
            }
            WindowEvent::Ime(Ime::Disabled) => {
                self.swallow_ime = false;
                false
            }
            WindowEvent::PinchGesture { phase, .. }
            | WindowEvent::RotationGesture { phase, .. }
            | WindowEvent::PanGesture { phase, .. } => self.gesture(*phase, now),
            WindowEvent::CursorMoved { .. } | WindowEvent::DoubleTapGesture { .. } => {
                self.activity(now)
            }
            WindowEvent::HoveredFile(_) | WindowEvent::DroppedFile(_) => {
                self.activity(now);
                false
            }
            _ => false,
        }
    }
    fn gesture(&mut self, phase: TouchPhase, now: Instant) -> bool {
        let consume = self.activity(now) || self.scroll_until.is_some_and(|until| now < until);
        self.scroll_until =
            if consume && !matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                Some(now + Duration::from_millis(300))
            } else {
                None
            };
        consume
    }
    fn key(&mut self, key: PhysicalKey, state: ElementState, now: Instant) -> bool {
        let consume = self.activity(now) || self.swallowed_keys.contains(&key);
        if state == ElementState::Pressed {
            if consume {
                self.swallowed_keys.insert(key);
                self.swallow_ime = true;
            } else {
                self.swallow_ime = false;
            }
        } else {
            self.swallowed_keys.remove(&key);
        }
        consume
    }
    fn button(&mut self, button: MouseButton, state: ElementState, now: Instant) -> bool {
        let consume = self.activity(now) || self.swallowed_buttons.contains(&button);
        if state == ElementState::Pressed && consume {
            self.swallowed_buttons.insert(button);
        } else if state == ElementState::Released {
            self.swallowed_buttons.remove(&button);
        }
        consume
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::KeyCode;
    #[test]
    fn unset_main_viewport_scale_preserves_scene_resolution_and_aspect() {
        assert_eq!(
            scene_pixels([1200.0, 800.0], [0.0; 2], [2.0; 2]),
            [1920, 1280]
        );
        assert_eq!(scene_pixels([400.0, 600.0], [1.5; 2], [2.0; 2]), [600, 900]);
        assert_eq!(
            scene_pixels([1200.0, 800.0], [f32::NAN; 2], [2.0; 2]),
            [1920, 1280]
        );
    }
    #[test]
    fn idle_activates_for_the_focused_window_and_resets_when_it_changes() {
        let now = Instant::now();
        let mut idle = IdleController::new(now);
        let delay = Duration::from_secs(60);
        assert_eq!(idle.update(now, Some(7), true, delay), None);
        assert_eq!(idle.update(now + delay, Some(7), true, delay), Some(7));
        assert_eq!(idle.update(now + delay, Some(7), false, delay), None);
        assert_eq!(idle.update(now + delay * 3, None, true, delay), None);
        assert_eq!(idle.update(now + delay * 4, Some(8), true, delay), None);
        assert_eq!(idle.update(now + delay * 5, Some(8), true, delay), Some(8));
    }
    #[test]
    fn waking_key_and_click_consume_the_whole_sequence() {
        let now = Instant::now();
        let mut idle = IdleController::new(now);
        let delay = Duration::from_secs(1);
        idle.update(now, Some(1), true, delay);
        idle.update(now + delay, Some(1), true, delay);
        let key = PhysicalKey::Code(KeyCode::KeyA);
        assert!(idle.key(key, ElementState::Pressed, now + delay));
        assert!(idle.key(key, ElementState::Pressed, now + delay));
        assert!(idle.key(key, ElementState::Released, now + delay));
        assert!(!idle.key(key, ElementState::Pressed, now + delay));
        assert!(!idle.swallow_ime);
        idle.update(now + delay * 2, Some(1), true, delay);
        assert!(idle.button(MouseButton::Left, ElementState::Pressed, now + delay * 2));
        assert!(idle.button(MouseButton::Left, ElementState::Released, now + delay * 2));
        assert!(!idle.button(MouseButton::Left, ElementState::Pressed, now + delay * 2));
    }
    #[test]
    fn waking_mouse_movement_and_focus_reset_idle() {
        let now = Instant::now();
        let mut idle = IdleController::new(now);
        let delay = Duration::from_secs(1);
        idle.update(now, Some(1), true, delay);
        idle.update(now + delay, Some(1), true, delay);
        assert!(idle.activity(now + delay));
        assert_eq!(idle.update(now + delay, Some(1), true, delay), None);
        assert!(!idle.intercept(&WindowEvent::Focused(false), now + delay));
        assert_eq!(idle.update(now + delay * 2, Some(1), true, delay), None);
    }
    #[test]
    fn waking_ime_preedit_consumes_its_commit_and_then_recovers() {
        let now = Instant::now();
        let mut idle = IdleController::new(now);
        let delay = Duration::from_secs(1);
        idle.update(now, Some(1), true, delay);
        idle.update(now + delay, Some(1), true, delay);
        assert!(idle.intercept(
            &WindowEvent::Ime(Ime::Preedit("é".into(), Some((0, 2)))),
            now + delay
        ));
        assert!(idle.intercept(
            &WindowEvent::Ime(Ime::Preedit(String::new(), None)),
            now + delay
        ));
        assert!(idle.intercept(&WindowEvent::Ime(Ime::Commit("é".into())), now + delay));
        assert!(!idle.intercept(&WindowEvent::Ime(Ime::Commit("x".into())), now + delay));
    }
    #[test]
    fn focus_change_clears_lost_releases_and_waking_gestures_consume_continuations() {
        let now = Instant::now();
        let mut idle = IdleController::new(now);
        let delay = Duration::from_secs(1);
        idle.update(now, Some(1), true, delay);
        idle.update(now + delay, Some(1), true, delay);
        let key = PhysicalKey::Code(KeyCode::KeyA);
        assert!(idle.key(key, ElementState::Pressed, now + delay));
        idle.intercept(&WindowEvent::Focused(false), now + delay);
        assert!(!idle.key(key, ElementState::Pressed, now + delay));
        idle.update(now + delay, Some(1), true, delay);
        idle.update(now + delay * 2, Some(1), true, delay);
        assert!(idle.gesture(TouchPhase::Started, now + delay * 2));
        assert!(idle.gesture(
            TouchPhase::Moved,
            now + delay * 2 + Duration::from_millis(30)
        ));
        assert!(idle.gesture(
            TouchPhase::Ended,
            now + delay * 2 + Duration::from_millis(60)
        ));
        assert!(!idle.gesture(
            TouchPhase::Started,
            now + delay * 2 + Duration::from_millis(70)
        ));
    }
}
