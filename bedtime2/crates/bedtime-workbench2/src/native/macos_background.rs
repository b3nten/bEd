//! Passive AppKit material behind Winit's unchanged content view and Metal layer.
use super::{WindowBackground, WindowBlur, WindowTitlebarInset};
use bevy::{prelude::*, window::RawHandleWrapper};
use objc2::{MainThreadOnly, Message, define_class, msg_send, rc::Retained};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSAutoresizingMaskOptions, NSColor, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView,
};
use objc2_foundation::{MainThreadMarker, NSPoint};
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
    applied: Option<([f32; 4], bool)>,
}

impl Drop for Background {
    fn drop(&mut self) {
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
        Option<&WindowTitlebarInset>,
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
        let top = if bevy_window.fullsize_content_view && bevy_window.titlebar_shown {
            (content.bounds().size.height - window.contentLayoutRect().size.height).max(0.0) as f32
        } else {
            0.0
        };
        if inset.is_none_or(|inset| inset.0 != top) {
            commands.entity(entity).insert(WindowTitlebarInset(top));
        }
    }
}

pub(super) fn sync(
    windows: Query<(
        Entity,
        &WindowBackground,
        &RawHandleWrapper,
        Option<&WindowBlur>,
    )>,
    mut backgrounds: NonSendMut<Backgrounds>,
) {
    if windows.is_empty() {
        backgrounds.0.clear();
        return;
    }
    let mtm = MainThreadMarker::new().expect("AppKit window updates run on the main thread");
    let mut alive = HashSet::new();
    for (entity, background, handle, blur) in &windows {
        let RawWindowHandle::AppKit(handle) = handle.get_window_handle() else {
            continue;
        };
        // SAFETY: Bevy retains this live NSView. AppKit work is main-thread only.
        let content = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let window = content
            .window()
            .expect("Live AppKit content view has a window");
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
                applied: None,
            }
        });
        let blur = blur.is_some_and(|b| b.0);
        if state.applied == Some((background.0, blur)) {
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
        let appearance = NSAppearance::appearanceNamed(unsafe {
            if r * 0.2126 + g * 0.7152 + b * 0.0722 > 0.5 {
                NSAppearanceNameAqua
            } else {
                NSAppearanceNameDarkAqua
            }
        });
        window.setAppearance(appearance.as_deref());
        state.applied = Some((background.0, blur));
    }
    backgrounds.0.retain(|e, _| alive.contains(e));
}
