//! Per-window native caption adapter translated from ned util/windows_window.cpp.
//! The portable hit-test state is also exercised on non-Windows test hosts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptionHit {
    #[default]
    Client,
    Min,
    Max,
    Close,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitlebarAction {
    Sidebar,
    Terminal,
    Settings,
    Search,
    Diagnostics,
    Structure,
    SplitRight,
    SplitDown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowHit {
    Client,
    Caption,
    Button(CaptionHit),
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}
#[derive(Clone, Copy)]
struct Exclude {
    min: [f32; 2],
    max: [f32; 2],
    hit: CaptionHit,
}
pub struct CaptionState {
    height: f32,
    resize_border: f32,
    excludes: Vec<Exclude>,
    hover: CaptionHit,
}
impl Default for CaptionState {
    fn default() -> Self {
        Self {
            height: 32.0,
            resize_border: 8.0,
            excludes: Vec::new(),
            hover: CaptionHit::Client,
        }
    }
}
impl CaptionState {
    pub fn set_height(&mut self, height: f32) {
        self.height = height.max(24.0);
        self.resize_border = (height * 0.22).max(6.0);
    }
    pub fn height(&self) -> f32 {
        self.height
    }
    pub fn clear_excludes(&mut self) {
        self.excludes.clear();
    }
    pub fn exclude(&mut self, min: [f32; 2], max: [f32; 2], hit: CaptionHit) {
        self.excludes.push(Exclude { min, max, hit });
    }
    pub fn hover(&self) -> CaptionHit {
        self.hover
    }
    pub fn reset_hover(&mut self) {
        self.hover = CaptionHit::Client;
    }
    pub fn excluded(&self, point: [f32; 2]) -> Option<CaptionHit> {
        self.excludes
            .iter()
            .find(|e| {
                point[0] >= e.min[0]
                    && point[0] < e.max[0]
                    && point[1] >= e.min[1]
                    && point[1] < e.max[1]
            })
            .map(|e| e.hit)
    }
    pub fn hit_test(
        &mut self,
        point: [i32; 2],
        client: [i32; 2],
        size: [i32; 2],
        maximized: bool,
    ) -> WindowHit {
        if let Some(hit) = self.excluded([client[0] as f32, client[1] as f32]) {
            self.hover = hit;
            return if hit == CaptionHit::Client {
                WindowHit::Client
            } else {
                WindowHit::Button(hit)
            };
        }
        self.reset_hover();
        let [x, y] = point;
        let [w, h] = size;
        let b = self.resize_border.max(6.0) as i32;
        if !maximized {
            if x < b && y < b {
                return WindowHit::TopLeft;
            }
            if x >= w - b && y < b {
                return WindowHit::TopRight;
            }
            if x < b && y >= h - b {
                return WindowHit::BottomLeft;
            }
            if x >= w - b && y >= h - b {
                return WindowHit::BottomRight;
            }
            if x < b {
                return WindowHit::Left;
            }
            if x >= w - b {
                return WindowHit::Right;
            }
            if y < b {
                return WindowHit::Top;
            }
            if y >= h - b {
                return WindowHit::Bottom;
            }
        }
        if client[1] >= 0 && client[1] < self.height as i32 {
            WindowHit::Caption
        } else {
            WindowHit::Client
        }
    }
}

/// Surface-independent part of the original Workbench Windows caption drawing.
pub trait CaptionHost {
    fn set_titlebar_height(&mut self, height: f32);
    fn clear_caption_excludes(&mut self);
    /// Logical client coordinates; native adapters apply the window's DPI scale.
    fn exclude_caption_rect(&mut self, min: [f32; 2], max: [f32; 2], hit: CaptionHit);
    fn caption_hover(&self) -> CaptionHit;
    fn is_maximized(&self) -> bool;
    fn minimize(&self);
    fn toggle_maximize(&self);
    fn close(&self);
}
pub fn draw_titlebar(
    host: &mut impl CaptionHost,
    ui: &dear_imgui_rs::Ui,
    _settings: &crate::util::settings::Settings,
    icons: &crate::util::icons::Icons,
    title: &str,
) -> Vec<TitlebarAction> {
    use dear_imgui_rs::{Condition, StyleColor, StyleVar, WindowFlags};

    let fs = ui.current_font_size();
    let h = (fs * 1.7).max(28.0);
    host.set_titlebar_height(h);
    host.clear_caption_excludes();
    let _padding = ui.push_style_var(StyleVar::WindowPadding([0.0; 2]));
    let _border = ui.push_style_var(StyleVar::WindowBorderSize(0.0));
    let _round = ui.push_style_var(StyleVar::WindowRounding(0.0));
    let _spacing = ui.push_style_var(StyleVar::ItemSpacing([0.0; 2]));
    let mut actions = Vec::new();
    ui.set_next_window_viewport(ui.main_viewport().id());
    ui.window("##bed_win_titlebar")
        .position(ui.main_viewport().pos(), Condition::Always)
        .size([ui.io().display_size()[0], h], Condition::Always)
        .flags(
            WindowFlags::NO_DECORATION
                | WindowFlags::NO_MOVE
                | WindowFlags::NO_RESIZE
                | WindowFlags::NO_SAVED_SETTINGS
                | WindowFlags::NO_SCROLLBAR
                | WindowFlags::NO_SCROLL_WITH_MOUSE
                | WindowFlags::NO_DOCKING
                | WindowFlags::NO_NAV
                | WindowFlags::NO_BRING_TO_FRONT_ON_FOCUS,
        )
        .build(|| {
            let origin = ui.window_pos();
            let client = |p: [f32; 2]| [p[0] - origin[0], p[1] - origin[1]];
            let dl = ui.get_window_draw_list();
            let ink = ui.style_color(StyleColor::Text);
            let stroke = (fs * 0.07).max(1.0);
            let bw = (fs * 2.15).max(36.0);
            let caption_x = ui.window_size()[0] - bw * 3.0;
            let gap = 2.0;
            let tx = fs * 0.7;
            let aw = (fs * 1.7)
                .max(28.0)
                .min(((caption_x - tx * 2.0 - gap * 7.0) / 8.0).max(1.0));
            let tools_width = aw * 8.0 + gap * 7.0;
            let ts = ui.calc_text_size(title);
            let title_space = (caption_x - tx * 2.0 - tools_width - fs * 0.85).max(0.0);
            let mut tool_x = tx;
            if title_space >= fs * 2.0 {
                let visible_width = ts[0].min(title_space);
                let _clip = ui.push_clip_rect(
                    [origin[0] + tx, origin[1]],
                    [origin[0] + tx + visible_width, origin[1] + h],
                    true,
                );
                ui.set_cursor_pos([tx, (h - ts[1]) * 0.5]);
                ui.text(title);
                tool_x += visible_width + fs * 0.85;
            }
            for (id, tip, action) in [
                ("##tb_sidebar", "New File Explorer", TitlebarAction::Sidebar),
                ("##tb_term", "New Terminal", TitlebarAction::Terminal),
                ("##tb_search", "New Project Search", TitlebarAction::Search),
                ("##tb_structure", "New Structure", TitlebarAction::Structure),
                (
                    "##tb_diagnostics",
                    "New Diagnostics",
                    TitlebarAction::Diagnostics,
                ),
                (
                    "##tb_split_right",
                    "Split Editor Right",
                    TitlebarAction::SplitRight,
                ),
                (
                    "##tb_split_down",
                    "Split Editor Down",
                    TitlebarAction::SplitDown,
                ),
                ("##tb_set", "New Settings", TitlebarAction::Settings),
            ] {
                ui.set_cursor_pos([tool_x, 0.0]);
                let hit = ui.invisible_button(id, [aw, h]);
                let a = ui.item_rect_min();
                let b = ui.item_rect_max();
                host.exclude_caption_rect(client(a), client(b), CaptionHit::Client);
                if ui.is_item_hovered() {
                    ui.tooltip_text(tip);
                    dl.add_rect(a, b, ui.style_color(StyleColor::ButtonHovered))
                        .filled(true)
                        .build();
                }
                draw_tool_icon(&dl, action, a, b, ink, fs, icons);
                if hit {
                    actions.push(action);
                }
                tool_x += aw + gap;
            }
            let mut x = caption_x;
            for (id, part) in [
                ("##tb_min", CaptionHit::Min),
                ("##tb_max", CaptionHit::Max),
                ("##tb_close", CaptionHit::Close),
            ] {
                ui.set_cursor_pos([x, 0.0]);
                let clicked = ui.invisible_button(id, [bw, h]);
                let a = ui.item_rect_min();
                let b = ui.item_rect_max();
                host.exclude_caption_rect(client(a), client(b), part);
                let hovered = ui.is_item_hovered() || host.caption_hover() == part;
                if hovered {
                    dl.add_rect(
                        a,
                        b,
                        if part == CaptionHit::Close {
                            [232.0 / 255.0, 17.0 / 255.0, 35.0 / 255.0, 1.0]
                        } else {
                            ui.style_color(StyleColor::ButtonHovered)
                        },
                    )
                    .filled(true)
                    .build();
                }
                let col = if hovered && part == CaptionHit::Close {
                    [1.0; 4]
                } else {
                    ink
                };
                let c = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
                match part {
                    CaptionHit::Min => {
                        let w = fs * 0.45;
                        dl.add_line([c[0] - w, c[1]], [c[0] + w, c[1]], col)
                            .thickness(stroke)
                            .build();
                        if clicked {
                            host.minimize();
                        }
                    }
                    CaptionHit::Max => {
                        let s = fs * 0.42;
                        if host.is_maximized() {
                            dl.add_rect(
                                [c[0] - s + 2.0, c[1] - s],
                                [c[0] + s, c[1] + s - 2.0],
                                col,
                            )
                            .thickness(stroke)
                            .build();
                            dl.add_rect(
                                [c[0] - s, c[1] - s + 3.0],
                                [c[0] + s - 2.0, c[1] + s],
                                col,
                            )
                            .thickness(stroke)
                            .build();
                        } else {
                            dl.add_rect([c[0] - s, c[1] - s], [c[0] + s, c[1] + s], col)
                                .thickness(stroke)
                                .build();
                        }
                        if clicked {
                            host.toggle_maximize();
                        }
                    }
                    CaptionHit::Close => {
                        let s = fs * 0.38;
                        dl.add_line([c[0] - s, c[1] - s], [c[0] + s, c[1] + s], col)
                            .thickness(stroke)
                            .build();
                        dl.add_line([c[0] + s, c[1] - s], [c[0] - s, c[1] + s], col)
                            .thickness(stroke)
                            .build();
                        if clicked {
                            host.close();
                        }
                    }
                    CaptionHit::Client => {}
                }
                x += bw;
            }
        });
    actions
}

fn draw_tool_icon(
    dl: &dear_imgui_rs::DrawListMut<'_>,
    action: TitlebarAction,
    a: [f32; 2],
    b: [f32; 2],
    ink: [f32; 4],
    font_size: f32,
    icons: &crate::util::icons::Icons,
) {
    let stroke = (font_size * 0.07).max(1.0);
    let side = (b[0] - a[0]).min(b[1] - a[1]) * 0.46;
    let c = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
    let s = side * 0.5;
    let p0 = [c[0] - s, c[1] - s];
    let p1 = [c[0] + s, c[1] + s];
    match action {
        TitlebarAction::Sidebar
        | TitlebarAction::Terminal
        | TitlebarAction::SplitRight
        | TitlebarAction::SplitDown => {
            dl.add_rect(p0, p1, ink)
                .rounding(1.0)
                .thickness(stroke)
                .build();
            if matches!(action, TitlebarAction::Sidebar | TitlebarAction::SplitRight) {
                let split = p0[0]
                    + side
                        * if action == TitlebarAction::Sidebar {
                            0.32
                        } else {
                            0.5
                        };
                dl.add_line([split, p0[1]], [split, p1[1]], ink)
                    .thickness(stroke)
                    .build();
            } else {
                let split = p0[1]
                    + side
                        * if action == TitlebarAction::Terminal {
                            0.67
                        } else {
                            0.5
                        };
                dl.add_line([p0[0], split], [p1[0], split], ink)
                    .thickness(stroke)
                    .build();
            }
        }
        TitlebarAction::Search => {
            let center = [c[0] - s * 0.25, c[1] - s * 0.25];
            dl.add_circle(center, s * 0.7, ink)
                .thickness(stroke)
                .build();
            dl.add_line([c[0] + s * 0.28, c[1] + s * 0.28], p1, ink)
                .thickness(stroke)
                .build();
        }
        TitlebarAction::Diagnostics => {
            dl.add_triangle([c[0], p0[1]], [p0[0], p1[1]], p1, ink)
                .thickness(stroke)
                .build();
            dl.add_line([c[0], c[1] - s * 0.25], [c[0], c[1] + s * 0.25], ink)
                .thickness(stroke)
                .build();
            dl.add_circle([c[0], c[1] + s * 0.62], stroke * 0.5, ink)
                .filled(true)
                .build();
        }
        TitlebarAction::Structure => {
            let trunk = c[0] - s * 0.65;
            dl.add_line([trunk, p0[1]], [trunk, p1[1]], ink)
                .thickness(stroke)
                .build();
            for y in [p0[1] + side * 0.2, c[1], p1[1] - side * 0.1] {
                dl.add_line([trunk, y], [c[0] - s * 0.05, y], ink)
                    .thickness(stroke)
                    .build();
                dl.add_line([c[0] + s * 0.2, y], [p1[0], y], ink)
                    .thickness(stroke)
                    .build();
            }
        }
        TitlebarAction::Settings => {
            if let Some(texture) = icons.get("gear") {
                dl.add_image(texture, p0, p1, [0.0; 2], [1.0; 2], ink);
            } else {
                dl.add_circle(c, s * 0.66, ink).thickness(stroke).build();
                dl.add_circle(c, s * 0.25, ink).thickness(stroke).build();
                for i in 0..8 {
                    let angle = i as f32 * std::f32::consts::FRAC_PI_4;
                    let v = [angle.cos(), angle.sin()];
                    dl.add_line(
                        [c[0] + v[0] * s * 0.66, c[1] + v[1] * s * 0.66],
                        [c[0] + v[0] * s, c[1] + v[1] * s],
                        ink,
                    )
                    .thickness(stroke)
                    .build();
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod native {
    use super::*;
    use dear_imgui_rs::Ui;
    use std::{cell::RefCell, io, sync::Arc};
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        Graphics::{Dwm::*, Gdi::*},
        UI::{
            Controls::{MARGINS, WM_MOUSELEAVE},
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    };
    use winit::{
        raw_window_handle::{HasWindowHandle, RawWindowHandle},
        window::Window,
    };

    const SUBCLASS_ID: usize = 0x424544;
    pub struct WindowsWindow {
        window: Arc<Window>,
        hwnd: HWND,
        state: Box<RefCell<CaptionState>>,
        caption_ui_height: f32,
    }
    impl WindowsWindow {
        pub fn configure(window: Arc<Window>) -> io::Result<Self> {
            let RawWindowHandle::Win32(handle) =
                window.window_handle().map_err(io::Error::other)?.as_raw()
            else {
                return Err(io::Error::other("window has no HWND"));
            };
            let hwnd = handle.hwnd.get() as HWND;
            let state = Box::new(RefCell::new(CaptionState::default()));
            // SAFETY: Winit owns the live HWND. Box keeps subclass state at a fixed
            // address until Drop removes our procedure; unhandled messages forward.
            unsafe {
                let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                SetWindowLongPtrW(
                    hwnd,
                    GWL_STYLE,
                    (style & !((WS_POPUP | WS_CHILD) as isize))
                        | ((WS_OVERLAPPEDWINDOW | WS_CLIPSIBLINGS | WS_CLIPCHILDREN) as isize),
                );
                let margins = MARGINS {
                    cxLeftWidth: 0,
                    cxRightWidth: 0,
                    cyTopHeight: 1,
                    cyBottomHeight: 0,
                };
                DwmExtendFrameIntoClientArea(hwnd, &margins);
                let dark: i32 = 1;
                DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                    (&dark as *const i32).cast(),
                    4,
                );
                corner_pref(hwnd, false);
                if SetWindowSubclass(
                    hwnd,
                    Some(window_proc),
                    SUBCLASS_ID,
                    (&*state as *const RefCell<CaptionState>) as usize,
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
                );
            }
            Ok(Self {
                window,
                hwnd,
                state,
                caption_ui_height: 32.0,
            })
        }
        pub fn set_titlebar_height(&mut self, height: f32) {
            self.caption_ui_height = height.max(24.0);
            self.state
                .borrow_mut()
                .set_height(height * self.window.scale_factor() as f32);
        }
        pub fn titlebar_inset(&self) -> f32 {
            self.caption_ui_height
        }
        pub fn clear_caption_excludes(&mut self) {
            self.state.borrow_mut().clear_excludes();
        }
        pub fn exclude_caption_rect(&mut self, min: [f32; 2], max: [f32; 2], hit: CaptionHit) {
            let scale = self.window.scale_factor() as f32;
            self.state
                .borrow_mut()
                .exclude(min.map(|x| x * scale), max.map(|x| x * scale), hit);
        }
        pub fn caption_hover(&self) -> CaptionHit {
            self.state.borrow().hover()
        }
        pub fn is_maximized(&self) -> bool {
            unsafe { IsZoomed(self.hwnd) != 0 }
        }
        pub fn minimize(&self) {
            unsafe {
                ShowWindow(self.hwnd, SW_MINIMIZE);
            }
        }
        pub fn toggle_maximize(&self) {
            unsafe {
                ShowWindow(
                    self.hwnd,
                    if IsZoomed(self.hwnd) != 0 {
                        SW_RESTORE
                    } else {
                        SW_MAXIMIZE
                    },
                );
            }
        }
        pub fn close(&self) {
            unsafe {
                PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
            }
        }
        pub fn draw_titlebar(
            &mut self,
            ui: &Ui,
            settings: &crate::util::settings::Settings,
            icons: &crate::util::icons::Icons,
            title: &str,
        ) -> Vec<TitlebarAction> {
            let _ = Self::apply_theme_to_window(
                &self.window,
                settings.text_color(),
                settings.background_color(),
            );
            super::draw_titlebar(self, ui, settings, icons, title)
        }
        pub fn apply_theme_to_window(
            window: &Window,
            _text: [f32; 4],
            background: [f32; 4],
        ) -> io::Result<()> {
            let RawWindowHandle::Win32(handle) =
                window.window_handle().map_err(io::Error::other)?.as_raw()
            else {
                return Err(io::Error::other("window has no HWND"));
            };
            let dark = i32::from(bed_core::util::color::relative_luminance(background) <= 0.179);
            unsafe {
                DwmSetWindowAttribute(
                    handle.hwnd.get() as HWND,
                    DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                    (&dark as *const i32).cast(),
                    4,
                );
            }
            Ok(())
        }
    }
    impl super::CaptionHost for WindowsWindow {
        fn set_titlebar_height(&mut self, height: f32) {
            WindowsWindow::set_titlebar_height(self, height);
        }
        fn clear_caption_excludes(&mut self) {
            WindowsWindow::clear_caption_excludes(self);
        }
        fn exclude_caption_rect(&mut self, min: [f32; 2], max: [f32; 2], hit: CaptionHit) {
            WindowsWindow::exclude_caption_rect(self, min, max, hit);
        }
        fn caption_hover(&self) -> CaptionHit {
            WindowsWindow::caption_hover(self)
        }
        fn is_maximized(&self) -> bool {
            WindowsWindow::is_maximized(self)
        }
        fn minimize(&self) {
            WindowsWindow::minimize(self);
        }
        fn toggle_maximize(&self) {
            WindowsWindow::toggle_maximize(self);
        }
        fn close(&self) {
            WindowsWindow::close(self);
        }
    }
    impl Drop for WindowsWindow {
        fn drop(&mut self) {
            unsafe {
                if IsWindow(self.hwnd) != 0 {
                    RemoveWindowSubclass(self.hwnd, Some(window_proc), SUBCLASS_ID);
                }
            }
        }
    }
    fn native_hit(hit: WindowHit) -> i32 {
        match hit {
            WindowHit::Client => HTCLIENT as i32,
            WindowHit::Caption => HTCAPTION as i32,
            WindowHit::Button(CaptionHit::Min) => HTMINBUTTON as i32,
            WindowHit::Button(CaptionHit::Max) => HTMAXBUTTON as i32,
            WindowHit::Button(CaptionHit::Close) => HTCLOSE as i32,
            WindowHit::Button(CaptionHit::Client) => HTCLIENT as i32,
            WindowHit::Left => HTLEFT as i32,
            WindowHit::Right => HTRIGHT as i32,
            WindowHit::Top => HTTOP as i32,
            WindowHit::Bottom => HTBOTTOM as i32,
            WindowHit::TopLeft => HTTOPLEFT as i32,
            WindowHit::TopRight => HTTOPRIGHT as i32,
            WindowHit::BottomLeft => HTBOTTOMLEFT as i32,
            WindowHit::BottomRight => HTBOTTOMRIGHT as i32,
        }
    }
    unsafe fn corner_pref(hwnd: HWND, max: bool) {
        let pref: u32 = if max { 1 } else { 2 };
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                (&pref as *const u32).cast(),
                4,
            );
        }
    }
    fn message_point(param: LPARAM) -> POINT {
        POINT {
            x: (param as u16 as i16) as i32,
            y: ((param as usize >> 16) as u16 as i16) as i32,
        }
    }
    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        w: WPARAM,
        l: LPARAM,
        _id: usize,
        data: usize,
    ) -> LRESULT {
        // SAFETY: SetWindowSubclass supplies our boxed stable state; main-thread
        // callbacks release all RefCell borrows before native calls can reenter.
        let state = unsafe { &*(data as *const RefCell<CaptionState>) };
        unsafe {
            match msg {
                WM_NCCALCSIZE if w != 0 => {
                    let p = &mut *(l as *mut NCCALCSIZE_PARAMS);
                    if IsZoomed(hwnd) != 0 {
                        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                        let mut info: MONITORINFO = std::mem::zeroed();
                        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                        if GetMonitorInfoW(mon, &mut info) != 0 {
                            p.rgrc[0] = info.rcWork;
                        }
                    }
                    return 0;
                }
                WM_NCHITTEST => {
                    let pt = message_point(l);
                    let mut rect = std::mem::zeroed();
                    GetWindowRect(hwnd, &mut rect);
                    let mut client = pt;
                    ScreenToClient(hwnd, &mut client);
                    let hit = state.borrow_mut().hit_test(
                        [pt.x - rect.left, pt.y - rect.top],
                        [client.x, client.y],
                        [rect.right - rect.left, rect.bottom - rect.top],
                        IsZoomed(hwnd) != 0,
                    );
                    return native_hit(hit) as LRESULT;
                }
                WM_NCLBUTTONDOWN
                    if [HTMINBUTTON as usize, HTMAXBUTTON as usize, HTCLOSE as usize]
                        .contains(&w) =>
                {
                    return 0;
                }
                WM_NCLBUTTONUP => {
                    let mut pt = message_point(l);
                    ScreenToClient(hwnd, &mut pt);
                    let part = state.borrow().excluded([pt.x as f32, pt.y as f32]);
                    match part {
                        Some(CaptionHit::Min) => {
                            ShowWindow(hwnd, SW_MINIMIZE);
                        }
                        Some(CaptionHit::Max) => {
                            ShowWindow(
                                hwnd,
                                if IsZoomed(hwnd) != 0 {
                                    SW_RESTORE
                                } else {
                                    SW_MAXIMIZE
                                },
                            );
                        }
                        Some(CaptionHit::Close) => {
                            PostMessageW(hwnd, WM_CLOSE, 0, 0);
                        }
                        _ => {}
                    }
                    if matches!(
                        part,
                        Some(CaptionHit::Min | CaptionHit::Max | CaptionHit::Close)
                    ) || [HTMINBUTTON as usize, HTMAXBUTTON as usize, HTCLOSE as usize]
                        .contains(&w)
                    {
                        return 0;
                    }
                }
                WM_NCLBUTTONDBLCLK if w == HTCAPTION as usize => {
                    ShowWindow(
                        hwnd,
                        if IsZoomed(hwnd) != 0 {
                            SW_RESTORE
                        } else {
                            SW_MAXIMIZE
                        },
                    );
                    return 0;
                }
                WM_NCMOUSELEAVE | WM_MOUSELEAVE => state.borrow_mut().reset_hover(),
                WM_SIZE => corner_pref(hwnd, w == SIZE_MAXIMIZED as usize),
                WM_NCDESTROY => {
                    RemoveWindowSubclass(hwnd, Some(window_proc), SUBCLASS_ID);
                }
                _ => {}
            }
            DefSubclassProc(hwnd, msg, w, l)
        }
    }
}
#[cfg(target_os = "windows")]
pub use native::WindowsWindow;

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{Context, MouseButton};
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    #[derive(Default)]
    struct ToolbarHost {
        rects: Vec<Exclude>,
    }
    impl CaptionHost for ToolbarHost {
        fn set_titlebar_height(&mut self, _: f32) {}
        fn clear_caption_excludes(&mut self) {
            self.rects.clear();
        }
        fn exclude_caption_rect(&mut self, min: [f32; 2], max: [f32; 2], hit: CaptionHit) {
            self.rects.push(Exclude { min, max, hit });
        }
        fn caption_hover(&self) -> CaptionHit {
            CaptionHit::Client
        }
        fn is_maximized(&self) -> bool {
            false
        }
        fn minimize(&self) {}
        fn toggle_maximize(&self) {}
        fn close(&self) {}
    }
    fn toolbar_frame(
        context: &mut Context,
        host: &mut ToolbarHost,
        settings: &crate::util::settings::Settings,
        width: f32,
    ) -> Vec<TitlebarAction> {
        toolbar_frame_at(context, host, settings, width, [0.0; 2])
    }
    fn toolbar_frame_at(
        context: &mut Context,
        host: &mut ToolbarHost,
        settings: &crate::util::settings::Settings,
        width: f32,
        origin: [f32; 2],
    ) -> Vec<TitlebarAction> {
        context.io_mut().set_display_size([width, 200.0]);
        context.io_mut().set_delta_time(1.0 / 60.0);
        let ui = context.frame();
        // Simulate the platform's desktop origin without creating an OS window
        // or installing multi-viewport backend callbacks in this native UI test.
        ui.with_bound_context(|| unsafe {
            let viewport = dear_imgui_rs::sys::igGetMainViewport();
            (*viewport).Pos = origin.into();
            (*viewport).WorkPos = origin.into();
        });
        let actions = draw_titlebar(
            host,
            ui,
            settings,
            &crate::util::icons::Icons::default(),
            "bEd",
        );
        assert!(context.render_legacy().draw_data().total_vtx_count() > 0);
        actions
    }
    fn check_toolbar_geometry(host: &ToolbarHost) {
        assert_eq!(host.rects.len(), 11);
        let tools = &host.rects[..8];
        let first = tools[0];
        for button in tools {
            assert_eq!(button.hit, CaptionHit::Client);
            assert!((button.max[0] - button.min[0] - first.max[0] + first.min[0]).abs() < 0.01);
            assert_eq!(button.min[1], first.min[1]);
            assert_eq!(button.max[1], first.max[1]);
        }
        for pair in tools.windows(2) {
            assert!((pair[1].min[0] - pair[0].max[0] - 2.0).abs() < 0.01);
        }
        assert!(tools[7].max[0] <= host.rects[8].min[0]);
        assert_eq!(host.rects[8].hit, CaptionHit::Min);
        assert_eq!(host.rects[9].hit, CaptionHit::Max);
        assert_eq!(host.rects[10].hit, CaptionHit::Close);
    }

    #[test]
    fn toolbar_measured_geometry_and_all_eight_click_actions() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "bed-caption-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let settings = crate::util::settings::Settings::with_paths(
            path.clone(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
        let settings_before = settings.settings.clone();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut host = ToolbarHost::default();
        for width in [800.0, 480.0, 320.0] {
            toolbar_frame(&mut context, &mut host, &settings, width);
            toolbar_frame(&mut context, &mut host, &settings, width);
            check_toolbar_geometry(&host);
        }
        toolbar_frame(&mut context, &mut host, &settings, 800.0);
        let rects = host.rects.clone();
        for (rect, expected) in rects.iter().zip([
            TitlebarAction::Sidebar,
            TitlebarAction::Terminal,
            TitlebarAction::Search,
            TitlebarAction::Structure,
            TitlebarAction::Diagnostics,
            TitlebarAction::SplitRight,
            TitlebarAction::SplitDown,
            TitlebarAction::Settings,
        ]) {
            context.io_mut().add_mouse_pos_event([
                (rect.min[0] + rect.max[0]) * 0.5,
                (rect.min[1] + rect.max[1]) * 0.5,
            ]);
            assert!(toolbar_frame(&mut context, &mut host, &settings, 800.0).is_empty());
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, true);
            assert!(toolbar_frame(&mut context, &mut host, &settings, 800.0).is_empty());
            context
                .io_mut()
                .add_mouse_button_event(MouseButton::Left, false);
            assert_eq!(
                toolbar_frame(&mut context, &mut host, &settings, 800.0),
                vec![expected]
            );
        }
        assert_eq!(settings.settings, settings_before);
        assert!(!settings.show_settings_window);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn toolbar_hit_rectangles_are_client_relative_at_a_nonzero_desktop_origin() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let path =
            std::env::temp_dir().join(format!("bed-caption-origin-test-{}", std::process::id()));
        let settings = crate::util::settings::Settings::with_paths(
            path.clone(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut host = ToolbarHost::default();
        toolbar_frame(&mut context, &mut host, &settings, 800.0);
        toolbar_frame(&mut context, &mut host, &settings, 800.0);
        let original = host.rects.clone();
        let origin = [345.0, 167.0];
        for _ in 0..2 {
            toolbar_frame_at(&mut context, &mut host, &settings, 800.0, origin);
        }
        check_toolbar_geometry(&host);
        for (before, after) in original.iter().zip(&host.rects) {
            for (before, after) in before
                .min
                .into_iter()
                .chain(before.max)
                .zip(after.min.into_iter().chain(after.max))
            {
                assert!((before - after).abs() < 0.0001);
            }
            assert_eq!(before.hit, after.hit);
        }
        // Win32 supplies physical client points. Retain native DPI conversion
        // while excluding toolbar buttons from dragging and preserving Snap hits.
        let mut state = CaptionState::default();
        state.set_height(host.rects[0].max[1] * 2.0);
        for rect in &host.rects {
            state.exclude(
                rect.min.map(|v| v * 2.0),
                rect.max.map(|v| v * 2.0),
                rect.hit,
            );
        }
        for rect in &host.rects {
            let point = [
                (rect.min[0] + rect.max[0]) as i32,
                (rect.min[1] + rect.max[1]) as i32,
            ];
            let expected = if rect.hit == CaptionHit::Client {
                WindowHit::Client
            } else {
                WindowHit::Button(rect.hit)
            };
            assert_eq!(state.hit_test(point, point, [1600, 400], false), expected);
        }
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn excludes_precede_grips_and_use_half_open_bounds() {
        let mut s = CaptionState::default();
        s.exclude([0.0, 0.0], [40.0, 32.0], CaptionHit::Max);
        assert_eq!(
            s.hit_test([1, 1], [1, 1], [100, 100], false),
            WindowHit::Button(CaptionHit::Max)
        );
        assert_eq!(s.hover(), CaptionHit::Max);
        assert_eq!(
            s.hit_test([40, 1], [40, 1], [100, 100], false),
            WindowHit::Top
        );
        assert_eq!(s.hover(), CaptionHit::Client);
        s.clear_excludes();
        s.exclude([0.0, 0.0], [40.0, 32.0], CaptionHit::Client);
        assert_eq!(
            s.hit_test([1, 1], [1, 1], [100, 100], false),
            WindowHit::Client
        );
    }
    #[test]
    fn resize_edges_maximized_caption_and_fractional_height_follow_source() {
        let mut s = CaptionState::default();
        s.set_height(28.9);
        for (p, expected) in [
            ([1, 1], WindowHit::TopLeft),
            ([99, 1], WindowHit::TopRight),
            ([1, 99], WindowHit::BottomLeft),
            ([99, 99], WindowHit::BottomRight),
            ([1, 50], WindowHit::Left),
            ([99, 50], WindowHit::Right),
            ([50, 1], WindowHit::Top),
            ([50, 99], WindowHit::Bottom),
        ] {
            assert_eq!(s.hit_test(p, p, [100, 100], false), expected);
        }
        assert_eq!(
            s.hit_test([1, 1], [1, 1], [100, 100], true),
            WindowHit::Caption
        );
        assert_eq!(
            s.hit_test([50, 28], [50, 28], [100, 100], true),
            WindowHit::Client
        );
        assert_eq!(
            s.hit_test([50, 27], [50, 27], [100, 100], true),
            WindowHit::Caption
        );
        assert_eq!(
            s.hit_test([50, -1], [50, -1], [100, 100], true),
            WindowHit::Client
        );
    }
}
