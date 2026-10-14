//! Renderer-independent configuration for Bed visual effects.

pub const SETTINGS_OWNER: &str = "bedtime.effects";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EffectPreset {
    Sharp,
    #[default]
    Off,
    Legacy,
    Custom,
}

impl EffectPreset {
    pub fn name(self) -> &'static str {
        match self {
            Self::Sharp => "sharp",
            Self::Off => "off",
            Self::Legacy => "legacy",
            Self::Custom => "custom",
        }
    }

    pub fn from_preferences(settings: &serde_json::Value) -> Self {
        match settings["effect_preset"].as_str() {
            Some("sharp") => Self::Sharp,
            Some("legacy") => Self::Legacy,
            Some("custom") => Self::Custom,
            _ => Self::Off,
        }
    }
}

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
        let mut settings = Self::subtle();
        settings.enabled = false;
        settings
    }
}

impl ShaderSettings {
    pub fn from_preferences(settings: &serde_json::Value) -> Self {
        match EffectPreset::from_preferences(settings) {
            EffectPreset::Sharp => Self::subtle(),
            EffectPreset::Off => Self::default(),
            EffectPreset::Legacy => {
                let mut legacy = Self::from_json(settings);
                legacy.enabled = true;
                legacy.pulse_intensity = settings["pulse_intensity"]
                    .as_f64()
                    .filter(|value| value.is_finite())
                    .map_or(0.05, |value| value as f32);
                legacy
            }
            EffectPreset::Custom => Self::from_json(settings),
        }
    }

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

#[cfg(feature = "desktop")]
pub fn register_preferences(world: &mut bevy::prelude::World) {
    use bedtime_api::settings::SettingDefinition;
    let defaults = ShaderSettings::legacy();
    let mut definitions = vec![
        SettingDefinition::new("effect_preset", "Effect preset", "off")
            .validated_by(validate_preset),
        SettingDefinition::new("shader_toggle", "Enable custom effects", false),
        SettingDefinition::new("pixel_width", "Pixel width", defaults.pixel_width)
            .validated_by(validate_positive),
        SettingDefinition::new(
            "pixelation_intensity",
            "Pixelation",
            defaults.pixelation_intensity,
        )
        .validated_by(validate_number),
        SettingDefinition::new("burnin_intensity", "Burn-in", defaults.burnin_intensity)
            .validated_by(validate_burnin),
    ];
    definitions.extend(
        [
            (
                "scanline_intensity",
                "Scanlines",
                defaults.scanline_intensity,
            ),
            ("vignet_intensity", "Vignette", defaults.vignet_intensity),
            ("bloom_intensity", "Bloom", defaults.bloom_intensity),
            ("static_intensity", "Static", defaults.static_intensity),
            (
                "colorshift_intensity",
                "RGB shift",
                defaults.colorshift_intensity,
            ),
            ("jitter_intensity", "Jitter", defaults.jitter_intensity),
            (
                "curvature_intensity",
                "Curvature",
                defaults.curvature_intensity,
            ),
            ("pulse_intensity", "Pulse", defaults.pulse_intensity),
        ]
        .map(|(key, label, value)| {
            SettingDefinition::new(key, label, value).validated_by(validate_nonnegative)
        }),
    );
    bedtime_api::settings::register(world, SETTINGS_OWNER, definitions)
        .expect("Effects preferences install once");
}

#[cfg(feature = "desktop")]
fn validate_preset(value: &serde_json::Value) -> Result<(), String> {
    match value.as_str() {
        Some("off" | "sharp" | "legacy" | "custom") => Ok(()),
        _ => Err("Choose off, sharp, legacy, or custom".into()),
    }
}

#[cfg(feature = "desktop")]
fn validate_number(value: &serde_json::Value) -> Result<(), String> {
    if value
        .as_f64()
        .is_some_and(|value| (value as f32).is_finite())
    {
        Ok(())
    } else {
        Err("Effect values must be finite and fit in a shader float".into())
    }
}

#[cfg(feature = "desktop")]
fn validate_nonnegative(value: &serde_json::Value) -> Result<(), String> {
    validate_number(value)?;
    if value.as_f64().expect("Validated number") >= 0.0 {
        Ok(())
    } else {
        Err("Effect intensity must be nonnegative".into())
    }
}

#[cfg(feature = "desktop")]
fn validate_positive(value: &serde_json::Value) -> Result<(), String> {
    validate_number(value)?;
    if value.as_f64().expect("Validated number") as f32 > 0.0 {
        Ok(())
    } else {
        Err("Pixel width must be positive".into())
    }
}

#[cfg(feature = "desktop")]
fn validate_burnin(value: &serde_json::Value) -> Result<(), String> {
    validate_number(value)?;
    if (0.0..=1.0).contains(&value.as_f64().expect("Validated number")) {
        Ok(())
    } else {
        Err("Burn-in must be between zero and one".into())
    }
}
