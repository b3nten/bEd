//! Translated from ned editor/views/hover_trigger.h; see LICENSE and NOTICE.
//! The caller provides ImGui time/style delay so this machine has no GUI types.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Zone {
    #[default]
    None,
    Text,
    Gutter,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub zone: Zone,
    pub row: i32,
    pub column: i32,
}
impl Default for Target {
    fn default() -> Self {
        Self {
            zone: Zone::None,
            row: -1,
            column: 0,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub active: bool,
    pub zone: Zone,
    pub row: i32,
    pub column: i32,
}
impl Default for Info {
    fn default() -> Self {
        Self {
            active: false,
            zone: Zone::None,
            row: -1,
            column: 0,
        }
    }
}
#[derive(Default)]
pub struct HoverTrigger {
    armed: bool,
    armed_at: f64,
    armed_target: Target,
    current: Info,
    #[cfg(test)]
    test_delay: Option<f64>,
}
impl HoverTrigger {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn update_at(
        &mut self,
        now: f64,
        mouse_moved: bool,
        dismissed: bool,
        target: Target,
        hover_delay_normal: f64,
    ) {
        if mouse_moved || dismissed || target.zone == Zone::None {
            self.armed = false;
            self.current = Info::default();
        }
        let delay = 0.25_f64.max(hover_delay_normal);
        #[cfg(test)]
        let delay = self.test_delay.unwrap_or(delay);
        if mouse_moved && !dismissed && target.zone != Zone::None {
            self.armed = true;
            self.armed_at = now;
            self.armed_target = target;
        } else if self.armed
            && !self.current.active
            && target.zone != Zone::None
            && now - self.armed_at >= delay
        {
            self.current = Info {
                active: true,
                zone: self.armed_target.zone,
                row: self.armed_target.row,
                column: self.armed_target.column,
            };
        }
    }
    pub fn info(&self) -> Info {
        self.current
    }
    #[cfg(test)]
    fn set_delay_for_test(&mut self, seconds: f64) {
        self.test_delay = (seconds >= 0.0).then_some(seconds);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn word() -> Target {
        Target {
            zone: Zone::Text,
            row: 4,
            column: 10,
        }
    }
    #[test]
    fn upstream_rest_shows_any_movement_hides_and_rearms() {
        let mut trigger = HoverTrigger::new();
        trigger.set_delay_for_test(0.3);
        trigger.update_at(0.0, true, false, word(), 0.0);
        trigger.update_at(0.1, false, false, word(), 0.0);
        assert!(!trigger.info().active);
        trigger.update_at(0.4, false, false, word(), 0.0);
        assert!(trigger.info().active);
        assert_eq!(trigger.info().row, 4);
        assert_eq!(trigger.info().column, 10);
        trigger.update_at(0.4, true, false, word(), 0.0);
        assert!(!trigger.info().active);
        trigger.update_at(0.5, false, false, word(), 0.0);
        assert!(!trigger.info().active);
        trigger.update_at(0.8, false, false, word(), 0.0);
        assert!(trigger.info().active);
    }
    #[test]
    fn upstream_key_click_scroll_and_zone_exit_dismiss() {
        for (moved, dismissed, target) in [
            (false, true, word()),
            (true, true, word()),
            (false, false, Target::default()),
        ] {
            let mut trigger = HoverTrigger::new();
            trigger.set_delay_for_test(0.05);
            trigger.update_at(0.0, true, false, word(), 0.0);
            trigger.update_at(0.1, false, false, word(), 0.0);
            assert!(trigger.info().active);
            trigger.update_at(0.1, moved, dismissed, target, 0.0);
            assert!(!trigger.info().active);
        }
    }
    #[test]
    fn upstream_dismissed_parked_mouse_cannot_rearm() {
        let mut trigger = HoverTrigger::new();
        trigger.set_delay_for_test(0.05);
        trigger.update_at(0.0, true, false, word(), 0.0);
        trigger.update_at(0.1, false, false, word(), 0.0);
        assert!(trigger.info().active);
        trigger.update_at(0.1, false, true, word(), 0.0);
        trigger.update_at(10.1, false, false, word(), 0.0);
        trigger.update_at(20.1, false, false, word(), 0.0);
        assert!(!trigger.info().active);
        trigger.update_at(20.1, true, false, word(), 0.0);
        trigger.update_at(20.2, false, false, word(), 0.0);
        assert!(trigger.info().active);
    }
    #[test]
    fn upstream_continuous_movement_never_fires() {
        let mut trigger = HoverTrigger::new();
        trigger.set_delay_for_test(0.05);
        for step in 0..10 {
            trigger.update_at(step as f64 * 0.1, true, false, word(), 0.0);
            assert!(!trigger.info().active);
        }
    }
    #[test]
    fn style_floor_and_frozen_target_survive_stationary_content_change() {
        let mut trigger = HoverTrigger::new();
        trigger.update_at(0.0, true, false, word(), 0.1);
        let changed = Target { row: 8, ..word() };
        trigger.update_at(0.249, false, false, changed, 0.1);
        assert!(!trigger.info().active);
        trigger.update_at(0.25, false, false, changed, 0.1);
        assert_eq!(trigger.info().row, 4);
        trigger.update_at(0.3, true, false, changed, 0.6);
        trigger.update_at(0.89, false, false, changed, 0.6);
        assert!(!trigger.info().active);
        trigger.update_at(0.91, false, false, changed, 0.6);
        assert_eq!(trigger.info().row, 8);
    }
}
