//! Example-only verification of the real renderer. No OS input is injected.
use crate::viewport_demo::DemoState;
use bed2_demo_game::GameCamera;
use bedtime_ui2::{UiDiagnostics, UiViewport};
use bedtime_workbench2::{AreaPresentation, InArea};
use bedtime_workbench2::{PanelPresentation, Workbench};
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
    window::PrimaryWindow,
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Resource, Clone, Default)]
struct PipelinesReady(Arc<AtomicBool>);

#[derive(Resource, Default)]
struct Smoke {
    directory: PathBuf,
    frame: u32,
    attempts: u32,
    captures: HashMap<&'static str, (u64, [u8; 3])>,
    initial_size: UVec2,
    camera: Option<Entity>,
    drag_origin: Vec2,
    drag_target: Vec2,
    tabs_verified: bool,
    hover_tab: Option<(Entity, Rect)>,
    themes_before: Option<(Entity, [bedtime_ui2::Theme; 2])>,
    content_fade: Option<ContentFadeCheck>,
}

struct ContentFadeCheck {
    previous_layout: Entity,
    layout: Entity,
    panel: Entity,
    destination: Entity,
    policy: bedtime_ui2::MotionPolicy,
    last_opacity: f32,
}

#[derive(Component)]
struct ContentFadeSample {
    opacity: f32,
    nested_opacity: f32,
    marker: bedtime_ui2::egui::Rect,
}
#[derive(Component)]
struct Region {
    name: &'static str,
    rect: Rect,
    window: Vec2,
}
pub(crate) fn install(app: &mut App) {
    let mut args = std::env::args();
    if args.any(|arg| arg == "--viewport-smoke-dir" || arg == "--workbench-smoke-dir") {
        let directory = PathBuf::from(args.next().expect("--viewport-smoke-dir needs a directory"));
        std::fs::create_dir_all(&directory).unwrap();
        let ready = PipelinesReady::default();
        app.insert_resource(ready.clone());
        app.sub_app_mut(bevy::render::RenderApp)
            .insert_resource(ready)
            .add_systems(
                bevy::render::Render,
                (|cache: Res<bevy::render::render_resource::PipelineCache>,
                  ready: Res<PipelinesReady>| {
                    ready.0.store(
                        cache.waiting_pipelines().next().is_none(),
                        Ordering::Release,
                    );
                })
                .in_set(bevy::render::RenderSystems::Cleanup),
            );
        app.insert_resource(Smoke {
            directory,
            ..default()
        })
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(1.0 / 60.0),
        ))
        .add_systems(Update, step)
        .init_resource::<crate::panel_smoke::ScriptedPointer>()
        .add_systems(
            PreUpdate,
            crate::panel_smoke::flush_pointer
                .after(bevy::input::InputSystems)
                .before(bevy_egui::EguiPreUpdateSet::ProcessInput),
        )
        .add_systems(
            PreUpdate,
            crate::panel_smoke::isolate_pointer
                .after(bevy_egui::EguiPreUpdateSet::ProcessInput)
                .before(bedtime_ui2::UiSystems::Draw),
        );
    }
}

fn step(world: &mut World) {
    let ready = world.resource::<PipelinesReady>().0.load(Ordering::Acquire);
    let frame = {
        let mut smoke = world.resource_mut::<Smoke>();
        smoke.attempts += 1;
        assert!(
            smoke.attempts < 4000,
            "Viewport pipelines/captures timed out"
        );
        if !ready {
            return;
        }
        smoke.frame += 1;
        smoke.frame
    };
    let window = world
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(world)
        .unwrap();
    // Scripted input is independent of OS focus (the user may be in another
    // application). Supply focus observations only inside this smoke fixture.
    if !world.get::<Window>(window).unwrap().focused {
        world.get_mut::<Window>(window).unwrap().focused = true;
        world.write_message(bevy::window::WindowEvent::WindowFocused(
            bevy::window::WindowFocused {
                window,
                focused: true,
            },
        ));
    }
    let view = world
        .query_filtered::<Entity, With<DemoState>>()
        .iter(world)
        .next();
    if frame == 380 {
        let camera = world
            .query_filtered::<Entity, With<GameCamera>>()
            .single(world)
            .unwrap();
        world.resource_mut::<Smoke>().camera = Some(camera);
        world.resource_mut::<Smoke>().initial_size = world
            .get::<UiViewport>(view.unwrap())
            .unwrap()
            .physical_size;
        let viewport = world.get::<UiViewport>(view.unwrap()).unwrap();
        let panel = world.get::<PanelPresentation>(view.unwrap()).unwrap();
        assert!(
            (viewport.rect.min.x - panel.rect.min.x).abs() < 0.05,
            "Scene reaches the left content edge"
        );
        assert!(
            (viewport.rect.max - panel.rect.max).abs().max_element() < 0.05,
            "Scene reaches the right and bottom content edges"
        );
        let gallery = main_gallery(world);
        let panel = world.get::<PanelPresentation>(gallery).unwrap();
        let state = world
            .get::<bedtime_ui2::gallery::GalleryState>(gallery)
            .unwrap();
        let padding = state.observations["tooltip"].min.x - panel.rect.min.x;
        // egui Frames align their margins to physical pixels.
        assert!(
            padding >= 13.0,
            "Gallery opts into content padding, got {padding}"
        );
    }
    if std::env::args().any(|a| a == "--blur") {
        // Exercise material independently of tint: translucent without blur,
        // then opaque with blur, before restoring the demo's initial settings.
        if frame == 420 || frame == 470 {
            world
                .entity_mut(window)
                .insert(bedtime_workbench2::WindowBlur(frame == 470));
        }
        if frame == 515 || frame == 550 {
            for mut theme in world.query::<&mut bedtime_ui2::Theme>().iter_mut(world) {
                theme.palette.background[3] = if frame == 515 { 1.0 } else { 0.85 };
            }
        }
    }
    // Resize the actual egui window using native-shaped events through our adapter.
    // This exercises image resizing, camera projection refresh and GPU view replacement.
    if frame == 475 {
        let gallery = main_gallery(world);
        let area = world.get::<InArea>(gallery).unwrap().0;
        let rect = world.get::<AreaPresentation>(area).unwrap().rect;
        world.resource_mut::<Smoke>().drag_origin = Vec2::new(rect.max.x, rect.center().y);
    }
    if frame == 850 {
        let gallery = main_gallery(world);
        Workbench::new(world)
            .move_panel(
                view.unwrap(),
                gallery,
                Some(bedtime_workbench2::SplitAxis::Horizontal),
            )
            .unwrap();
    }
    if frame == 865 {
        let gallery = main_gallery(world);
        let origin = world
            .get::<PanelPresentation>(view.unwrap())
            .unwrap()
            .tab_rect
            .center();
        let target = world
            .get::<PanelPresentation>(gallery)
            .unwrap()
            .tab_rect
            .center();
        let mut smoke = world.resource_mut::<Smoke>();
        smoke.drag_origin = origin;
        smoke.drag_target = target;
    }
    if frame == 885 {
        let gallery = main_gallery(world);
        world.resource_mut::<Smoke>().drag_origin = world
            .get::<PanelPresentation>(gallery)
            .unwrap()
            .tab_rect
            .center();
    }
    if frame == 910 {
        world.resource_mut::<Smoke>().drag_origin = world
            .get::<PanelPresentation>(view.unwrap())
            .unwrap()
            .tab_rect
            .center();
    }
    let origin = world.resource::<Smoke>().drag_origin;
    let target = world.resource::<Smoke>().drag_target;
    let pointer = match frame {
        480..=483 => Some(origin.to_array()),
        484..=487 => Some((origin + Vec2::new(-100.0, 100.0)).to_array()),
        870..=873 | 890..=895 | 920..=925 => Some(origin.to_array()),
        874..=877 => Some(target.to_array()),
        _ => None,
    };
    if let Some([x, y]) = pointer {
        let button = if [482, 486, 872, 876, 892, 894, 922, 924].contains(&frame) {
            Some((MouseButton::Left, [482, 872, 892, 922].contains(&frame)))
        } else {
            None
        };
        crate::panel_smoke::pointer(world, Vec2::new(x, y), button);
    }
    if frame == 510 {
        let viewport = world.get::<UiViewport>(view.unwrap()).unwrap();
        assert_ne!(
            viewport.physical_size,
            world.resource::<Smoke>().initial_size,
            "The native drag must resize the scene target"
        );
        assert_eq!(
            world
                .resource::<Assets<Image>>()
                .get(&viewport.image)
                .unwrap()
                .size(),
            viewport.physical_size
        );
    }
    if frame == 560 {
        world.get_mut::<DemoState>(view.unwrap()).unwrap().overlay = true;
    }
    if frame == 680 {
        Workbench::new(world).close_panel(view.unwrap()).unwrap();
    }
    if frame == 700 {
        assert_eq!(
            world
                .get::<UiDiagnostics>(window)
                .unwrap()
                .viewport_textures,
            0
        );
        let camera = world.resource::<Smoke>().camera.unwrap();
        assert!(!world.get::<Camera>(camera).unwrap().is_active);
    }
    if frame == 710 {
        crate::viewport_demo::show(world);
    }
    let name = match frame {
        400 => Some("panel"),
        460 => Some("spinning"),
        530 => Some("resized"),
        620 => Some("overlay"),
        770 => Some("reopened"),
        _ => None,
    };
    if let Some(name) = name {
        let size = world.get::<Window>(window).unwrap().resolution.size();
        let rect = if name == "overlay" {
            Rect::from_corners(size * 0.25, size * 0.75)
        } else {
            world.get::<UiViewport>(view.unwrap()).unwrap().rect
        };
        world
            .spawn((
                Screenshot::primary_window(),
                Region {
                    name,
                    rect,
                    window: size,
                },
            ))
            .observe(inspect);
    }
    // Exercise retention through the public lifecycle API, after visual checks.
    if frame == 800 {
        let context = world.resource::<crate::workbench_demo::DemoWorkbench>();
        let (reading, scratch) = (context.reading, context.scratch);
        let mut w = Workbench::new(world);
        w.activate_layout(reading).unwrap();
        w.activate_workspace(scratch).unwrap();
    }
    if frame == 805 {
        let cube = world
            .query_filtered::<Entity, With<DemoState>>()
            .single(world)
            .unwrap();
        assert!(!world.get::<PanelPresentation>(cube).unwrap().visible);
        let camera = world.resource::<Smoke>().camera.unwrap();
        assert!(!world.get::<Camera>(camera).unwrap().is_active);
        let note = world
            .query::<&crate::workbench_demo::Notes>()
            .single(world)
            .unwrap();
        assert!(note.text.contains("panel-local state"));
        let context = world.resource::<crate::workbench_demo::DemoWorkbench>();
        let (home, split) = (context.home, context.split);
        let mut w = Workbench::new(world);
        w.activate_workspace(home).unwrap();
        w.activate_layout(split).unwrap();
    }
    if frame == 880 {
        let gallery = main_gallery(world);
        let a = world.get::<PanelPresentation>(gallery).unwrap().tab_rect;
        let b = world
            .get::<PanelPresentation>(view.unwrap())
            .unwrap()
            .tab_rect;
        assert!(
            (a.center().x - b.center().x).abs() < 400.0,
            "Native tab drag must merge the split groups"
        );
    }
    if frame == 900 {
        let gallery = main_gallery(world);
        assert!(
            world.get::<PanelPresentation>(gallery).unwrap().visible,
            "Native click must select the gallery tab"
        );
        assert!(
            !world
                .get::<PanelPresentation>(view.unwrap())
                .unwrap()
                .visible
        );
        assert!(
            !world
                .get::<Camera>(world.resource::<Smoke>().camera.unwrap())
                .unwrap()
                .is_active
        );
        let rect = world.get::<PanelPresentation>(gallery).unwrap().rect;
        let size = world.get::<Window>(window).unwrap().resolution.size();
        world
            .spawn((
                Screenshot::primary_window(),
                Region {
                    name: "tabs",
                    rect,
                    window: size,
                },
            ))
            .observe(inspect_tabs);
    }
    crate::panel_smoke::step(world, frame);
    // Status widgets use real input and resize with the window independently of
    // the panel/workspace implementation that contributes their contents.
    if frame == 2370 {
        let shell = *world
            .query::<&bedtime_workbench2::WorkbenchPresentation>()
            .single(world)
            .unwrap();
        assert_eq!(shell.titlebar.min, shell.bounds.min);
        assert_eq!(shell.menu.min.y, shell.titlebar.max.y);
        assert_eq!(shell.panels.min.y, shell.menu.max.y);
        assert_eq!(shell.panels.max.y, shell.status_bar.min.y);
        assert_eq!(shell.status_bar.max, shell.bounds.max);
        let host = world
            .query::<&bedtime_workbench2::WorkbenchHost>()
            .single(world)
            .unwrap();
        let native_menu = world
            .get::<bedtime_workbench2::NativeMenuBar>(host.menu.unwrap())
            .is_some()
            && world
                .get_resource::<bedtime_workbench2::NativeCapabilities>()
                .is_some_and(|capabilities| capabilities.native_menus);
        assert_eq!(
            shell.menu.height() == 0.0,
            native_menu,
            "Only an in-app menu reserves a shell row"
        );
        let bar = world
            .query::<&bedtime_ui2::StatusBar>()
            .single(world)
            .unwrap();
        assert_eq!(
            bar.rect.max.y,
            world.get::<Window>(window).unwrap().height()
        );
        let height = bar.rect.height();
        assert_eq!(
            world
                .query::<&bedtime_workbench2::WorkbenchPresentation>()
                .single(world)
                .unwrap()
                .status_bar
                .height(),
            height
        );
        assert!(
            world
                .query::<&bedtime_ui2::StatusItemPresentation>()
                .iter(world)
                .all(|p| p.visible && !p.collapsed && p.icon_rect.max.x < p.content_rect.min.x)
        );
    }
    if frame == 2380 {
        let point = world
            .query::<(
                &bedtime_ui2::StatusItem,
                &bedtime_ui2::StatusItemPresentation,
            )>()
            .iter(world)
            .find_map(|(item, presentation)| {
                (item.label == "Motion setting").then_some(presentation.content_rect.center())
            })
            .unwrap();
        world.resource_mut::<Smoke>().drag_origin = point;
    }
    if (2380..=2396).contains(&frame) {
        let point = world.resource::<Smoke>().drag_origin;
        crate::panel_smoke::pointer(
            world,
            point,
            match frame {
                2382 | 2392 => Some((MouseButton::Left, true)),
                2384 | 2394 => Some((MouseButton::Left, false)),
                _ => None,
            },
        );
    }
    if frame == 2390 {
        assert!(
            !world
                .get::<bedtime_ui2::MotionPolicy>(window)
                .unwrap()
                .enabled,
            "Status widget receives clicks before panel input"
        );
    }
    if frame == 2400 {
        assert!(
            world
                .get::<bedtime_ui2::MotionPolicy>(window)
                .unwrap()
                .enabled
        );
        world
            .get_mut::<Window>(window)
            .unwrap()
            .resolution
            .set(700.0, 700.0);
    }
    if frame == 2440 {
        let (item, presentation) = world
            .query::<(
                &bedtime_ui2::StatusItem,
                &bedtime_ui2::StatusItemPresentation,
            )>()
            .iter(world)
            .find(|(item, _)| item.label == "UI rendering")
            .unwrap();
        assert!(
            presentation.collapsed && presentation.visible,
            "{} collapses to its icon",
            item.label
        );
        world.resource_mut::<Smoke>().drag_origin = presentation.icon_rect.center();
    }
    if (2440..=2495).contains(&frame) {
        let point = world.resource::<Smoke>().drag_origin;
        crate::panel_smoke::pointer(world, point, None);
    }
    if frame == 2495 {
        let presentation = world
            .query::<(
                &bedtime_ui2::StatusItem,
                &bedtime_ui2::StatusItemPresentation,
            )>()
            .iter(world)
            .find_map(|(item, presentation)| (item.label == "UI rendering").then_some(presentation))
            .unwrap();
        assert!(
            presentation.tooltip_visible,
            "Collapsed status retains its plugin tooltip"
        );
    }
    if frame == 2500 {
        world
            .get_mut::<Window>(window)
            .unwrap()
            .resolution
            .set(1440.0, 960.0);
        crate::panel_smoke::pointer(world, Vec2::new(900.0, 500.0), None);
    }
    if frame == 2540 {
        assert!(
            world
                .query::<&bedtime_ui2::StatusItemPresentation>()
                .iter(world)
                .all(|p| p.visible && !p.collapsed),
            "Status items expand again when space returns"
        );
        info!(
            "Status bar smoke passed: reserved height, icon ordering, widget clicks, icon collapse, tooltips and expansion"
        );
    }
    if frame == 2542 {
        let gallery = main_gallery(world);
        let rect = world.get::<PanelPresentation>(gallery).unwrap().tab_rect;
        world.resource_mut::<Smoke>().hover_tab = Some((gallery, rect));
    }
    if (2542..=2578).contains(&frame) {
        let (tab, rect) = world.resource::<Smoke>().hover_tab.unwrap();
        assert_eq!(
            world.get::<PanelPresentation>(tab).unwrap().tab_rect,
            rect,
            "Hover backgrounds must not move tab geometry"
        );
        crate::panel_smoke::pointer(world, rect.center(), None);
    }
    if frame == 2580 {
        let bounds = Rect::from_corners(Vec2::new(80.0, 100.0), Vec2::new(1380.0, 900.0));
        world
            .query::<&mut bedtime_workbench2::WorkbenchHost>()
            .single_mut(world)
            .unwrap()
            .bounds = Some(bounds);
    }
    if frame == 2590 {
        let shell = *world
            .query::<&bedtime_workbench2::WorkbenchPresentation>()
            .single(world)
            .unwrap();
        assert_eq!(
            shell.bounds,
            Rect::from_corners(Vec2::new(80.0, 100.0), Vec2::new(1380.0, 900.0))
        );
        assert_eq!(
            shell.titlebar.height(),
            0.0,
            "Inset workbenches do not reserve native chrome outside their bounds"
        );
        assert_eq!(shell.status_bar.max, shell.bounds.max);
        assert_eq!(shell.panels.max.y, shell.status_bar.min.y);
        assert_eq!(
            world
                .query::<&bedtime_ui2::StatusBar>()
                .single(world)
                .unwrap()
                .rect,
            shell.status_bar
        );
    }
    if frame == 2592 {
        world
            .query::<&mut bedtime_workbench2::WorkbenchHost>()
            .single_mut(world)
            .unwrap()
            .bounds = None;
        world
            .query::<&mut bevy_egui::EguiContext>()
            .single_mut(world)
            .unwrap()
            .get_mut()
            .set_zoom_factor(1.5);
    }
    if frame == 2610 {
        let shell = *world
            .query::<&bedtime_workbench2::WorkbenchPresentation>()
            .single(world)
            .unwrap();
        let window_size = world.get::<Window>(window).unwrap().resolution.size();
        let native_top = world
            .get::<bedtime_workbench2::WindowTitlebarInset>(window)
            .unwrap()
            .0;
        assert!((shell.bounds.width() - window_size.x / 1.5).abs() < 0.1);
        assert!(
            (shell.titlebar.height() - native_top / 1.5).abs() < 0.1,
            "Native titlebar measurements must be converted to egui points"
        );
        assert_eq!(shell.panels.min.y, shell.menu.max.y);
        assert_eq!(shell.panels.max.y, shell.status_bar.min.y);
        info!(
            "Shell layout smoke passed: native/menu/status reservations, inset bounds and UI zoom"
        );
    }
    if frame == 2612 {
        world
            .query::<&mut bevy_egui::EguiContext>()
            .single_mut(world)
            .unwrap()
            .get_mut()
            .set_zoom_factor(1.0);
    }
    if frame == 2620 {
        crate::panel_smoke::pointer(world, Vec2::new(1100.0, 600.0), None);
        assert!(
            world
                .get::<bedtime_workbench2::PanelSettings>(window)
                .is_none()
        );
        assert!(
            painted_corner_targets(world).is_empty(),
            "Corner debug is off by default"
        );
    }
    if frame == 2622 || frame == 2626 {
        world
            .entity_mut(window)
            .insert(bedtime_workbench2::PanelSettings {
                debug: true,
                hit_size: if frame == 2622 { 8.0 } else { 28.0 },
                panel_gap: if frame == 2622 { 0.0 } else { 20.0 },
                ..default()
            });
    }
    if frame == 2624 || frame == 2628 {
        let size = if frame == 2624 { 8.0 } else { 28.0 };
        let targets = painted_corner_targets(world);
        let gap = world
            .get::<bedtime_workbench2::PanelSettings>(window)
            .unwrap()
            .panel_gap;
        let areas = world
            .query::<(&bedtime_workbench2::Area, &AreaPresentation)>()
            .iter(world)
            .filter(|(_, area)| area.visible)
            .map(|(model, area)| {
                let model = model.rect();
                for axis in 0..2 {
                    let low = if model.min[axis] == 0 { gap } else { gap * 0.5 };
                    let high = if model.max[axis] == bedtime_workbench2::LAYOUT_EXTENT {
                        gap
                    } else {
                        gap * 0.5
                    };
                    assert!((area.surface.min[axis] - area.rect.min[axis] - low).abs() < 0.001);
                    assert!((area.rect.max[axis] - area.surface.max[axis] - high).abs() < 0.001);
                }
                area.rect
            })
            .collect::<Vec<_>>();
        assert_eq!(targets.len(), areas.len() * 4);
        for area in areas {
            for anchor in [
                area.min,
                Vec2::new(area.max.x, area.min.y),
                Vec2::new(area.min.x, area.max.y),
                area.max,
            ] {
                assert!(
                    targets.iter().any(|target| {
                        (target.width() - size).abs() < 0.01
                            && (target.height() - size).abs() < 0.01
                            && [
                                target.left_top(),
                                target.right_top(),
                                target.left_bottom(),
                                target.right_bottom(),
                            ]
                            .iter()
                            .any(|corner| {
                                (Vec2::new(corner.x, corner.y) - anchor).abs().max_element() < 0.01
                            })
                    }),
                    "Target stays anchored at its area's outside corner as size changes"
                );
            }
        }
    }
    if frame == 2630 {
        world
            .entity_mut(window)
            .remove::<bedtime_workbench2::PanelSettings>();
    }
    if frame == 2634 {
        assert!(painted_corner_targets(world).is_empty());
        info!(
            "Panel settings smoke passed: live panel gaps, debug visibility and outside-corner sizing"
        );
    }
    if frame == 2635 {
        let context = world.resource::<crate::workbench_demo::DemoWorkbench>();
        let themes = [context.light, context.dark]
            .map(|entity| world.get::<bedtime_ui2::Theme>(entity).unwrap().clone());
        let active = world.get::<bedtime_ui2::UsesTheme>(window).unwrap().0;
        world.resource_mut::<Smoke>().themes_before = Some((active, themes));
        let gallery = main_gallery(world);
        Workbench::new(world).focus_panel(gallery).unwrap();
    }
    if frame == 2635 || frame == 2645 {
        let command = world
            .query::<(&bedtime_workbench2::MenuItem, &bedtime_workbench2::Invokes)>()
            .iter(world)
            .find_map(|(item, invokes)| {
                (item.label == "Open Theme Editor").then_some(invokes.0.clone())
            })
            .unwrap();
        world
            .resource_mut::<bedtime_workbench2::ContextCommandRequests>()
            .invoke_id(command);
    }
    if frame == 2640 {
        let panel = world
            .query_filtered::<Entity, With<crate::theme_editor::ThemeEditor>>()
            .single(world)
            .unwrap();
        assert!(world.get::<PanelPresentation>(panel).unwrap().visible);
    }
    if frame == 2645 || frame == 2665 {
        let context = world.resource::<crate::workbench_demo::DemoWorkbench>();
        let theme = if frame == 2645 {
            context.dark
        } else {
            context.light
        };
        world
            .entity_mut(window)
            .insert(bedtime_ui2::UsesTheme(theme));
        let mut theme = world.get_mut::<bedtime_ui2::Theme>(theme).unwrap();
        theme.palette.accent = [0.2, 0.4, 0.6, 0.5];
        theme.metrics.font_size = 20.0;
        theme.metrics.padding = [12.0, 9.0];
    }
    if frame == 2645 {
        world
            .entity_mut(window)
            .insert(bedtime_workbench2::PanelSettings {
                rounding: 18.0,
                padding: [18.0, 16.0],
                panel_gap: 12.0,
                ..default()
            });
    }
    for base in [2650, 2670] {
        if frame == base || frame == base + 2 {
            let output = world
                .query::<&bevy_egui::EguiFullOutput>()
                .single(world)
                .unwrap();
            let point = output
                .0
                .as_ref()
                .unwrap()
                .shapes
                .iter()
                .find_map(|shape| {
                    if let bedtime_ui2::egui::Shape::Text(text) = &shape.shape {
                        (text.galley.job.text == "Reset theme").then_some(
                            Vec2::new(text.pos.x, text.pos.y)
                                + Vec2::new(text.galley.size().x, text.galley.size().y) * 0.5,
                        )
                    } else {
                        None
                    }
                })
                .expect("Theme editor reset button is visible");
            crate::panel_smoke::pointer(world, point, Some((MouseButton::Left, frame == base)));
        }
        if frame == base + 8 {
            assert_eq!(
                world
                    .query::<&crate::theme_editor::ThemeEditor>()
                    .iter(world)
                    .count(),
                1,
                "Opening the theme editor again focuses the existing panel"
            );
            let active = world.get::<bedtime_ui2::UsesTheme>(window).unwrap().0;
            let expected = if base == 2650 {
                bedtime_ui2::themes::dark()
            } else {
                bedtime_ui2::themes::light()
            };
            assert_eq!(world.get::<bedtime_ui2::Theme>(active).unwrap(), &expected);
            let settings = world
                .get::<bedtime_workbench2::PanelSettings>(window)
                .unwrap();
            assert_eq!(
                settings.rounding, 18.0,
                "Theme reset preserves panel rounding"
            );
            assert_eq!(
                settings.padding,
                [18.0, 16.0],
                "Theme reset preserves panel padding"
            );
            assert_eq!(settings.panel_gap, 12.0, "Theme reset preserves panel gaps");
        }
    }
    if frame == 2680 {
        world
            .entity_mut(window)
            .remove::<bedtime_workbench2::PanelSettings>();
        let (active, themes) = world.resource_mut::<Smoke>().themes_before.take().unwrap();
        let context = world.resource::<crate::workbench_demo::DemoWorkbench>();
        let entities = [context.light, context.dark];
        for (entity, theme) in entities.into_iter().zip(themes) {
            *world.get_mut::<bedtime_ui2::Theme>(entity).unwrap() = theme;
        }
        world
            .entity_mut(window)
            .insert(bedtime_ui2::UsesTheme(active));
        let panel = world
            .query_filtered::<Entity, With<crate::theme_editor::ThemeEditor>>()
            .single(world)
            .unwrap();
        Workbench::new(world).close_panel(panel).unwrap();
        let cube = world
            .query_filtered::<Entity, With<DemoState>>()
            .single(world)
            .unwrap();
        Workbench::new(world).focus_panel(cube).unwrap();
        info!(
            "Theme editor smoke passed: opening/focusing, live theme changes and light/dark default resets"
        );
    }
    if frame == 2700 {
        let policy = *world.get::<bedtime_ui2::MotionPolicy>(window).unwrap();
        *world.get_mut::<bedtime_ui2::MotionPolicy>(window).unwrap() = bedtime_ui2::MotionPolicy {
            enabled: true,
            seconds: 0.16,
        };
        let mut w = Workbench::new(world);
        let previous_layout = w.active_layout();
        let workspace = w.active_workspace();
        let layout = w.create_layout(workspace, "Content fade check").unwrap();
        w.activate_layout(layout).unwrap();
        let area = w.destination_area(layout).unwrap();
        w.create_panel_in_area(area, "Retained").unwrap();
        let destination = w
            .split_area(
                area,
                bedtime_workbench2::SplitAxis::Horizontal,
                5000,
                [100, 100],
            )
            .unwrap();
        let panel = bedtime_workbench2::register_panel_in_area(
            world,
            area,
            "Content fade",
            |frame: bedtime_ui2::UiFrame, mut samples: Query<&mut ContentFadeSample>| {
                let mut sample = samples.get_mut(frame.view).unwrap();
                sample.opacity = frame.ui.opacity();
                sample.marker = frame
                    .ui
                    .allocate_exact_size(
                        bedtime_ui2::egui::vec2(20.0, 20.0),
                        bedtime_ui2::egui::Sense::hover(),
                    )
                    .0;
                frame.ui.painter().rect_filled(
                    sample.marker,
                    0.0,
                    bedtime_ui2::egui::Color32::WHITE,
                );
                frame.ui.scope(|ui| {
                    sample.nested_opacity = ui.opacity();
                    ui.label("Nested panel content");
                });
            },
        )
        .unwrap();
        world.entity_mut(panel).insert(ContentFadeSample {
            opacity: 0.0,
            nested_opacity: 0.0,
            marker: bedtime_ui2::egui::Rect::NOTHING,
        });
        world.resource_mut::<Smoke>().content_fade = Some(ContentFadeCheck {
            previous_layout,
            layout,
            panel,
            destination,
            policy,
            last_opacity: 0.0,
        });
    }
    if frame == 2713 {
        world
            .get_mut::<bedtime_ui2::MotionPolicy>(window)
            .unwrap()
            .seconds = 0.5;
    }
    if frame == 2714 {
        let panel = world
            .resource::<Smoke>()
            .content_fade
            .as_ref()
            .unwrap()
            .panel;
        assert_eq!(
            world.get::<ContentFadeSample>(panel).unwrap().opacity,
            1.0,
            "Changing the motion duration must not replay a settled fade"
        );
        world
            .get_mut::<bedtime_ui2::MotionPolicy>(window)
            .unwrap()
            .seconds = 0.16;
    }
    if frame == 2715 {
        let check = world.resource::<Smoke>().content_fade.as_ref().unwrap();
        let (panel, destination) = (check.panel, check.destination);
        Workbench::new(world)
            .move_tab(panel, destination, 0)
            .unwrap();
    }
    if frame == 2730 {
        let check = world.resource::<Smoke>().content_fade.as_ref().unwrap();
        let (panel, layout) = (check.panel, check.layout);
        let source = world.get::<InArea>(panel).unwrap().0;
        let target = world
            .get::<bedtime_workbench2::LayoutAreas>(layout)
            .unwrap()
            .entities()
            .iter()
            .copied()
            .find(|&area| area != source)
            .unwrap();
        Workbench::new(world)
            .join_areas(source, target, [100, 100])
            .unwrap();
    }
    for (start, end) in [(2701, 2712), (2716, 2728), (2731, 2742)] {
        if (start..=end).contains(&frame) {
            let panel = world
                .resource::<Smoke>()
                .content_fade
                .as_ref()
                .unwrap()
                .panel;
            let (opacity, nested_opacity, marker) = {
                let sample = world.get::<ContentFadeSample>(panel).unwrap();
                (sample.opacity, sample.nested_opacity, sample.marker)
            };
            assert_eq!(nested_opacity, opacity, "Children inherit the content fade");
            let painted_alpha = world
                .query::<&bevy_egui::EguiFullOutput>()
                .single(world)
                .unwrap()
                .0
                .as_ref()
                .unwrap()
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    bedtime_ui2::egui::Shape::Rect(rect) if rect.rect == marker => {
                        Some(rect.fill.a())
                    }
                    _ => None,
                })
                .expect("Panel content is drawn throughout its fade");
            assert!(painted_alpha.abs_diff((opacity * 255.0).round() as u8) <= 1);
            let mut smoke = world.resource_mut::<Smoke>();
            let check = smoke.content_fade.as_mut().unwrap();
            if frame == start {
                assert!(
                    opacity > 0.0 && opacity < 1.0,
                    "Creation and relocation restart the fade"
                );
            } else {
                assert!(
                    opacity >= check.last_opacity,
                    "Layout interpolation must not restart the fade"
                );
            }
            if frame == end {
                assert_eq!(
                    opacity, 1.0,
                    "Content reaches full opacity within the motion duration"
                );
            }
            check.last_opacity = opacity;
        }
    }
    if frame == 2744 {
        world
            .get_mut::<bedtime_ui2::MotionPolicy>(window)
            .unwrap()
            .enabled = false;
        let panel = world
            .resource::<Smoke>()
            .content_fade
            .as_ref()
            .unwrap()
            .panel;
        let area = world.get::<InArea>(panel).unwrap().0;
        Workbench::new(world)
            .split_tab(
                panel,
                area,
                bedtime_workbench2::SplitAxis::Horizontal,
                true,
                [100, 100],
            )
            .unwrap();
    }
    if frame == 2745 || frame == 2747 {
        let panel = world
            .resource::<Smoke>()
            .content_fade
            .as_ref()
            .unwrap()
            .panel;
        assert_eq!(
            world.get::<ContentFadeSample>(panel).unwrap().opacity,
            1.0,
            "Reduced motion settles without replaying when re-enabled"
        );
    }
    if frame == 2746 {
        world
            .get_mut::<bedtime_ui2::MotionPolicy>(window)
            .unwrap()
            .enabled = true;
    }
    if frame == 2750 {
        let check = world.resource_mut::<Smoke>().content_fade.take().unwrap();
        *world.get_mut::<bedtime_ui2::MotionPolicy>(window).unwrap() = check.policy;
        let mut w = Workbench::new(world);
        w.activate_layout(check.previous_layout).unwrap();
        w.delete_layout(check.layout).unwrap();
        info!(
            "Panel content fade passed: creation, tab moves, area relocation, nested painting and reduced motion"
        );
    }
    if let Some(name) = match frame {
        1425 => Some("corner-split-preview.png"),
        2300 => Some("egui-menu-bar.png"),
        2495 => Some("status-collapsed.png"),
        2540 => Some("status-expanded.png"),
        2545 => Some("tab-hover-start.png"),
        2575 => Some("tab-hover-settled.png"),
        2590 => Some("shell-inset.png"),
        2610 => Some("shell-zoom.png"),
        2628 => Some("hit-targets-debug.png"),
        2640 => Some("theme-editor.png"),
        2648 => Some("panel-rounding.png"),
        2702 => Some("content-fade-start.png"),
        2712 => Some("content-fade-settled.png"),
        _ => None,
    } {
        let path = world.resource::<Smoke>().directory.join(name);
        world.spawn(Screenshot::primary_window()).observe(
            move |capture: On<ScreenshotCaptured>| {
                capture
                    .image
                    .clone()
                    .try_into_dynamic()
                    .unwrap()
                    .save(&path)
                    .unwrap();
            },
        );
    }
    if frame > 2760
        && world.resource::<Smoke>().captures.len() == 5
        && world.resource::<Smoke>().tabs_verified
    {
        let smoke = world.resource::<Smoke>();
        let camera = smoke.camera.unwrap();
        assert_ne!(
            smoke.captures["panel"].0, smoke.captures["spinning"].0,
            "The cube must visibly rotate"
        );
        assert_eq!(
            world
                .query_filtered::<Entity, With<GameCamera>>()
                .single(world)
                .unwrap(),
            camera
        );
        assert!(world.get::<UiDiagnostics>(window).unwrap().error.is_none());
        let cube = world
            .query_filtered::<Entity, With<DemoState>>()
            .single(world)
            .unwrap();
        assert!(world.get::<PanelPresentation>(cube).unwrap().visible);
        info!(
            "Workbench smoke passed: spinning panel, splitter input, full-window overlay/alpha, actual close/recreate, retained workspace/layout switches, native tab drag/selection and five pixel-checked captures"
        );
        world.write_message(AppExit::Success);
    }
    assert!(frame < 2800, "Viewport smoke timed out");
}

fn painted_corner_targets(world: &mut World) -> Vec<bedtime_ui2::egui::Rect> {
    let output = world
        .query::<&bevy_egui::EguiFullOutput>()
        .single(world)
        .unwrap();
    output
        .0
        .as_ref()
        .unwrap()
        .shapes
        .iter()
        .filter_map(|shape| {
            if let bedtime_ui2::egui::Shape::Rect(rect) = &shape.shape {
                (rect.fill == bedtime_ui2::egui::Color32::from_rgba_unmultiplied(255, 0, 255, 40))
                    .then_some(rect.rect)
            } else {
                None
            }
        })
        .collect()
}

fn inspect(
    capture: On<ScreenshotCaptured>,
    regions: Query<&Region>,
    mut smoke: ResMut<Smoke>,
    galleries: Query<&InArea, With<bedtime_ui2::gallery::GalleryState>>,
    areas: Query<&AreaPresentation>,
) {
    let region = regions.get(capture.entity).unwrap();
    let image = capture.image.clone().try_into_dynamic().unwrap().to_rgba8();
    image
        .save(smoke.directory.join(format!("{}.png", region.name)))
        .unwrap();
    let scale = image.width() as f32 / region.window.x;
    let sample = |position: Vec2| -> [u8; 3] {
        let x = (position.x * scale)
            .round()
            .clamp(0.0, image.width() as f32 - 1.0) as u32;
        let y = (position.y * scale)
            .round()
            .clamp(0.0, image.height() as f32 - 1.0) as u32;
        let pixel = image.get_pixel(x, y).0;
        [pixel[0], pixel[1], pixel[2]]
    };
    if ["panel", "spinning", "resized"].contains(&region.name)
        && std::env::args().any(|a| a == "--blur")
    {
        let expected = if region.name == "resized" { 255 } else { 217 };
        let area = galleries
            .iter()
            .filter_map(|owner| areas.get(owner.0).ok())
            .find(|a| a.visible)
            .unwrap();
        let point = area.surface.max - Vec2::splat(12.0);
        let alpha = image
            .get_pixel(
                (point.x * scale).round() as u32,
                (point.y * scale).round() as u32,
            )
            .0[3];
        assert!(
            alpha.abs_diff(expected) <= 2,
            "Panel tint must apply alpha once, got {alpha}"
        );
        let point = Vec2::new(area.rect.max.x - 0.5, area.rect.center().y);
        let gap_alpha = image
            .get_pixel((point.x * scale) as u32, (point.y * scale) as u32)
            .0[3];
        assert!(
            gap_alpha.abs_diff(expected) <= 2,
            "Window gaps must adopt opacity, got {gap_alpha}"
        );
        let titlebar_alpha = image.get_pixel(image.width() / 2, 4).0[3];
        assert!(
            titlebar_alpha.abs_diff(expected) <= 2,
            "Titlebar must adopt opacity, got {titlebar_alpha}"
        );
        let status_alpha = image.get_pixel(image.width() / 2, image.height() - 2).0[3];
        assert!(
            status_alpha.abs_diff(expected) <= 2,
            "Status bar must adopt opacity, got {status_alpha}"
        );
        info!(
            "Shell tint passed ({}): panel {alpha}, gap {gap_alpha}, titlebar {titlebar_alpha}, status {status_alpha}",
            region.name
        );
    }
    let mut bright = 0;
    let mut hash = 0_u64;
    let min = region.rect.min + Vec2::splat(4.0);
    let max = region.rect.max - Vec2::splat(4.0);
    for y in (min.y as u32..max.y as u32).step_by(3) {
        for x in (min.x as u32..max.x as u32).step_by(3) {
            let rgb = sample(Vec2::new(x as f32, y as f32));
            bright += usize::from(rgb.iter().map(|&v| v as u32).sum::<u32>() > 220);
            for value in rgb {
                hash = hash.wrapping_mul(16777619) ^ value as u64;
            }
        }
    }
    assert!(
        bright > 80,
        "{} must contain rendered cube pixels, got {bright}",
        region.name
    );
    let clear = if region.name == "overlay" {
        // The in-application menu now covers the top edge. Sample unobstructed
        // scene output at the right edge, away from chrome and the alpha patch.
        sample(Vec2::new(region.window.x - 20.0, region.window.y * 0.5))
    } else {
        sample(region.rect.min + Vec2::splat(8.0))
    };
    if region.name == "overlay" {
        let reference = smoke.captures["panel"].1;
        for channel in 0..3 {
            assert!(
                clear[channel].abs_diff(reference[channel]) <= 3,
                "Panel and full-window output must preserve scene colors: {clear:?} vs {reference:?}"
            );
        }
        let patch = sample(region.window - Vec2::new(90.0, 70.0));
        let decode = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let encode = |v: f32| {
            if v <= 0.0031308 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            }
        };
        for (channel, ink) in [0.8, 0.2, 0.3].into_iter().enumerate() {
            let expected = (encode(decode(ink) * 0.5 + decode(clear[channel] as f32 / 255.0) * 0.5)
                * 255.0)
                .round() as u8;
            assert!(
                patch[channel].abs_diff(expected) <= 5,
                "Transparent UI must blend in linear light: {patch:?}, channel {channel}, expected {expected}"
            );
        }
    }
    info!(
        "Viewport capture {}: {bright} cube samples, background {clear:?}",
        region.name
    );
    smoke.captures.insert(region.name, (hash, clear));
}

fn main_gallery(world: &mut World) -> Entity {
    let split = world
        .resource::<crate::workbench_demo::DemoWorkbench>()
        .split;
    world.query_filtered::<(Entity, &bedtime_workbench2::InArea), With<bedtime_ui2::gallery::GalleryState>>()
        .iter(world).find_map(|(e,l)| (world.get::<bedtime_workbench2::InLayout>(l.0).unwrap().0 == split).then_some(e)).unwrap()
}

fn inspect_tabs(
    capture: On<ScreenshotCaptured>,
    regions: Query<&Region>,
    mut smoke: ResMut<Smoke>,
) {
    let region = regions.get(capture.entity).unwrap();
    let image = capture.image.clone().try_into_dynamic().unwrap().to_rgba8();
    image.save(smoke.directory.join("tabs.png")).unwrap();
    let scale = image.width() as f32 / region.window.x;
    let mut ink = 0;
    // The first column must remain visible after the content window is reparented.
    // Its headings/buttons/text are all brighter than the uniform background.
    for y in (160..450).step_by(2) {
        for x in (5..240).step_by(2) {
            let pixel = image
                .get_pixel(
                    ((region.rect.min.x + x as f32) * scale) as u32,
                    ((region.rect.min.y + y as f32) * scale) as u32,
                )
                .0;
            ink += usize::from(pixel[..3].iter().map(|&v| v as u32).sum::<u32>() > 300);
        }
    }
    assert!(
        ink > 100,
        "Regrouped gallery must retain visible first-column widgets, got {ink} ink samples"
    );
    smoke.tabs_verified = true;
}
