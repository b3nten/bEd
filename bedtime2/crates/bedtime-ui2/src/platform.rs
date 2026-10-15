//! Single-window platform adaptation. Native windows are only borrowed during
//! calls; the IME callback publishes owned caret data instead of retaining one.
use dear_imgui_rs::{BackendFlags, ConfigFlags, Context, Key, Ui};
use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Instant};
use winit::{
    dpi::{LogicalPosition, LogicalSize},
    event::{ElementState, Ime, MouseScrollDelta, TouchPhase, WindowEvent},
    window::{CursorIcon, Window},
};

#[derive(Clone, Copy, Default)]
struct Modifiers {
    shift: bool,
    ctrl: bool,
    alt: bool,
    super_key: bool,
}

#[derive(Clone, Copy, Default)]
struct ImeCaret {
    position: [f32; 2],
    height: f32,
}

thread_local! {
    static IME_CARETS: RefCell<HashMap<usize, Rc<RefCell<Option<ImeCaret>>>>> = RefCell::new(HashMap::new());
}

/// Native input/cursor/IME state for the primary window. No native handle is retained.
pub struct UiPlatform {
    last_frame: Instant,
    context_address: usize,
    caret: Rc<RefCell<Option<ImeCaret>>>,
    composing: bool,
    native_keys: std::collections::HashSet<winit::keyboard::KeyCode>,
    native_buttons: std::collections::HashSet<winit::event::MouseButton>,
    native_modifiers: Modifiers,
}

impl UiPlatform {
    pub fn new(context: &mut Context) -> Self {
        context
            .set_platform_name(Some("bedtime_ui2"))
            .expect("platform name");
        let flags = context.io().backend_flags()
            | BackendFlags::HAS_MOUSE_CURSORS
            | BackendFlags::HAS_SET_MOUSE_POS;
        context.io_mut().set_backend_flags(flags);
        let flags = context.io().config_flags() & !ConfigFlags::VIEWPORTS_ENABLE;
        context.io_mut().set_config_flags(flags);
        let caret = Rc::new(RefCell::new(None));
        // SAFETY: the binding enters this Context. We install only a function
        // pointer; its state is owned by the registry and removed on Drop.
        let address = context.binding().with_bound_context(|| unsafe {
            let address = dear_imgui_rs::sys::igGetCurrentContext() as usize;
            (*dear_imgui_rs::sys::igGetPlatformIO_Nil()).Platform_SetImeDataFn = Some(ime_caret);
            address
        });
        IME_CARETS.with(|carets| {
            carets.borrow_mut().insert(address, Rc::clone(&caret));
        });
        Self {
            last_frame: Instant::now(),
            context_address: address,
            caret,
            composing: false,
            native_keys: Default::default(),
            native_buttons: Default::default(),
            native_modifiers: Default::default(),
        }
    }

    pub fn shutdown(&mut self, context: &mut Context) {
        IME_CARETS.with(|carets| {
            carets.borrow_mut().remove(&self.context_address);
        });
        // SAFETY: the callback state has been removed and this Context is still
        // alive and bound. No native window publication was installed.
        context.binding().with_bound_context(|| unsafe {
            (*dear_imgui_rs::sys::igGetPlatformIO_Nil()).Platform_SetImeDataFn = None;
        });
    }

    pub fn prepare_frame(&mut self, context: &mut Context, window: &Window) {
        let now = Instant::now();
        context
            .io_mut()
            .set_delta_time((now - self.last_frame).as_secs_f32().max(0.000001));
        self.last_frame = now;
        let scale = window.scale_factor() as f32;
        let size = window.inner_size();
        context
            .io_mut()
            .set_display_size([size.width as f32 / scale, size.height as f32 / scale]);
        context.io_mut().set_display_framebuffer_scale([scale; 2]);
        if context.io().want_set_mouse_pos() {
            let pos = context.io().mouse_pos();
            if pos.iter().all(|value| value.is_finite()) {
                let _ = window.set_cursor_position(LogicalPosition::new(pos[0], pos[1]));
            }
        }
    }

    pub fn prepare_render(&mut self, ui: &Ui, window: &Window) {
        use dear_imgui_rs::MouseCursor;
        if !ui
            .io()
            .config_flags()
            .contains(ConfigFlags::NO_MOUSE_CURSOR_CHANGE)
        {
            let cursor = if ui.io().mouse_draw_cursor() {
                None
            } else {
                ui.mouse_cursor()
            };
            window.set_cursor_visible(cursor.is_some());
            if let Some(cursor) = cursor {
                window.set_cursor(match cursor {
                    MouseCursor::Arrow => CursorIcon::Default,
                    MouseCursor::TextInput => CursorIcon::Text,
                    MouseCursor::ResizeAll => CursorIcon::Move,
                    MouseCursor::ResizeNS => CursorIcon::NsResize,
                    MouseCursor::ResizeEW => CursorIcon::EwResize,
                    MouseCursor::ResizeNESW => CursorIcon::NeswResize,
                    MouseCursor::ResizeNWSE => CursorIcon::NwseResize,
                    MouseCursor::Hand => CursorIcon::Pointer,
                    MouseCursor::NotAllowed => CursorIcon::NotAllowed,
                    _ => CursorIcon::Default,
                });
            }
        }
        window.set_ime_allowed(ui.io().want_text_input());
    }

    /// Apply the caret published while closing the previous frame.
    pub fn update_ime(&mut self, window: &Window) {
        if let Some(caret) = self.caret.borrow_mut().take() {
            window.set_ime_cursor_area(
                LogicalPosition::new(caret.position[0], caret.position[1]),
                LogicalSize::new(caret.height, caret.height),
            );
        }
    }

    fn feed_modifiers(&self, context: &mut Context) {
        let a = self.native_modifiers;
        for (key, pressed) in [
            (Key::ModShift, a.shift),
            (Key::ModCtrl, a.ctrl),
            (Key::ModAlt, a.alt),
            (Key::ModSuper, a.super_key),
        ] {
            context.io_mut().add_key_event(key, pressed);
        }
    }

    pub fn handle_event(
        &mut self,
        context: &mut Context,
        window: &Window,
        event: &WindowEvent,
    ) -> bool {
        match event {
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(key) =
                    crate::input_map::winit_key_to_imgui_key(&event.logical_key, event.physical_key)
                {
                    context
                        .io_mut()
                        .add_key_event(key, event.state == ElementState::Pressed);
                }
                if event.state.is_pressed()
                    && !self.composing
                    && let Some(text) = &event.text
                {
                    for ch in text.chars().filter(|ch| !ch.is_control()) {
                        context.io_mut().add_input_character(ch);
                    }
                }
                context.io().want_capture_keyboard()
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                self.native_modifiers = Modifiers {
                    shift: state.shift_key(),
                    ctrl: state.control_key(),
                    alt: state.alt_key(),
                    super_key: state.super_key(),
                };
                self.feed_modifiers(context);
                false
            }
            WindowEvent::MouseInput { button, state, .. } => {
                if state.is_pressed() {
                    self.native_buttons.insert(*button);
                } else {
                    self.native_buttons.remove(button);
                }
                let pressed = state.is_pressed();
                if let Some(imgui_button) = crate::input_map::to_imgui_mouse_button(*button) {
                    context
                        .io_mut()
                        .add_mouse_button_event(imgui_button, pressed);
                }
                context.io().want_capture_mouse()
            }
            WindowEvent::CursorMoved { position, .. } => {
                let position = position.to_logical::<f64>(window.scale_factor());
                if position.x.is_finite() && position.y.is_finite() {
                    context
                        .io_mut()
                        .add_mouse_pos_event([position.x as f32, position.y as f32]);
                }
                context.io().want_capture_mouse()
            }
            WindowEvent::CursorLeft { .. } => {
                context.io_mut().add_mouse_pos_event([-f32::MAX; 2]);
                false
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                if *phase != TouchPhase::Cancelled {
                    let wheel = match delta {
                        MouseScrollDelta::LineDelta(x, y) => [*x, *y],
                        MouseScrollDelta::PixelDelta(pos) => [
                            (pos.x / window.scale_factor() * 0.01) as f32,
                            (pos.y / window.scale_factor() * 0.01) as f32,
                        ],
                    };
                    if wheel.iter().all(|value| value.is_finite()) {
                        context.io_mut().add_mouse_wheel_event(wheel);
                    }
                }
                context.io().want_capture_mouse()
            }
            WindowEvent::Focused(focused) => {
                context.io_mut().add_focus_event(*focused);
                if !focused {
                    self.composing = false;
                    self.native_keys.clear();
                    self.native_buttons.clear();
                    self.native_modifiers = Default::default();
                }
                false
            }
            WindowEvent::Ime(Ime::Preedit(text, _)) => {
                self.composing = !text.is_empty();
                false
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                self.composing = false;
                for ch in text.chars().filter(|ch| !ch.is_control()) {
                    context.io_mut().add_input_character(ch);
                }
                context.io().want_capture_keyboard()
            }
            WindowEvent::Ime(Ime::Disabled) => {
                self.composing = false;
                false
            }
            _ => false,
        }
    }
}

impl Drop for UiPlatform {
    fn drop(&mut self) {
        IME_CARETS.with(|carets| {
            carets.borrow_mut().remove(&self.context_address);
        });
    }
}

unsafe extern "C" fn ime_caret(
    context: *mut dear_imgui_rs::sys::ImGuiContext,
    viewport: *mut dear_imgui_rs::sys::ImGuiViewport,
    data: *mut dear_imgui_rs::sys::ImGuiPlatformImeData,
) {
    // SAFETY: ImGui supplies live borrowed callback arguments. Copy only scalar
    // geometry before returning; no viewport/context/window pointer escapes.
    let _ = std::panic::catch_unwind(|| {
        if viewport.is_null() || data.is_null() {
            return;
        }
        let (data, viewport) = unsafe { (&*data, &*viewport) };
        if !data.WantVisible && !data.WantTextInput {
            return;
        }
        let position = [
            data.InputPos.x - viewport.Pos.x,
            data.InputPos.y - viewport.Pos.y,
        ];
        if !position.iter().all(|value| value.is_finite()) {
            return;
        }
        let height = if data.InputLineHeight.is_finite() && data.InputLineHeight > 0.0 {
            data.InputLineHeight
        } else {
            16.0
        };
        IME_CARETS.with(|carets| {
            if let Some(caret) = carets.borrow().get(&(context as usize)) {
                *caret.borrow_mut() = Some(ImeCaret { position, height });
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ime_copies_caret_geometry_and_releases_callback_state_before_context() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        let mut platform = UiPlatform::new(&mut context);
        assert!(
            !context
                .io()
                .config_flags()
                .contains(ConfigFlags::VIEWPORTS_ENABLE)
        );
        context.binding().with_bound_context(|| unsafe {
            let viewport = dear_imgui_rs::sys::igGetMainViewport();
            let mut data: dear_imgui_rs::sys::ImGuiPlatformImeData = std::mem::zeroed();
            data.WantTextInput = true;
            data.InputPos.x = (*viewport).Pos.x + 23.0;
            data.InputPos.y = (*viewport).Pos.y + 41.0;
            data.InputLineHeight = 18.0;
            ime_caret(
                dear_imgui_rs::sys::igGetCurrentContext(),
                viewport,
                &mut data,
            );
            // Overwrite the borrowed callback argument: deferred state must
            // already own the original values.
            data.InputPos.x = 900.0;
            assert_eq!(data.InputPos.x, 900.0);
        });
        let caret = platform.caret.borrow().expect("caret was published");
        assert_eq!(caret.position, [23.0, 41.0]);
        assert_eq!(caret.height, 18.0);
        platform.shutdown(&mut context);
        IME_CARETS.with(|carets| assert!(!carets.borrow().contains_key(&platform.context_address)));
        context.binding().with_bound_context(|| unsafe {
            assert!(
                (*dear_imgui_rs::sys::igGetPlatformIO_Nil())
                    .Platform_SetImeDataFn
                    .is_none()
            );
        });
        drop(platform);
        drop(context);
    }
}
