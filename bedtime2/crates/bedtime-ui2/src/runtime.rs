use crate::*;
use bevy::{
    asset::AssetId,
    ecs::{message::MessageCursor, relationship::RelationshipTarget},
    input::ButtonState,
    window::WindowEvent,
};
use bevy_egui::{EguiContext, EguiContextSettings, EguiFullOutput, EguiInput, EguiUserTextures};
use std::{collections::HashMap, time::Instant};

#[derive(Resource, Default)]
pub(crate) struct Runtime {
    icons: Mutex<icon_cache::IconCache>,
    images: HashMap<AssetId<Image>, egui::TextureId>,
    themes: HashMap<Entity, Theme>,
    events: MessageCursor<WindowEvent>,
    next_event: u64,
    modifier_keys: bevy::input::ButtonInput<bevy::input::keyboard::KeyCode>,
}

pub(crate) fn configure(world: &mut World) {
    let cameras: Vec<_> = world
        .query_filtered::<Entity, Added<UiCamera>>()
        .iter(world)
        .collect();
    for camera in cameras {
        world.entity_mut(camera).insert((
            EguiContextSettings {
                run_manually: true,
                ..default()
            },
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
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "PaperMono".into(),
            std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
                "../assets/fonts/PaperMono-Regular.ttf"
            ))),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(0, "PaperMono".into());
        }
        ctx.set_fonts(fonts);
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
        *world
            .resource_mut::<input::InputRouter>()
            .frame
            .get_mut()
            .unwrap() = default();
        world.resource_mut::<input::RoutedInput>().release_all();
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
    let panel_settings = world
        .get::<PanelSettings>(surface)
        .copied()
        .unwrap_or_default();
    assert!(
        [
            panel_settings.rounding,
            panel_settings.hit_size,
            panel_settings.panel_gap,
        ]
        .into_iter()
        .chain(panel_settings.padding)
        .all(|value| value.is_finite() && value >= 0.0),
        "Panel settings must be finite and nonnegative"
    );
    let ctx = world
        .get_mut::<EguiContext>(camera)
        .unwrap()
        .get_mut()
        .clone();
    let mut runtime = world.remove_resource::<Runtime>().unwrap();
    let mut router = world.remove_resource::<input::InputRouter>().unwrap();
    let previous_content = router.frame.get_mut().unwrap().focused_content;
    *router.frame.get_mut().unwrap() = input::UiInputFrame {
        focused_content: previous_content,
        ..default()
    };
    if let Some(events) = world.get_resource::<Messages<WindowEvent>>() {
        for event in runtime.events.read(events) {
            if let WindowEvent::KeyboardInput(k) = event {
                if k.state == ButtonState::Pressed {
                    runtime.modifier_keys.press(k.key_code);
                } else {
                    runtime.modifier_keys.release(k.key_code);
                }
            }
            if matches!(event,WindowEvent::WindowFocused(f) if !f.focused) {
                runtime.modifier_keys.reset_all();
            }
            use bevy::input::keyboard::KeyCode::*;
            let modifiers = input::InputModifiers {
                control: runtime
                    .modifier_keys
                    .any_pressed([ControlLeft, ControlRight]),
                shift: runtime.modifier_keys.any_pressed([ShiftLeft, ShiftRight]),
                alt: runtime.modifier_keys.any_pressed([AltLeft, AltRight]),
                meta: runtime.modifier_keys.any_pressed([SuperLeft, SuperRight]),
            };
            runtime.next_event += 1;
            router
                .frame
                .get_mut()
                .unwrap()
                .events
                .push(input::InputEvent {
                    id: runtime.next_event,
                    event: event.clone(),
                    modifiers,
                });
        }
    }
    if runtime.themes.get(&camera) != Some(&theme) {
        if let Err(error) = theme.validate() {
            world.write_message(UiError(error));
            world.insert_resource(runtime);
            world.insert_resource(router);
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
    let images: Vec<_> = world
        .query::<&UiViewport>()
        .iter(world)
        .map(|v| v.image.clone())
        .collect();
    world.resource_scope(|_, mut textures: Mut<EguiUserTextures>| {
        runtime.images.retain(|id, _| {
            let alive = images.iter().any(|h| h.id() == *id);
            if !alive {
                textures.remove_image(*id);
            }
            alive
        });
        for image in images {
            runtime
                .images
                .entry(image.id())
                .or_insert_with(|| textures.add_image(bevy_egui::EguiTextureHandle::Strong(image)));
        }
    });
    let mut raw = world.get_mut::<EguiInput>(camera).unwrap().take();
    // Upstream clipboard management is disabled; the same service is used by
    // application systems and egui text controls, including memory-only smoke.
    for event in &router.frame.get_mut().unwrap().events {
        if let Some(key) = event.key() {
            let primary = if cfg!(target_os = "macos") {
                event.modifiers.meta
            } else {
                event.modifiers.control
            };
            if primary && key.state.is_pressed() {
                use bevy::input::keyboard::KeyCode::*;
                match key.key_code {
                    KeyC => raw.events.push(egui::Event::Copy),
                    KeyX => raw.events.push(egui::Event::Cut),
                    KeyV => {
                        match world
                            .resource_mut::<bedtime_clipboard2::Clipboard>()
                            .read_text()
                        {
                            Ok(text) => raw.events.push(egui::Event::Paste(text)),
                            Err(error) => {
                                world.write_message(UiError(error.to_string()));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let views: Vec<_> = world
        .get::<SurfaceViews>(surface)
        .map(|v| v.iter().collect())
        .unwrap_or_default();
    let start = Instant::now();
    let mut active_animations = 0;
    let output = ctx.run_ui(raw, |ui| {
        if world.get::<UiBackground>(surface) == Some(&UiBackground::Theme) {
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, theme::color(theme.palette.background));
        }
        for view in &views {
            if world.get::<UiView>(*view).is_none() {
                continue;
            }
            ui.push_id(view.to_bits(), |ui| {
                let frame = UiFrame {
                    view: *view,
                    surface,
                    ui,
                    theme: &theme,
                    motion: policy,
                    panel_settings,
                    icons: &runtime.icons,
                    textures: &runtime.images,
                    input: &router.frame,
                };
                if let Err(error) =
                    world.run_system_with(SystemId::<UiFrame<'static>>::from_entity(*view), frame)
                {
                    world.write_message(UiError(error.to_string()));
                }
            });
        }
        active_animations = motion::active(&ctx);
    });
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
    router.frame.get_mut().unwrap().claim_widgets(&ctx);
    let modal = ctx.any_popup_open() || ctx.memory(|m| m.top_modal_layer().is_some());
    router.frame.get_mut().unwrap().modal = modal;
    if modal {
        let ids: Vec<_> = router
            .frame
            .get_mut()
            .unwrap()
            .events
            .iter()
            .filter(|e| {
                matches!(
                    e.event,
                    WindowEvent::KeyboardInput(_)
                        | WindowEvent::Ime(_)
                        | WindowEvent::CursorMoved(_)
                        | WindowEvent::MouseButtonInput(_)
                        | WindowEvent::MouseWheel(_)
                )
            })
            .map(|e| e.id)
            .collect();
        for id in ids {
            router.frame.get_mut().unwrap().consume(id, surface);
        }
    }
    if ctx.input(|i| i.pointer.any_pressed()) {
        let pos = ctx.input(|i| i.pointer.latest_pos());
        if !pos.is_some_and(|pos| {
            router
                .frame
                .get_mut()
                .unwrap()
                .regions
                .iter()
                .any(|r| r.rect.contains(pos))
        }) {
            router.frame.get_mut().unwrap().focused_content = None;
        }
    }
    if ctx.memory(|m| m.focused().is_some()) {
        router.frame.get_mut().unwrap().focused_content = None;
    }
    let remaining = ctx.input(|i| i.events.clone());
    for event in router.frame.get_mut().unwrap().events.clone() {
        if let Some(k) = event.key() {
            if let Some(key) = bevy_egui::helpers::bevy_to_egui_key(&k.logical_key) {
                let unconsumed = remaining.iter().any(|event| matches!(event,
                    egui::Event::Key { key:other,pressed,.. } if *other==key && *pressed==k.state.is_pressed()));
                if !unconsumed {
                    router.frame.get_mut().unwrap().consume(event.id, surface);
                }
            }
        }
    }
    router.frame.get_mut().unwrap().text_active = ctx.text_edit_focused();
    let capture = UiCapture {
        keyboard: ctx.egui_wants_keyboard_input(),
        pointer: ctx.egui_wants_pointer_input(),
        text: ctx.text_edit_focused(),
    };
    if capture.text
        && !ctx
            .memory(|m| m.focused())
            .is_some_and(|id| router.frame.lock().unwrap().text_widgets.contains(&id))
    {
        let ids: Vec<_> = router
            .frame
            .lock()
            .unwrap()
            .events
            .iter()
            .filter(|e| e.key().is_some() || matches!(e.event, WindowEvent::Ime(_)))
            .map(|e| e.id)
            .collect();
        for id in ids {
            router.frame.lock().unwrap().consume(id, surface);
        }
    }
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
            viewport_textures: runtime.images.len(),
            ..default()
        },
    ));
    if icons.needs_redraw {
        ctx.request_repaint();
    }
    drop(icons);
    **world.get_mut::<EguiFullOutput>(camera).unwrap() = Some(output);
    world.insert_resource(runtime);
    world.insert_resource(router);
}
