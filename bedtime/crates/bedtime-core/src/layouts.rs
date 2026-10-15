//! Document integration for the content-independent retained-layout controller.
use crate::{DocumentNavigation, active_panel_entities};
use bedtime_api::documents::PanelBinding;
pub use bedtime_plugin_layouts;
pub use bedtime_plugin_layouts::{
    LayoutDefinition, LayoutIntegration, LayoutKey, LayoutSlots, LiveLayout, PanelDefinition,
};
use bedtime_workbench::{PanelRecord, WorkbenchLayout};
use bevy::prelude::*;
use serde_json::{Value, json};
use std::io;

/// Adopt a saved scope while carrying live panels. Applications select the scope
/// and remove their launcher tools first. Retained instances take precedence;
/// additional carried tabs join the largest area without recreating frontends.
pub fn attach_retained<Key: LayoutKey>(
    world: &mut World,
    mut live: std::collections::HashMap<Key, LiveLayout<DocumentNavigation>>,
    fallback: &Value,
    preserved_terminal: Option<Entity>,
) -> io::Result<()> {
    let slot = world
        .resource::<LayoutSlots<Key, DocumentNavigation>>()
        .active();
    if let Some(target) = live.remove(&slot) {
        bedtime_plugin_layouts::activate(world, &target);
        DocumentLayouts::activate_context(world, Some(&target.context))?;
        let ids = active_panel_entities(world)
            .into_iter()
            .map(|panel| {
                world
                    .get::<bedtime_workbench::PanelPresentation>(panel)
                    .unwrap()
                    .id
            })
            .collect::<Vec<_>>();
        let mut tiling = world.resource::<WorkbenchLayout>().tiling.clone();
        let destination = tiling.layout.largest_area();
        for &id in &ids {
            if tiling.area_for(id).is_none() {
                tiling.assign(id, destination);
            }
        }
        bedtime_workbench::apply_layout(world, &tiling.to_value(ids)).map_err(io::Error::other)?;
    } else {
        let value = world
            .resource::<LayoutSlots<Key, DocumentNavigation>>()
            .saved()
            .get(&slot.saved_key())
            .cloned()
            .unwrap_or_else(|| fallback.clone());
        crate::restore_document_layout(world, &value, true, preserved_terminal)
            .map_err(io::Error::other)?;
    }
    world
        .resource_mut::<LayoutSlots<Key, DocumentNavigation>>()
        .replace_live(live);
    Ok(())
}

pub struct DocumentLayouts;
/// Install after the document host to drive the application's chosen layouts.
pub type DocumentLayoutsPlugin<Key> =
    bedtime_plugin_layouts::LayoutControllerPlugin<Key, DocumentLayouts>;
impl LayoutIntegration for DocumentLayouts {
    type Context = DocumentNavigation;
    fn capture_context(world: &World) -> DocumentNavigation {
        world.resource::<DocumentNavigation>().clone()
    }
    fn activate_context(world: &mut World, context: Option<&DocumentNavigation>) -> io::Result<()> {
        *world.resource_mut::<DocumentNavigation>() = context.cloned().unwrap_or_default();
        if context.is_some()
            && let Some(panel) = world.resource::<WorkbenchLayout>().focused
        {
            crate::focus_panel(world, panel, true).map_err(io::Error::other)?;
        }
        Ok(())
    }
    fn restore(world: &mut World, value: &Value) -> io::Result<()> {
        crate::restore_document_layout(world, value, false, None).map_err(io::Error::other)
    }
    fn retire(world: &mut World, panels: &[Entity]) -> io::Result<()> {
        crate::retire_panels(world, panels).map_err(io::Error::other)
    }
    fn ready(world: &World) -> bool {
        !crate::host_requests_pending(world)
    }
    fn prepare(world: &mut World) -> io::Result<()> {
        let panels = active_panel_entities(world);
        let documents = world
            .query::<(
                Entity,
                Option<&PanelBinding>,
                Option<&crate::models::DocumentBinding>,
            )>()
            .iter(world)
            .filter_map(|(entity, text, model)| {
                let owner = bedtime_api::workbench::panel_owner(world, entity).unwrap_or(entity);
                panels
                    .contains(&owner)
                    .then(|| {
                        text.and_then(|binding| binding.document)
                            .or_else(|| model.map(|binding| binding.0))
                    })
                    .flatten()
            })
            .collect::<Vec<_>>();
        crate::commit_document_drafts(world, &documents).map_err(io::Error::other)?;
        bedtime_plugin_layouts::persist(world)
    }
    fn describe(world: &World, panel: Entity) -> PanelDefinition {
        let mut spec = PanelDefinition::new(
            world
                .get::<PanelRecord>(panel)
                .expect("owned panel")
                .panel_type
                .clone(),
        );
        if let Some(viewer) = world
            .get::<PanelBinding>(panel)
            .and_then(|binding| binding.viewer.as_ref())
        {
            spec = spec.viewer(viewer.clone());
        }
        spec
    }
    fn create(world: &mut World, spec: &PanelDefinition) -> io::Result<Entity> {
        let mut definition = LayoutDefinition::new(Default::default());
        definition.add_panel(spec.clone(), 1)?;
        let before = active_panel_entities(world);
        crate::restore_document_layout(world, &definition_snapshot(&definition), true, None)
            .map_err(io::Error::other)?;
        Ok(active_panel_entities(world)
            .into_iter()
            .find(|panel| !before.contains(panel))
            .expect("restoring one definition creates one panel"))
    }
    fn focus(world: &mut World, panel: Entity) -> io::Result<()> {
        crate::focus_panel(world, panel, true).map_err(io::Error::other)
    }
}

/// A definition starts fresh inputs. A document snapshot instead restores its
/// saved bindings and navigation through `restore_document_layout`.
pub fn definition_snapshot(definition: &LayoutDefinition) -> Value {
    let mut value = definition.snapshot();
    value["version"] = json!(2);
    for panel in value["panels"].as_array_mut().expect("definition panels") {
        panel["default_input"] = json!(true);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DocumentBinding, DocumentModel};
    use bedtime_api::workbench::{
        Panel, PanelAction, PanelActionInvocation, PanelActionResult, PanelActions, PanelCreation,
        PanelResult, ShortcutInput, WorkbenchAction,
    };
    use dear_imgui_rs::Ui;
    #[derive(Clone)]
    struct Counter(u32);
    impl DocumentModel for Counter {
        type Change = u32;
        fn apply(&mut self, value: &u32) -> Result<u32, String> {
            Ok(std::mem::replace(&mut self.0, *value))
        }
        fn encode(&self) -> Result<Vec<u8>, String> {
            Ok(self.0.to_string().into_bytes())
        }
    }
    #[derive(Resource)]
    struct Document(crate::bedtime_documents::DocumentId);
    #[derive(Component)]
    struct Draft {
        value: u32,
        reject: bool,
    }
    struct ModelPanel;
    impl Panel for ModelPanel {
        fn title(&self, _: &World) -> String {
            "Model".into()
        }
        fn draw(&mut self, _: &Ui, _: &mut World, _: &mut Vec<WorkbenchAction>, _: &ShortcutInput) {
        }
    }
    fn commit(In(input): In<PanelActionInvocation>, world: &mut World) -> PanelActionResult {
        if !matches!(input.action, PanelAction::CommitEdit) {
            return Ok(false);
        }
        let draft = world.get::<Draft>(input.entity).unwrap();
        if draft.reject {
            return Err("Unfinished model draft".into());
        }
        let value = draft.value;
        let document = world.get::<DocumentBinding>(input.entity).unwrap().0;
        let revision = crate::models::model_document(world, document)
            .unwrap()
            .revision();
        crate::models::edit_model::<Counter>(
            world,
            document,
            revision,
            value,
            "Commit model draft",
        )?;
        Ok(true)
    }
    #[test]
    fn model_only_panels_commit_before_switching_and_rejected_drafts_keep_the_layout_live() {
        use bedtime_plugin_layouts::{LayoutId, transition};
        let mut app = App::new();
        app.add_plugins((
            crate::DocumentHostPlugin,
            DocumentLayoutsPlugin::new(LayoutId::default()),
        ));
        let document =
            crate::models::create_model_document(app.world_mut(), Counter(0), "Counter", None);
        app.insert_resource(Document(document));
        bedtime_workbench::register(app.world_mut(), "fixture.model", |registrar| {
            registrar.panel(
                "fixture.model.panel",
                "Model",
                |In(input): In<PanelCreation>, world: &mut World| -> PanelResult {
                    let document = world.resource::<Document>().0;
                    let action = world.register_system(commit);
                    world.entity_mut(input.entity).insert((
                        DocumentBinding(document),
                        Draft {
                            value: 9,
                            reject: false,
                        },
                        PanelActions(action),
                    ));
                    Ok(Box::new(ModelPanel))
                },
            );
        })
        .unwrap();
        let panel = crate::open_panel(
            app.world_mut(),
            &bedtime_api::PanelTypeId::new("fixture.model.panel"),
            bedtime_api::documents::PanelInput::None,
            Value::Null,
            None,
            None,
            false,
        )
        .unwrap();
        assert!(
            app.world()
                .get::<PanelBinding>(panel)
                .and_then(|binding| binding.document)
                .is_none()
        );
        let empty = definition_snapshot(&LayoutDefinition::new(Default::default()));
        transition::<LayoutId, DocumentLayouts>(app.world_mut(), LayoutId::new(2).unwrap(), &empty)
            .unwrap();
        assert_eq!(
            crate::models::model::<Counter>(app.world(), document)
                .unwrap()
                .0,
            9
        );
        assert!(app.world().entities().contains(panel));
        transition::<LayoutId, DocumentLayouts>(app.world_mut(), LayoutId::default(), &empty)
            .unwrap();
        let frontend = crate::panel_presentation(app.world(), panel).unwrap();
        app.world_mut().get_mut::<Draft>(frontend).unwrap().reject = true;
        let geometry = app.world().resource::<WorkbenchLayout>().clone();
        assert_eq!(
            transition::<LayoutId, DocumentLayouts>(
                app.world_mut(),
                LayoutId::new(2).unwrap(),
                &empty
            )
            .unwrap_err()
            .to_string(),
            "Unfinished model draft"
        );
        assert_eq!(app.world().resource::<WorkbenchLayout>(), &geometry);
        assert_eq!(
            app.world()
                .resource::<LayoutSlots<LayoutId, DocumentNavigation>>()
                .active(),
            LayoutId::default()
        );
        assert!(bedtime_api::workbench::panel_active(app.world(), panel));
        assert_eq!(
            crate::models::model::<Counter>(app.world(), document)
                .unwrap()
                .0,
            9
        );
    }
}
