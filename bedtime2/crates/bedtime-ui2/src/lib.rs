//! Bed presentation on upstream egui/Bevy integration; the host owns its window and camera.
mod combo_box;
pub mod gallery;
mod icon_cache;
pub mod icons;
pub mod input;
mod motion;
mod runtime;
mod status_bar;
pub mod theme;
mod viewport;
use bevy::{
    ecs::system::{SystemId, SystemInput},
    prelude::*,
};
pub use combo_box::ComboBox;
pub use egui;
pub use icons::{BuiltinIcon, Icon, IconSet, IconSource, UiIconStatus, UiIcons};
pub use motion::MotionPolicy;
pub use status_bar::{
    InStatusBar, StatusBar, StatusBarItems, StatusItem, StatusItemPresentation, StatusSide,
    create_status_bar, draw_status_bar, register_status_item, register_status_tooltip,
    status_bar_height,
};
use std::{collections::HashMap, sync::Mutex};
pub use theme::{Theme, themes};
pub use viewport::{UiViewport, viewport_image};

#[derive(Component, Default)]
#[require(UiCapture, MotionPolicy, UiDiagnostics, UiIconStatus, UiBackground)]
pub struct UiSurface;

/// Optional panel settings on a UI surface, independent of its color theme.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct PanelSettings {
    pub rounding: f32,
    /// Opt-in content padding; panel hosts still provide the complete content area.
    pub padding: [f32; 2],
    /// Spacing in egui points between panels and from panels to the shell edges.
    pub panel_gap: f32,
    /// Corner-target side length in egui points, measured inward from the area's
    /// outside corner, including its gutter. Zero disables corner handles.
    pub hit_size: f32,
    pub debug: bool,
}

impl Default for PanelSettings {
    fn default() -> Self {
        Self {
            rounding: 7.0,
            padding: [14.0, 12.0],
            panel_gap: 5.0,
            hit_size: 13.0,
            debug: false,
        }
    }
}

/// Choose whether UI2 paints the theme background or the application paints its
/// own surfaces. UI cameras always clear to transparent before drawing.
#[derive(Component, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiBackground {
    #[default]
    Theme,
    Transparent,
}

/// Attach upstream egui to an application-owned camera targeting this window.
#[derive(Component)]
#[require(bevy_egui::EguiContext)]
pub struct UiCamera(pub Entity);

#[derive(Component, Default, Clone, Copy, Debug)]
pub struct UiCapture {
    pub keyboard: bool,
    pub pointer: bool,
    pub text: bool,
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
    pub viewport_textures: usize,
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
pub struct UiView;

#[derive(Message, Clone, Debug)]
pub struct UiError(pub String);

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UiSystems {
    Draw,
    PublishInput,
    PrepareTargets,
}

pub struct UiFrame<'a> {
    pub view: Entity,
    pub surface: Entity,
    pub ui: &'a mut egui::Ui,
    pub theme: &'a Theme,
    pub motion: MotionPolicy,
    pub panel_settings: PanelSettings,
    pub(crate) icons: &'a Mutex<icon_cache::IconCache>,
    pub(crate) textures: &'a HashMap<bevy::asset::AssetId<Image>, egui::TextureId>,
    pub(crate) input: &'a Mutex<input::UiInputFrame>,
}

impl SystemInput for UiFrame<'_> {
    type Param<'i> = UiFrame<'i>;
    type Inner<'i> = UiFrame<'i>;
    fn wrap(this: Self::Inner<'_>) -> Self::Param<'_> {
        this
    }
}

impl UiFrame<'_> {
    pub fn for_view<'a>(&'a self, view: Entity, ui: &'a mut egui::Ui) -> UiFrame<'a> {
        UiFrame {
            view,
            surface: self.surface,
            ui,
            theme: self.theme,
            motion: self.motion,
            panel_settings: self.panel_settings,
            icons: self.icons,
            textures: self.textures,
            input: self.input,
        }
    }

    pub fn scope<R>(&mut self, builder: egui::UiBuilder, draw: impl FnOnce(UiFrame<'_>) -> R) -> R {
        let Self {
            view,
            surface,
            ui,
            theme,
            motion,
            panel_settings,
            icons,
            textures,
            input,
        } = self;
        ui.scope_builder(builder, |ui| {
            draw(UiFrame {
                view: *view,
                surface: *surface,
                ui,
                theme,
                motion: *motion,
                panel_settings: *panel_settings,
                icons,
                textures,
                input,
            })
        })
        .inner
    }

    pub fn scroll<R>(
        &mut self,
        id: impl std::hash::Hash + std::fmt::Debug,
        draw: impl FnOnce(UiFrame<'_>) -> R,
    ) -> R {
        let Self {
            view,
            surface,
            ui,
            theme,
            motion,
            panel_settings,
            icons,
            textures,
            input,
        } = self;
        // Keep the track clear of rounded container corners without padding
        // or shrinking the content itself.
        let bounds = ui.available_rect_before_wrap().intersect(ui.clip_rect());
        let inset = panel_settings.rounding.min(bounds.height() * 0.5);
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
                    panel_settings: *panel_settings,
                    icons,
                    textures,
                    input,
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
            panel_settings,
            icons,
            textures,
            input,
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
                    panel_settings: *panel_settings,
                    icons,
                    textures,
                    input,
                })
            })
            .inner
    }

    pub fn claim_pointer(&self) {
        let mut frame = self.input.lock().unwrap();
        let ids: Vec<_> = frame
            .events
            .iter()
            .filter(|e| {
                matches!(
                    e.event,
                    bevy::window::WindowEvent::CursorMoved(_)
                        | bevy::window::WindowEvent::MouseButtonInput(_)
                        | bevy::window::WindowEvent::MouseWheel(_)
                )
            })
            .map(|e| e.id)
            .collect();
        for id in ids {
            frame.consume(id, self.view);
        }
    }

    pub fn claim_key(&self, key: KeyCode) {
        let mut frame = self.input.lock().unwrap();
        let ids: Vec<_> = frame
            .events
            .iter()
            .filter(|e| e.key().is_some_and(|k| k.key_code == key))
            .map(|e| e.id)
            .collect();
        for id in ids {
            frame.consume(id, self.view);
        }
    }

    pub fn text_edit(&mut self, edit: egui::TextEdit<'_>) -> egui::text_edit::TextEditOutput {
        let before = self.ui.ctx().memory(|m| m.focused());
        let output = edit.show(self.ui);
        if before == Some(output.response.id)
            || output.response.has_focus()
            || output.response.lost_focus()
        {
            self.input
                .lock()
                .unwrap()
                .claim_text(self.view, output.response.id);
        }
        output
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
        let mut ink = p.background;
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
        let side = self.theme.metrics.font_size;
        let size = egui::vec2(side, side);
        let pixels = (side * self.ui.ctx().pixels_per_point()).ceil().max(1.0) as u32;
        let texture = self.icons.lock().unwrap().texture(&icon.into(), pixels);
        let button = if let Some(texture) = texture {
            egui::Button::image((texture, size)).image_tint_follows_text_color(true)
        } else {
            egui::Button::new(egui::Atom::custom(egui::Id::new("icon-placeholder"), size))
        };
        self.ui.add(button)
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
                theme::mix(base, p.background, 0.18)
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

    pub fn viewport(
        &mut self,
        id: &str,
        viewport: &mut UiViewport,
        size: [f32; 2],
    ) -> egui::Response {
        let (rect, _) = self.ui.allocate_exact_size(
            egui::vec2(size[0].max(1.0), size[1].max(1.0)),
            egui::Sense::hover(),
        );
        let response = self.ui.interact(
            rect,
            self.ui.id().with(id),
            egui::Sense::CLICK | egui::Sense::DRAG,
        );
        viewport.rect = Rect::from_corners(
            Vec2::new(rect.min.x, rect.min.y),
            Vec2::new(rect.max.x, rect.max.y),
        );
        viewport.visible = self.ui.is_rect_visible(rect);
        viewport.hovered = response.hovered();
        viewport.physical_size = (viewport.rect.size() * self.ui.ctx().pixels_per_point())
            .ceil()
            .as_uvec2()
            .max(UVec2::ONE);
        if let Some(texture) = self.textures.get(&viewport.image.id()) {
            egui::Image::new(egui::load::SizedTexture::new(*texture, rect.size()))
                .corner_radius(viewport.corner_radius)
                .paint_at(self.ui, rect);
        }
        let clicked = response.clicked()
            || response.is_pointer_button_down_on() && self.ui.input(|i| i.pointer.any_pressed());
        if clicked {
            self.ui.ctx().memory_mut(|memory| {
                if let Some(focused) = memory.focused() {
                    memory.surrender_focus(focused);
                }
            });
        }
        self.input
            .lock()
            .unwrap()
            .region(self.view, self.ui.id().with(id), rect, clicked);
        response
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
        bevy::asset::load_internal_asset!(
            app,
            bevy_egui::render::EGUI_SHADER_HANDLE,
            "linear_egui.wesl",
            bevy::shader::Shader::from_wesl
        );
        icons::install(app);
        app.add_message::<UiError>()
            .init_resource::<bedtime_clipboard2::Clipboard>()
            .init_resource::<runtime::Runtime>()
            .init_resource::<input::RoutedInput>()
            .init_resource::<input::InputRouter>()
            .configure_sets(
                PreUpdate,
                UiSystems::Draw.after(bevy_egui::EguiPreUpdateSet::ProcessInput),
            )
            .configure_sets(PreUpdate, UiSystems::PublishInput.after(UiSystems::Draw))
            .add_systems(PreUpdate, input::publish.in_set(UiSystems::PublishInput))
            .configure_sets(
                PostUpdate,
                UiSystems::PrepareTargets.before(bevy::camera::CameraUpdateSystems),
            )
            .add_systems(
                PreUpdate,
                runtime::configure.before(bevy_egui::EguiPreUpdateSet::InitContexts),
            )
            .add_systems(
                PreUpdate,
                (viewport::reset_visibility, runtime::draw)
                    .chain()
                    .in_set(UiSystems::Draw),
            )
            .add_systems(
                PostUpdate,
                viewport::resize_targets.in_set(UiSystems::PrepareTargets),
            );
    }
}

pub fn register_view<M>(
    world: &mut World,
    surface: Entity,
    draw: impl IntoSystem<UiFrame<'static>, (), M> + 'static,
) -> Entity {
    assert!(
        world.get::<UiSurface>(surface).is_some(),
        "Drawing requires a UiSurface"
    );
    let id: SystemId<UiFrame<'static>> = world.register_system(draw);
    world
        .entity_mut(id.entity())
        .insert((UiView, OnSurface(surface)));
    id.entity()
}

#[cfg(test)]
mod icon_tests;
#[cfg(test)]
mod tests;
