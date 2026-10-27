//! Passive AppKit material behind Winit's unchanged content view and Metal layer.
use super::{WindowBackground, WindowBlur, WindowDrag, WindowTitlebarGeometry};
#[cfg(feature = "ui")]
use bevy::window::{CursorMoved, FileDragAndDrop, WindowClosed, WindowEvent};
use bevy::{prelude::*, window::RawHandleWrapper};
use objc2::{MainThreadOnly, Message, define_class, msg_send, rc::Retained};
#[cfg(feature = "ui")]
use objc2::{
    runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel},
    sel,
};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSAutoresizingMaskOptions, NSColor, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindowButton,
};
use objc2_foundation::{MainThreadMarker, NSPoint};
#[cfg(feature = "ui")]
use objc2_foundation::{NSRect, NSString, NSUserDefaults};
use raw_window_handle::RawWindowHandle;
use std::collections::{HashMap, HashSet};

define_class!(
    // SAFETY: This view only supplies a background. Input stays on Winit's view.
    #[unsafe(super=NSVisualEffectView)]
    #[name="Bedtime2BackgroundMaterial"]
    #[thread_kind=MainThreadOnly]
    struct Material;
    impl Material {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self,_point:NSPoint)->*mut NSView {std::ptr::null_mut()}
    }
);

struct Background {
    content: Retained<NSView>,
    material: Retained<Material>,
    // Some(None) records an inherited appearance, which must be restored as nil.
    original_appearance: Option<Option<Retained<NSAppearance>>>,
    applied_appearance: Option<Retained<NSAppearance>>,
    #[cfg(feature = "ui")]
    drag_view_classes: Option<(&'static AnyClass, &'static AnyClass)>,
    applied: Option<([f32; 4], bool, bool)>,
}

impl Background {
    fn restore_titlebar_appearance(&mut self) {
        if let Some(original) = self.original_appearance.take()
            && let Some(window) = self.content.window()
            && window.appearance() == self.applied_appearance
        {
            // Do not overwrite a native appearance the application has already
            // installed while taking ownership in this same frame.
            window.setAppearance(original.as_deref());
        }
        self.applied_appearance = None;
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        #[cfg(feature = "ui")]
        if let Some(classes) = self.drag_view_classes.take() {
            restore_automatic_window_drag(&self.content, classes);
        }
        self.restore_titlebar_appearance();
        self.material.removeFromSuperview();
    }
}

#[derive(Default)]
pub(super) struct Backgrounds(HashMap<Entity, Background>);

/// Geometry is an observation of the window, independent of tint/blur settings.
/// Publish it before UI layout, including on the first frame with a native handle.
pub(super) fn measure_titlebar(
    _main_thread: bevy::ecs::system::NonSendMarker,
    windows: Query<(
        Entity,
        &Window,
        &RawHandleWrapper,
        Option<&WindowTitlebarGeometry>,
    )>,
    mut commands: Commands,
) {
    for (entity, bevy_window, handle, inset) in &windows {
        let RawWindowHandle::AppKit(handle) = handle.get_window_handle() else {
            continue;
        };
        let _mtm =
            MainThreadMarker::new().expect("AppKit window measurements run on the main thread");
        // SAFETY: Bevy retains this live view. This system runs on the main thread.
        let content = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let window = content
            .window()
            .expect("Live AppKit content view has a window");
        let mut geometry = WindowTitlebarGeometry::default();
        if bevy_window.fullsize_content_view && bevy_window.titlebar_shown {
            let bounds = content.bounds();
            geometry.height =
                (bounds.size.height - window.contentLayoutRect().size.height).max(0.0) as f32;
            for kind in [
                NSWindowButton::CloseButton,
                NSWindowButton::MiniaturizeButton,
                NSWindowButton::ZoomButton,
            ] {
                if let Some(button) = window.standardWindowButton(kind)
                    && !button.isHiddenOrHasHiddenAncestor()
                {
                    let rect = content.convertRect_fromView(button.bounds(), Some(&button));
                    let x = (rect.origin.x - bounds.origin.x) as f32;
                    let y = if content.isFlipped() {
                        rect.origin.y - bounds.origin.y
                    } else {
                        bounds.origin.y + bounds.size.height - rect.origin.y - rect.size.height
                    } as f32;
                    let rect = Rect::from_corners(
                        Vec2::new(x, y),
                        Vec2::new(x + rect.size.width as f32, y + rect.size.height as f32),
                    );
                    geometry.controls =
                        Some(geometry.controls.map_or(rect, |bounds| bounds.union(rect)));
                }
            }
        }
        if inset != Some(&geometry) {
            commands.entity(entity).insert(geometry);
        }
    }
}

/// Winit 0.30.13's NSDraggingDestination emits file hover/drop/cancel, but no
/// CursorMoved or draggingUpdated:. Observe the pointer on rendered hover frames
/// and once on the drop frame, before egui reads the ordinary window messages.
#[cfg(feature = "ui")]
pub(super) fn sample_file_drag_cursor(
    _main_thread: bevy::ecs::system::NonSendMarker,
    windows: Query<(&Window, &RawHandleWrapper)>,
    mut files: MessageReader<FileDragAndDrop>,
    mut closed: MessageReader<WindowClosed>,
    mut hovered: Local<HashSet<Entity>>,
    mut cursors: MessageWriter<CursorMoved>,
    mut events: MessageWriter<WindowEvent>,
    mut redraw: MessageWriter<bevy::window::RequestRedraw>,
) {
    // Only live native windows enter persistent state; retired windows cannot
    // accumulate even if their drag never delivered a cancellation.
    hovered.retain(|window| windows.contains(*window));
    let mut dropped = HashSet::new();
    for file in files.read() {
        match file {
            FileDragAndDrop::HoveredFile { window, .. } => {
                if windows.contains(*window) {
                    hovered.insert(*window);
                }
            }
            FileDragAndDrop::DroppedFile { window, .. } => {
                // Real Winit drags enter before dropping. Drop-only injection
                // supplies its own CursorMoved; never replace it with the OS mouse.
                if hovered.remove(window) {
                    dropped.insert(*window);
                }
            }
            FileDragAndDrop::HoveredFileCanceled { window } => {
                hovered.remove(window);
            }
        }
    }
    for event in closed.read() {
        hovered.remove(&event.window);
        dropped.remove(&event.window);
    }
    // Deduplicate multi-file drops and hover+drop delivered in the same frame.
    dropped.extend(hovered.iter().copied());
    if dropped.is_empty() {
        return;
    }
    // A reactive app otherwise gets only the initial HoveredFile event: Winit
    // supplies no draggingUpdated events to schedule subsequent pointer samples.
    if !hovered.is_empty() {
        redraw.write(bevy::window::RequestRedraw);
    }
    let _mtm = MainThreadMarker::new().expect("AppKit file drag sampling runs on the main thread");
    for entity in dropped {
        let Ok((bevy_window, handle)) = windows.get(entity) else {
            continue;
        };
        let RawWindowHandle::AppKit(handle) = handle.get_window_handle() else {
            continue;
        };
        // SAFETY: Bevy retains this live NSView for the queried handle. The
        // NonSendMarker and main-thread check confine this borrow to AppKit's thread.
        let content = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let Some(window) = content.window() else {
            continue;
        };
        // nil means window coordinates, so AppKit accounts for content-view
        // placement/insets before we normalize its bounds to Bevy logical pixels.
        let point = content.convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None);
        let Some(position) = logical_file_drag_position(
            point,
            content.bounds(),
            content.isFlipped(),
            bevy_window.size(),
        ) else {
            continue;
        };
        let cursor = CursorMoved {
            window: entity,
            position,
            delta: None,
        };
        cursors.write(cursor.clone());
        events.write(WindowEvent::CursorMoved(cursor));
    }
}

/// Convert a point already in NSView coordinates to top-left content coordinates.
/// Window::size includes Bevy's scale-factor override; AppKit points need not match
/// those logical pixels. The titlebar remains part of a full-size content view.
#[cfg(feature = "ui")]
fn logical_file_drag_position(
    point: NSPoint,
    bounds: NSRect,
    flipped: bool,
    logical_size: Vec2,
) -> Option<Vec2> {
    if ![
        point.x,
        point.y,
        bounds.origin.x,
        bounds.origin.y,
        bounds.size.width,
        bounds.size.height,
    ]
    .into_iter()
    .all(f64::is_finite)
        || !logical_size.is_finite()
        || bounds.size.width <= 0.0
        || bounds.size.height <= 0.0
        || logical_size.x <= 0.0
        || logical_size.y <= 0.0
    {
        return None;
    }
    let x = point.x - bounds.origin.x;
    let y = if flipped {
        point.y - bounds.origin.y
    } else {
        bounds.origin.y + bounds.size.height - point.y
    };
    let position = Vec2::new(
        (x / bounds.size.width * f64::from(logical_size.x)) as f32,
        (y / bounds.size.height * f64::from(logical_size.y)) as f32,
    );
    position.is_finite().then_some(position)
}

/// AppKit's blocking window drag can consume mouse-up before Winit sees it.
/// Restore the ordinary input stream before Bevy and egui process the next frame.
pub(super) fn finish_window_drags(
    _main_thread: bevy::ecs::system::NonSendMarker,
    windows: Query<Entity, With<WindowDrag>>,
    mut commands: Commands,
    mut events: MessageWriter<bevy::window::WindowEvent>,
    mut buttons: MessageWriter<bevy::input::mouse::MouseButtonInput>,
) {
    if objc2_app_kit::NSEvent::pressedMouseButtons() & 1 != 0 {
        return;
    }
    for window in &windows {
        let release = bevy::input::mouse::MouseButtonInput {
            window,
            button: bevy::input::mouse::MouseButton::Left,
            state: bevy::input::ButtonState::Released,
        };
        buttons.write(release);
        events.write(bevy::window::WindowEvent::MouseButtonInput(release));
        commands.entity(window).remove::<WindowDrag>();
    }
}

#[cfg(feature = "ui")]
pub(super) fn double_click(handle: &RawHandleWrapper) {
    let RawWindowHandle::AppKit(handle) = handle.get_window_handle() else {
        return;
    };
    let _mtm = MainThreadMarker::new().expect("Titlebar gestures run on the main thread");
    // SAFETY: Bevy owns this live view; this function runs on the main thread.
    let content = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let window = content.window().expect("Live content view has a window");
    let action = NSUserDefaults::standardUserDefaults()
        .stringForKey(&NSString::from_str("AppleActionOnDoubleClick"))
        .map(|action| action.to_string());
    match action.as_deref() {
        Some("None") => {}
        Some("Minimize") => window.performMiniaturize(None),
        _ => window.performZoom(None),
    }
}

pub(super) fn sync(
    windows: Query<(
        Entity,
        &Window,
        &WindowBackground,
        &RawHandleWrapper,
        Option<&WindowBlur>,
    )>,
    #[cfg(feature = "ui")] hosts: Query<(Entity, &crate::WorkbenchShell)>,
    mut backgrounds: NonSendMut<Backgrounds>,
) {
    if windows.is_empty() {
        backgrounds.0.clear();
        return;
    }
    let mtm = MainThreadMarker::new().expect("AppKit window updates run on the main thread");
    let mut alive = HashSet::new();
    for (entity, bevy_window, background, handle, blur) in &windows {
        let RawWindowHandle::AppKit(handle) = handle.get_window_handle() else {
            continue;
        };
        // SAFETY: Bevy retains this live NSView. AppKit work is main-thread only.
        let content = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let window = content
            .window()
            .expect("Live AppKit content view has a window");
        #[cfg(feature = "ui")]
        let titlebar_ownership = hosts
            .iter()
            .find(|(surface, _)| *surface == entity)
            .map(|(_, host)| host.titlebar_ownership);
        #[cfg(feature = "ui")]
        let owns_titlebar = titlebar_ownership != Some(crate::TitlebarOwnership::Application);
        #[cfg(not(feature = "ui"))]
        let owns_titlebar = {
            let _ = bevy_window;
            true
        };
        alive.insert(entity);
        if backgrounds
            .0
            .get(&entity)
            .is_some_and(|old| !std::ptr::eq(&*old.content, content))
        {
            backgrounds.0.remove(&entity);
        }
        let state = backgrounds.0.entry(entity).or_insert_with(|| {
            // SAFETY: NSView's inherited initializer is correct for this subclass.
            let material: Retained<Material> =
                unsafe { msg_send![Material::alloc(mtm),initWithFrame:content.bounds()] };
            material.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            material.setMaterial(NSVisualEffectMaterial::HUDWindow);
            material.setState(NSVisualEffectState::Active);
            material.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            material.setWantsLayer(true);
            content.addSubview(&material);
            if let Some(layer) = material.layer() {
                layer.setZPosition(-1.0);
            }
            Background {
                content: content.retain(),
                material,
                original_appearance: None,
                applied_appearance: None,
                #[cfg(feature = "ui")]
                drag_view_classes: None,
                applied: None,
            }
        });
        #[cfg(feature = "ui")]
        {
            let overrides_drag = bevy_window.fullsize_content_view
                && titlebar_ownership == Some(crate::TitlebarOwnership::Workbench);
            if overrides_drag && state.drag_view_classes.is_none() {
                // Transparent NSViews otherwise start a native drag before
                // egui can decide which widget owns a titlebar press.
                state.drag_view_classes = disable_automatic_window_drag(content);
            } else if !overrides_drag && let Some(classes) = state.drag_view_classes.take() {
                restore_automatic_window_drag(content, classes);
            }
        }
        if owns_titlebar && state.original_appearance.is_none() {
            state.original_appearance = Some(window.appearance());
        } else if !owns_titlebar {
            state.restore_titlebar_appearance();
        }
        let blur = blur.is_some_and(|b| b.0);
        if state.applied == Some((background.0, blur, owns_titlebar)) {
            continue;
        }
        let [r, g, b, a] = background.0.map(f64::from);
        window.setOpaque(!blur && a >= 1.0);
        // GPU surfaces carry the tint/alpha once. An additional native tint
        // underneath would compound opacity and hide the desktop/material.
        let color = if blur || a < 1.0 {
            NSColor::clearColor()
        } else {
            NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 1.0)
        };
        window.setBackgroundColor(Some(&color));
        window.setAlphaValue(1.0);
        state.material.setHidden(!blur);
        if owns_titlebar {
            let appearance = NSAppearance::appearanceNamed(unsafe {
                if r * 0.2126 + g * 0.7152 + b * 0.0722 > 0.5 {
                    NSAppearanceNameAqua
                } else {
                    NSAppearanceNameDarkAqua
                }
            });
            window.setAppearance(appearance.as_deref());
            state.applied_appearance = appearance;
        }
        state.applied = Some((background.0, blur, owns_titlebar));
    }
    backgrounds.0.retain(|e, _| alive.contains(e));
}

#[cfg(feature = "ui")]
fn disable_automatic_window_drag(
    content: &NSView,
) -> Option<(&'static AnyClass, &'static AnyClass)> {
    if !content.mouseDownCanMoveWindow() {
        return None;
    }
    extern "C" fn mouse_down_can_move_window(_: &AnyObject, _: Sel) -> Bool {
        Bool::NO
    }
    let parent = AnyObject::class(content);
    let name = std::ffi::CString::new(format!(
        "Bedtime2Titlebar_{}",
        parent.name().to_string_lossy()
    ))
    .unwrap();
    let class = AnyClass::get(&name).unwrap_or_else(|| {
        let mut class = ClassBuilder::new(&name, parent).expect("Unique titlebar view class");
        // SAFETY: This overrides NSView's BOOL getter with the identical ABI.
        unsafe {
            class.add_method(
                sel!(mouseDownCanMoveWindow),
                mouse_down_can_move_window as extern "C" fn(_, _) -> _,
            );
        }
        class.register()
    });
    // SAFETY: Main-thread-only, per-instance subclassing. The subclass adds no
    // ivars and overrides only the compatible BOOL getter above. Winit retains
    // the same view, inherited ivars, responder methods, delegate and Metal layer.
    // Its inherited deallocation remains valid for the lifetime of this view.
    let previous = unsafe { AnyObject::set_class(content, class) };
    assert!(
        std::ptr::eq(previous, parent),
        "Native content view class changed concurrently"
    );
    Some((parent, class))
}

#[cfg(feature = "ui")]
fn restore_automatic_window_drag(content: &NSView, (original, installed): (&AnyClass, &AnyClass)) {
    // Undo only our override; another native integration may have changed the
    // class itself. Neither class adds storage, so the original ivars stay valid.
    if std::ptr::eq(AnyObject::class(content), installed) {
        // SAFETY: Same view and original class retained from our installation.
        unsafe { AnyObject::set_class(content, original) };
    }
}

#[cfg(all(test, feature = "ui"))]
mod tests {
    use super::logical_file_drag_position;
    use bevy::{prelude::*, window::WindowResolution};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    #[test]
    fn flipped_content_uses_top_left_coordinates() {
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0));
        assert_eq!(
            logical_file_drag_position(
                NSPoint::new(120.0, 45.0),
                bounds,
                true,
                Vec2::new(800.0, 600.0),
            ),
            Some(Vec2::new(120.0, 45.0)),
        );
    }

    #[test]
    fn unflipped_content_reverses_y_without_removing_titlebar() {
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0));
        assert_eq!(
            logical_file_drag_position(
                NSPoint::new(120.0, 585.0),
                bounds,
                false,
                Vec2::new(800.0, 600.0),
            ),
            Some(Vec2::new(120.0, 15.0)),
        );
    }

    #[test]
    fn translated_content_bounds_are_relative_to_the_view_not_the_window() {
        // AppKit has already converted window placement/insets to this view's
        // coordinates. A nonzero bounds origin must not become a pointer offset.
        let bounds = NSRect::new(NSPoint::new(30.0, 50.0), NSSize::new(800.0, 600.0));
        for (flipped, y) in [(true, 95.0), (false, 605.0)] {
            assert_eq!(
                logical_file_drag_position(
                    NSPoint::new(150.0, y),
                    bounds,
                    flipped,
                    Vec2::new(800.0, 600.0),
                ),
                Some(Vec2::new(120.0, 45.0)),
            );
        }
    }

    #[test]
    fn content_size_ratio_honors_bevy_scale_factor_override() {
        // Pure Window data: no native window or desktop session is created.
        // A 2x backing surface spans 800x600 AppKit points; the 4x Bevy override
        // instead exposes 400x300 logical pixels.
        let window = Window {
            resolution: WindowResolution::new(1600, 1200).with_scale_factor_override(4.0),
            ..default()
        };
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0));
        assert_eq!(
            logical_file_drag_position(NSPoint::new(200.0, 150.0), bounds, true, window.size()),
            Some(Vec2::new(100.0, 75.0)),
        );
        assert_eq!(
            logical_file_drag_position(
                NSPoint::new(200.0, 150.0),
                bounds,
                true,
                Vec2::new(1600.0, 300.0),
            ),
            Some(Vec2::new(400.0, 75.0)),
        );
    }

    #[test]
    fn points_outside_content_are_not_clamped_to_a_drop_target() {
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0));
        assert_eq!(
            logical_file_drag_position(
                NSPoint::new(-10.0, 620.0),
                bounds,
                true,
                Vec2::new(800.0, 600.0),
            ),
            Some(Vec2::new(-10.0, 620.0)),
        );
    }

    #[test]
    fn empty_or_nonfinite_geometry_does_not_emit_a_cursor_position() {
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0));
        let empty = NSRect::new(bounds.origin, NSSize::new(0.0, 600.0));
        assert_eq!(
            logical_file_drag_position(NSPoint::new(10.0, 20.0), empty, true, Vec2::splat(100.0)),
            None,
        );
        assert_eq!(
            logical_file_drag_position(NSPoint::new(10.0, 20.0), bounds, true, Vec2::ZERO),
            None,
        );
        assert_eq!(
            logical_file_drag_position(
                NSPoint::new(f64::NAN, 20.0),
                bounds,
                true,
                Vec2::splat(100.0),
            ),
            None,
        );
    }
}
