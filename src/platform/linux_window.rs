//! Native pointer access for Xdnd, which does not produce Winit CursorMoved.
use std::sync::OnceLock;
use winit::{
    raw_window_handle::{HasDisplayHandle, RawDisplayHandle},
    window::Window,
};
use x11_dl::xlib;

/// Read the live X11 pointer using Winit's existing display connection.
///
/// The multi-viewport backend uses physical desktop pixels on Linux. Its
/// single-window fallback uses logical coordinates relative to the client area.
/// Wayland has no global pointer query and returns None; Winit 0.30 does not
/// deliver native file-drop events from that backend either.
pub fn external_drag_position(window: &Window, native_viewports: bool) -> Option<[f32; 2]> {
    let state = query_pointer(window)?;
    let origin = if native_viewports {
        [0, 0]
    } else {
        let position = window.inner_position().ok()?;
        [position.x, position.y]
    };
    let scale = if native_viewports {
        1.0
    } else {
        window.scale_factor()
    };
    pointer_coordinates(state.root, origin, scale)
}

/// Read control, shift, alt and super from the live X11 modifier mask. Native
/// Xdnd can suppress Winit key events while the source owns the drag loop.
pub fn external_drag_modifiers(window: &Window) -> Option<[bool; 4]> {
    Some(modifier_flags(query_pointer(window)?.modifiers))
}

struct PointerState {
    root: [i32; 2],
    modifiers: u32,
}

fn query_pointer(window: &Window) -> Option<PointerState> {
    let RawDisplayHandle::Xlib(handle) = window.display_handle().ok()?.as_raw() else {
        return None;
    };
    let display = handle.display?.as_ptr().cast::<xlib::Display>();
    static XLIB: OnceLock<Option<xlib::Xlib>> = OnceLock::new();
    let xlib = XLIB.get_or_init(|| xlib::Xlib::open().ok()).as_ref()?;
    let mut root = 0;
    let mut child = 0;
    let mut root_x = 0;
    let mut root_y = 0;
    let mut client_x = 0;
    let mut client_y = 0;
    let mut modifiers = 0;
    // SAFETY: Called on the Winit event-loop thread using its live borrowed Xlib
    // display. The query reads pointer state and neither owns nor closes it.
    let found = unsafe {
        let screen_root = (xlib.XRootWindow)(display, handle.screen);
        (xlib.XQueryPointer)(
            display,
            screen_root,
            &mut root,
            &mut child,
            &mut root_x,
            &mut root_y,
            &mut client_x,
            &mut client_y,
            &mut modifiers,
        )
    };
    if found == 0 {
        return None;
    }
    Some(PointerState {
        root: [root_x, root_y],
        modifiers,
    })
}

fn modifier_flags(mask: u32) -> [bool; 4] {
    [
        mask & xlib::ControlMask != 0,
        mask & xlib::ShiftMask != 0,
        mask & xlib::Mod1Mask != 0,
        mask & xlib::Mod4Mask != 0,
    ]
}

fn pointer_coordinates(root: [i32; 2], origin: [i32; 2], scale: f64) -> Option<[f32; 2]> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    Some([
        ((f64::from(root[0]) - f64::from(origin[0])) / scale) as f32,
        ((f64::from(root[1]) - f64::from(origin[1])) / scale) as f32,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn x11_coordinates_match_viewport_and_single_window_backend_units() {
        assert_eq!(
            pointer_coordinates([1320, 480], [0, 0], 1.0),
            Some([1320.0, 480.0])
        );
        assert_eq!(
            pointer_coordinates([1320, 480], [1000, 200], 2.0),
            Some([160.0, 140.0])
        );
        assert_eq!(
            pointer_coordinates([-200, -100], [0, 0], 1.0),
            Some([-200.0, -100.0])
        );
        assert_eq!(pointer_coordinates([1, 2], [0, 0], 0.0), None);
    }

    #[test]
    fn x11_modifier_masks_map_drag_keys_and_ignore_locks_and_buttons() {
        assert_eq!(modifier_flags(0), [false; 4]);
        assert_eq!(
            modifier_flags(xlib::ControlMask | xlib::ShiftMask),
            [true, true, false, false]
        );
        assert_eq!(
            modifier_flags(xlib::Mod1Mask | xlib::Mod4Mask),
            [false, false, true, true]
        );
        assert_eq!(
            modifier_flags(xlib::LockMask | xlib::Mod2Mask | xlib::Button1Mask),
            [false; 4]
        );
    }
}
