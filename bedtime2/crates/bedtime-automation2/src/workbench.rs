use crate::*;
use bedtime_ui2::{Theme, UsesTheme};
use bedtime_workbench2::*;

pub(crate) const OPERATIONS: &[&str] = &[
    "invoke",
    "layout.create",
    "layout.activate",
    "layout.delete",
    "panel.focus",
    "panel.close",
    "panel.move",
    "area.split",
    "area.close",
    "surface.configure",
    "surface.detach",
    "surface.attach",
    "theme.set",
];
fn layout(world: &mut World, args: &Value) -> Result<Entity, String> {
    if args.get("layout").is_some() {
        let e = entity(world, args, "layout")?;
        world.get::<Layout>(e).ok_or("Entity is not a Layout")?;
        Ok(e)
    } else {
        active_layout(world).ok_or_else(|| "No active layout".into())
    }
}
fn done(state: &State, value: Value) -> Stage {
    Stage::Complete {
        after: state.frame + 1,
        value,
    }
}

pub(crate) fn start(
    world: &mut World,
    state: &mut State,
    op: &str,
    args: &Value,
) -> Result<Stage, String> {
    if !world.contains_resource::<WorkbenchRequests>() {
        return Err("Install WorkbenchCorePlugin for workbench operations".into());
    }
    // Validate shared options before an operation can create entities.
    let _ = optional::<bool>(args, "activate")?;
    let value = match op {
        "layout.create" => {
            let name: String = required(args, "name")?;
            let layout = Workbench::new(world).create_layout(name)?;
            if optional::<bool>(args, "activate")?.unwrap_or(false) {
                Workbench::for_entity(world, layout)?.activate_layout(layout)?;
            }
            json!({"layout":entity_id(layout)})
        }
        "layout.activate" => {
            if args.get("layout").is_some_and(Value::is_null) {
                if let Some(current) = active_layout(world) {
                    world.entity_mut(current).remove::<ActiveLayout>();
                }
            } else {
                let e = layout(world, args)?;
                Workbench::for_entity(world, e)?.activate_layout(e)?;
            }
            json!({"applied":true})
        }
        "layout.delete" => {
            let e = layout(world, args)?;
            Workbench::for_entity(world, e)?.delete_layout(e)?;
            json!({"deleted":entity_id(e)})
        }
        "panel.focus" | "panel.close" => {
            let e = entity(world, args, "panel")?;
            let mut workbench = Workbench::for_entity(world, e)?;
            if op == "panel.focus" {
                workbench.focus_panel(e)?;
            } else {
                workbench.close_panel(e)?;
            }
            json!({"panel":entity_id(e)})
        }
        "panel.move" => {
            let panel = entity(world, args, "panel")?;
            let area = entity(world, args, "area")?;
            let index = optional(args, "index")?.unwrap_or(panels_in_area(world, area).len());
            let target_layout = world
                .get::<InLayout>(area)
                .ok_or("Destination area is unavailable")?
                .0;
            let mut workbench = Workbench::for_entity(world, panel)?;
            if workbench.panel_layout(panel)? != target_layout {
                workbench.relocate_panel(panel, area)?;
            }
            workbench.move_tab(panel, area, index)?;
            json!({"panel":entity_id(panel)})
        }
        "area.split" => {
            let area = entity(world, args, "area")?;
            let rect = world
                .get::<Area>(area)
                .ok_or("Entity is not an Area")?
                .rect();
            let axis: String = required(args, "axis")?;
            let (axis, index) = match axis.as_str() {
                "horizontal" => (SplitAxis::Horizontal, 0),
                "vertical" => (SplitAxis::Vertical, 1),
                _ => return Err("axis must be horizontal or vertical".into()),
            };
            let fraction: f32 = optional(args, "fraction")?.unwrap_or(0.5);
            if !fraction.is_finite() || !(0.0..1.0).contains(&fraction) {
                return Err("fraction must be between 0 and 1".into());
            }
            let at = rect.min[index]
                + ((rect.max[index] - rect.min[index]) as f32 * fraction).round() as i32;
            let next = Workbench::for_entity(world, area)?.split_area(area, axis, at, [1, 1])?;
            json!({"area":entity_id(next)})
        }
        "area.close" => {
            let e = entity(world, args, "area")?;
            Workbench::for_entity(world, e)?.close_area(e)?;
            json!({"closed":entity_id(e)})
        }
        "surface.detach" => {
            let window = window(world, args)?;
            let settings = *world
                .get::<WorkbenchSurface>(window)
                .ok_or("Window has no WorkbenchSurface")?;
            state.detached.insert(window, settings);
            world.entity_mut(window).remove::<WorkbenchSurface>();
            json!({"window":entity_id(window)})
        }
        "surface.attach" => {
            let window = window(world, args)?;
            if world.get::<WorkbenchSurface>(window).is_some() {
                return Err("WorkbenchSurface is already attached".into());
            }
            if world
                .query_filtered::<Entity, With<WorkbenchSurface>>()
                .iter(world)
                .next()
                .is_some()
            {
                return Err("Only one WorkbenchSurface is supported".into());
            }
            let cameras: Vec<_> = world
                .query_filtered::<Entity, With<WorkbenchCamera>>()
                .iter(world)
                .collect();
            if cameras.len() != 1 || world.get::<Camera>(cameras[0]).is_none() {
                return Err("Attach requires one explicit WorkbenchCamera on a Bevy camera".into());
            }
            let camera = cameras[0];
            if world
                .get::<bedtime_ui2::UiCamera>(camera)
                .is_some_and(|c| c.0 != window && c.0 != Entity::PLACEHOLDER)
            {
                return Err("WorkbenchCamera is bound to another UI surface".into());
            }
            match world.get::<bevy::camera::RenderTarget>(camera) {
                Some(bevy::camera::RenderTarget::Window(bevy::window::WindowRef::Primary)) => {}
                Some(bevy::camera::RenderTarget::Window(bevy::window::WindowRef::Entity(
                    target,
                ))) if *target == window => {}
                _ => return Err("WorkbenchCamera must target this window".into()),
            }
            let settings = state.detached.remove(&window).unwrap_or_default();
            world.entity_mut(window).insert(settings);
            json!({"window":entity_id(window)})
        }
        "surface.configure" => {
            let window = window(world, args)?;
            let current = world
                .get::<WorkbenchSurface>(window)
                .ok_or("Window has no WorkbenchSurface")?;
            let mut value = serde_json::to_value(current).unwrap();
            if let Some(patch) = args.get("settings") {
                merge(&mut value, patch)?;
            }
            let next: WorkbenchSurface =
                serde_json::from_value(value).map_err(|e| e.to_string())?;
            if [next.rounding, next.hit_size, next.panel_gap]
                .into_iter()
                .chain(next.padding)
                .any(|v| !v.is_finite() || v < 0.0)
            {
                return Err("Surface dimensions must be finite and nonnegative".into());
            }
            let height: Option<f32> = optional(args, "titlebar_height")?;
            if height.is_some_and(|h| !h.is_finite() || h <= 0.0) {
                return Err("titlebar_height must be positive".into());
            }
            if height.is_some() && world.get::<Titlebar>(window).is_none() {
                return Err("Window has no Titlebar".into());
            }
            let visible: Option<bool> = optional(args, "panels_visible")?;
            let ownership: Option<String> = optional(args, "titlebar_ownership")?;
            let ownership = ownership
                .map(|v| match v.as_str() {
                    "workbench" => Ok(TitlebarOwnership::Workbench),
                    "application" => Ok(TitlebarOwnership::Application),
                    _ => Err("Invalid titlebar_ownership".to_owned()),
                })
                .transpose()?;
            let blur: Option<bool> = optional(args, "blur")?;
            world.entity_mut(window).insert(next);
            if let Some(h) = height {
                world.get_mut::<Titlebar>(window).unwrap().height = h;
            }
            if let Some(mut shell) = world.get_mut::<WorkbenchShell>(window) {
                if let Some(v) = visible {
                    shell.panels_visible = v;
                }
                if let Some(o) = ownership {
                    shell.titlebar_ownership = o;
                }
            }
            if let Some(blur) = blur {
                world.entity_mut(window).insert(WindowBlur(blur));
            }
            json!(next)
        }
        "theme.set" => {
            let window = window(world, args)?;
            let e = world
                .get::<UsesTheme>(window)
                .ok_or("Window has no theme")?
                .0;
            let current = world.get::<Theme>(e).ok_or("Theme is missing")?;
            let mut value = serde_json::to_value(current).unwrap();
            if let Some(preset) = optional::<String>(args, "preset")? {
                value = serde_json::to_value(match preset.as_str() {
                    "dark" => bedtime_ui2::themes::dark(),
                    "light" => bedtime_ui2::themes::light(),
                    _ => return Err("Unknown theme preset".into()),
                })
                .unwrap();
            }
            if let Some(patch) = args.get("theme") {
                merge(&mut value, patch)?;
            }
            let next: Theme = serde_json::from_value(value).map_err(|e| e.to_string())?;
            next.validate()?;
            world.entity_mut(e).insert(next.clone());
            json!(next)
        }
        "invoke" => {
            let key: String = required(args, "command")?;
            let context = if args.get("panel").is_some() {
                InvocationContext::Panel(entity(world, args, "panel")?)
            } else if optional::<bool>(args, "global")?.unwrap_or(false) {
                InvocationContext::Global
            } else {
                InvocationContext::Focused
            };
            let command = world
                .query::<&ContextCommandInfo>()
                .iter(world)
                .find_map(|info| (info.command().as_str() == key).then(|| info.command()))
                .ok_or("Unregistered command")?;
            let source = world.spawn_empty().id();
            let request = ContextCommandRequest {
                command,
                source: Some(source),
                context,
            };
            world
                .resource_mut::<ContextCommandRequests>()
                .submit(request);
            return Ok(Stage::Invoke { source });
        }
        _ => return Err(format!("Unknown workbench operation: {op}")),
    };
    Ok(done(state, value))
}
fn merge(current: &mut Value, patch: &Value) -> Result<(), String> {
    let patch = patch
        .as_object()
        .ok_or("Configuration patch must be an object")?;
    let object = current
        .as_object_mut()
        .ok_or("Configuration is not an object")?;
    for (key, value) in patch {
        let slot = object
            .get_mut(key)
            .ok_or_else(|| format!("Unknown configuration field: {key}"))?;
        if slot.is_object() {
            merge(slot, value)?;
        } else {
            *slot = value.clone();
        }
    }
    Ok(())
}

pub(crate) fn inspect(world: &mut World) -> Value {
    let layouts: Vec<_> = world
        .query_filtered::<Entity, With<Layout>>()
        .iter(world)
        .collect();
    let layouts:Vec<_> = layouts.into_iter().filter_map(|layout| {
            let data = world.get::<Layout>(layout)?;
            let areas:Vec<_> = world.get::<LayoutAreas>(layout).map_or(&[][..],|a|a.entities()).iter().map(|&area| {
                let data = world.get::<Area>(area).unwrap();
                let p = world.get::<AreaPresentation>(area).copied().unwrap_or_default();
                let panels:Vec<_> = panels_in_area(world,area).iter().map(|&panel| {
                    let data = world.get::<PanelInstance>(panel).unwrap(); let p = world.get::<PanelPresentation>(panel).copied().unwrap_or_default();
                    json!({"entity":entity_id(panel),"key":data.key,"title":data.title,"visible":p.visible,"focused":p.focused,"rect":inspect::rect(p.rect),"tab_rect":inspect::rect(p.tab_rect),"close_rect":inspect::rect(p.close_rect)})
                }).collect();
                json!({"entity":entity_id(area),"rect":data.rect(),"visible":p.visible,"surface":inspect::rect(p.surface),"content":inspect::rect(p.content),"tab_bar":inspect::rect(p.tab_bar),"selected_panel":data.selected_panel().map(entity_id),"panels":panels})
            }).collect();
            Some(json!({"entity":entity_id(layout),"name":data.name,"active":world.get::<ActiveLayout>(layout).is_some(),"focused_area":data.focused_area().map(entity_id),"areas":areas}))
        }).collect();
    let surfaces:Vec<_> = world.query::<(Entity,&WorkbenchSurface)>().iter(world).map(|(e,s)| {
        let p = world.get::<WorkbenchPresentation>(e).copied().unwrap_or_default();
        json!({"window":entity_id(e),"settings":s,"titlebar_height":world.get::<Titlebar>(e).map(|t|t.height),"blur":world.get::<WindowBlur>(e).map(|b|b.0),"theme":world.get::<UsesTheme>(e).and_then(|u|world.get::<Theme>(u.0)),"panels_visible":world.get::<WorkbenchShell>(e).map(|s|s.panels_visible),"presentation":{"bounds":inspect::rect(p.bounds),"titlebar":inspect::rect(p.titlebar),"titlebar_content":inspect::rect(p.titlebar_content),"panels":inspect::rect(p.panels),"menu":inspect::rect(p.menu),"status_bar":inspect::rect(p.status_bar)}})
    }).collect();
    let commands: Vec<_> = world
        .query::<&ContextCommandInfo>()
        .iter(world)
        .map(|c| (c.command(), c.label.clone()))
        .collect();
    let commands:Vec<_> = commands.into_iter().map(|(command,label)|json!({"key":command.as_str(),"label":label,"available":command_available(world,command.clone(),InvocationContext::Focused)})).collect();
    json!({"layouts":layouts,"surfaces":surfaces,"commands":commands,"geometry_units":"egui points; multiply by ui surface zoom for input coordinates"})
}
