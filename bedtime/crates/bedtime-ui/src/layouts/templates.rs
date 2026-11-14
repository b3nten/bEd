//! Layout templates: one layout's areas and panels, saved apart from the
//! workbench, to build new layouts from (an app's default layouts, say).
//!
//! A template holds what whole-workbench saves hold for that layout: persisted
//! components of the layout, its areas and their panels. It leaves out which
//! layout is active and the workbench's layout order, and any panel that
//! refers to an entity outside the layout (a document, say): such a panel
//! belongs to the data of one workspace, not to an arrangement.
use super::persistence::{leave_out_panels, leave_out_unsaved_panels};
use super::*;
use bedtime_persistence::{ReflectPersist, snapshot, snapshot::value};
use bevy_world_serialization::DynamicWorldBuilder;
use std::collections::HashSet;

/// One layout's arrangement and panels, as RON. Saved inside other RON (an
/// app's persisted resource), it's written as nested RON, not as a string.
#[derive(Clone, Debug, PartialEq, Eq, Reflect, serde::Serialize, serde::Deserialize)]
#[reflect(opaque, Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct LayoutTemplate(Box<ron::value::RawValue>);

impl LayoutTemplate {
    /// A template from RON text; it must at least be valid RON.
    pub fn from_ron(text: String) -> Result<Self, String> {
        ron::value::RawValue::from_boxed_ron(text.into_boxed_str())
            .map(Self)
            .map_err(|error| error.to_string())
    }

    pub fn as_str(&self) -> &str {
        self.0.get_ron()
    }
}

impl Layouts {
    /// Save `layout`'s areas and panels as a template.
    pub fn capture_layout(&mut self, layout: Entity) -> Result<LayoutTemplate, String> {
        let world = &mut self.world;
        world.get::<Layout>(layout).ok_or("That isn't a layout")?;
        let mut entities = vec![layout];
        let areas = world
            .get::<LayoutAreas>(layout)
            .map(|areas| areas.entities().to_vec())
            .unwrap_or_default();
        for area in areas {
            entities.push(area);
            entities.extend_from_slice(world.layouts().panels_in_area(area));
        }
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let components = snapshot::persisted_components(&registry).deny::<ActiveLayout>();
        let mut dynamic = DynamicWorldBuilder::from_world(world, &registry)
            .with_component_filter(components)
            .extract_entities(entities.into_iter())
            .build();
        dynamic
            .entities
            .retain(|entity| !entity.components.is_empty());
        leave_out_unsaved_panels(&mut dynamic);
        let captured: HashSet<_> = dynamic.entities.iter().map(|e| e.entity).collect();
        let elsewhere: HashSet<_> = dynamic
            .entities
            .iter()
            .filter(|entity| value::<InArea>(entity).is_some())
            .filter(|entity| {
                entity
                    .components
                    .iter()
                    .any(|value| snapshot::refers_outside(value.as_ref(), &registry, &captured))
            })
            .map(|entity| entity.entity)
            .collect();
        leave_out_panels(&mut dynamic, &elsewhere);
        snapshot::check_references(&dynamic, &registry)?;
        snapshot::serialize(dynamic, &registry).and_then(LayoutTemplate::from_ron)
    }

    /// Build a new layout from `template`, after the existing layouts. It isn't
    /// made active. On failure nothing is added.
    pub fn instantiate_layout(&mut self, template: &LayoutTemplate) -> Result<Entity, String> {
        let world = &mut self.world;
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let dynamic = snapshot::parse(template.as_str(), &registry)?;
        if !dynamic.resources.is_empty() {
            return Err("A layout template can't hold resources".into());
        }
        let layouts: Vec<_> = dynamic
            .entities
            .iter()
            .filter(|entity| value::<Layout>(entity).is_some())
            .map(|entity| entity.entity)
            .collect();
        let [layout] = layouts[..] else {
            return Err(format!(
                "A layout template holds one layout, not {}",
                layouts.len()
            ));
        };
        for value in dynamic
            .entities
            .iter()
            .flat_map(|entity| &entity.components)
        {
            let persisted = value
                .get_represented_type_info()
                .and_then(|info| registry.get(info.type_id()))
                .is_some_and(|registration| registration.data::<ReflectPersist>().is_some());
            if !persisted || value.represents::<ActiveLayout>() {
                return Err(format!(
                    "{} can't be in a layout template",
                    value.reflect_type_path()
                ));
            }
        }
        let map = snapshot::write(world, &dynamic, &registry)?;
        Ok(map[&layout])
    }
}
