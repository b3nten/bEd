//! Renderer-independent configuration for Bed visual effects.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShaderSettings {
    pub enabled: bool,
    pub scanline_intensity: f32,
    pub vignet_intensity: f32,
    pub bloom_intensity: f32,
    pub static_intensity: f32,
    pub colorshift_intensity: f32,
    pub jitter_intensity: f32,
    pub curvature_intensity: f32,
    pub pixelation_intensity: f32,
    pub pixel_width: f32,
    pub burnin_intensity: f32,
    pub pulse_intensity: f32,
}
impl Default for ShaderSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            ..Self::subtle()
        }
    }
}
impl ShaderSettings {
    pub fn subtle() -> Self {
        Self {
            enabled: true,
            scanline_intensity: 0.06,
            vignet_intensity: 0.04,
            bloom_intensity: 0.025,
            static_intensity: 0.0,
            colorshift_intensity: 0.0,
            jitter_intensity: 0.0,
            curvature_intensity: 0.0,
            pixelation_intensity: 0.0,
            pixel_width: 5000.0,
            burnin_intensity: 0.0,
            pulse_intensity: 0.0,
        }
    }
    pub fn legacy() -> Self {
        Self {
            scanline_intensity: 0.41,
            vignet_intensity: 0.3,
            bloom_intensity: 0.77,
            static_intensity: 0.5,
            jitter_intensity: 2.72,
            curvature_intensity: 0.19,
            pixelation_intensity: -0.312,
            burnin_intensity: 0.9,
            pulse_intensity: 0.05,
            ..Self::subtle()
        }
    }
    pub fn from_json(settings: &serde_json::Value) -> Self {
        let base = Self::subtle();
        let number = |key: &str, default| {
            settings
                .get(key)
                .and_then(serde_json::Value::as_f64)
                .map(|n| n as f32)
                .filter(|n| n.is_finite())
                .unwrap_or(default)
        };
        Self {
            enabled: settings
                .get("shader_toggle")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            scanline_intensity: number("scanline_intensity", base.scanline_intensity),
            vignet_intensity: number("vignet_intensity", base.vignet_intensity),
            bloom_intensity: number("bloom_intensity", base.bloom_intensity),
            static_intensity: number("static_intensity", base.static_intensity),
            colorshift_intensity: number("colorshift_intensity", base.colorshift_intensity),
            jitter_intensity: number("jitter_intensity", base.jitter_intensity),
            curvature_intensity: number("curvature_intensity", base.curvature_intensity),
            pixelation_intensity: number("pixelation_intensity", base.pixelation_intensity),
            pixel_width: number("pixel_width", base.pixel_width),
            burnin_intensity: number("burnin_intensity", base.burnin_intensity).clamp(0.0, 1.0),
            pulse_intensity: number("pulse_intensity", base.pulse_intensity),
        }
    }
    pub fn uniform_bytes(&self, width: u32, height: u32, time: f32, output_srgb: bool) -> Vec<u8> {
        [
            width as f32,
            height as f32,
            time,
            f32::from(self.enabled),
            self.scanline_intensity,
            self.vignet_intensity,
            self.bloom_intensity,
            self.static_intensity,
            self.colorshift_intensity,
            self.jitter_intensity,
            self.curvature_intensity,
            self.pixelation_intensity,
            self.pixel_width,
            self.burnin_intensity,
            f32::from(output_srgb),
            self.pulse_intensity,
        ]
        .into_iter()
        .flat_map(f32::to_ne_bytes)
        .collect()
    }
}
