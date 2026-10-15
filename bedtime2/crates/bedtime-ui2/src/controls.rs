//! Built-in controls inherit the prepared base style. Only variants push overrides.
use crate::{
    IconSource, MotionPolicy, Theme, UiViewport,
    icon_cache::IconCache,
    motion::{Animations, PopupKind, ScalarKind},
    theme::{Rgba, mix},
};
use bevy::prelude::Entity;
use dear_imgui_rs::{ListClipper, StyleColor, StyleVar, Ui, WindowFlags};
use std::cell::RefCell;

pub struct Controls<'a> {
    pub ui: &'a Ui,
    pub theme: &'a Theme,
    motion: MotionPolicy,
    view: Entity,
    animations: &'a RefCell<Animations>,
    icons: &'a RefCell<IconCache>,
    viewports: &'a RefCell<crate::viewport_textures::ViewportTextures>,
}

impl<'a> Controls<'a> {
    pub(crate) fn new(
        ui: &'a Ui,
        theme: &'a Theme,
        motion: MotionPolicy,
        view: Entity,
        animations: &'a RefCell<Animations>,
        icons: &'a RefCell<IconCache>,
        viewports: &'a RefCell<crate::viewport_textures::ViewportTextures>,
    ) -> Self {
        Self {
            ui,
            theme,
            motion,
            view,
            animations,
            icons,
            viewports,
        }
    }

    pub fn text(&self, text: impl AsRef<str>) {
        self.ui.text(text);
    }

    /// Display a Bevy camera's output. An ECS UiViewport is resized after drawing;
    /// the application owns its camera, render schedule and post-processing.
    pub fn viewport(&self, id: &str, viewport: &mut UiViewport, size: [f32; 2]) {
        assert!(
            size.into_iter()
                .all(|value| value.is_finite() && value > 0.0),
            "Viewport size must be positive and finite"
        );
        self.ui.invisible_button(id, size);
        let min = self.ui.item_rect_min();
        let max = self.ui.item_rect_max();
        viewport.rect = bevy::prelude::Rect::from_corners(min.into(), max.into());
        viewport.hovered = self.ui.is_item_hovered();
        viewport.visible = self.ui.is_item_visible();
        let scale = self.ui.io().display_framebuffer_scale();
        viewport.physical_size = bevy::prelude::UVec2::new(
            (size[0] * scale[0]).ceil().max(1.0) as u32,
            (size[1] * scale[1]).ceil().max(1.0) as u32,
        );
        if viewport.visible {
            let texture = self.viewports.borrow_mut().texture(&viewport.image);
            self.ui.get_window_draw_list().add_image(
                texture,
                min,
                max,
                [0.0; 2],
                [1.0; 2],
                self.ui.get_color_u32_from_rgba([1.0; 4]),
            );
        }
    }

    pub fn muted(&self, text: impl AsRef<str>) {
        self.ui.text_colored(self.theme.palette.muted, text);
    }

    pub fn heading(&self, text: impl AsRef<str>) {
        self.ui.spacing();
        self.ui.text_colored(self.theme.palette.accent, text);
        self.ui.separator();
    }

    pub fn button(&self, label: impl AsRef<str>) -> bool {
        let p = &self.theme.palette;
        self.paint_button(
            label.as_ref(),
            [0.0; 2],
            p.surface,
            mix(p.surface, p.accent, 0.25),
            mix(p.surface, p.accent, 0.35),
        )
    }

    pub fn primary_button(&self, label: impl AsRef<str>) -> bool {
        let p = &self.theme.palette;
        self.paint_button(
            label.as_ref(),
            [0.0; 2],
            p.selection,
            mix(p.selection, p.accent, 0.3),
            mix(p.selection, p.accent, 0.5),
        )
    }

    fn paint_button(
        &self,
        label: &str,
        size: [f32; 2],
        base: Rgba,
        hover: Rgba,
        active: Rgba,
    ) -> bool {
        let id = self.ui.get_id(label).raw();
        let amount = self
            .animations
            .borrow_mut()
            .hover(self.view, id, self.motion);
        let color = if self.motion.enabled {
            mix(base, hover, amount)
        } else {
            base
        };
        let _bg = self.ui.push_style_color(StyleColor::Button, color);
        // Native hover uses the interpolated color too; reduced motion uses its
        // final color immediately, including on the first hovered frame.
        let _hover = self.ui.push_style_color(
            StyleColor::ButtonHovered,
            if self.motion.enabled { color } else { hover },
        );
        let _active = self.ui.push_style_color(StyleColor::ButtonActive, active);
        let clicked = self.ui.button_with_size(label, size);
        self.animations.borrow_mut().retarget_hover(
            self.view,
            id,
            self.ui.is_item_hovered(),
            self.motion,
        );
        clicked
    }

    pub fn disabled(&self, disabled: bool, draw: impl FnOnce()) {
        let _disabled = self.ui.begin_disabled_with_cond(disabled);
        draw();
    }

    pub fn same_line(&self) {
        self.ui.same_line();
    }

    pub fn tooltip(&self, text: impl AsRef<str>) {
        if self.ui.is_item_hovered() {
            let text = text.as_ref();
            let id = self.ui.item_id().raw();
            let id = if id == 0 {
                self.ui.get_id(text).raw()
            } else {
                id
            };
            let min = self.ui.item_rect_min();
            let max = self.ui.item_rect_max();
            let anchor = [(min[0] + max[0]) * 0.5, max[1]];
            let _padding = self
                .ui
                .push_style_var(StyleVar::WindowPadding(self.theme.metrics.tooltip_padding));
            self.ui.tooltip(|| {
                self.ui.text(text);
                self.animations.borrow_mut().popup(
                    self.view,
                    id,
                    PopupKind::Tooltip,
                    self.ui,
                    anchor,
                    self.motion,
                );
            });
        }
    }

    pub fn checkbox(&self, label: impl AsRef<str>, value: &mut bool) -> bool {
        self.ui.checkbox(label, value)
    }

    pub fn input(&self, label: &str, value: &mut String) -> bool {
        self.ui.input_text(label, value).build()
    }

    pub fn slider(&self, label: impl AsRef<str>, value: &mut f32, min: f32, max: f32) -> bool {
        self.ui.slider(label, min, max, value)
    }

    pub fn select(&self, label: impl AsRef<str>, selected: &mut usize, options: &[&str]) -> bool {
        use dear_imgui_rs::sys;
        let label = label.as_ref();
        let _id = self.ui.push_id(label);
        let key = self.ui.get_id("##options").raw();
        let width = self.ui.calc_item_width();
        let height = self.ui.current_font_size() + self.theme.metrics.padding[1] * 2.0;
        let origin = self.ui.cursor_screen_pos();
        let preview = options.get(*selected).copied().unwrap_or("");
        // A native button and popup retain keyboard activation, navigation,
        // dismissal and scrolling while allowing an explicit gap at either edge.
        let clicked = {
            let _align = self
                .ui
                .push_style_var(StyleVar::ButtonTextAlign([0.0, 0.5]));
            self.ui
                .button_with_size(format!("{preview}###selector"), [width, height])
        };
        let center = [origin[0] + width - height * 0.5, origin[1] + height * 0.5];
        let ink = self.ui.get_color_u32_from_rgba(self.theme.palette.text);
        self.ui
            .get_window_draw_list()
            .add_triangle(
                [center[0] - 4.0, center[1] - 2.0],
                [center[0] + 4.0, center[1] - 2.0],
                [center[0], center[1] + 4.0],
                ink,
            )
            .filled(true)
            .build();
        if clicked {
            self.ui.open_popup("##options");
        }
        let visible_label = label.split("##").next().unwrap();
        if !visible_label.is_empty() {
            self.ui.same_line();
            self.ui.text(visible_label);
        }
        if !self.ui.is_popup_open("##options") {
            return false;
        }
        let m = &self.theme.metrics;
        let viewport = self.ui.main_viewport();
        let bounds = viewport.work_pos();
        let size = viewport.work_size();
        let desired_height = (self.ui.current_font_size() + m.menu_spacing[1])
            * options.len().min(8) as f32
            + m.menu_padding[1] * 2.0;
        let below = bounds[1] + size[1] - origin[1] - height - m.popup_gap;
        let above = origin[1] - m.popup_gap - bounds[1];
        let opens_below = below >= desired_height || below >= above;
        let available = if opens_below { below } else { above }.max(1.0);
        let popup_height = desired_height.min(available);
        let position = [
            origin[0].min(bounds[0] + size[0] - width).max(bounds[0]),
            if opens_below {
                origin[1] + height + m.popup_gap
            } else {
                origin[1] - m.popup_gap - popup_height
            },
        ];
        self.ui.with_bound_context(|| unsafe {
            // SAFETY: ordinary immediate-mode next-window settings, consumed
            // synchronously by the popup below; no native pointer is retained.
            sys::igSetNextWindowPos(position.into(), 0, [0.0; 2].into());
            sys::igSetNextWindowSize([width, popup_height].into(), 0);
        });
        let _padding = self
            .ui
            .push_style_var(StyleVar::WindowPadding(m.menu_padding));
        let _spacing = self
            .ui
            .push_style_var(StyleVar::ItemSpacing(m.menu_spacing));
        let mut changed = false;
        if let Some(_popup) = self.ui.begin_popup("##options") {
            for (index, option) in options.iter().enumerate() {
                let _id = self.ui.push_id(index);
                if self
                    .ui
                    .selectable_config(option)
                    .selected(*selected == index)
                    .build()
                {
                    changed = *selected != index;
                    *selected = index;
                }
                if *selected == index {
                    self.ui.set_item_default_focus();
                }
            }
            self.animations.borrow_mut().popup(
                self.view,
                key,
                PopupKind::Menu,
                self.ui,
                [
                    origin[0],
                    if opens_below {
                        origin[1] + height
                    } else {
                        origin[1]
                    },
                ],
                self.motion,
            );
        }
        changed
    }

    pub fn row(&self, label: &str, draw: impl FnOnce()) {
        let _id = self.ui.push_id(label);
        if let Some(_table) = self.ui.begin_table("##row", 2) {
            self.ui.table_next_column();
            self.ui.align_text_to_frame_padding();
            self.ui.text(label);
            self.ui.table_next_column();
            let _width = self.ui.push_item_width(-1.0);
            draw();
        }
    }

    /// Draw a monochrome icon at the current font size, inheriting theme ink.
    pub fn icon(&self, icon: impl Into<IconSource>) {
        self.icon_sized(icon, self.ui.current_font_size());
    }

    pub fn icon_sized(&self, icon: impl Into<IconSource>, side: f32) {
        assert!(
            side.is_finite() && side > 0.0,
            "Icon size must be positive and finite"
        );
        self.ui.dummy([side, side]);
        if self.ui.is_item_visible() {
            self.paint_icon(&icon.into(), self.ui.item_rect_min(), side);
        }
    }

    /// Native button interaction and hover motion; artwork comes from the selected pack.
    pub fn icon_button(&self, id: &str, icon: impl Into<IconSource>) -> bool {
        let glyph = self.ui.current_font_size();
        let side = glyph + self.theme.metrics.padding[1] * 2.0;
        let _id = self.ui.push_id(id);
        let p = &self.theme.palette;
        let clicked = self.paint_button(
            "##icon",
            [side, side],
            p.surface,
            mix(p.surface, p.accent, 0.25),
            mix(p.surface, p.accent, 0.35),
        );
        if self.ui.is_item_visible() {
            let min = self.ui.item_rect_min();
            let inset = (side - glyph) * 0.5;
            self.paint_icon(&icon.into(), [min[0] + inset, min[1] + inset], glyph);
        }
        clicked
    }

    fn paint_icon(&self, icon: &IconSource, min: [f32; 2], side: f32) {
        let scale = self.ui.io().display_framebuffer_scale();
        let pixels = (side * scale[0].max(scale[1])).ceil().max(1.0);
        assert!(
            pixels <= 4096.0,
            "Icon raster size must not exceed 4096 pixels"
        );
        if let Some(texture) = self.icons.borrow_mut().texture(icon, pixels as u32) {
            self.ui.get_window_draw_list().add_image(
                texture,
                min,
                [min[0] + side, min[1] + side],
                [0.0; 2],
                [1.0; 2],
                self.ui.get_color_u32_from_rgba(self.theme.palette.text),
            );
        }
    }

    /// Paint icon artwork into custom chrome without submitting a layout item.
    pub fn icon_at(&self, icon: impl Into<IconSource>, min: [f32; 2], side: f32) {
        assert!(
            side.is_finite() && side > 0.0 && min.into_iter().all(f32::is_finite),
            "Icon geometry must be finite and positive"
        );
        self.paint_icon(&icon.into(), min, side);
    }

    pub fn toggle(&self, label: &str, value: &mut bool) -> bool {
        let initial = if *value { 1.0 } else { 0.0 };
        let _id = self.ui.push_id(label);
        let height = self.ui.current_font_size() + self.theme.metrics.padding[1] * 2.0;
        let width = height * 1.7;
        let clicked = {
            let _base = self.ui.push_style_color(StyleColor::Button, [0.0; 4]);
            let _hover = self
                .ui
                .push_style_color(StyleColor::ButtonHovered, [0.0; 4]);
            let _active = self.ui.push_style_color(StyleColor::ButtonActive, [0.0; 4]);
            let _border = self.ui.push_style_var(StyleVar::FrameBorderSize(0.0));
            self.ui.button_with_size("##toggle", [width, height])
        };
        if clicked {
            *value = !*value;
        }
        let t = self.animations.borrow_mut().scalar(
            self.view,
            self.ui.item_id().raw(),
            ScalarKind::Toggle,
            initial,
            if *value { 1.0 } else { 0.0 },
            self.motion,
        );
        let min = self.ui.item_rect_min();
        let max = self.ui.item_rect_max();
        let draw = self.ui.get_window_draw_list();
        draw.add_rect(
            [min[0] + 2.0, min[1] + 2.0],
            [max[0] - 2.0, max[1] - 2.0],
            self.ui.get_color_u32_from_rgba(mix(
                self.theme.palette.border,
                self.theme.palette.accent,
                t,
            )),
        )
        .filled(true)
        .rounding(height * 0.5)
        .build();
        let radius = (height * 0.5 - 5.0).max(1.0);
        draw.add_circle(
            [
                min[0] + height * 0.5 + (width - height) * t,
                min[1] + height * 0.5,
            ],
            radius,
            self.ui.get_color_u32_from_rgba(self.theme.palette.text),
        )
        .filled(true)
        .build();
        self.ui.same_line();
        self.ui.text(label);
        clicked
    }

    /// Call immediately after the source item. Closed menus do no style work and
    /// never invoke their contents closure. The prepared base style covers popups.
    pub fn context_menu(&self, id: &str, draw: impl FnOnce()) {
        self.ui.open_popup_on_item_click(Some(id));
        self.popup_menu(id, draw);
    }

    /// Draw a menu opened explicitly by a custom chrome control.
    pub fn popup_menu(&self, id: &str, draw: impl FnOnce()) {
        if !self.ui.is_popup_open(id) {
            return;
        }
        let key = self.ui.get_id(id).raw();
        let anchor = self.ui.io().mouse_pos();
        let _padding = self
            .ui
            .push_style_var(StyleVar::WindowPadding(self.theme.metrics.menu_padding));
        let _spacing = self
            .ui
            .push_style_var(StyleVar::ItemSpacing(self.theme.metrics.menu_spacing));
        let _rounding = self
            .ui
            .push_style_var(StyleVar::ChildRounding(self.theme.metrics.popup_rounding));
        let _border = self.ui.push_style_var(StyleVar::ChildBorderSize(1.0));
        if let Some(_popup) = self.ui.begin_popup(id) {
            draw();
            self.animations.borrow_mut().popup(
                self.view,
                key,
                PopupKind::Menu,
                self.ui,
                anchor,
                self.motion,
            );
        }
    }

    pub fn open_dialog(&self, id: &str) {
        self.ui.open_popup(id);
    }

    pub fn dialog(&self, id: &str, draw: impl FnOnce()) {
        if !self.ui.is_popup_open(id) {
            return;
        }
        let key = self.ui.get_id(id).raw();
        if let Some(_popup) = self
            .ui
            .begin_modal_popup_config(id)
            .flags(WindowFlags::ALWAYS_AUTO_RESIZE)
            .begin()
        {
            draw();
            self.animations.borrow_mut().popup(
                self.view,
                key,
                PopupKind::Dialog,
                self.ui,
                self.ui.window_pos(),
                self.motion,
            );
            if self.ui.is_key_pressed(dear_imgui_rs::Key::Escape) {
                self.ui.close_current_popup();
            }
        }
    }

    pub fn close_popup(&self) {
        self.ui.close_current_popup();
    }

    pub fn disclosure(&self, label: &str, open: &mut bool, draw: impl FnOnce()) {
        use dear_imgui_rs::sys;
        let _id = self.ui.push_id(label);
        let initial = if *open { 1.0 } else { 0.0 };
        let prefix = if *open { "v" } else { ">" };
        if self
            .ui
            .selectable(format!("{prefix}  {label}###disclosure"))
        {
            *open = !*open;
        }
        let amount = self.animations.borrow_mut().scalar(
            self.view,
            self.ui.item_id().raw(),
            ScalarKind::Disclosure,
            initial,
            if *open { 1.0 } else { 0.0 },
            self.motion,
        );
        if amount <= 0.0 {
            return;
        }
        let origin = self.ui.cursor_pos();
        let screen = self.ui.cursor_screen_pos();
        // Measure the body once while submitting it. Its visible height updates
        // the surrounding layout, so callers do not supply a guessed pixel size.
        let (first_index, maximum) = self.ui.with_bound_context(|| unsafe {
            let window = &*sys::igGetCurrentWindowRead();
            ((*window.DrawList).IdxBuffer.Size, window.DC.CursorMaxPos)
        });
        let clip = self.ui.push_clip_rect(
            screen,
            [screen[0] + self.ui.content_region_avail()[0], f32::MAX],
            true,
        );
        {
            let inherited_alpha = (self.ui.get_color_u32_from_rgba([1.0; 4]) >> 24) as f32 / 255.0;
            let _alpha = self
                .ui
                .push_style_var(StyleVar::Alpha(inherited_alpha * amount));
            let _disabled = self.ui.begin_disabled_with_cond(!*open);
            self.ui.group(draw);
        }
        let height = (self.ui.cursor_pos()[1] - origin[1]).max(0.0) * amount;
        drop(clip);
        self.ui.with_bound_context(|| unsafe {
            // SAFETY: all geometry belongs to the current frame and window;
            // only the newly submitted body's commands and layout are changed.
            let window = &mut *sys::igGetCurrentWindow();
            let list = &mut *window.DrawList;
            for i in 0..list.CmdBuffer.Size {
                let command = &mut *list.CmdBuffer.Data.add(i as usize);
                if command.IdxOffset >= first_index as u32 && command.ElemCount > 0 {
                    command.ClipRect.w = command.ClipRect.w.min(screen[1] + height);
                }
            }
            window.DC.CursorMaxPos = maximum;
        });
        self.ui.set_cursor_pos(origin);
        self.ui.dummy([0.0, height]);
        self.ui.set_cursor_pos([origin[0], origin[1] + height]);
        self.ui.with_bound_context(|| unsafe {
            (*sys::igGetCurrentWindow()).DC.CursorMaxPos.y = maximum.y.max(screen[1] + height);
        });
    }

    /// Fixed-height list clipping. Only visible rows invoke the caller's painter.
    pub fn list(&self, count: usize, mut row: impl FnMut(usize)) {
        let mut clipper = ListClipper::new(count.min(i32::MAX as usize - 1)).begin(self.ui);
        while clipper.step() {
            for i in clipper.display_start()..clipper.display_end() {
                row(i as usize);
            }
        }
    }
}
