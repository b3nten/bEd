use super::*;
use crate::motion::{Animations, PopupMotion, ScalarKind, Transition};
use imgui::{Condition, Context, FramePrepareOptions};

#[test]
fn retargeting_is_continuous_and_reduced_motion_settles() {
    let policy = MotionPolicy::default();
    let mut transition = Transition::default();
    transition.retarget(1.0);
    transition.advance(0.05, policy);
    let before = transition.value();
    assert!(before > 0.0 && before < 1.0);
    transition.retarget(0.0);
    assert_eq!(transition.value(), before);
    transition.advance(0.02, policy);
    assert!(transition.value() < before);
    transition.retarget(1.0);
    transition.advance(
        0.0,
        MotionPolicy {
            enabled: false,
            ..policy
        },
    );
    assert_eq!(transition.value(), 1.0);
}

#[test]
fn local_motion_uses_real_time_and_reduced_motion_settles_immediately() {
    let mut world = World::new();
    let view = world.spawn_empty().id();
    let mut clock = Time::<Real>::default();
    let mut simulation = Time::<Virtual>::default();
    simulation.pause();
    let mut cache = Animations::default();
    let policy = MotionPolicy::default();
    cache.begin_frame(1, clock.elapsed_secs_f64());
    assert_eq!(
        cache.scalar(view, 7, ScalarKind::Toggle, 0.0, 1.0, policy),
        0.0
    );
    clock.advance_by(std::time::Duration::from_millis(40));
    cache.begin_frame(2, clock.elapsed_secs_f64());
    let partial = cache.scalar(view, 7, ScalarKind::Toggle, 1.0, 1.0, policy);
    assert!(partial > 0.0 && partial < 1.0);
    assert_eq!(
        cache.scalar(
            view,
            7,
            ScalarKind::Toggle,
            1.0,
            0.0,
            MotionPolicy {
                enabled: false,
                ..policy
            }
        ),
        0.0
    );
    assert!(simulation.is_paused());
}

#[test]
fn animation_identity_follows_items_across_reordering_and_is_scoped_to_the_view() {
    let mut world = World::new();
    let view = world.spawn_empty().id();
    let other = world.spawn_empty().id();
    let mut cache = Animations::default();
    let policy = MotionPolicy::default();
    cache.begin_frame(1, 0.0);
    cache.scalar(view, 10, ScalarKind::Toggle, 0.0, 1.0, policy);
    cache.scalar(view, 20, ScalarKind::Toggle, 0.0, 0.0, policy);
    cache.finish_frame(|_| true);
    cache.begin_frame(2, 0.04);
    // The same items are painted in reverse order, without transferring motion.
    assert_eq!(
        cache.scalar(view, 20, ScalarKind::Toggle, 0.0, 0.0, policy),
        0.0
    );
    let partial = cache.scalar(view, 10, ScalarKind::Toggle, 1.0, 1.0, policy);
    assert!(partial > 0.0 && partial < 1.0);
    assert_eq!(
        cache.scalar(other, 10, ScalarKind::Toggle, 0.0, 0.0, policy),
        0.0
    );
}

#[test]
fn hidden_or_retired_controls_stop_redraws_and_reappear_without_stale_motion() {
    let view = World::new().spawn_empty().id();
    let policy = MotionPolicy::default();
    let mut cache = Animations::default();
    cache.begin_frame(1, 0.0);
    cache.scalar(view, 7, ScalarKind::Toggle, 0.0, 1.0, policy);
    assert_eq!(cache.finish_frame(|_| true).1, 1);
    cache.begin_frame(2, 0.04);
    assert_eq!(cache.finish_frame(|_| true).1, 0);
    cache.begin_frame(3, 0.08);
    cache.finish_frame(|_| true);
    cache.begin_frame(4, 0.12);
    assert_eq!(
        cache.scalar(view, 7, ScalarKind::Toggle, 1.0, 1.0, policy),
        1.0
    );
    cache.scalar(view, 8, ScalarKind::Toggle, 0.0, 1.0, policy);
    assert_eq!(
        cache.finish_frame(|_| false).1,
        0,
        "Retired views must release active motion"
    );
    cache.begin_frame(5, 0.16);
    assert_eq!(
        cache.scalar(view, 8, ScalarKind::Toggle, 0.0, 0.0, policy),
        0.0
    );
}

#[test]
fn surface_ownership_retires_views_and_parts_but_keeps_shared_themes() {
    let mut world = World::new();
    let theme = world.spawn(themes::light()).id();
    let surface = world.spawn((UiSurface, UsesTheme(theme))).id();
    let other = world.spawn((UiSurface, UsesTheme(theme))).id();
    let view = register_view(&mut world, surface, |_: UiFrame| {});
    let part = world.spawn((Counter::default(), InView(view))).id();
    world.despawn(surface);
    assert!(world.get_entity(view).is_err());
    assert!(world.get_entity(part).is_err());
    assert!(world.get::<Theme>(theme).is_some());
    assert!(world.get::<UsesTheme>(other).is_some());
    world.despawn(theme);
    assert!(world.get::<UsesTheme>(other).is_none());
}

#[derive(Component, Default)]
struct Counter(usize);
fn draw_counter(frame: UiFrame, mut state: Query<&mut Counter>) {
    state.get_mut(frame.view).unwrap().0 += 1;
    frame.ui.window("counter").build(|| {
        frame.controls().button("A themed button");
    });
}

#[test]
fn borrowed_frame_runs_regular_bevy_systems_and_closed_menus_skip_contents() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut world = World::new();
    let surface = world.spawn(UiSurface).id();
    let view = register_view(&mut world, surface, draw_counter);
    world.entity_mut(view).insert(Counter::default());
    let mut context = Context::create();
    context
        .set_ini_filename(None::<std::path::PathBuf>)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let theme = themes::dark();
    theme.apply(&mut context);
    context.prepare_frame(FramePrepareOptions::new([800.0, 600.0], 1.0 / 60.0));
    let frame = context.begin_frame();
    let animations = RefCell::new(Animations::default());
    let icons = RefCell::new(crate::icon_cache::IconCache::default());
    let viewports = RefCell::new(crate::viewport_textures::ViewportTextures::default());
    animations
        .borrow_mut()
        .begin_frame(frame.ui().frame_count(), 0.0);
    world
        .run_system_with(
            SystemId::<UiFrame<'static>>::from_entity(view),
            UiFrame {
                animation_owner: view,
                view,
                surface,
                ui: frame.ui(),
                theme: &theme,
                motion: MotionPolicy::default(),
                animations: &animations,
                icons: &icons,
                viewports: &viewports,
            },
        )
        .unwrap();
    let mut called = false;
    frame
        .ui()
        .window("menus")
        .position([10.0, 10.0], Condition::Always)
        .build(|| {
            let controls = Controls::new(
                frame.ui(),
                &theme,
                MotionPolicy::default(),
                view,
                &animations,
                &icons,
                &viewports,
            );
            for _ in 0..1000 {
                controls.button("row");
                controls.context_menu("closed", || {
                    called = true;
                });
            }
        });
    assert!(!called);
    assert_eq!(world.get::<Counter>(view).unwrap().0, 1);
    drop(frame);
}

#[test]
fn built_in_themes_are_valid_and_bad_external_values_are_rejected() {
    themes::light().validate().unwrap();
    themes::dark().validate().unwrap();
    let mut theme = themes::light();
    theme.palette.text[0] = f32::NAN;
    assert!(theme.validate().is_err());
    theme = themes::dark();
    theme.metrics.font_size = 0.0;
    assert!(theme.validate().is_err());
}

#[test]
fn selector_keeps_a_gap_at_either_edge_and_selects_duplicate_labels() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    for opens_below in [true, false] {
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let theme = themes::dark();
        theme.apply(&mut context);
        let view = World::new().spawn_empty().id();
        let animations = RefCell::new(Animations::default());
        let icons = RefCell::new(crate::icon_cache::IconCache::default());
        let viewports = RefCell::new(crate::viewport_textures::ViewportTextures::default());
        let mut selected = 0;
        let mut changes = 0;
        let mut target = [0.0; 2];
        for frame in 0..6 {
            if frame >= 2 {
                context.io_mut().add_mouse_pos_event(target);
            }
            if frame == 3 || frame == 4 {
                context
                    .io_mut()
                    .add_mouse_button_event(imgui::MouseButton::Left, frame == 3);
            }
            context.prepare_frame(FramePrepareOptions::new([400.0, 300.0], 1.0 / 60.0));
            let token = context.begin_frame();
            let ui = token.ui();
            animations
                .borrow_mut()
                .begin_frame(ui.frame_count(), frame as f64 / 60.0);
            ui.window("selector")
                .position([0.0, 0.0], Condition::Always)
                .size([400.0, 300.0], Condition::Always)
                .flags(imgui::WindowFlags::NO_DECORATION)
                .build(|| {
                    ui.set_cursor_pos([20.0, if opens_below { 30.0 } else { 250.0 }]);
                    let origin = ui.cursor_screen_pos();
                    if frame == 1 {
                        let _id = ui.push_id("Density");
                        ui.open_popup("##options");
                    }
                    ui.set_next_item_width(180.0);
                    let c = Controls::new(
                        ui,
                        &theme,
                        MotionPolicy {
                            enabled: false,
                            ..default()
                        },
                        view,
                        &animations,
                        &icons,
                        &viewports,
                    );
                    changes +=
                        usize::from(c.select("Density", &mut selected, &["Same", "Same", "Quiet"]));
                    if frame == 1 || frame == 2 {
                        ui.with_bound_context(|| unsafe {
                            let native = &*imgui::sys::igGetCurrentContext();
                            assert!(native.OpenPopupStack.Size > 0);
                            let popup = &*(*native.OpenPopupStack.Data).Window;
                            let height = ui.current_font_size() + theme.metrics.padding[1] * 2.0;
                            if opens_below {
                                assert_eq!(
                                    popup.Pos.y,
                                    origin[1] + height + theme.metrics.popup_gap
                                );
                            } else {
                                assert_eq!(
                                    popup.Pos.y + popup.Size.y,
                                    origin[1] - theme.metrics.popup_gap
                                );
                            }
                            target = [
                                popup.Pos.x + 30.0,
                                popup.Pos.y
                                    + theme.metrics.menu_padding[1]
                                    + (ui.current_font_size() + theme.metrics.menu_spacing[1])
                                        * 1.5,
                            ];
                        });
                    }
                });
            drop(token.render_legacy());
        }
        assert_eq!(
            selected, 1,
            "The second identically labelled option must remain independently selectable"
        );
        assert_eq!(changes, 1);
    }
}

#[test]
fn dense_lists_only_submit_visible_rows_and_open_menus_run_contents() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = Context::create();
    context
        .set_ini_filename(None::<std::path::PathBuf>)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let theme = themes::light();
    theme.apply(&mut context);
    context.prepare_frame(FramePrepareOptions::new([800.0, 600.0], 1.0 / 60.0));
    let token = context.begin_frame();
    let ui = token.ui();
    let animations = RefCell::new(Animations::default());
    let icons = RefCell::new(crate::icon_cache::IconCache::default());
    let viewports = RefCell::new(crate::viewport_textures::ViewportTextures::default());
    animations.borrow_mut().begin_frame(ui.frame_count(), 0.0);
    let view = World::new().spawn_empty().id();
    let c = Controls::new(
        ui,
        &theme,
        MotionPolicy::default(),
        view,
        &animations,
        &icons,
        &viewports,
    );
    let mut submitted = 0;
    let mut opened = false;
    ui.window("list")
        .position([0.0, 0.0], Condition::Always)
        .size([400.0, 300.0], Condition::Always)
        .build(|| {
            ui.child_window("rows").size([300.0, 150.0]).build(ui, || {
                c.list(10_000, |i| {
                    submitted += 1;
                    ui.text(format!("Row {i}"));
                });
            });
            c.button("source");
            ui.open_popup("open");
            c.context_menu("open", || {
                opened = true;
                c.text("Menu contents");
            });
        });
    assert!(
        submitted > 0 && submitted < 30,
        "Submitted {submitted} rows for a small viewport"
    );
    assert!(opened);
    drop(token);
}

#[test]
fn theme_switch_changes_base_style_without_replacing_fonts() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let context = Context::create();
    let font = context
        .font_atlas()
        .add_font(&[imgui::FontSource::default_font_with_size(16.0)]);
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let mut context = context;
    let dark = themes::dark();
    dark.apply(&mut context);
    assert_eq!(
        context.style().color(imgui::StyleColor::WindowBg),
        dark.palette.background
    );
    let light = themes::light();
    light.apply(&mut context);
    assert_eq!(
        context.style().color(imgui::StyleColor::WindowBg),
        light.palette.background
    );
    // A prepared font handle remains usable across theme changes.
    context.prepare_frame(FramePrepareOptions::new([320.0, 240.0], 1.0 / 60.0));
    let token = context.begin_frame();
    let _font = token.ui().push_font(font);
    token.ui().text("Still the same prepared font");
}

#[test]
fn popups_restart_for_new_sources_and_reopening_and_honor_reduced_motion() {
    let policy = MotionPolicy::default();
    let mut popup = PopupMotion::default();
    assert_eq!(popup.reveal(10, 7, policy), 0.0);
    popup.advance(0.04, policy);
    let partial = popup.reveal(11, 7, policy);
    assert!(partial > 0.0 && partial < 1.0);
    assert_eq!(
        popup.reveal(12, 8, policy),
        0.0,
        "A different tooltip/control restarts motion"
    );
    popup.advance(0.04, policy);
    assert_eq!(
        popup.reveal(16, 8, policy),
        0.0,
        "Reopening restarts motion"
    );
    assert_eq!(
        popup.reveal(
            17,
            8,
            MotionPolicy {
                enabled: false,
                ..policy
            }
        ),
        1.0
    );
}

#[test]
fn popup_scale_and_fade_transform_children_about_the_control_without_moving_layout() {
    use imgui::sys;
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = Context::create();
    context
        .set_ini_filename(None::<std::path::PathBuf>)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let mut popup = PopupMotion::default();
    let policy = MotionPolicy::default();
    let anchor = [40.0, 60.0];
    for frame in 0..3 {
        popup.advance(1.0 / 60.0, policy);
        context.prepare_frame(FramePrepareOptions::new([800.0, 600.0], 1.0 / 60.0));
        let token = context.begin_frame();
        let ui = token.ui();
        let mut ids = Vec::new();
        ui.window("host")
            .position([0.0, 0.0], Condition::Always)
            .size([700.0, 500.0], Condition::Always)
            .build(|| {
                ui.tooltip(|| {
                    ids.push(
                        ui.with_bound_context(|| unsafe { (*sys::igGetCurrentWindowRead()).ID }),
                    );
                    ui.text("A tooltip with custom-painted content");
                    ui.get_window_draw_list()
                        .add_rect([160.0, 180.0], [220.0, 210.0], [1.0, 0.0, 0.0, 1.0])
                        .filled(true)
                        .build();
                    ui.child_window("tooltip-child")
                        .size([180.0, 80.0])
                        .build(ui, || {
                            ids.push(ui.with_bound_context(|| unsafe {
                                (*sys::igGetCurrentWindowRead()).ID
                            }));
                            ui.text("A clipped child");
                        });
                    popup.reveal(ui.frame_count(), 7, policy);
                    popup.present(ui, anchor, policy.popup_initial_scale);
                });
            });
        let geometry = || {
            ui.with_bound_context(|| unsafe {
                let ctx = &*sys::igGetCurrentContext();
                (0..ctx.Windows.Size)
                    .filter_map(|i| {
                        let w = &**ctx.Windows.Data.add(i as usize);
                        if !ids.contains(&w.ID) {
                            return None;
                        }
                        let list = &*w.DrawList;
                        let vertices = (0..list.VtxBuffer.Size)
                            .map(|v| *list.VtxBuffer.Data.add(v as usize))
                            .collect::<Vec<_>>();
                        let clips = (0..list.CmdBuffer.Size)
                            .map(|c| (*list.CmdBuffer.Data.add(c as usize)).ClipRect)
                            .collect::<Vec<_>>();
                        Some((w.ID, w.Pos, w.Size, vertices, clips))
                    })
                    .collect::<Vec<_>>()
            })
        };
        let before = geometry();
        let effect = popup.presentation.unwrap();
        crate::motion::apply_popups(ui, &[effect]);
        let after = geometry();
        if frame == 2 {
            assert!(effect.alpha > 0.0 && effect.alpha < 1.0);
            assert!(effect.scale > policy.popup_initial_scale && effect.scale < 1.0);
            assert_eq!(before.len(), 2);
            for (old, new) in before.iter().zip(&after) {
                assert_eq!(old.1, new.1, "Hit-test position stays final");
                assert_eq!(old.2, new.2, "Layout size stays final");
                assert!(!old.3.is_empty());
                for (a, b) in old.3.iter().zip(&new.3) {
                    assert!(
                        (b.pos.x - (anchor[0] + (a.pos.x - anchor[0]) * effect.scale)).abs()
                            < 0.001
                    );
                    assert!(
                        (b.pos.y - (anchor[1] + (a.pos.y - anchor[1]) * effect.scale)).abs()
                            < 0.001
                    );
                    assert_eq!(
                        b.col >> 24,
                        ((a.col >> 24) as f32 * effect.alpha).round() as u32
                    );
                }
                for (a, b) in old.4.iter().zip(&new.4) {
                    assert!((b.x - (anchor[0] + (a.x - anchor[0]) * effect.scale)).abs() < 0.001);
                    assert!((b.w - (anchor[1] + (a.w - anchor[1]) * effect.scale)).abs() < 0.001);
                }
            }
        }
        drop(token);
    }
}

#[test]
fn hovered_button_uses_the_interpolated_color_in_its_draw_output() {
    let _lock = IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = Context::create();
    context
        .set_ini_filename(None::<std::path::PathBuf>)
        .unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let theme = themes::dark();
    theme.apply(&mut context);
    let mut reference = Transition::default();
    let animations = RefCell::new(Animations::default());
    let icons = RefCell::new(crate::icon_cache::IconCache::default());
    let viewports = RefCell::new(crate::viewport_textures::ViewportTextures::default());
    let view = World::new().spawn_empty().id();
    for frame in 0..3 {
        context.io_mut().add_mouse_pos_event([40.0, 30.0]);
        context.prepare_frame(FramePrepareOptions::new([400.0, 300.0], 1.0 / 60.0));
        let token = context.begin_frame();
        let ui = token.ui();
        animations
            .borrow_mut()
            .begin_frame(ui.frame_count(), frame as f64 / 60.0);
        reference.advance(1.0 / 60.0, MotionPolicy::default());
        let mut hovered = false;
        ui.window("hover")
            .position([0.0, 0.0], Condition::Always)
            .size([300.0, 200.0], Condition::Always)
            .flags(imgui::WindowFlags::NO_DECORATION)
            .build(|| {
                Controls::new(
                    ui,
                    &theme,
                    MotionPolicy::default(),
                    view,
                    &animations,
                    &icons,
                    &viewports,
                )
                .button("Animated");
                hovered = ui.is_item_hovered();
            });
        let expected = ui.get_color_u32_from_rgba(crate::theme::mix(
            theme.palette.surface,
            theme.palette.accent,
            reference.value() * 0.25,
        ));
        reference.retarget(if hovered { 1.0 } else { 0.0 });
        let rendered = token.render_legacy();
        if frame == 2 {
            assert!(hovered);
            assert!(
                rendered
                    .draw_data()
                    .draw_lists()
                    .any(|list| list.vtx_buffer().iter().any(|v| v.col == expected)),
                "Hovered background must retain its interpolated color"
            );
        }
    }
}
