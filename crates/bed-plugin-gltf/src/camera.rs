use crate::model::Scene;
#[cfg(test)]
use glam::Mat4;
use glam::Vec3;
use serde_json::{Value, json};

const FOV: f32 = std::f32::consts::FRAC_PI_4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Camera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}
impl Camera {
    pub fn restore(state: &Value) -> (Self, bool) {
        let finite = |value: &Value| {
            value
                .as_f64()
                .filter(|v| v.is_finite() && v.abs() <= 1e15)
                .map(|v| v as f32)
        };
        let mut camera = Self {
            target: Vec3::ZERO,
            yaw: 0.65,
            pitch: 0.35,
            distance: 4.0,
        };
        let value = &state["camera"];
        let restored = (|| {
            camera.target = Vec3::new(
                finite(&value["target"][0])?,
                finite(&value["target"][1])?,
                finite(&value["target"][2])?,
            );
            camera.yaw = finite(&value["yaw"])?.rem_euclid(std::f32::consts::TAU);
            camera.pitch = finite(&value["pitch"])?.clamp(-1.5, 1.5);
            camera.distance = finite(&value["distance"])?.clamp(1e-6, 1e15);
            Some(())
        })()
        .is_some();
        (camera, restored)
    }
    pub fn save(self) -> Value {
        json!({ "camera": { "target": self.target.to_array(), "yaw": self.yaw,
            "pitch": self.pitch, "distance": self.distance } })
    }
    pub fn eye(self) -> Vec3 {
        self.target
            + Vec3::new(
                self.yaw.sin() * self.pitch.cos(),
                self.pitch.sin(),
                self.yaw.cos() * self.pitch.cos(),
            ) * self.distance
    }
    pub fn fit(&mut self, scene: &Scene, aspect: f32) {
        self.target = scene.center();
        let half_fov = ((FOV * 0.5).tan() * aspect.min(1.0)).atan();
        self.distance = scene.radius() / half_fov.sin() * 1.15;
    }
    pub fn orbit(&mut self, delta: [f32; 2]) {
        self.yaw = (self.yaw - delta[0] * 0.008).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + delta[1] * 0.008).clamp(-1.5, 1.5);
    }
    pub fn pan(&mut self, delta: [f32; 2], height: f32) {
        let forward = (self.target - self.eye()).normalize_or_zero();
        let right = forward.cross(Vec3::Y).normalize_or_zero();
        let up = right.cross(forward);
        let scale = self.distance * (FOV * 0.5).tan() * 2.0 / height.max(1.0);
        self.target += (-right * delta[0] + up * delta[1]) * scale;
    }
    pub fn zoom(&mut self, wheel: f32, radius: f32) {
        self.distance =
            (self.distance * (-wheel * 0.15).exp()).clamp(radius * 0.02, radius * 1000.0);
    }
    #[cfg(test)]
    pub fn view_projection(self, aspect: f32, radius: f32) -> Mat4 {
        let near = (radius * 0.001).max(1e-6);
        let far = (self.distance + radius * 20.0).max(near * 100.0);
        glam::camera::rh::proj::directx::perspective(FOV, aspect, near, far)
            * glam::camera::rh::view::look_at_mat4(self.eye(), self.target, Vec3::Y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn camera_round_trips_and_remains_finite_at_orbit_and_zoom_limits() {
        let (mut camera, _) = Camera::restore(&Value::Null);
        camera.orbit([1e6, 1e6]);
        camera.zoom(1e6, 2.0);
        camera.pan([20.0, -30.0], 400.0);
        assert!(camera.eye().is_finite());
        assert!(camera.view_projection(1.5, 2.0).is_finite());
        let (restored, valid) = Camera::restore(&camera.save());
        assert!(valid);
        assert_eq!(camera, restored);
        let (_, valid) = Camera::restore(&json!({"camera": {"distance": "bad"}}));
        assert!(!valid);
    }
}
