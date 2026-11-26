use crate::*;
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
    let dark = Theme::dark();
    dark.validate().unwrap();
    dark.apply(&crate::Metrics::default(), &ctx);
    assert_eq!(
        ctx.global_style().visuals.panel_fill,
        theme::color(dark.palette.panel_background)
    );
    assert_eq!(
        ctx.global_style().visuals.hyperlink_color,
        theme::color(dark.palette.accent)
    );
    let light = Theme::light();
    light.validate().unwrap();
    light.apply(&crate::Metrics::default(), &ctx);
    assert_eq!(
        ctx.global_style().visuals.panel_fill,
        theme::color(light.palette.panel_background)
    );
    assert_eq!(
        ctx.global_style().visuals.hyperlink_color,
        theme::color(light.palette.accent)
    );
    let bad = crate::Metrics {
        font_size: f32::NAN,
        ..default()
    };
    assert!(bad.validate().is_err());
    let mut bad = dark;
    bad.palette.text[0] = 2.0;
    assert!(bad.validate().is_err());
}

#[test]
fn terminal_palettes_match_everforest_medium_ansi_order() {
    for (theme, expected) in [
        (
            Theme::dark(),
            [
                0x475258_u32,
                0xe67e80,
                0xa7c080,
                0xdbbc7f,
                0x7fbbb3,
                0xd699b6,
                0x83c092,
                0xd3c6aa,
            ],
        ),
        (
            Theme::light(),
            [
                0x5c6a72, 0xf85552, 0x8da101, 0xdfa000, 0x3a94c5, 0xdf69ba, 0x35a77c, 0xe6e2cc,
            ],
        ),
    ] {
        // Everforest Medium uses the same eight colors for normal and bright ANSI.
        for (index, color) in theme.palette.terminal.into_iter().enumerate() {
            let [_, r, g, b] = expected[index % 8].to_be_bytes();
            assert_eq!(
                theme::color(color).to_array(),
                [r, g, b, 255],
                "ANSI entry {index}"
            );
        }
    }
}

#[test]
fn terminal_colors_validate_every_entry_and_channel() {
    for theme in [Theme::dark(), Theme::light()] {
        for entry in 0..theme.palette.terminal.len() {
            for channel in 0..4 {
                for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01, 1.01] {
                    let mut invalid = theme.clone();
                    invalid.palette.terminal[entry][channel] = value;
                    assert!(
                        invalid.validate().is_err(),
                        "ANSI entry {entry}, channel {channel} must reject {value}"
                    );
                }
                for value in [0.0, 1.0] {
                    let mut boundary = theme.clone();
                    boundary.palette.terminal[entry][channel] = value;
                    boundary.validate().unwrap();
                }
            }
        }
    }
}

#[test]
fn themes_roundtrip_through_theme_files() {
    // Theme files hold 8-bit colors.
    let byte = |v: u8| v as f32 / 255.0;
    for preset in [Theme::dark(), Theme::light()] {
        let mut edited = preset.clone();
        edited.palette.terminal[0] = [byte(26), byte(51), byte(77), byte(102)];
        edited.palette.terminal[15] = [byte(153), byte(179), byte(204), 1.0];
        for theme in [preset, edited] {
            let text = theme.to_ron();
            assert!(text.contains("\"#"), "colors are #rrggbb: {text}");
            assert_eq!(Theme::from_ron(&text).unwrap(), theme);
        }
    }
    let mut text = Theme::dark().to_ron();
    text = text.replacen("\"#", "\"", 1);
    assert!(Theme::from_ron(&text).unwrap_err().contains("#rrggbb"));
}

#[test]
fn terminal_colors_are_named_and_only_named() {
    let theme = Theme::dark();
    let text = theme.to_ron();
    assert!(text.contains("bright_white:"), "{text}");
    assert_eq!(Theme::from_ron(&text).unwrap(), theme);
    // A mistake says what it is.
    let typo = text.replace("bright_red:", "brigt_red:");
    assert!(Theme::from_ron(&typo).unwrap_err().contains("brigt_red"));
    // The earlier format, a list of sixteen, isn't read.
    let named = text[text.find("terminal:").unwrap()..]
        .split_once("),")
        .unwrap()
        .0
        .to_owned()
        + "),";
    let colors: Vec<&str> = theme
        .palette
        .terminal
        .iter()
        .map(|_| "\"#000000\"")
        .collect();
    let list = format!("terminal: [{}],", colors.join(", "));
    assert!(Theme::from_ron(&text.replace(&named, &list)).is_err());
}

#[test]
fn scroll_track_clears_rounded_corners_without_insetting_content() {
    let ctx = egui::Context::default();
    let theme = Theme::dark();
    let corner_radius = 18.0;
    theme.apply(&crate::Metrics::default(), &ctx);
    ctx.global_style_mut(|s| {
        s.spacing.scroll = egui::style::ScrollStyle::solid();
        s.animation_time = 0.0;
    });
    let icons = Mutex::new(icon_cache::IconCache::default());
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
                    metrics: &crate::Metrics::default(),
                    motion: default(),
                    icons: &icons,
                };
                frame.scroll("overflow", corner_radius, |frame| {
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
            egui::Shape::Rect(r) if r.fill == theme::color(theme.palette.panel_background) => {
                Some(r.rect)
            }
            _ => None,
        });
        output.textures_delta.clear();
    }
    let track = track.expect("Visible scrollbar track");
    assert!(track.min.y >= bounds.min.y + corner_radius);
    assert!(track.max.y <= bounds.max.y - corner_radius);
    assert_eq!(
        body.min, bounds.min,
        "The scrolling content keeps its original origin"
    );
    assert_eq!(clip.y_range(), bounds.y_range(), "Only the track is inset");
}

#[derive(Component, Default)]
struct Counter(u32);
fn draw_counter(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>) {
    world.get_mut::<Counter>(context.view).unwrap().0 += 1;
    ui.label("An ordinary drawing function");
}

#[test]
fn borrowed_frames_execute_once_and_despawning_a_surface_takes_its_views() {
    let mut world = World::new();
    let surface = world.spawn(UiSurface).id();
    let view = register_view(&mut world, surface, draw_counter);
    world.entity_mut(view).insert(Counter::default());
    let part = world.spawn(InView(view)).id();
    let ctx = egui::Context::default();
    ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::new(1).unwrap());
    let icons = std::sync::Mutex::new(icon_cache::IconCache::default());
    let theme_data = Theme::dark();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let context = UiContext {
            view,
            surface,
            theme: &theme_data,
            metrics: &crate::Metrics::default(),
            motion: default(),
            icons: &icons,
        };
        let draw = world.get::<UiView>(view).unwrap().draw;
        draw(ui, &mut world, &context);
        ui.ctx()
            .request_discard("Do not replay stateful drawing systems");
    });
    output.textures_delta.clear();
    assert!(!output.shapes.is_empty());
    assert_eq!(world.get::<Counter>(view).unwrap().0, 1);
    world.despawn(surface);
    assert!(world.get_entity(view).is_err());
    assert!(world.get_entity(part).is_err());
}

#[test]
fn borrowed_props_compose_in_normal_egui_layout_and_keep_instances_independent() {
    struct Detail;
    impl crate::UiComponent for Detail {
        type Props<'a> = (&'a str, Entity);
        fn draw(
            ui: &mut egui::Ui,
            world: &mut World,
            _: &UiContext<'_>,
            (label, owner): Self::Props<'_>,
        ) {
            world.get_mut::<Counter>(owner).unwrap().0 += label.len() as u32;
            ui.label(label);
        }
    }
    struct Row;
    impl crate::UiComponent for Row {
        type Props<'a> = [(&'a str, Entity); 2];
        fn draw(
            ui: &mut egui::Ui,
            world: &mut World,
            context: &UiContext<'_>,
            props: Self::Props<'_>,
        ) {
            ui.horizontal(|ui| {
                for props in props {
                    Detail::draw(ui, world, context, props);
                }
            });
        }
    }
    let mut world = World::new();
    let a = world.spawn(Counter::default()).id();
    let b = world.spawn(Counter::default()).id();
    let theme = Theme::dark();
    let icons = default();
    let context = UiContext {
        view: a,
        surface: a,
        theme: &theme,
        metrics: &crate::Metrics::default(),
        motion: default(),
        icons: &icons,
    };
    let labels = [String::from("One"), String::from("Second")];
    let ui = egui::Context::default();
    let mut output = ui.run_ui(default(), |ui| {
        Row::draw(ui, &mut world, &context, [(&labels[0], a), (&labels[1], b)]);
    });
    output.textures_delta.clear();
    assert_eq!(world.get::<Counter>(a).unwrap().0, 3);
    assert_eq!(world.get::<Counter>(b).unwrap().0, 6);
}

#[test]
fn tooltip_contents_fade_at_their_final_size_and_pixel_grid() {
    let ctx = egui::Context::default();
    let theme = Theme::dark();
    theme.apply(&crate::Metrics::default(), &ctx);
    ctx.global_style_mut(|style| {
        style.animation_time = MotionPolicy::default().seconds;
        style.interaction.tooltip_delay = 0.0;
        style.interaction.show_tooltips_only_when_still = false;
    });
    let icons = std::sync::Mutex::new(icon_cache::IconCache::default());
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
                    metrics: &crate::Metrics::default(),
                    motion: default(),
                    icons: &icons,
                };
                shown = frame
                    .tooltip_ui(&response, |frame| {
                        frame.ui.label("Popup text");
                    })
                    .is_some();
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
    Theme::dark().apply(&crate::Metrics::default(), &ctx);
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
