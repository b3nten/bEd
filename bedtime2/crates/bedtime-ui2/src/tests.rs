use crate::*;
use bevy::input::{
    ButtonState,
    keyboard::{Key, KeyCode, KeyboardInput},
};
use bevy::window::WindowEvent;
#[test]
fn retargeting_is_continuous_and_reduced_motion_settles() {
    let mut value = motion::Transition::settled(0.0);
    value.retarget(1.0);
    value.advance(0.08, MotionPolicy::default());
    let halfway = value.value();
    assert!(halfway > 0.0 && halfway < 1.0);
    value.retarget(0.0);
    assert_eq!(value.value(), halfway);
    value.advance(0.01, MotionPolicy::default());
    assert!(value.value() < halfway);
    value.advance(
        0.0,
        MotionPolicy {
            enabled: false,
            ..default()
        },
    );
    assert_eq!(value.value(), 0.0);
    assert!(!value.is_active());
}
#[test]
fn themes_validate_boundary_values_and_map_to_egui() {
    let ctx = egui::Context::default();
    let dark = themes::dark();
    dark.validate().unwrap();
    dark.apply(&ctx);
    assert_eq!(
        ctx.global_style().visuals.panel_fill,
        theme::color(dark.palette.background)
    );
    let light = themes::light();
    light.validate().unwrap();
    light.apply(&ctx);
    assert_eq!(
        ctx.global_style().visuals.panel_fill,
        theme::color(light.palette.background)
    );
    let mut bad = light;
    bad.metrics.font_size = f32::NAN;
    assert!(bad.validate().is_err());
    bad = dark;
    bad.palette.text[0] = 2.0;
    assert!(bad.validate().is_err());
}

#[test]
fn primary_buttons_resolve_hover_and_press_colors_without_restyling_other_widgets() {
    for theme in [themes::dark(), themes::light()] {
        let ctx = egui::Context::default();
        ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::new(1).unwrap());
        theme.apply(&ctx);
        ctx.global_style_mut(|s| s.animation_time = 0.0);
        let icons = std::sync::Mutex::new(icon_cache::IconCache::default());
        let textures = HashMap::new();
        let input = std::sync::Mutex::new(input::UiInputFrame::default());
        let mut target = egui::pos2(-100.0, -100.0);
        let mut fills = Vec::new();
        for step in 0..5 {
            let mut raw = egui::RawInput::default();
            raw.events.push(egui::Event::PointerMoved(target));
            if step == 2 || step == 3 {
                raw.events.push(egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: step == 2,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let mut output = ctx.run_ui(raw, |ui| {
                let mut frame = UiFrame {
                    view: Entity::PLACEHOLDER,
                    surface: Entity::PLACEHOLDER,
                    ui,
                    theme: &theme,
                    motion: default(),
                    panel_settings: default(),
                    icons: &icons,
                    textures: &textures,
                    input: &input,
                };
                let response = frame.primary_button("Primary action");
                target = response.rect.center();
                assert_eq!(response.clicked(), step == 3);
                if step == 3 {
                    frame
                        .ui
                        .ctx()
                        .memory_mut(|m| m.surrender_focus(response.id));
                }
                assert_eq!(
                    frame.ui.visuals().widgets.inactive.weak_bg_fill,
                    theme::color(theme.palette.surface)
                );
                let _ = frame.ui.button("Secondary action");
            });
            let fill = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) if rect.rect.contains(target) => Some(rect.fill),
                    _ => None,
                })
                .expect("Painted primary button");
            fills.push(fill);
            output.textures_delta.clear();
        }
        assert_eq!(fills[0], theme::color(theme.palette.accent));
        assert_ne!(fills[0], fills[1], "Hover must be visible");
        assert_ne!(fills[1], fills[2], "Press must be visible");
        assert_eq!(
            fills[4], fills[1],
            "Hover returns after the click loses focus"
        );
    }
}
#[test]
fn custom_control_colors_fade_through_hover_press_and_release() {
    let ctx = egui::Context::default();
    let theme = themes::dark();
    theme.apply(&ctx);
    let icons = Mutex::new(icon_cache::IconCache::default());
    let textures = HashMap::new();
    let input = Mutex::new(input::UiInputFrame::default());
    let render = |time, pointer, button: Option<bool>, enabled| {
        ctx.global_style_mut(|s| {
            s.animation_time = if enabled {
                MotionPolicy::default().seconds
            } else {
                0.0
            }
        });
        let mut events = vec![egui::Event::PointerMoved(pointer)];
        if let Some(pressed) = button {
            events.push(egui::Event::PointerButton {
                pos: pointer,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: default(),
            });
        }
        let mut rect = egui::Rect::NOTHING;
        let mut output = ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                events,
                ..default()
            },
            |ui| {
                let mut frame = UiFrame {
                    view: Entity::PLACEHOLDER,
                    surface: Entity::PLACEHOLDER,
                    ui,
                    theme: &theme,
                    motion: MotionPolicy {
                        enabled,
                        ..default()
                    },
                    panel_settings: default(),
                    icons: &icons,
                    textures: &textures,
                    input: &input,
                };
                let response = frame.icon_button(BuiltinIcon::Sun);
                rect = response.rect;
                ctx.memory_mut(|m| m.surrender_focus(response.id));
            },
        );
        let fill = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Rect(r) if r.rect.contains(rect.center()) => Some(r.fill),
                _ => None,
            })
            .unwrap();
        output.textures_delta.clear();
        (rect.center(), fill)
    };
    let outside = egui::pos2(-100.0, -100.0);
    let (inside, idle) = render(0.0, outside, None, true);
    render(0.02, inside, None, true);
    let (_, hover_mid) = render(0.10, inside, None, true);
    let (_, hover) = render(0.30, inside, None, true);
    assert_ne!(hover_mid, idle);
    assert_ne!(
        hover_mid, hover,
        "Hover color must pass through an intermediate shade"
    );
    render(0.32, inside, Some(true), true);
    let (_, press_mid) = render(0.40, inside, None, true);
    let (_, press) = render(0.60, inside, None, true);
    assert_ne!(press_mid, hover);
    assert_ne!(press_mid, press);
    render(0.62, inside, Some(false), true);
    render(0.64, inside, None, true);
    let (_, release_mid) = render(0.72, inside, None, true);
    assert_ne!(release_mid, press);
    assert_ne!(release_mid, hover);
    assert_eq!(render(0.90, inside, None, true).1, hover);
    render(0.92, outside, None, true);
    let (_, exit_mid) = render(1.00, outside, None, true);
    assert_ne!(exit_mid, hover);
    assert_ne!(exit_mid, idle);
    assert_eq!(render(1.20, outside, None, true).1, idle);
    assert_eq!(
        render(1.22, inside, None, false).1,
        hover,
        "Reduced motion settles immediately"
    );
}

#[test]
fn scroll_track_clears_rounded_corners_without_insetting_content() {
    let ctx = egui::Context::default();
    let theme = themes::dark();
    let panel_settings = PanelSettings {
        rounding: 18.0,
        ..default()
    };
    theme.apply(&ctx);
    ctx.global_style_mut(|s| {
        s.spacing.scroll = egui::style::ScrollStyle::solid();
        s.animation_time = 0.0;
    });
    let icons = Mutex::new(icon_cache::IconCache::default());
    let textures = HashMap::new();
    let input = Mutex::new(input::UiInputFrame::default());
    let mut bounds = egui::Rect::NOTHING;
    let mut clip = egui::Rect::NOTHING;
    let mut body = egui::Rect::NOTHING;
    let mut track = None;
    for _ in 0..3 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(300.0, 200.0),
                )),
                ..default()
            },
            |ui| {
                bounds = ui.available_rect_before_wrap();
                let mut frame = UiFrame {
                    view: Entity::PLACEHOLDER,
                    surface: Entity::PLACEHOLDER,
                    ui,
                    theme: &theme,
                    motion: default(),
                    panel_settings,
                    icons: &icons,
                    textures: &textures,
                    input: &input,
                };
                frame.scroll("overflow", |frame| {
                    clip = frame.ui.clip_rect();
                    body = frame
                        .ui
                        .allocate_exact_size(
                            egui::vec2(frame.ui.available_width(), 1000.0),
                            egui::Sense::hover(),
                        )
                        .0;
                });
            },
        );
        track = output.shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Rect(r) if r.fill == theme::color(theme.palette.background) => {
                Some(r.rect)
            }
            _ => None,
        });
        output.textures_delta.clear();
    }
    let track = track.expect("Visible scrollbar track");
    assert!(track.min.y >= bounds.min.y + panel_settings.rounding);
    assert!(track.max.y <= bounds.max.y - panel_settings.rounding);
    assert_eq!(
        body.min, bounds.min,
        "The scrolling content keeps its original origin"
    );
    assert_eq!(clip.y_range(), bounds.y_range(), "Only the track is inset");
}

#[derive(Component, Default)]
struct Counter(u32);
fn draw_counter(frame: UiFrame, mut counters: Query<&mut Counter>) {
    counters.get_mut(frame.view).unwrap().0 += 1;
    frame.ui.label("A normal registered Bevy system");
}
#[test]
fn borrowed_frames_execute_once_and_surface_ownership_keeps_shared_themes() {
    let mut world = World::new();
    let theme = world.spawn(themes::dark()).id();
    let surface = world.spawn((UiSurface, UsesTheme(theme))).id();
    let view = register_view(&mut world, surface, draw_counter);
    world.entity_mut(view).insert(Counter::default());
    let part = world.spawn(InView(view)).id();
    let ctx = egui::Context::default();
    ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::new(1).unwrap());
    let icons = std::sync::Mutex::new(icon_cache::IconCache::default());
    let textures = HashMap::new();
    let input = std::sync::Mutex::new(input::UiInputFrame::default());
    let theme_data = themes::dark();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        world
            .run_system_with(
                SystemId::<UiFrame<'static>>::from_entity(view),
                UiFrame {
                    view,
                    surface,
                    ui,
                    theme: &theme_data,
                    motion: default(),
                    panel_settings: default(),
                    icons: &icons,
                    textures: &textures,
                    input: &input,
                },
            )
            .unwrap();
        ui.ctx()
            .request_discard("Do not replay stateful drawing systems");
    });
    output.textures_delta.clear();
    assert!(!output.shapes.is_empty());
    assert_eq!(world.get::<Counter>(view).unwrap().0, 1);
    world.despawn(surface);
    assert!(world.get_entity(view).is_err());
    assert!(world.get_entity(part).is_err());
    assert!(world.get::<Theme>(theme).is_some());
}
fn key_event(
    id: u64,
    key_code: KeyCode,
    text: Option<&str>,
    modifiers: input::InputModifiers,
) -> input::InputEvent {
    input::InputEvent {
        id,
        modifiers,
        event: WindowEvent::KeyboardInput(KeyboardInput {
            key_code,
            logical_key: Key::Character("a".into()),
            state: ButtonState::Pressed,
            text: text.map(Into::into),
            repeat: false,
            window: Entity::PLACEHOLDER,
        }),
    }
}
#[test]
fn text_ownership_reserves_typing_noop_edits_and_leaves_save_available() {
    let primary = input::InputModifiers {
        control: !cfg!(target_os = "macos"),
        meta: cfg!(target_os = "macos"),
        ..default()
    };
    let mut frame = input::UiInputFrame {
        events: vec![
            key_event(
                1,
                KeyCode::KeyA,
                Some("A"),
                input::InputModifiers {
                    shift: true,
                    ..default()
                },
            ),
            key_event(2, KeyCode::Backspace, None, default()),
            key_event(3, KeyCode::KeyS, None, primary),
            key_event(4, KeyCode::Enter, None, default()),
            input::InputEvent {
                id: 5,
                modifiers: default(),
                event: WindowEvent::Ime(bevy::window::Ime::Commit {
                    window: Entity::PLACEHOLDER,
                    value: "漢字".into(),
                }),
            },
            key_event(6, KeyCode::KeyC, None, primary),
        ],
        ..default()
    };
    frame.claim_text(Entity::PLACEHOLDER, egui::Id::new("text"));
    assert!(frame.claims.contains_key(&1));
    assert!(frame.claims.contains_key(&2));
    assert!(frame.claims.contains_key(&4));
    assert!(frame.claims.contains_key(&5));
    assert!(frame.claims.contains_key(&6));
    assert!(!frame.claims.contains_key(&3));
}
#[test]
fn routed_state_releases_on_focus_loss() {
    let mut routed = input::RoutedInput::default();
    routed.admit(key_event(1, KeyCode::KeyA, Some("a"), default()));
    assert!(routed.keys.pressed(KeyCode::KeyA));
    routed.admit(input::InputEvent {
        id: 2,
        modifiers: default(),
        event: WindowEvent::WindowFocused(bevy::window::WindowFocused {
            window: Entity::PLACEHOLDER,
            focused: false,
        }),
    });
    assert!(!routed.keys.pressed(KeyCode::KeyA));
    assert!(routed.keys.just_released(KeyCode::KeyA));
}

#[test]
fn tooltip_contents_fade_at_their_final_size_and_pixel_grid() {
    let ctx = egui::Context::default();
    let theme = themes::dark();
    theme.apply(&ctx);
    ctx.global_style_mut(|style| {
        style.animation_time = MotionPolicy::default().seconds;
        style.interaction.tooltip_delay = 0.0;
        style.interaction.show_tooltips_only_when_still = false;
    });
    let icons = std::sync::Mutex::new(icon_cache::IconCache::default());
    let textures = HashMap::new();
    let input = std::sync::Mutex::new(input::UiInputFrame::default());
    let render = |time| {
        let mut shown = false;
        let mut output = ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                events: vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
                ..default()
            },
            |ui| {
                let response = ui.button("Hover for a tooltip");
                let frame = UiFrame {
                    view: Entity::PLACEHOLDER,
                    surface: Entity::PLACEHOLDER,
                    ui,
                    theme: &theme,
                    motion: default(),
                    panel_settings: default(),
                    icons: &icons,
                    textures: &textures,
                    input: &input,
                };
                shown = frame.tooltip(&response, "Popup text");
            },
        );
        output.textures_delta.clear();
        let text = output
            .shapes
            .into_iter()
            .find_map(|shape| match shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == "Popup text" => Some(text),
                _ => None,
            });
        (shown, text)
    };
    render(0.0);
    render(0.01); // Popup sizing pass.
    let (shown, moving) = render(0.08);
    assert!(shown);
    let (_, settled) = render(0.25);
    let moving = moving.unwrap();
    let settled = settled.unwrap();
    assert_eq!(moving.pos, settled.pos);
    assert_eq!(moving.galley.rect, settled.galley.rect);
    assert_eq!(moving.galley.mesh_bounds, settled.galley.mesh_bounds);
    assert!(moving.fallback_color.a() < settled.fallback_color.a());
    assert_eq!(
        moving.galley.job, settled.galley.job,
        "Keep the original text layout job"
    );
    for (moving, settled) in moving.galley.rows.iter().zip(&settled.galley.rows) {
        for (moving, settled) in moving
            .row
            .visuals
            .mesh
            .vertices
            .iter()
            .zip(&settled.row.visuals.mesh.vertices)
        {
            assert_eq!(moving.pos, settled.pos);
            assert_eq!(
                moving.uv, settled.uv,
                "Opacity changes keep glyph sampling fixed"
            );
        }
    }
}

#[test]
fn dropdown_clears_its_anchor_and_long_lists_still_scroll() {
    let ctx = egui::Context::default();
    themes::dark().apply(&ctx);
    let render = |time, events| {
        let mut button = None;
        let mut first = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 480.0),
                )),
                events,
                ..default()
            },
            |ui| {
                let combo = ComboBox::from_label("Options")
                    .selected_text("First")
                    .height(80.0)
                    .show_ui(ui, |ui| {
                        first = Some(ui.label("First item").rect.min.y);
                        for i in 1..30 {
                            ui.label(format!("Item {i}"));
                        }
                    });
                button = Some(combo.response);
            },
        );
        output.textures_delta.clear();
        (button.unwrap(), first)
    };
    let (button, _) = render(0.0, vec![]);
    let popup = button.id.with("popup");
    egui::Popup::open_id(&ctx, popup);
    render(0.01, vec![]);
    let (button, first) = render(0.03, vec![]);
    let popup_rect = ctx.read_response(popup).unwrap().rect;
    assert!(
        popup_rect.top() >= button.rect.bottom() + 4.0,
        "Dropdown must have a visible gap below its anchor"
    );
    render(0.04, vec![egui::Event::PointerMoved(popup_rect.center())]);
    render(
        0.05,
        vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            phase: egui::TouchPhase::Move,
            delta: egui::vec2(0.0, -60.0),
            modifiers: default(),
        }],
    );
    let (_, scrolled) = render(0.1, vec![]);
    assert!(
        scrolled.unwrap() < first.unwrap(),
        "Hiding the scrollbar must preserve wheel scrolling"
    );
    assert!(egui::Popup::is_id_open(&ctx, popup));
}

#[test]
fn enter_ending_text_focus_is_claimed_in_the_same_pass() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("editing");
    let icons = std::sync::Mutex::new(icon_cache::IconCache::default());
    let textures = HashMap::new();
    let input = std::sync::Mutex::new(input::UiInputFrame::default());
    let theme = themes::dark();
    let mut text = "Some text".to_owned();
    ctx.memory_mut(|m| m.request_focus(id));
    for step in 0..2 {
        let mut raw = egui::RawInput::default();
        if step == 1 {
            raw.events.push(egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: Some(egui::Key::Enter),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
            input
                .lock()
                .unwrap()
                .events
                .push(key_event(1, KeyCode::Enter, None, default()));
        }
        let mut output = ctx.run_ui(raw, |ui| {
            let mut frame = UiFrame {
                view: Entity::PLACEHOLDER,
                surface: Entity::PLACEHOLDER,
                ui,
                theme: &theme,
                motion: default(),
                panel_settings: default(),
                icons: &icons,
                textures: &textures,
                input: &input,
            };
            let response = frame
                .text_edit(egui::TextEdit::singleline(&mut text).id(id))
                .response;
            if step == 1 {
                assert!(response.lost_focus());
            }
        });
        output.textures_delta.clear();
    }
    assert!(input.lock().unwrap().claims.contains_key(&1));
}
#[test]
fn ui_owned_held_key_cannot_become_an_application_press_after_focus_changes() {
    let mut router = input::InputRouter::default();
    let mut application = input::RoutedInput::default();
    router.frame.get_mut().unwrap().events =
        vec![key_event(1, KeyCode::KeyA, Some("a"), default())];
    router
        .frame
        .get_mut()
        .unwrap()
        .claims
        .insert(1, input::InputOwner::Ui);
    router.publish(&mut application);
    let mut repeat = key_event(2, KeyCode::KeyA, Some("a"), default());
    if let WindowEvent::KeyboardInput(k) = &mut repeat.event {
        k.repeat = true;
    }
    *router.frame.get_mut().unwrap() = input::UiInputFrame {
        events: vec![repeat],
        ..default()
    };
    router.publish(&mut application);
    assert!(!application.keys.pressed(KeyCode::KeyA));
    assert!(application.events.is_empty());
    let mut released = key_event(3, KeyCode::KeyA, None, default());
    if let WindowEvent::KeyboardInput(k) = &mut released.event {
        k.state = ButtonState::Released;
    }
    router.frame.get_mut().unwrap().events =
        vec![released, key_event(4, KeyCode::KeyA, Some("a"), default())];
    router.publish(&mut application);
    assert!(application.keys.just_pressed(KeyCode::KeyA));
    assert_eq!(application.events.len(), 1);
}

#[test]
fn widgets_claim_pointer_presses_before_panel_input_but_viewport_regions_delegate_them() {
    for delegate in [false, true] {
        let ctx = egui::Context::default();
        let mut target = egui::pos2(10.0, 10.0);
        for pressed in [false, true] {
            let mut raw = egui::RawInput::default();
            raw.events.push(egui::Event::PointerMoved(target));
            if pressed {
                raw.events.push(egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let mut frame = input::UiInputFrame::default();
            if pressed {
                frame.events.push(input::InputEvent {
                    id: 1,
                    modifiers: default(),
                    event: WindowEvent::MouseButtonInput(bevy::input::mouse::MouseButtonInput {
                        window: Entity::PLACEHOLDER,
                        button: MouseButton::Left,
                        state: ButtonState::Pressed,
                    }),
                });
            }
            let mut output = ctx.run_ui(raw, |ui| {
                let response = ui.button("Interactive content");
                target = response.rect.center();
                if delegate {
                    frame.region(Entity::PLACEHOLDER, response.id, response.rect, false);
                }
            });
            output.textures_delta.clear();
            frame.claim_widgets(&ctx);
            if pressed {
                assert_eq!(frame.claims.contains_key(&1), !delegate);
            }
        }
    }
}
