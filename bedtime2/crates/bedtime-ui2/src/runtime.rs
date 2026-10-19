use crate::*;
use bevy::ecs::relationship::RelationshipTarget;
use bevy_egui::{EguiContext, EguiContextSettings, EguiFullOutput, EguiInput};
use std::{collections::HashMap, time::Instant};

#[derive(Resource, Default)]
pub(crate) struct Runtime {
    icons: Mutex<icon_cache::IconCache>,
    themes: HashMap<Entity, Theme>,
}

#[derive(Component)]
struct CameraState {
    clear_color: ClearColorConfig,
    output_mode: bevy::camera::CameraOutputMode,
    compositing: Option<bevy::camera::CompositingSpace>,
    settings: Option<EguiContextSettings>,
}

pub(crate) fn remove_camera(
    mut world: bevy::ecs::world::DeferredWorld,
    context: bevy::ecs::lifecycle::HookContext,
) {
    let entity = context.entity;
    // Clear output before despawning the camera can drop pending texture deltas.
    if let Some(mut output) = world.get_mut::<EguiFullOutput>(entity) {
        if let Some(output) = (**output).as_mut() {
            output.textures_delta.clear();
        }
        **output = None;
    }
    if let Some(mut output) = world.get_mut::<bevy_egui::EguiRenderOutput>(entity) {
        output.paint_jobs.clear();
        output.textures_delta.clear();
    }
    if let Some(mut textures) = world.get_resource_mut::<bevy_egui::EguiManagedTextures>() {
        textures.retain(|(camera, _), _| *camera != entity);
    }
    if let Some(mut runtime) = world.get_resource_mut::<Runtime>() {
        runtime.themes.remove(&entity);
    }
    world.commands().queue(move |world: &mut World| {
        let Ok(mut camera) = world.get_entity_mut(entity) else {
            return;
        };
        if let Some(state) = camera.take::<CameraState>() {
            if let Some(mut camera) = world.get_mut::<Camera>(entity) {
                camera.clear_color = state.clear_color;
                camera.output_mode = state.output_mode;
            }
            if let Some(compositing) = state.compositing {
                world.entity_mut(entity).insert(compositing);
            } else {
                world
                    .entity_mut(entity)
                    .remove::<bevy::camera::CompositingSpace>();
            }
            if let Some(settings) = state.settings {
                world.entity_mut(entity).insert(settings);
            } else {
                world.entity_mut(entity).remove::<EguiContextSettings>();
            }
        }
        // End this manual egui session so upstream does not keep processing a
        // detached context. Reattaching UiCamera creates a fresh context and
        // uploads its complete font atlas and textures again.
        world.entity_mut(entity).remove::<EguiContext>();
    });
}

pub(crate) fn configure(world: &mut World) {
    let cameras: Vec<_> = world
        .query_filtered::<Entity, Added<UiCamera>>()
        .iter(world)
        .collect();
    for camera in cameras {
        let settings = world
            .get::<Camera>(camera)
            .expect("UiCamera requires a Bevy camera");
        let previous = CameraState {
            clear_color: settings.clear_color,
            output_mode: settings.output_mode,
            compositing: world.get::<bevy::camera::CompositingSpace>(camera).copied(),
            settings: world.get::<EguiContextSettings>(camera).cloned(),
        };
        let mut configured = previous.settings.clone().unwrap_or_default();
        configured.run_manually = true;
        world.entity_mut(camera).insert((
            previous,
            configured,
            bevy::camera::CompositingSpace::Linear,
        ));
        world
            .get_mut::<Camera>(camera)
            .expect("UiCamera requires a Bevy camera")
            .output_mode = bevy::camera::CameraOutputMode::Write {
            blend_state: Some(
                bevy::render::render_resource::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
            ),
            clear_color: ClearColorConfig::Custom(Color::NONE),
        };
        world.get_mut::<Camera>(camera).unwrap().clear_color =
            ClearColorConfig::Custom(Color::NONE);
        let ctx = world
            .get_mut::<EguiContext>(camera)
            .unwrap()
            .get_mut()
            .clone();
        ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::new(1).unwrap());
        ctx.set_fonts(crate::fonts::definitions());
    }
}

pub(crate) fn draw(world: &mut World) {
    let cameras: Vec<_> = world
        .query::<(Entity, &UiCamera)>()
        .iter(world)
        .map(|(e, c)| (e, c.0))
        .collect();
    if cameras.is_empty() {
        world
            .resource_mut::<Runtime>()
            .icons
            .get_mut()
            .unwrap()
            .clear();
        return;
    }
    assert_eq!(
        cameras.len(),
        1,
        "UI2 currently supports one active surface"
    );
    let (camera, surface) = cameras[0];
    let Some(uses) = world.get::<UsesTheme>(surface) else {
        return;
    };
    let theme = world.get::<Theme>(uses.0).expect("Surface theme").clone();
    let policy = *world.get::<MotionPolicy>(surface).unwrap();
    let ctx = world
        .get_mut::<EguiContext>(camera)
        .unwrap()
        .get_mut()
        .clone();
    let mut runtime = world.remove_resource::<Runtime>().unwrap();
    if runtime.themes.get(&camera) != Some(&theme) {
        if let Err(error) = theme.validate() {
            world.write_message(UiError(error));
            world.insert_resource(runtime);
            return;
        }
        theme.apply(&ctx);
        runtime.themes.insert(camera, theme.clone());
    }
    ctx.global_style_mut(|s| s.animation_time = if policy.enabled { policy.seconds } else { 0.0 });
    let pixels = (theme.metrics.font_size * ctx.pixels_per_point())
        .ceil()
        .max(1.0) as u32;
    let icons_result = runtime.icons.get_mut().unwrap().prepare(
        &mut ctx.clone(),
        world.resource::<Assets<Icon>>(),
        world.resource::<Assets<IconSet>>(),
        world.resource::<icons::BundledIcons>(),
        world.get::<UiIcons>(surface),
        world.resource::<AssetServer>(),
        pixels,
    );
    if let Err(error) = icons_result {
        world.write_message(UiError(error.to_string()));
    }
    let mut raw = world.get_mut::<EguiInput>(camera).unwrap().take();
    // Upstream clipboard management is disabled; the same service is used by
    // application systems and egui text controls, including memory-only smoke.
    let clipboard_keys: Vec<_> = raw
        .events
        .iter()
        .filter_map(|event| match event {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if modifiers.command => Some(*key),
            _ => None,
        })
        .collect();
    for key in clipboard_keys {
        match key {
            egui::Key::C => raw.events.push(egui::Event::Copy),
            egui::Key::X => raw.events.push(egui::Event::Cut),
            egui::Key::V => match world
                .resource_mut::<bedtime_clipboard2::Clipboard>()
                .read_text_if_present()
            {
                Ok(Some(text)) => raw.events.push(egui::Event::Paste(text)),
                Ok(None) => {}
                Err(error) => {
                    world.write_message(UiError(error.to_string()));
                }
            },
            _ => {}
        }
    }
    let views: Vec<_> = world
        .get::<SurfaceViews>(surface)
        .map(|v| v.iter().collect())
        .unwrap_or_default();
    let start = Instant::now();
    let mut active_animations = 0;
    let text_before = ctx.text_edit_focused();
    let modal_before = ctx.any_popup_open() || ctx.memory(|m| m.top_modal_layer().is_some());
    let mut output = ctx.run_ui(raw, |ui| {
        if world.get::<UiBackground>(surface) == Some(&UiBackground::Theme) {
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, theme::color(theme.palette.background));
        }
        for view in &views {
            if world.get::<UiView>(*view).is_none() {
                continue;
            }
            ui.push_id(view.to_bits(), |ui| {
                let context = UiContext {
                    view: *view,
                    surface,
                    theme: &theme,
                    motion: policy,
                    icons: &runtime.icons,
                };
                let draw = world.get::<UiView>(*view).unwrap().draw;
                draw(ui, world, &context);
            });
        }
        active_animations = motion::active(&ctx);
    });
    // A callback may detach the UI while its pass is running. Do not republish
    // that pass's shapes or capture state after the removal hook cleared them.
    if world.get::<UiCamera>(camera).is_none() {
        // The session was removed, so its unsubmitted texture updates can be
        // discarded along with its shapes.
        output.textures_delta.clear();
        runtime.themes.remove(&camera);
        runtime.icons.get_mut().unwrap().clear();
        world.insert_resource(runtime);
        return;
    }
    for command in &output.platform_output.commands {
        if let egui::OutputCommand::CopyText(text) = command {
            if let Err(error) = world
                .resource_mut::<bedtime_clipboard2::Clipboard>()
                .write_text(text)
            {
                world.write_message(UiError(error.to_string()));
            }
        }
    }
    let capture = UiCapture {
        keyboard: ctx.egui_wants_keyboard_input(),
        pointer: ctx.egui_wants_pointer_input(),
        text: text_before || ctx.text_edit_focused(),
        modal: modal_before
            || ctx.any_popup_open()
            || ctx.memory(|m| m.top_modal_layer().is_some()),
    };
    let meshes = ctx.tessellate(output.shapes.clone(), output.pixels_per_point);
    let (vertices, indices) = meshes.iter().fold((0, 0), |(v, i), p| {
        if let egui::epaint::Primitive::Mesh(m) = &p.primitive {
            (v + m.vertices.len(), i + m.indices.len())
        } else {
            (v, i)
        }
    });
    let frames = world.get::<UiDiagnostics>(surface).unwrap().frames + 1;
    let icons = runtime.icons.lock().unwrap();
    world.entity_mut(surface).insert((
        capture,
        icons.status.clone(),
        UiDiagnostics {
            frames,
            vertices,
            indices,
            active_animations,
            draw_ms: start.elapsed().as_secs_f64() * 1000.0,
            icon_textures: icons.textures(),
            icon_rasterizations: icons.rasterizations,
            ..default()
        },
    ));
    if icons.needs_redraw {
        ctx.request_repaint();
    }
    drop(icons);
    **world.get_mut::<EguiFullOutput>(camera).unwrap() = Some(output);
    world.insert_resource(runtime);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detaching_camera_during_drawing_does_not_publish_the_removed_ui() {
        #[derive(Component)]
        struct CameraLink(Entity);
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>()
            .init_resource::<Runtime>()
            .add_message::<UiError>()
            .insert_resource(bedtime_clipboard2::Clipboard::memory());
        icons::install(&mut app);
        let world = app.world_mut();
        let theme = world.spawn(themes::dark()).id();
        let surface = world
            .spawn((Window::default(), UiSurface, UsesTheme(theme)))
            .id();
        let camera = world.spawn((Camera2d, UiCamera(surface))).id();
        configure(world);
        let view = register_view(world, surface, |ui, world, context| {
            ui.label("Detached during this pass");
            let camera = world.get::<CameraLink>(context.view).unwrap().0;
            world.entity_mut(camera).remove::<UiCamera>();
        });
        world.entity_mut(view).insert(CameraLink(camera));
        draw(world);
        assert!(world.get::<UiCamera>(camera).is_none());
        assert!(world.get::<EguiContext>(camera).is_none());
        assert!(world.get::<Camera>(camera).is_some());
        assert!(world.get::<EguiFullOutput>(camera).unwrap().is_none());
        assert!(
            world
                .get::<bevy_egui::EguiRenderOutput>(camera)
                .unwrap()
                .paint_jobs
                .is_empty()
        );
        assert_eq!(world.get::<UiDiagnostics>(surface).unwrap().frames, 0);
        // The next ordinary frame with no UI camera is also harmless.
        draw(world);
    }

    #[test]
    fn removing_ui_camera_restores_camera_settings_and_clears_stale_output() {
        let mut world = World::new();
        let clear = ClearColorConfig::Custom(Color::srgb(0.2, 0.3, 0.4));
        let original_settings = EguiContextSettings {
            enable_ime: false,
            ..default()
        };
        let camera = world
            .spawn((
                Camera2d,
                Camera {
                    clear_color: clear,
                    output_mode: bevy::camera::CameraOutputMode::Skip,
                    order: 7,
                    ..default()
                },
                original_settings.clone(),
                bevy::camera::CompositingSpace::Srgb,
                UiCamera(Entity::PLACEHOLDER),
            ))
            .id();
        for _ in 0..2 {
            configure(&mut world);
            assert!(
                world
                    .get::<EguiContextSettings>(camera)
                    .unwrap()
                    .run_manually
            );
            assert!(!world.get::<EguiContextSettings>(camera).unwrap().enable_ime);
            assert_eq!(
                *world.get::<bevy::camera::CompositingSpace>(camera).unwrap(),
                bevy::camera::CompositingSpace::Linear
            );
            **world.get_mut::<EguiFullOutput>(camera).unwrap() = Some(default());
            world
                .get_mut::<bevy_egui::EguiRenderOutput>(camera)
                .unwrap()
                .paint_jobs
                .push(egui::ClippedPrimitive {
                    clip_rect: egui::Rect::EVERYTHING,
                    primitive: egui::epaint::Primitive::Mesh(default()),
                });
            world.entity_mut(camera).remove::<UiCamera>();
            let restored = world.get::<Camera>(camera).unwrap();
            assert!(
                matches!(restored.clear_color, ClearColorConfig::Custom(color) if color == Color::srgb(0.2, 0.3, 0.4))
            );
            assert!(matches!(
                restored.output_mode,
                bevy::camera::CameraOutputMode::Skip
            ));
            assert_eq!(restored.order, 7);
            assert_eq!(
                world.get::<EguiContextSettings>(camera),
                Some(&original_settings)
            );
            assert_eq!(
                *world.get::<bevy::camera::CompositingSpace>(camera).unwrap(),
                bevy::camera::CompositingSpace::Srgb
            );
            assert!(world.get::<EguiFullOutput>(camera).unwrap().is_none());
            assert!(world.get::<EguiContext>(camera).is_none());
            assert!(
                world
                    .get::<bevy_egui::EguiRenderOutput>(camera)
                    .unwrap()
                    .paint_jobs
                    .is_empty()
            );
            world
                .entity_mut(camera)
                .insert(UiCamera(Entity::PLACEHOLDER));
        }
    }

    #[derive(Component)]
    struct Text(String);

    #[test]
    fn egui_clipboard_and_text_focus_work_without_a_second_input_stream() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>()
            .init_resource::<Runtime>()
            .add_message::<UiError>()
            .insert_resource(bedtime_clipboard2::Clipboard::memory());
        icons::install(&mut app);
        let world = app.world_mut();
        let theme = world.spawn(themes::dark()).id();
        let surface = world
            .spawn((Window::default(), UiSurface, UsesTheme(theme)))
            .id();
        let camera = world.spawn((Camera2d, UiCamera(surface))).id();
        configure(world);
        let view = register_view(world, surface, |ui, world, context| {
            let mut text = world.get_mut::<Text>(context.view).unwrap();
            ui.add(egui::TextEdit::singleline(&mut text.0).id(egui::Id::new("text")));
        });
        world.entity_mut(view).insert(Text("before".into()));
        let ctx = world
            .get_mut::<EguiContext>(camera)
            .unwrap()
            .get_mut()
            .clone();
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("text")));
        let draw_input = |world: &mut World, events| {
            **world.get_mut::<EguiInput>(camera).unwrap() = egui::RawInput {
                events,
                ..default()
            };
            draw(world);
            // This test exercises the UI pass without a renderer to upload deltas.
            (**world.get_mut::<EguiFullOutput>(camera).unwrap())
                .as_mut()
                .unwrap()
                .textures_delta
                .clear();
            assert!(world.resource::<Messages<UiError>>().is_empty());
        };
        draw_input(world, vec![]);
        assert!(world.get::<UiCapture>(surface).unwrap().text);
        let command = egui::Modifiers {
            command: true,
            mac_cmd: cfg!(target_os = "macos"),
            ctrl: !cfg!(target_os = "macos"),
            ..default()
        };
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        };
        world
            .resource_mut::<bedtime_clipboard2::Clipboard>()
            .write_text("pasted")
            .unwrap();
        draw_input(world, vec![key(egui::Key::V, command)]);
        assert!(world.get::<Text>(view).unwrap().0.contains("pasted"));
        let before_file_paste = world.get::<Text>(view).unwrap().0.clone();
        let files = vec![std::path::PathBuf::from("/project/file.txt")];
        world
            .resource_mut::<bedtime_clipboard2::Clipboard>()
            .write_files(&files)
            .unwrap();
        // Primary+V still reaches application file-list commands, but there is no
        // text representation to send to egui and no clipboard failure to report.
        draw_input(world, vec![key(egui::Key::V, command)]);
        assert_eq!(world.get::<Text>(view).unwrap().0, before_file_paste);
        assert_eq!(
            world
                .resource_mut::<bedtime_clipboard2::Clipboard>()
                .read_files()
                .unwrap(),
            files
        );
        draw_input(
            world,
            vec![key(egui::Key::A, command), key(egui::Key::C, command)],
        );
        let copied = world
            .resource_mut::<bedtime_clipboard2::Clipboard>()
            .read_text()
            .unwrap();
        assert_eq!(copied, world.get::<Text>(view).unwrap().0);
        draw_input(world, vec![key(egui::Key::Enter, default())]);
        assert!(!ctx.text_edit_focused());
        assert!(
            world.get::<UiCapture>(surface).unwrap().text,
            "Enter ending text focus still belongs to that edit for this frame"
        );
        world
            .resource_mut::<Runtime>()
            .icons
            .get_mut()
            .unwrap()
            .clear();
        let mut output = ctx.run_ui(default(), |_| {});
        output.textures_delta.clear();
    }
}
