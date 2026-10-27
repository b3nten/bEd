//! Declarative binding layers; runtime handlers remain ordinary Workbench handlers.
use crate::persistence::ReflectGlobalPersist;
use crate::persistence::ReflectProjectPersist;
use bedtime_workbench2::*;
use bevy::prelude::*;

pub fn install(app: &mut App) {
    let world = app.world_mut();
    world.spawn(Binding::new::<SaveDocument>(KeyChord::new(
        Modifiers::PRIMARY,
        KeyCode::KeyS,
    )));
    world.spawn(Binding::new::<crate::menus::OpenFolder>(KeyChord::new(
        Modifiers::PRIMARY,
        KeyCode::KeyO,
    )));
    world.spawn(Binding::new::<crate::menus::Quit>(KeyChord::new(
        Modifiers::PRIMARY,
        KeyCode::KeyQ,
    )));
    let defaults: Vec<_> = world
        .query_filtered::<(Entity, &Binding), Without<InPanel>>()
        .iter(world)
        .map(|(e, b)| {
            (
                e,
                Declaration {
                    chord: b.chord,
                    command: Some(b.command.clone()),
                    repeat: b.repeat,
                },
            )
        })
        .collect();
    for (e, _) in &defaults {
        world.despawn(*e);
    }
    app.insert_resource(Defaults(defaults.into_iter().map(|(_, d)| d).collect()))
        .init_resource::<GlobalBindings>()
        .init_resource::<ProjectBindings>()
        .init_resource::<Applied>()
        .add_systems(
            PreUpdate,
            apply
                .before(bedtime_ui2::UiSystems::Draw)
                .before(ContextCommandSystems::CollectInput),
        );
}

use serde::{Deserialize, Serialize};

/// Missing chord inherits; a present chord with no command explicitly unbinds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    pub chord: KeyChord,
    pub command: Option<ContextCommandId>,
    #[serde(default)]
    pub repeat: bool,
}

#[derive(Resource, Reflect, Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(opaque)]
#[reflect(Resource, Serialize, Deserialize, GlobalPersist)]
pub struct GlobalBindings(pub Vec<Declaration>);

#[derive(Resource, Reflect, Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(opaque)]
#[reflect(Resource, Serialize, Deserialize, ProjectPersist)]
pub struct ProjectBindings(pub Vec<Declaration>);

#[derive(Resource, Default)]
struct Defaults(Vec<Declaration>);

#[derive(Resource, Default)]
struct Applied(Option<(GlobalBindings, ProjectBindings)>);

#[derive(Component)]
struct ResolvedBinding;
fn chord_identity(chord: KeyChord) -> (Modifiers, KeyCode) {
    (chord.modifiers.resolved(), chord.key)
}

pub fn validate(values: &[Declaration]) -> Result<(), String> {
    if values.len() > 256 {
        return Err("Too many binding declarations".into());
    }
    let mut seen = std::collections::HashSet::new();
    for value in values {
        if !value.chord.modifiers.is_valid() {
            return Err("Invalid chord modifiers".into());
        }
        if !seen.insert(chord_identity(value.chord)) {
            return Err("Duplicate binding chord".into());
        }
        if let Some(command) = &value.command {
            let key = command.as_str();
            if key.len() > 256
                || !key.contains('.')
                || key.split('.').any(|part| {
                    part.is_empty()
                        || !part
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                })
            {
                return Err("Invalid binding command identity".into());
            }
        }
    }
    Ok(())
}

pub fn merge(
    baseline: &[Declaration],
    live: &[Declaration],
    disk: &[Declaration],
) -> Vec<Declaration> {
    let mut result = disk.to_vec();
    let mut keys = Vec::new();
    for declaration in baseline.iter().chain(live) {
        let key = chord_identity(declaration.chord);
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    for key in keys {
        let before = baseline.iter().find(|d| chord_identity(d.chord) == key);
        let after = live.iter().find(|d| chord_identity(d.chord) == key);
        if before != after {
            result.retain(|d| chord_identity(d.chord) != key);
            if let Some(after) = after {
                result.push(after.clone());
            }
        }
    }
    result
}

fn resolve(
    defaults: &[Declaration],
    global: &[Declaration],
    project: &[Declaration],
) -> Vec<Declaration> {
    let mut result = defaults.to_vec();
    for layer in [global, project] {
        for value in layer {
            result.retain(|d| chord_identity(d.chord) != chord_identity(value.chord));
            result.push(value.clone());
        }
    }
    result
}

fn apply(world: &mut World) {
    let global = world.resource::<GlobalBindings>().clone();
    let project = world.resource::<ProjectBindings>().clone();
    if world.resource::<Applied>().0.as_ref() == Some(&(global.clone(), project.clone())) {
        return;
    }
    let values = resolve(&world.resource::<Defaults>().0, &global.0, &project.0);
    let old: Vec<_> = world
        .query_filtered::<Entity, With<ResolvedBinding>>()
        .iter(world)
        .collect();
    for entity in old {
        world.despawn(entity);
    }
    for value in values {
        if let Some(command) = value.command {
            world.spawn((
                Binding {
                    command,
                    chord: value.chord,
                    repeat: value.repeat,
                },
                ResolvedBinding,
            ));
        }
    }
    world.resource_mut::<Applied>().0 = Some((global, project));
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn equivalent_primary_chords_merge_as_one_override() {
        let chord = KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyS);
        let baseline = vec![Declaration {
            chord,
            command: Some(ContextCommandId::of::<SaveDocument>()),
            repeat: false,
        }];
        let live = vec![Declaration {
            chord: KeyChord::new(Modifiers::PRIMARY.resolved(), KeyCode::KeyS),
            command: None,
            repeat: false,
        }];
        let merged = merge(&baseline, &live, &baseline);
        assert_eq!(merged, live);
        validate(&merged).unwrap();
        assert!(validate(&[baseline[0].clone(), live[0].clone()]).is_err());
    }
    
    #[test]
    fn declarations_preserve_inherit_override_and_explicit_unbind() {
        let chord = KeyChord::new(Modifiers::PRIMARY, KeyCode::KeyS);
        let defaults = vec![Declaration {
            chord,
            command: Some(ContextCommandId::of::<SaveDocument>()),
            repeat: false,
        }];
        let unbind = vec![Declaration {
            chord,
            command: None,
            repeat: false,
        }];
        assert!(resolve(&defaults, &unbind, &[])[0].command.is_none());
        assert!(resolve(&defaults, &unbind, &defaults)[0].command.is_some());
        assert_eq!(resolve(&defaults, &[], &[]), defaults);
        assert_eq!(merge(&defaults, &[], &defaults), Vec::<Declaration>::new());
        assert!(validate(&[defaults[0].clone(), defaults[0].clone()]).is_err());
    }
}
