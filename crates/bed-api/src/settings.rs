//! Registered global preferences and ordered requests to their single owner.
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

pub type SettingValidator = fn(&Value) -> Result<(), String>;

#[derive(Clone, Debug)]
pub struct SettingDefinition {
    pub key: &'static str,
    pub label: &'static str,
    pub default: Value,
    pub validate: Option<SettingValidator>,
}
impl SettingDefinition {
    pub fn new(key: &'static str, label: &'static str, default: impl Into<Value>) -> Self {
        Self {
            key,
            label,
            default: default.into(),
            validate: None,
        }
    }
    pub fn validated_by(mut self, validate: SettingValidator) -> Self {
        self.validate = Some(validate);
        self
    }
    pub fn validate_value(&self, value: &Value) -> Result<(), String> {
        let same_kind = matches!(
            (&self.default, value),
            (Value::Null, Value::Null)
                | (Value::Bool(_), Value::Bool(_))
                | (Value::Number(_), Value::Number(_))
                | (Value::String(_), Value::String(_))
                | (Value::Array(_), Value::Array(_))
                | (Value::Object(_), Value::Object(_))
        );
        if !same_kind {
            return Err(format!("Invalid value type for {}", self.key));
        }
        if let Some(validate) = self.validate {
            validate(value)?;
        }
        Ok(())
    }
}

/// Definitions are installed during application construction. Registration is atomic.
#[derive(Resource, Default, Clone, Debug)]
pub struct SettingsRegistry {
    owners: BTreeMap<String, Vec<SettingDefinition>>,
    revision: u64,
    sealed: bool,
}
impl SettingsRegistry {
    pub fn register(
        &mut self,
        owner: &str,
        definitions: Vec<SettingDefinition>,
    ) -> Result<(), String> {
        if self.sealed {
            return Err("Settings registration has finished".into());
        }
        if !crate::workbench::valid_id(owner) {
            return Err(format!("Invalid settings owner: {owner}"));
        }
        let mut keys = std::collections::HashSet::new();
        for definition in &definitions {
            if !crate::workbench::valid_id(definition.key)
                || !keys.insert(definition.key)
                || self.definition(owner, definition.key).is_some()
            {
                return Err(format!(
                    "Duplicate or invalid setting: {owner}.{}",
                    definition.key
                ));
            }
            definition.validate_value(&definition.default)?;
        }
        self.owners
            .entry(owner.into())
            .or_default()
            .extend(definitions);
        self.revision = self
            .revision
            .checked_add(1)
            .expect("Settings registration revision exhausted");
        Ok(())
    }
    pub fn definition(&self, owner: &str, key: &str) -> Option<&SettingDefinition> {
        self.owners
            .get(owner)?
            .iter()
            .find(|definition| definition.key == key)
    }
    pub fn owners(&self) -> impl Iterator<Item = (&str, &[SettingDefinition])> {
        self.owners
            .iter()
            .map(|(owner, definitions)| (owner.as_str(), definitions.as_slice()))
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn set_defaults(
        &mut self,
        owner: &str,
        defaults: &[(String, Value)],
    ) -> Result<(), String> {
        if self.sealed {
            return Err("Settings registration has finished".into());
        }
        for (key, value) in defaults {
            self.definition(owner, key)
                .ok_or_else(|| format!("Setting is not registered: {owner}.{key}"))?
                .validate_value(value)?;
        }
        let definitions = self
            .owners
            .get_mut(owner)
            .ok_or_else(|| format!("Settings owner is not registered: {owner}"))?;
        for (key, value) in defaults {
            definitions
                .iter_mut()
                .find(|definition| definition.key == key)
                .expect("Defaults validated")
                .default = value.clone();
        }
        self.revision = self
            .revision
            .checked_add(1)
            .expect("Settings registration revision exhausted");
        Ok(())
    }
    pub fn seal(&mut self) {
        self.sealed = true;
    }
}

pub fn register(
    world: &mut World,
    owner: &str,
    definitions: Vec<SettingDefinition>,
) -> Result<(), String> {
    world.init_resource::<SettingsRegistry>();
    world
        .resource_mut::<SettingsRegistry>()
        .register(owner, definitions)
}

/// Published effective values. The core settings owner is its sole writer.
#[derive(Resource, Clone, Debug)]
pub struct SettingsSnapshot {
    pub settings: Value,
    pub default_viewers: Value,
    pub revision: u64,
    pub error: Option<String>,
}
impl Default for SettingsSnapshot {
    fn default() -> Self {
        Self {
            settings: json!({}),
            default_viewers: json!({}),
            revision: 0,
            error: None,
        }
    }
}
impl SettingsSnapshot {
    pub fn get(&self, owner: &str, key: &str) -> Option<&Value> {
        self.settings.get(owner)?.get(key)
    }
    pub fn for_owner(&self, owner: &str) -> &Value {
        &self.settings[owner]
    }
    pub fn keybindings(&self) -> bed_keybindings::Overrides {
        self.get("bed.workbench", "keybindings").map_or_else(
            bed_keybindings::Overrides::default,
            |value| {
                serde_json::from_value(value.clone())
                    .expect("Published keybindings were validated by their settings owner")
            },
        )
    }
}

pub fn validate_keybindings(value: &Value) -> Result<(), String> {
    bed_keybindings::Overrides::parse(&serde_json::to_vec(value).expect("JSON values serialize"))
        .map(|_| ())
}

#[derive(Clone, Debug, PartialEq)]
pub enum SettingsChange {
    Set {
        owner: String,
        key: String,
        value: Value,
    },
    Reset {
        owner: String,
        key: String,
    },
}
impl SettingsChange {
    pub fn set(owner: impl Into<String>, key: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::Set {
            owner: owner.into(),
            key: key.into(),
            value: value.into(),
        }
    }
    pub fn reset(owner: impl Into<String>, key: impl Into<String>) -> Self {
        Self::Reset {
            owner: owner.into(),
            key: key.into(),
        }
    }
    pub fn owner_key(&self) -> (&str, &str) {
        match self {
            Self::Set { owner, key, .. } | Self::Reset { owner, key } => (owner, key),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SettingsRequestId(pub u64);

#[derive(Message, Clone, Debug)]
pub struct SettingsRequest {
    pub changes: Vec<SettingsChange>,
    pub sequence: u64,
    pub receipt: Option<SettingsRequestId>,
}
impl SettingsRequest {
    pub fn new(owner: impl Into<String>, key: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::batch(vec![SettingsChange::set(owner, key, value)])
    }
    pub fn reset(owner: impl Into<String>, key: impl Into<String>) -> Self {
        Self::batch(vec![SettingsChange::reset(owner, key)])
    }
    pub fn batch(changes: Vec<SettingsChange>) -> Self {
        Self {
            changes,
            sequence: crate::features::next_request_sequence(),
            receipt: None,
        }
    }
    pub fn tracked(mut self) -> Self {
        self.receipt = Some(SettingsRequestId(self.sequence));
        self
    }
}

#[derive(Resource, Default)]
pub struct SettingsResults(pub HashMap<SettingsRequestId, Result<u64, String>>);
impl SettingsResults {
    pub fn take(&mut self, id: SettingsRequestId) -> Option<Result<u64, String>> {
        self.0.remove(&id)
    }
}

#[derive(Clone, Debug)]
pub struct SettingsSection {
    pub id: &'static str,
    pub plugin: &'static str,
    pub label: &'static str,
    pub draw: Option<fn(&Ui, &mut Value) -> bool>,
}
