//! Bevy owns color data and commits requests after the frontend returns.
use super::{ColorDrafts, ColorState, MODULE_ID, OPEN_COMMAND, PANEL_ID, color, read_color, ui};
use bed_editor_api::{EditorFrame, EditorIntents, HostRequest};
use bed_workbench_api::{
    Action, CommandInvocation, CommandResult, Panel, PanelCreation, PanelResult, register,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;
use serde_json::{Value, json};

pub const APPLY_COMMAND: &str = "bed.color.apply";

/// Optional palette storage for applications without an editor settings resource.
/// An editor's `EditorFrame` and settings reducer remain the palette's owner.
#[derive(Resource, Default)]
pub struct ColorPalette(pub Vec<Value>);

pub struct ColorPlugin;

impl Plugin for ColorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<bed_editor_api::Registry>();
        app.world_mut()
            .resource_mut::<bed_editor_api::Registry>()
            .register_with(MODULE_ID, |registrar| {
                registrar.panel_options(
                    PANEL_ID,
                    "Color Picker",
                    true,
                    bed_editor_api::PanelPlacement::Center,
                    None,
                );
                registrar.command(OPEN_COMMAND, "Color Picker", Some("image"));
                registrar.menu(bed_editor_api::MenuSlot::Application, OPEN_COMMAND);
            })
            .expect("feature metadata is installed once");
        register(app.world_mut(), MODULE_ID, |registrar| {
            registrar.panel(PANEL_ID, "Color Picker", create_panel);
            registrar.command(OPEN_COMMAND, "Color Picker", open_panel);
            registrar.command(APPLY_COMMAND, "Apply color", apply_color);
            registrar.menu(OPEN_COMMAND);
        })
        .expect("ColorPlugin must be installed once");
    }
}

fn open_panel(In(invocation): In<CommandInvocation>) -> CommandResult {
    Ok(vec![Action::OpenPanel {
        panel_type: PANEL_ID.into(),
        state: Value::Null,
        area: Some(invocation.area),
    }])
}

fn create_panel(In(creation): In<PanelCreation>, world: &mut World) -> PanelResult {
    world.entity_mut(creation.entity).insert(bed_workbench_api::PanelDocumentation::new(&[
        ("Color", "Choose a color with the picker or edit its numeric values. The preview includes opacity."),
        ("Palette", "Use the value outputs and swatches to copy colors and keep a reusable palette."),
    ]));
    let state = ColorState::restore(&creation.state);
    let rgba = state.rgba;
    world.entity_mut(creation.entity).insert(state);
    Ok(Box::new(ColorFrontend {
        entity: creation.entity,
        drafts: ColorDrafts::new(rgba),
        observed_rgba: rgba,
    }))
}

/// Only widget drafts and the entity association live outside the ECS.
pub struct ColorFrontend {
    pub entity: Entity,
    drafts: ColorDrafts,
    observed_rgba: [f32; 4],
}

impl Panel for ColorFrontend {
    fn title(&self, _: &World) -> String {
        "Color Picker".into()
    }

    fn draw(
        &mut self,
        ui: &Ui,
        world: &mut World,
        requests: &mut Vec<Action>,
        _shortcuts: &bed_workbench_api::ShortcutInput,
    ) {
        let original = world
            .get::<ColorState>(self.entity)
            .expect("a live ColorFrontend has ColorState");
        if original.rgba != self.observed_rgba {
            self.drafts.fields = color::outputs(original.rgba);
            self.drafts.error = None;
            self.drafts.copied = None;
        }
        let mut state = original.clone();
        let mut swatches = palette(world);
        let palette_changed = ui::draw_controls(ui, &mut state, &mut self.drafts, &mut swatches);
        if &state != original || palette_changed {
            requests.push(Action::Invoke(CommandInvocation {
                command: APPLY_COMMAND.into(),
                source: Some(self.entity),
                // This command changes an existing entity and never chooses an area.
                area: 0,
                arguments: json!({
                    "rgba":state.rgba,"recent":state.recent,
                    "name":state.name,"selected":state.selected,
                    "swatches":palette_changed.then_some(swatches),
                }),
            }));
        }
        self.observed_rgba = state.rgba;
    }

    fn save(&self, world: &World) -> Value {
        world
            .get::<ColorState>(self.entity)
            .expect("a live ColorFrontend has ColorState")
            .save()
    }
}

fn palette(world: &World) -> Vec<Value> {
    if let Some(frame) = world.get_resource::<EditorFrame>() {
        frame.context().settings_for(MODULE_ID)["swatches"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    } else {
        world
            .get_resource::<ColorPalette>()
            .map(|palette| palette.0.clone())
            .unwrap_or_default()
    }
}

fn apply_color(In(invocation): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let entity = invocation.source.ok_or("Color change needs a panel")?;
    if world.get::<ColorState>(entity).is_none() {
        return Err("Color panel is no longer open".into());
    }
    let value = &invocation.arguments;
    let rgba = read_color(&value["rgba"]).ok_or("Invalid RGBA color")?;
    let recent = value["recent"].as_array().ok_or("Invalid recent colors")?;
    if recent.len() > 16 {
        return Err("At most 16 recent colors are supported".into());
    }
    let recent = recent
        .iter()
        .map(|value| read_color(value).ok_or_else(|| "Invalid recent color".to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let name = value["name"].as_str().ok_or("Invalid swatch name")?.into();
    let selected = if value["selected"].is_null() {
        None
    } else {
        Some(
            value["selected"]
                .as_u64()
                .and_then(|index| usize::try_from(index).ok())
                .ok_or("Invalid swatch selection")?,
        )
    };
    let swatches = if value["swatches"].is_null() {
        None
    } else {
        // Retain saved entries and indices; the controls skip invalid colors.
        Some(
            value["swatches"]
                .as_array()
                .ok_or("Invalid palette")?
                .clone(),
        )
    };
    world.entity_mut(entity).insert(ColorState {
        rgba,
        recent,
        name,
        selected,
    });
    if let Some(swatches) = swatches {
        if world.contains_resource::<EditorFrame>() {
            world.init_resource::<EditorIntents>();
            world.resource_mut::<EditorIntents>().push(
                Some(entity),
                HostRequest::SetSetting {
                    plugin: MODULE_ID.into(),
                    key: "swatches".into(),
                    value: Value::Array(swatches),
                },
            );
        } else {
            world.insert_resource(ColorPalette(swatches));
        }
    }
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_workbench::{
        WorkbenchLayout, WorkbenchPlugin, WorkbenchRequests, capture_command, panel_state,
    };
    use bed_workbench_api::Registry;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((WorkbenchPlugin, ColorPlugin));
        app
    }

    fn open(app: &mut App) -> Entity {
        let invocation = capture_command(app.world(), OPEN_COMMAND, Value::Null);
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(Action::Invoke(invocation));
        app.update();
        assert!(
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .take_errors()
                .is_empty()
        );
        app.world().resource::<WorkbenchLayout>().focused.unwrap()
    }

    fn apply(app: &mut App, entity: Entity, arguments: Value) -> CommandResult {
        let handler = app
            .world()
            .resource::<Registry>()
            .command(APPLY_COMMAND)
            .unwrap()
            .handler;
        app.world_mut()
            .run_system_with(
                handler,
                CommandInvocation {
                    command: APPLY_COMMAND.into(),
                    source: Some(entity),
                    area: 0,
                    arguments,
                },
            )
            .unwrap()
    }

    #[test]
    fn plugin_opens_independent_entities_and_restores_existing_saved_colors() {
        let mut app = app();
        let first = open(&mut app);
        let second = open(&mut app);
        assert_ne!(first, second);
        let second_before = app.world().get::<ColorState>(second).unwrap().clone();
        let rgba = [0.25, 0.5, 0.75, 0.5];
        let recent = [[1.0, 0.0, 0.0, 1.0], rgba];
        let arguments = json!({"rgba":rgba,"recent":recent,"name":"Draft name","selected":2});
        // Registered command execution, rather than frontend storage, owns the change.
        let invocation = CommandInvocation {
            command: APPLY_COMMAND.into(),
            source: Some(first),
            area: 0,
            arguments,
        };
        app.world_mut()
            .resource_mut::<WorkbenchRequests>()
            .push(Action::Invoke(invocation));
        assert_ne!(app.world().get::<ColorState>(first).unwrap().rgba, rgba);
        app.update();
        assert_eq!(app.world().get::<ColorState>(first).unwrap().rgba, rgba);
        assert_eq!(
            app.world().get::<ColorState>(second).unwrap(),
            &second_before
        );
        assert_eq!(
            panel_state(app.world_mut(), first).unwrap(),
            json!({"rgba":rgba,"recent":recent})
        );
        let saved = bed_workbench::save(app.world_mut());
        bed_workbench::restore(app.world_mut(), &saved).unwrap();
        assert!(app.world().get_entity(first).is_err());
        assert!(app.world().get_entity(second).is_err());
        assert_eq!(bed_workbench::save(app.world_mut()), saved);
        let states = app
            .world_mut()
            .query::<&ColorState>()
            .iter(app.world())
            .collect::<Vec<_>>();
        assert_eq!(states.len(), 2);
        assert!(states.iter().any(|state| state.rgba == rgba));
        // Widget selection and name retain the original persistence semantics.
        assert!(
            states
                .iter()
                .all(|state| state.name.is_empty() && state.selected.is_none())
        );
    }

    #[test]
    fn invalid_color_commands_leave_state_and_palette_unchanged() {
        let mut app = app();
        let entity = open(&mut app);
        let original = app.world().get::<ColorState>(entity).unwrap().clone();
        for invalid in [
            json!({"rgba":[2.0,0.0,0.0,1.0],"recent":[],"name":"","selected":null}),
            json!({"rgba":[0.0,0.0,0.0,1.0],"recent":[[0.0,0.0,0.0,-1.0]],"name":"","selected":null}),
            json!({"rgba":[0.0,0.0,0.0,1.0],"recent":vec![[0.0,0.0,0.0,1.0];17],"name":"","selected":null}),
        ] {
            let mut invalid = invalid;
            invalid["swatches"] = json!([{"name":"Must not persist","rgba":[1.0,0.0,0.0,1.0]}]);
            assert!(apply(&mut app, entity, invalid).is_err());
            assert_eq!(app.world().get::<ColorState>(entity).unwrap(), &original);
            assert!(!app.world().contains_resource::<ColorPalette>());
            assert!(!app.world().contains_resource::<EditorIntents>());
        }
        app.world_mut().despawn(entity);
        assert!(apply(&mut app, entity, Value::Null).is_err());
    }

    #[test]
    fn palettes_use_editor_settings_or_standalone_storage_without_duplication() {
        let mut app = app();
        let entity = open(&mut app);
        let swatches = json!([{"name":"Favorite","rgba":[0.0,1.0,0.0,1.0]}]);
        let change = json!({"rgba":[0.0,1.0,0.0,1.0],"recent":[],"name":"Favorite","selected":0,"swatches":swatches});
        apply(&mut app, entity, change.clone()).unwrap();
        assert_eq!(Value::Array(palette(app.world())), swatches);
        assert!(app.world_mut().remove_resource::<ColorPalette>().is_some());
        let settings = json!({MODULE_ID:{"swatches":[]}});
        app.insert_resource(EditorFrame {
            settings: settings.clone(),
            ..Default::default()
        });
        apply(&mut app, entity, change).unwrap();
        assert!(!app.world().contains_resource::<ColorPalette>());
        assert_eq!(app.world().resource::<EditorFrame>().settings, settings);
        let intents = app.world_mut().resource_mut::<EditorIntents>().take();
        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].source, Some(entity));
        assert!(matches!(&intents[0].request,
            HostRequest::SetSetting {plugin,key,value}
                if plugin == MODULE_ID && key == "swatches" && value == &swatches));
    }

    #[test]
    fn frontend_queues_changes_and_refreshes_drafts_after_an_external_command() {
        use dear_imgui_rs::{Condition, Context, FontSource, FramePrepareOptions, MouseButton};

        fn frame(
            context: &mut Context,
            frontend: &mut ColorFrontend,
            world: &mut World,
        ) -> Vec<Action> {
            context.prepare_frame(FramePrepareOptions::new([1000.0; 2], 1.0 / 60.0));
            let ui = context.frame();
            let mut requests = Vec::new();
            ui.window("Color command fixture")
                .position([0.0, 0.0], Condition::Always)
                .size([1000.0; 2], Condition::Always)
                .build(|| {
                    frontend.draw(
                        ui,
                        world,
                        &mut requests,
                        &bed_workbench_api::ShortcutInput::default(),
                    )
                });
            drop(context.render_legacy());
            requests
        }

        let _context_lock = crate::COLOR_TEST_CONTEXT_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .add_font(&[FontSource::default_font_with_size(20.0)]);
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut app = app();
        let entity = open(&mut app);
        let rgba = app.world().get::<ColorState>(entity).unwrap().rgba;
        let mut frontend = ColorFrontend {
            entity,
            drafts: ColorDrafts::new(rgba),
            observed_rgba: rgba,
        };
        assert!(frame(&mut context, &mut frontend, app.world_mut()).is_empty());
        context
            .io_mut()
            .add_mouse_pos_event(frontend.drafts.copy_points[0]);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        let mut requests = frame(&mut context, &mut frontend, app.world_mut());
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, false);
        requests.extend(frame(&mut context, &mut frontend, app.world_mut()));
        assert_eq!(requests.len(), 1);
        assert!(
            app.world()
                .get::<ColorState>(entity)
                .unwrap()
                .recent
                .is_empty()
        );
        for request in requests {
            app.world_mut()
                .resource_mut::<WorkbenchRequests>()
                .push(request);
        }
        app.update();
        assert_eq!(
            app.world().get::<ColorState>(entity).unwrap().recent,
            vec![rgba]
        );

        // Incomplete text is local and survives unchanged ECS data.
        frontend.drafts.fields[0] = "#12".into();
        assert!(frame(&mut context, &mut frontend, app.world_mut()).is_empty());
        assert_eq!(frontend.drafts.fields[0], "#12");
        let red = [1.0, 0.0, 0.0, 1.0];
        apply(
            &mut app,
            entity,
            json!({"rgba":red,"recent":[],"name":"","selected":null}),
        )
        .unwrap();
        assert!(frame(&mut context, &mut frontend, app.world_mut()).is_empty());
        assert_eq!(frontend.drafts.fields, color::outputs(red));
    }
}
