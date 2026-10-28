//! Bed presentation on upstream egui/Bevy integration; the host owns its window and camera.
mod clipping;
mod combo_box;
mod component;
mod embedded;
pub mod fonts;
pub mod gallery;
mod icon_cache;
pub mod icons;
#[cfg(target_os = "macos")]
mod macos_output;
mod motion;
mod runtime;
pub mod theme;
pub mod tree;
mod viewport;
use bevy::prelude::*;
pub use combo_box::ComboBox;
pub use component::{Component, UiContext, UiDraw};
pub use egui;
pub use embedded::{EmbeddedRenderPlugin, ViewportHostPlugin, ViewportOutput};
pub use icons::{BuiltinIcon, Icon, IconSet, IconSource, UiIconStatus, UiIcons};
pub use motion::MotionPolicy;
use std::sync::Mutex;
pub use theme::{Theme, themes};
pub use viewport::{UiViewport, viewport_image};

/// While this resource exists, each frame's AccessKit tree (every widget's
/// role, label and bounds in points) is recorded in it. egui builds the whole
/// tree each frame.
#[cfg(feature = "accessibility")]
#[derive(Resource, Default)]
pub struct AccessibilityTree(pub Option<egui::accesskit::TreeUpdate>);

/// bEd's selected text/caret cycle: three sine waves, 120 degrees apart.
/// Repaint only while a consumer is visible; reduced motion uses the theme accent.
pub fn rainbow_ink(ui: &egui::Ui, motion: MotionPolicy) -> egui::Color32 {
    if !motion.enabled {
        return ui.visuals().selection.stroke.color;
    }
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(16));
    let t = ui.input(|i| i.time) as f32 * 2.0;
    theme::color([
        t.sin() * 0.5 + 0.5,
        (t + 2.0944).sin() * 0.5 + 0.5,
        (t + 4.1888).sin() * 0.5 + 0.5,
        1.0,
    ])
}

/// Shared filled disclosure triangle; openness rotates right to down.
pub fn paint_chevron(
    painter: &egui::Painter,
    center: egui::Pos2,
    openness: f32,
    ink: egui::Color32,
) {
    let angle = openness * std::f32::consts::FRAC_PI_2;
    let rotate = |p: egui::Vec2| {
        center
            + egui::vec2(
                p.x * angle.cos() - p.y * angle.sin(),
                p.x * angle.sin() + p.y * angle.cos(),
            )
    };
    painter.add(egui::Shape::convex_polygon(
        vec![
            rotate(egui::vec2(-2.5, -4.0)),
            rotate(egui::vec2(2.5, 0.0)),
            rotate(egui::vec2(-2.5, 4.0)),
        ],
        ink,
        egui::Stroke::NONE,
    ));
}

/// bEd's 160 ms smoothstep expansion, including reversals and reduced motion.
pub fn animate_expansion(
    ctx: &egui::Context,
    id: egui::Id,
    open: bool,
    motion: MotionPolicy,
) -> f32 {
    ctx.animate_bool_with_time_and_easing(id, open, if motion.enabled { 0.16 } else { 0.0 }, |t| {
        t * t * (3.0 - 2.0 * t)
    })
}

/// Shared destination treatment for panel and content drag-and-drop. The caller
/// owns eligibility, geometry, clipping and any motion; invalid targets must not
/// call this painter. Thin targets retain the panel insertion-line treatment.
pub fn paint_drop_overlay(
    painter: &egui::Painter,
    palette: &theme::Palette,
    rect: egui::Rect,
    rounding: f32,
) {
    let border =
        std::array::from_fn(|i| palette.border[i] + (palette.text[i] - palette.border[i]) * 0.75);
    let fill = [border[0], border[1], border[2], border[3] * 0.14];
    let stroke = egui::Stroke::new(2.0, theme::color(border));
    if rect.width().abs() < 0.5 || rect.height().abs() < 0.5 {
        painter.line_segment([rect.min, rect.max], stroke);
    } else {
        painter.rect_filled(rect, rounding, theme::color(fill));
        painter.rect_stroke(rect, rounding, stroke, egui::StrokeKind::Inside);
    }
}

#[derive(Component, Default)]
#[require(UiCapture, MotionPolicy, UiDiagnostics, UiIconStatus, UiBackground)]
pub struct UiSurface;

/// Choose whether UI paints the theme background or the application paints its
/// own surfaces. UI cameras always clear to transparent before drawing.
#[derive(Component, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiBackground {
    #[default]
    Theme,
    Transparent,
}

/// Attach upstream egui to an application-owned camera targeting this window.
/// Removing this component ends its egui session and restores camera settings.
/// Reattaching creates a fresh EguiContext.
#[derive(Component)]
#[component(on_remove = runtime::remove_camera)]
#[require(bevy_egui::EguiContext)]
pub struct UiCamera(pub Entity);

#[derive(Component, Default, Clone, Copy, Debug)]
pub struct UiCapture {
    pub keyboard: bool,
    pub pointer: bool,
    /// Text editing was active during this frame, including when Enter ended it.
    pub text: bool,
    /// A popup or modal was active during this frame, including its closing key.
    pub modal: bool,
}

#[derive(Component, Default, Clone, Debug)]
pub struct UiDiagnostics {
    pub frames: u64,
    pub vertices: usize,
    pub indices: usize,
    pub draw_ms: f64,
    pub error: Option<String>,
    pub active_animations: usize,
    pub icon_textures: usize,
    pub icon_rasterizations: u64,
}

#[derive(Component)]
#[relationship(relationship_target = ThemeUsers)]
pub struct UsesTheme(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = UsesTheme)]
pub struct ThemeUsers(Vec<Entity>);

#[derive(Component)]
#[relationship(relationship_target = SurfaceViews)]
pub struct OnSurface(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = OnSurface, linked_spawn)]
pub struct SurfaceViews(Vec<Entity>);

#[derive(Component)]
#[relationship(relationship_target = ViewParts)]
pub struct InView(pub Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = InView, linked_spawn)]
pub struct ViewParts(Vec<Entity>);

#[derive(Component)]
pub struct UiView {
    pub draw: UiDraw,
}

#[derive(Message, Clone, Debug)]
pub struct UiError(pub String);

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UiSystems {
    Configure,
    Draw,
    /// PostUpdate: route scene cameras and copy simulation state using this
    /// frame's viewport observations. Runs before target resizing and Bevy's
    /// camera updates, so new targets receive their projection refresh together.
    SyncViewports,
    PrepareTargets,
    /// Core2d/Core3d render schedules: final platform color/alpha conversion.
    /// Application post-processing runs after egui_pass and before this set.
    PrepareOutput,
}

pub struct UiFrame<'a> {
    pub view: Entity,
    pub surface: Entity,
    pub ui: &'a mut egui::Ui,
    pub theme: &'a Theme,
    pub motion: MotionPolicy,
    pub(crate) icons: &'a Mutex<icon_cache::IconCache>,
}

impl<'services> UiFrame<'services> {
    /// Reborrow the drawing services separately from the current egui allocation.
    pub fn context(&self) -> UiContext<'services> {
        UiContext {
            view: self.view,
            surface: self.surface,
            theme: self.theme,
            motion: self.motion,
            icons: self.icons,
        }
    }

    pub fn for_view<'a>(&'a self, view: Entity, ui: &'a mut egui::Ui) -> UiFrame<'a> {
        UiFrame {
            view,
            surface: self.surface,
            ui,
            theme: self.theme,
            motion: self.motion,
            icons: self.icons,
        }
    }

    pub fn scope<R>(&mut self, builder: egui::UiBuilder, draw: impl FnOnce(UiFrame<'_>) -> R) -> R {
        let Self {
            view,
            surface,
            ui,
            theme,
            motion,
            icons,
        } = self;
        ui.scope_builder(builder, |ui| {
            draw(UiFrame {
                view: *view,
                surface: *surface,
                ui,
                theme,
                motion: *motion,
                icons,
            })
        })
        .inner
    }

    /// Constrain content painting to a rounded boundary, even if a child painter
    /// expands its rectangular clip. Popups on separate layers keep their bounds.
    /// GPU paint callbacks crossing the boundary must use a viewport image instead.
    pub fn clip_rounded<R>(
        &mut self,
        rect: egui::Rect,
        radius: impl Into<egui::CornerRadius>,
        draw: impl FnOnce(UiFrame<'_>) -> R,
    ) -> R {
        let ctx = self.ui.ctx().clone();
        let layer = self.ui.layer_id();
        let start = ctx.graphics_mut(|g| g.entry(layer).next_idx());
        let content_clip = self.ui.clip_rect().intersect(rect);
        let result = self.scope(egui::UiBuilder::new(), |frame| {
            frame.ui.set_clip_rect(content_clip);
            draw(frame)
        });
        clipping::finish(&ctx, layer, start, content_clip, rect, radius.into());
        result
    }

    /// Scroll vertically, keeping the track clear of the caller's corner radius
    /// without padding or shrinking the content itself. Use zero for square edges.
    pub fn scroll<R>(
        &mut self,
        id: impl std::hash::Hash + std::fmt::Debug,
        corner_radius: f32,
        draw: impl FnOnce(UiFrame<'_>) -> R,
    ) -> R {
        let Self {
            view,
            surface,
            ui,
            theme,
            motion,
            icons,
        } = self;
        // Keep the track clear of rounded container corners without padding
        // or shrinking the content itself.
        let bounds = ui.available_rect_before_wrap().intersect(ui.clip_rect());
        let inset = corner_radius.min(bounds.height() * 0.5);
        let track = bounds.shrink2(egui::vec2(0.0, inset));
        egui::ScrollArea::vertical()
            .id_salt(id)
            .scroll_bar_rect(track)
            .show(ui, |ui| {
                draw(UiFrame {
                    view: *view,
                    surface: *surface,
                    ui,
                    theme,
                    motion: *motion,
                    icons,
                })
            })
            .inner
    }

    /// Opt into padding around this content. Panel hosts provide the full area.
    pub fn padded<R>(&mut self, padding: [f32; 2], draw: impl FnOnce(UiFrame<'_>) -> R) -> R {
        let Self {
            view,
            surface,
            ui,
            theme,
            motion,
            icons,
        } = self;
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(padding[0] as i8, padding[1] as i8))
            .show(ui, |ui| {
                draw(UiFrame {
                    view: *view,
                    surface: *surface,
                    ui,
                    theme,
                    motion: *motion,
                    icons,
                })
            })
            .inner
    }

    pub fn text_edit(&mut self, edit: egui::TextEdit<'_>) -> egui::text_edit::TextEditOutput {
        edit.show(self.ui)
    }

    pub fn icon_at(&self, icon: impl Into<IconSource>, min: [f32; 2], side: f32) {
        let pixels = (side * self.ui.ctx().pixels_per_point()).ceil().max(1.0) as u32;
        if let Some(texture) = self.icons.lock().unwrap().texture(&icon.into(), pixels) {
            self.ui.painter().image(
                texture,
                egui::Rect::from_min_size(min.into(), egui::vec2(side, side)),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                theme::color(self.theme.palette.text),
            );
        }
    }

    /// An accent button using egui's normal hover, press and focus interaction.
    pub fn primary_button(&mut self, text: impl Into<egui::WidgetText>) -> egui::Response {
        let p = &self.theme.palette;
        let mut ink = p.panel_background;
        ink[3] = 1.0;
        self.ui
            .scope(|ui| {
                let visuals = &mut ui.style_mut().visuals;
                visuals.override_text_color = Some(theme::color(ink));
                for (widget, fill) in [
                    (&mut visuals.widgets.inactive, p.accent),
                    (
                        &mut visuals.widgets.hovered,
                        theme::mix(p.accent, p.text, 0.2),
                    ),
                    (&mut visuals.widgets.active, theme::mix(p.accent, ink, 0.2)),
                ] {
                    widget.bg_fill = theme::color(fill);
                    widget.weak_bg_fill = theme::color(fill);
                    widget.bg_stroke = egui::Stroke::NONE;
                    widget.fg_stroke = egui::Stroke::new(1.0, theme::color(ink));
                }
                ui.button(text)
            })
            .inner
    }

    /// Fade a state with this surface's cubic timing and reduced-motion policy.
    /// Retargeting starts at the displayed value; the initial state is settled.
    pub fn animate_bool(&self, id: egui::Id, target: bool) -> f32 {
        self.animate_bool_with_duration(id, target, self.motion.seconds)
    }

    /// Override the fade duration while preserving this surface's reduced-motion policy.
    pub fn animate_bool_with_duration(&self, id: egui::Id, target: bool, seconds: f32) -> f32 {
        let value = if target { 1.0 } else { 0.0 };
        motion::scalar(
            self.ui.ctx(),
            id,
            value,
            value,
            MotionPolicy {
                seconds,
                ..self.motion
            },
        )
    }

    pub fn icon_button(&mut self, icon: impl Into<IconSource>) -> egui::Response {
        self.icon_button_sized(icon, self.theme.metrics.font_size)
    }

    /// Fixed artwork size in points; button padding still comes from egui spacing.
    pub fn icon_button_sized(&mut self, icon: impl Into<IconSource>, side: f32) -> egui::Response {
        let icon = icon.into();
        let size = egui::vec2(side, side);
        let pixels = (side * self.ui.ctx().pixels_per_point()).ceil().max(1.0) as u32;
        let texture = self.icons.lock().unwrap().texture(&icon, pixels);
        let padding = self.ui.spacing().button_padding;
        let (rect, response) = self
            .ui
            .allocate_exact_size(size + padding * 2.0, egui::Sense::click());
        // Accessibility (and scripts) name a built-in icon's button by its key.
        let enabled = self.ui.is_enabled();
        let name = match &icon {
            IconSource::Builtin(icon) => icon.key(),
            IconSource::Asset(_) => "",
        };
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name));
        let t = self.animate_bool(
            response.id.with("icon-hover"),
            response.hovered() || response.has_focus(),
        );
        let p = &self.theme.palette;
        let pressed = self.animate_bool(
            response.id.with("icon-press"),
            response.is_pointer_button_down_on(),
        );
        let fill = theme::color(theme::mix(
            theme::mix(p.background, p.border, t * 0.65),
            p.text,
            pressed * 0.18,
        ));
        let ink = theme::color(theme::mix(
            theme::mix(p.muted, p.text, t),
            p.accent,
            pressed * 0.4,
        ));
        let fill = if self.ui.visuals().button_frame {
            fill
        } else {
            fill.gamma_multiply(t.max(pressed))
        };
        self.ui.painter().rect_filled(rect, 4.0, fill);
        if let Some(texture) = texture {
            self.ui.painter().image(
                texture,
                egui::Rect::from_center_size(rect.center(), size),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                ink,
            );
        }
        response
    }

    pub fn toggle(&mut self, value: &mut bool, label: &str) -> egui::Response {
        let (rect, response) = self.ui.allocate_exact_size(
            egui::vec2(
                48.0 + self
                    .ui
                    .painter()
                    .layout_no_wrap(
                        label.into(),
                        egui::FontId::proportional(self.theme.metrics.font_size),
                        egui::Color32::WHITE,
                    )
                    .size()
                    .x,
                28.0,
            ),
            egui::Sense::click(),
        );
        if response.clicked() {
            *value = !*value;
        }
        let enabled = self.ui.is_enabled();
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, *value, label)
        });
        let t = motion::scalar(
            self.ui.ctx(),
            response.id.with("toggle-motion"),
            if *value { 1.0 } else { 0.0 },
            if *value { 1.0 } else { 0.0 },
            self.motion,
        );
        let track =
            egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 3.0), egui::vec2(38.0, 22.0));
        let p = &self.theme.palette;
        let base = if *value { p.accent } else { p.border };
        let fill =
            if response.is_pointer_button_down_on() || response.has_focus() || response.clicked() {
                theme::mix(base, p.panel_background, 0.18)
            } else if response.hovered() || response.highlighted() {
                theme::mix(base, p.text, 0.12)
            } else {
                base
            };
        let fill = self.ui.ctx().animate_color_with_time(
            response.id.with("toggle-fill"),
            theme::color(fill),
            self.ui.style().animation_time,
        );
        self.ui.painter().rect_filled(track, 11.0, fill);
        self.ui.painter().circle_filled(
            egui::pos2(track.min.x + 11.0 + t * 16.0, track.center().y),
            8.0,
            theme::color(self.theme.palette.text),
        );
        self.ui.painter().text(
            rect.min + egui::vec2(48.0, 4.0),
            egui::Align2::LEFT_TOP,
            label,
            egui::FontId::proportional(self.theme.metrics.font_size),
            theme::color(self.theme.palette.text),
        );
        response
    }

    /// Returns whether the tooltip was shown this frame.
    pub fn tooltip(&self, response: &egui::Response, text: impl Into<String>) -> bool {
        let text = text.into();
        self.tooltip_ui(response, |frame| {
            frame.ui.label(text);
        })
        .is_some()
    }

    /// Themed tooltip content with the same view identity and motion policy.
    pub fn tooltip_ui<R>(
        &self,
        response: &egui::Response,
        draw: impl FnOnce(UiFrame<'_>) -> R,
    ) -> Option<R> {
        let mut tooltip = egui::Tooltip::for_enabled(response).gap(self.theme.metrics.popup_gap);
        tooltip.popup = tooltip
            .popup
            .frame(
                egui::Frame::popup(self.ui.style()).inner_margin(egui::Margin::symmetric(
                    self.theme.metrics.tooltip_padding[0] as i8,
                    self.theme.metrics.tooltip_padding[1] as i8,
                )),
            );
        tooltip
            .show(|ui| draw(self.for_view(self.view, ui)))
            .map(|response| response.inner)
    }
}

pub struct UiPlugin;
impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.get_sub_app(bevy::render::RenderApp).is_some(),
            "Install Bevy's renderer before UiPlugin"
        );
        assert!(
            app.is_plugin_added::<bevy::winit::WinitPlugin>(),
            "Install Bevy's WinitPlugin before UiPlugin"
        );
        app.insert_resource(bevy_egui::EguiGlobalSettings {
            auto_create_primary_context: false,
            ..default()
        })
        .add_plugins(bevy_egui::EguiPlugin::default());
        #[cfg(target_os = "macos")]
        macos_output::install(app);
        if !app.is_plugin_added::<ViewportHostPlugin>() {
            app.add_plugins(ViewportHostPlugin);
        }
        bevy::asset::load_internal_asset!(
            app,
            bevy_egui::render::EGUI_SHADER_HANDLE,
            "linear_egui.wesl",
            bevy::shader::Shader::from_wesl
        );
        icons::install(app);
        viewport::install(app);
        app.add_message::<UiError>()
            .init_resource::<bedtime_clipboard::Clipboard>()
            .init_resource::<runtime::Runtime>()
            .configure_sets(
                PreUpdate,
                UiSystems::Draw.after(bevy_egui::EguiPreUpdateSet::ProcessInput),
            )
            .add_systems(
                PreUpdate,
                runtime::configure
                    .in_set(UiSystems::Configure)
                    .before(bevy_egui::EguiPreUpdateSet::InitContexts),
            )
            .add_systems(
                PreUpdate,
                (viewport::reset_visibility, runtime::draw)
                    .chain()
                    .in_set(UiSystems::Draw),
            );
        #[cfg(feature = "system-fonts")]
        app.add_systems(PreUpdate, fonts::installed::poll.before(UiSystems::Draw));
    }
}

/// Create a surface root renderer. Nested components are ordinary function calls.
pub fn register_view(world: &mut World, surface: Entity, draw: UiDraw) -> Entity {
    assert!(
        world.get::<UiSurface>(surface).is_some(),
        "Drawing requires a UiSurface"
    );
    world.spawn((UiView { draw }, OnSurface(surface))).id()
}

/// Mount a component with no external props as a surface root. Components with
/// props are composed by their caller and need no registration.
pub fn register_component<C>(world: &mut World, surface: Entity) -> Entity
where
    for<'a> C: Component<Props<'a> = ()>,
{
    register_view(world, surface, |ui, world, context| {
        C::draw(ui, world, context, ())
    })
}

#[cfg(test)]
mod icon_tests;
#[cfg(test)]
mod tests;
