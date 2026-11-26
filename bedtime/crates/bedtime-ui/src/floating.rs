//! Controls floating over content that fills its space (an image, a model, a
//! video): a small rounded bar at a corner or the middle of an edge. It shows
//! while the pointer is over the content, or one of its menus is open, and
//! fades away otherwise, so the content keeps the whole panel.
use crate::{Button, Tip, UiFrame, theme};
use bevy_egui::egui;

/// A bar of controls over `rect`, at `anchor` (`Align2::RIGHT_TOP`,
/// `Align2::CENTER_BOTTOM`, ...).
pub struct Floating {
    id: egui::Id,
    rect: egui::Rect,
    anchor: egui::Align2,
    shown: bool,
}

impl Floating {
    pub fn new(id: impl egui::AsId, rect: egui::Rect, anchor: egui::Align2) -> Self {
        Self {
            id: egui::Id::new(id),
            rect,
            anchor,
            shown: false,
        }
    }

    /// Show it even when the pointer is elsewhere (a paused video's controls).
    pub fn shown(mut self, shown: bool) -> Self {
        self.shown = shown;
        self
    }

    pub fn show<R>(self, frame: &mut UiFrame<'_>, draw: impl FnOnce(&mut UiFrame<'_>) -> R) -> R {
        let ctx = frame.ui.ctx().clone();
        let menu_open = self.id.with("menu-open");
        let pointer = ctx
            .pointer_latest_pos()
            .is_some_and(|at| self.rect.contains(at));
        let had_menu = ctx.data(|data| data.get_temp::<bool>(menu_open).unwrap_or(false));
        let visible = self.shown || pointer || (had_menu && egui::Popup::is_any_open(&ctx));
        let t = frame.animate_bool(self.id.with("fade"), visible);
        let inset = 8.0;
        let at = self.rect.shrink(inset);
        let position = egui::pos2(
            match self.anchor.x() {
                egui::Align::Min => at.left(),
                egui::Align::Center => at.center().x,
                egui::Align::Max => at.right(),
            },
            match self.anchor.y() {
                egui::Align::Min => at.top(),
                egui::Align::Center => at.center().y,
                egui::Align::Max => at.bottom(),
            },
        );
        let p = &frame.theme.palette;
        let mut fill = p.panel_background;
        fill[3] = 1.0;
        let fill = theme::color(fill);
        let stroke = egui::Stroke::new(1.0, theme::color(p.border));
        let area = egui::Area::new(self.id)
            .order(egui::Order::Middle)
            .pivot(self.anchor)
            .fixed_pos(position)
            .constrain_to(at)
            .interactable(t > 0.05)
            .show(&ctx, |ui| {
                ui.set_opacity(t);
                ui.set_clip_rect(self.rect);
                egui::Frame::new()
                    .fill(fill)
                    .stroke(stroke)
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::same(3))
                    .shadow(egui::Shadow {
                        offset: [0, 2],
                        blur: 10,
                        spread: 0,
                        color: egui::Color32::from_black_alpha((40.0 * t) as u8),
                    })
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        // Buttons show their fill only when hovered.
                        ui.visuals_mut().button_frame = false;
                        ui.horizontal(|ui| {
                            let mut inner = frame.for_view(frame.view, ui);
                            draw(&mut inner)
                        })
                        .inner
                    })
                    .inner
            });
        // Menus opened from the bar keep it shown.
        let any_open = egui::Popup::is_any_open(&ctx);
        let opened_here = area.response.contains_pointer() && any_open;
        ctx.data_mut(|data| data.insert_temp(menu_open, opened_here || (had_menu && any_open)));
        area.inner
    }
}

impl UiFrame<'_> {
    /// An icon button for a floating bar, with a tooltip. Its name (for
    /// scripts and screen readers) is the tooltip without its keys:
    /// "Fit (Double-Click)" is "Fit".
    pub fn floating_button(
        &mut self,
        icon: impl Into<crate::IconSource>,
        tooltip: &str,
    ) -> egui::Response {
        self.floating_tool(icon, tooltip, false)
    }

    fn floating_tool(
        &mut self,
        icon: impl Into<crate::IconSource>,
        tooltip: &str,
        on: bool,
    ) -> egui::Response {
        let name = tooltip.split(" (").next().unwrap_or(tooltip);
        let button = Button::icon_only(icon, name).ghost().selected(on);
        self.ui.add(button).tip(tooltip)
    }

    /// A toggled icon button for a floating bar: it looks chosen while on.
    pub fn floating_toggle(
        &mut self,
        icon: impl Into<crate::IconSource>,
        tooltip: &str,
        on: bool,
    ) -> egui::Response {
        self.floating_tool(icon, tooltip, on)
    }

    /// Small, quiet text in a floating bar ("100%", "0:12 / 1:30").
    pub fn floating_label(&mut self, text: impl Into<String>) -> egui::Response {
        self.ui
            .add(crate::Text::caption(text).muted().unselectable())
    }
}
