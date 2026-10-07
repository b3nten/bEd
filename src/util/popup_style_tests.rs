use super::*;
use dear_imgui_rs::{
    Condition, Context, FramePrepareOptions, StyleColor, StyleVar, WindowFlags, sys,
};
use std::{ffi::c_void, path::PathBuf};

fn context() -> Context {
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    context
}

/// Exercise real ImGui multi-viewport selection without creating OS windows.
pub(crate) struct PlatformCleanup(dear_imgui_rs::ContextBinding);
impl Drop for PlatformCleanup {
    fn drop(&mut self) {
        self.0.with_bound_context(|| unsafe {
            sys::igDestroyPlatformWindows();
        });
    }
}
pub(crate) fn enable_viewports(context: &mut Context) -> PlatformCleanup {
    unsafe extern "C" fn create(viewport: *mut sys::ImGuiViewport) {
        unsafe {
            (*viewport).PlatformHandle = std::ptr::dangling_mut::<c_void>();
        }
    }
    unsafe extern "C" fn destroy(viewport: *mut sys::ImGuiViewport) {
        unsafe {
            (*viewport).PlatformHandle = std::ptr::null_mut();
        }
    }
    unsafe extern "C" fn noop(_: *mut sys::ImGuiViewport) {}
    unsafe extern "C" fn set_pos(_: *mut sys::ImGuiViewport, _: sys::ImVec2_c) {}
    unsafe extern "C" fn get_pos(viewport: *mut sys::ImGuiViewport) -> sys::ImVec2_c {
        unsafe { (*viewport).Pos }
    }
    unsafe extern "C" fn get_size(viewport: *mut sys::ImGuiViewport) -> sys::ImVec2_c {
        unsafe { (*viewport).Size }
    }
    unsafe extern "C" fn title(_: *mut sys::ImGuiViewport, _: *const std::ffi::c_char) {}
    unsafe extern "C" fn no(_: *mut sys::ImGuiViewport) -> bool {
        false
    }
    unsafe {
        context
            .platform_io_mut()
            .set_monitors(&[sys::ImGuiPlatformMonitor {
                MainPos: [0.0, 0.0].into(),
                MainSize: [1920.0, 1080.0].into(),
                WorkPos: [0.0, 0.0].into(),
                WorkSize: [1920.0, 1080.0].into(),
                DpiScale: 1.0,
                ..Default::default()
            }]);
    }
    context.binding().with_bound_context(|| unsafe {
        let io = &mut *sys::igGetIO_Nil();
        io.ConfigFlags |= sys::ImGuiConfigFlags_ViewportsEnable;
        io.BackendFlags |= sys::ImGuiBackendFlags_PlatformHasViewports
            | sys::ImGuiBackendFlags_RendererHasViewports;
        io.ConfigViewportsNoAutoMerge = true;
        let platform = &mut *sys::igGetPlatformIO_Nil();
        platform.Platform_CreateWindow = Some(create);
        platform.Platform_DestroyWindow = Some(destroy);
        platform.Platform_ShowWindow = Some(noop);
        platform.Platform_SetWindowPos = Some(set_pos);
        platform.Platform_GetWindowPos = Some(get_pos);
        platform.Platform_SetWindowSize = Some(set_pos);
        platform.Platform_GetWindowSize = Some(get_size);
        platform.Platform_SetWindowTitle = Some(title);
        platform.Platform_SetWindowFocus = Some(noop);
        platform.Platform_GetWindowFocus = Some(no);
        platform.Platform_GetWindowMinimized = Some(no);
        (*sys::igGetMainViewport()).PlatformHandle = std::ptr::dangling_mut::<c_void>();
    });
    PlatformCleanup(context.binding())
}

#[derive(Clone, Copy, Debug)]
struct Popup {
    viewport: u32,
    host_viewport: u32,
    owned: bool,
    rounding: f32,
    border: f32,
    pos: [f32; 2],
    size: [f32; 2],
    bounds: [f32; 4],
}

fn popup_frame(
    context: &mut Context,
    guarded: bool,
    detached: bool,
    open: bool,
    close: bool,
) -> Option<Popup> {
    let anchor = if detached {
        [1395.0, 495.0]
    } else {
        [635.0, 475.0]
    };
    context.io_mut().add_mouse_pos_event(anchor);
    context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
    let ui = context.frame();
    if !detached {
        ui.set_next_window_viewport(ui.main_viewport().id());
    }
    let mut result = None;
    ui.window("Popup host")
        .position(
            if detached {
                [800.0, 100.0]
            } else {
                [20.0, 20.0]
            },
            Condition::Always,
        )
        .size([600.0, 400.0], Condition::Always)
        .flags(WindowFlags::NO_TITLE_BAR | WindowFlags::NO_MOVE | WindowFlags::NO_RESIZE)
        .build(|| {
            let host_viewport = ui.window_viewport().id().raw();
            if open {
                ui.open_popup("Same menu");
            }
            let before = ui.clone_style();
            let _style = guarded.then(|| context_menu_style(ui));
            {
                if let Some(_popup) = ui.begin_popup("Same menu") {
                    ui.menu_item("A menu entry with a consistent surface");
                    ui.menu_item("Second entry");
                    ui.with_bound_context(|| unsafe {
                        let window = &*sys::igGetCurrentWindowRead();
                        let viewport = &(*window.Viewport)._ImGuiViewport;
                        result = Some(Popup {
                            viewport: window.ViewportId,
                            host_viewport,
                            owned: window.ViewportOwned,
                            rounding: window.WindowRounding,
                            border: window.WindowBorderSize,
                            pos: [window.Pos.x, window.Pos.y],
                            size: [window.Size.x, window.Size.y],
                            bounds: [
                                viewport.WorkPos.x,
                                viewport.WorkPos.y,
                                viewport.WorkPos.x + viewport.WorkSize.x,
                                viewport.WorkPos.y + viewport.WorkSize.y,
                            ],
                        });
                    });
                    if close {
                        ui.close_current_popup();
                    }
                }
            }
            drop(_style);
            assert_eq!(ui.clone_style().popup_rounding(), before.popup_rounding());
            assert_eq!(
                ui.clone_style().popup_border_size(),
                before.popup_border_size()
            );
            assert_eq!(ui.clone_style().child_rounding(), before.child_rounding());
            assert_eq!(
                ui.clone_style().child_border_size(),
                before.child_border_size()
            );
            assert_eq!(ui.clone_style().window_padding(), before.window_padding());
        });
    drop(context.render_legacy());
    context.binding().with_bound_context(|| unsafe {
        sys::igUpdatePlatformWindows();
    });
    result
}

#[test]
fn reopening_menu_keeps_rounding_border_and_originating_viewport_across_contexts() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = context();
    let _backend = enable_viewports(&mut context);
    context.style_mut().set_popup_rounding(POPUP_ROUNDING);
    // Reproduce the native corner change: escaping the source OS window makes
    // an unscoped popup own a viewport, and ImGui forces its rounding to zero.
    popup_frame(&mut context, false, true, true, false);
    let escaped = popup_frame(&mut context, false, true, false, true).unwrap();
    assert!(
        escaped.owned,
        "boundary popup must reproduce native ownership: {escaped:?}"
    );
    assert_eq!(escaped.rounding, 0.0);
    for (detached, font, inherited_rounding, inherited_border) in [
        (true, 13.0, 0.0, 0.0),
        (true, 25.0, 28.0, 5.0),
        (false, 17.0, 0.0, 3.0),
    ] {
        context.style_mut().set_font_size_base(font);
        context.style_mut().set_popup_rounding(inherited_rounding);
        context.style_mut().set_popup_border_size(inherited_border);
        context.style_mut().set_child_rounding(inherited_rounding);
        context.style_mut().set_child_border_size(inherited_border);
        popup_frame(&mut context, true, detached, true, false);
        let popup = popup_frame(&mut context, true, detached, false, false).unwrap();
        assert_eq!(popup.viewport, popup.host_viewport, "{popup:?}");
        assert!(!popup.owned, "{popup:?}");
        assert_eq!(popup.rounding, POPUP_ROUNDING);
        assert_eq!(popup.border, POPUP_BORDER_SIZE);
        assert!(
            popup.pos[0] >= popup.bounds[0] && popup.pos[1] >= popup.bounds[1],
            "{popup:?}"
        );
        assert!(
            popup.pos[0] + popup.size[0] <= popup.bounds[2] + 1.0
                && popup.pos[1] + popup.size[1] <= popup.bounds[3] + 1.0,
            "{popup:?}"
        );
        popup_frame(&mut context, true, detached, false, true);
    }
}

#[test]
fn unused_context_style_restores_next_window_state_and_themed_input_cursor() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = context();
    let original_cursor = [0.8, 0.1, 0.7, 1.0];
    context
        .style_mut()
        .set_color(StyleColor::InputTextCursor, original_cursor);
    context
        .style_mut()
        .set_color(StyleColor::WindowBg, [0.97, 0.95, 0.90, 1.0]);
    context
        .style_mut()
        .set_color(StyleColor::Text, [0.12, 0.16, 0.22, 1.0]);
    context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
    let ui = context.frame();
    ui.window("Scoped styles").build(|| {
        let before = ui.with_bound_context(|| unsafe {
            (*sys::igGetCurrentContext()).NextWindowData.HasFlags
        });
        {
            let _style = context_menu_style(ui);
            assert_eq!(
                ui.style_color(StyleColor::InputTextCursor),
                ui.style_color(StyleColor::Text)
            );
            assert!(
                bed_core::util::color::contrast_ratio(
                    ui.style_color(StyleColor::InputTextCursor),
                    ui.style_color(StyleColor::FrameBg)
                ) >= 4.49
            );
        }
        assert_eq!(ui.style_color(StyleColor::InputTextCursor), original_cursor);
        assert_eq!(
            ui.with_bound_context(|| unsafe {
                (*sys::igGetCurrentContext()).NextWindowData.HasFlags
            }),
            before
        );
        let _dialog = dialog_style(ui);
        assert_eq!(
            ui.style_color(StyleColor::InputTextCursor),
            ui.style_color(StyleColor::Text)
        );
        let mut name = String::new();
        ui.set_keyboard_focus_here();
        ui.input_text("Name", &mut name).build();
    });
    drop(context.render_legacy());
    assert_eq!(
        context.style().color(StyleColor::InputTextCursor),
        original_cursor
    );
}

#[test]
fn dock_tab_list_near_viewport_edge_keeps_rounded_host_surface_on_reopen() {
    const FIRST: &std::ffi::CStr =
        c"An intentionally long document label for this dock menu###dock_one";
    const SECOND: &std::ffi::CStr = c"Second document###dock_two";
    fn frame(context: &mut Context, right: &mut u32, open: bool, close: bool) -> Option<Popup> {
        context.io_mut().add_mouse_pos_event([630.0, 30.0]);
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        let mut popup = None;
        let _padding = ui.push_style_var(StyleVar::WindowPadding([0.0; 2]));
        ui.set_next_window_viewport(ui.main_viewport().id());
        ui.window("Dock menu host")
            .position([0.0; 2], Condition::Always)
            .size([640.0, 480.0], Condition::Always)
            .flags(WindowFlags::NO_DECORATION | WindowFlags::NO_DOCKING)
            .build(|| {
                let host_viewport = ui.window_viewport().id().raw();
                let dock = ui.get_id("dock_menu_space").raw();
                ui.with_bound_context(|| unsafe {
                    if *right == 0 {
                        sys::igDockBuilderAddNode(dock, sys::ImGuiDockNodeFlags_DockSpace);
                        sys::igDockBuilderSetNodeSize(dock, [640.0, 480.0].into());
                        let mut left = 0;
                        sys::igDockBuilderSplitNode(
                            dock,
                            sys::ImGuiDir_Right,
                            0.25,
                            right,
                            &mut left,
                        );
                        sys::igDockBuilderDockWindow(FIRST.as_ptr(), *right);
                        sys::igDockBuilderDockWindow(SECOND.as_ptr(), *right);
                        sys::igDockBuilderFinish(dock);
                    }
                    if open {
                        // The native button opens this popup under the node's
                        // ID override; exercise the same ID and source anchor.
                        sys::igPushOverrideID(*right);
                        sys::igOpenPopup_Str(c"#WindowMenu".as_ptr(), 0);
                        sys::igPopID();
                    }
                });
                let _style = context_menu_style(ui);
                ui.with_bound_context(|| unsafe {
                    sys::igDockSpace(dock, [0.0; 2].into(), 0, std::ptr::null());
                    let native = &*sys::igGetCurrentContext();
                    if native.OpenPopupStack.Size > 0 {
                        let window = (*native.OpenPopupStack.Data).Window;
                        if !window.is_null() && (*window).Active && !(*window).Hidden {
                            let window = &*window;
                            let viewport = &(*window.Viewport)._ImGuiViewport;
                            popup = Some(Popup {
                                viewport: window.ViewportId,
                                host_viewport,
                                owned: window.ViewportOwned,
                                rounding: window.WindowRounding,
                                border: window.WindowBorderSize,
                                pos: [window.Pos.x, window.Pos.y],
                                size: [window.Size.x, window.Size.y],
                                bounds: [
                                    viewport.WorkPos.x,
                                    viewport.WorkPos.y,
                                    viewport.WorkPos.x + viewport.WorkSize.x,
                                    viewport.WorkPos.y + viewport.WorkSize.y,
                                ],
                            });
                        }
                        if close {
                            sys::igClosePopupToLevel(0, true);
                        }
                    }
                });
            });
        drop(_padding);
        for name in [FIRST, SECOND] {
            ui.window(name.to_str().unwrap())
                .build(|| ui.text("Document"));
        }
        drop(context.render_legacy());
        context.binding().with_bound_context(|| unsafe {
            sys::igUpdatePlatformWindows();
        });
        popup
    }
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = context();
    let _backend = enable_viewports(&mut context);
    context.binding().with_bound_context(|| unsafe {
        (*sys::igGetIO_Nil()).ConfigFlags |= sys::ImGuiConfigFlags_DockingEnable;
    });
    let mut right = 0;
    for _ in 0..3 {
        frame(&mut context, &mut right, false, false);
    }
    for (rounding, border) in [(0.0, 4.0), (30.0, 0.0)] {
        context.style_mut().set_popup_rounding(rounding);
        context.style_mut().set_popup_border_size(border);
        frame(&mut context, &mut right, true, false);
        let popup = frame(&mut context, &mut right, false, false)
            .expect("native dock tab-list menu must open");
        assert_eq!(popup.viewport, popup.host_viewport, "{popup:?}");
        assert!(!popup.owned, "{popup:?}");
        assert_eq!(popup.rounding, POPUP_ROUNDING);
        assert_eq!(popup.border, POPUP_BORDER_SIZE);
        assert!(
            popup.size[0] < 640.0,
            "menu entries must fit the source viewport: {popup:?}"
        );
        assert!(
            popup.pos[0] >= popup.bounds[0] && popup.pos[1] >= popup.bounds[1],
            "{popup:?}"
        );
        assert!(
            popup.pos[0] + popup.size[0] <= popup.bounds[2] + 1.0
                && popup.pos[1] + popup.size[1] <= popup.bounds[3] + 1.0,
            "{popup:?}"
        );
        frame(&mut context, &mut right, false, true);
    }
}

#[test]
fn fallback_scope_uses_main_viewport_and_respects_an_explicit_detached_destination() {
    fn frame(
        context: &mut Context,
        explicit: bool,
        open: bool,
        close: bool,
    ) -> Option<(u32, u32, bool)> {
        context.io_mut().add_mouse_pos_event(if explicit {
            [1190.0, 390.0]
        } else {
            [630.0, 470.0]
        });
        context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
        let ui = context.frame();
        let mut detached = ui.main_viewport().id();
        ui.window("Explicit detached popup host")
            .position([800.0, 100.0], Condition::Always)
            .size([400.0, 300.0], Condition::Always)
            .build(|| {
                detached = ui.window_viewport().id();
                ui.text("Host");
            });
        assert_ne!(detached, ui.main_viewport().id());
        assert!(
            ui.with_bound_context(|| unsafe { (*sys::igGetCurrentWindowRead()).IsFallbackWindow })
        );
        if open {
            ui.open_popup("Fallback actions");
        }
        if explicit {
            ui.set_next_window_viewport(detached);
        }
        let destination = if explicit {
            detached
        } else {
            ui.main_viewport().id()
        };
        let mut popup = None;
        {
            let _style = context_menu_style(ui);
            if let Some(_popup) = ui.begin_popup("Fallback actions") {
                ui.menu_item("Close");
                ui.menu_item("Close Others");
                popup = Some(ui.with_bound_context(|| unsafe {
                    let window = &*sys::igGetCurrentWindowRead();
                    assert_eq!(window.WindowRounding, POPUP_ROUNDING);
                    assert_eq!(window.WindowBorderSize, POPUP_BORDER_SIZE);
                    (window.ViewportId, destination.raw(), window.ViewportOwned)
                }));
                if close {
                    ui.close_current_popup();
                }
            }
        }
        drop(context.render_legacy());
        context.binding().with_bound_context(|| unsafe {
            sys::igUpdatePlatformWindows();
        });
        popup
    }
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = context();
    let _backend = enable_viewports(&mut context);
    frame(&mut context, false, false, false);
    for explicit in [false, true] {
        frame(&mut context, explicit, true, false);
        let (actual, expected, owned) = frame(&mut context, explicit, false, false)
            .expect("fallback-scoped popup must remain visible");
        assert_eq!(actual, expected);
        assert!(
            !owned,
            "a fallback-scoped popup must use a live host viewport"
        );
        frame(&mut context, explicit, false, true);
    }
}
