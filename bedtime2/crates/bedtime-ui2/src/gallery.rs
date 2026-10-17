//! A reusable gallery view. Its state is ordinary ECS data, not a global singleton.
use crate::*;
use bedtime_clipboard2::Clipboard;

#[derive(Component, Clone)]
pub struct GalleryState {
    pub visible: bool,
    /// Draw inside an application/workbench-owned content window.
    pub embedded: bool,
    pub light: Entity,
    pub dark: Entity,
    pub text: String,
    pub checked: bool,
    pub toggled: bool,
    pub value: f32,
    pub choice: usize,
    pub open: bool,
    pub clicks: u32,
    pub selected: usize,
    pub icon_browser: bool,
    pub dialog: bool,
    pub tooltip_visible: bool,
    pub observations: std::collections::BTreeMap<&'static str, egui::Rect>,
    pub icon_filter: String,
    pub icon_size: f32,
    pub custom_icons: Handle<IconSet>,
    pub custom_icon: Handle<Icon>,
}

pub fn spawn(world: &mut World, surface: Entity, light: Entity, dark: Entity) -> Entity {
    let state = create_state(world, light, dark);
    let view = register_view(world, surface, draw_gallery);
    world.entity_mut(view).insert(state);
    view
}

pub fn create_state(world: &mut World, light: Entity, dark: Entity) -> GalleryState {
    let custom_icon = world.resource_mut::<Assets<Icon>>().add(
        Icon::from_svg(include_bytes!("../assets/gallery/custom.svg"))
            .expect("Gallery SVG is valid"),
    );
    let custom_icons = world.resource_mut::<Assets<IconSet>>().add(IconSet {
        overrides: [BuiltinIcon::Folder, BuiltinIcon::Plus, BuiltinIcon::Save]
            .into_iter()
            .map(|slot| (slot, custom_icon.clone()))
            .collect(),
    });
    GalleryState {
        visible: true,
        embedded: false,
        light,
        dark,
        text: "A little room for good ideas".into(),
        checked: true,
        toggled: false,
        value: 0.65,
        choice: 0,
        open: false,
        clicks: 0,
        selected: 0,
        icon_browser: false,
        dialog: false,
        tooltip_visible: false,
        observations: default(),
        icon_filter: String::new(),
        icon_size: 32.0,
        custom_icons,
        custom_icon,
    }
}

pub fn draw_gallery(
    frame: UiFrame,
    mut states: Query<&mut GalleryState>,
    mut surfaces: Query<(&mut MotionPolicy, &UiDiagnostics, &UiIconStatus)>,
    mut commands: Commands,
    mut clipboard: ResMut<Clipboard>,
) {
    let mut state = states.get_mut(frame.view).expect("Gallery state");
    let (mut policy, diag, status) = surfaces.get_mut(frame.surface).unwrap();
    let surface = frame.surface;
    for action in draw_contents(frame, &mut state, &mut policy, diag, status, &mut clipboard) {
        match action {
            GalleryAction::Theme(theme) => {
                commands.entity(surface).insert(UsesTheme(theme));
            }
            GalleryAction::Icons(Some(icons)) => {
                commands.entity(surface).insert(UiIcons(icons));
            }
            GalleryAction::Icons(None) => {
                commands.entity(surface).remove::<UiIcons>();
            }
        }
    }
}

/// Changes to the hosting surface, applied by its owner after drawing.
pub enum GalleryAction {
    Theme(Entity),
    Icons(Option<Handle<IconSet>>),
}

/// Render with explicitly supplied surface services, including across ECS worlds.
/// Theme entities in returned actions belong to the host that created the state.
pub fn draw_contents(
    frame: UiFrame,
    state: &mut GalleryState,
    policy: &mut MotionPolicy,
    diag: &UiDiagnostics,
    status: &UiIconStatus,
    clipboard: &mut Clipboard,
) -> Vec<GalleryAction> {
    if !state.visible {
        return Vec::new();
    }
    let mut actions = Vec::new();
    let embedded = state.embedded;
    let mut draw = |mut frame: UiFrame| {
        state.observations.clear();
        frame.ui.label(
            egui::RichText::new("B E D T I M E   /   U I   2")
                .color(theme::color(frame.theme.palette.muted)),
        );
        frame.ui.heading("A place for every little interaction.");
        frame
            .ui
            .label("Built-in egui controls, shared themes, and Bedtime motion.");
        let row = egui::Rect::from_min_size(
            frame.ui.cursor().min,
            egui::vec2(frame.ui.available_width(), 36.0),
        );
        frame.scope(
            egui::UiBuilder::new()
                .max_rect(row)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |mut f| {
                let light = f.icon_button(BuiltinIcon::Sun);
                state.observations.insert("light", light.rect);
                f.tooltip(&light, "Light / Everforest Medium");
                if light.clicked() {
                    actions.push(GalleryAction::Theme(state.light));
                }
                let dark = f.icon_button(BuiltinIcon::Moon);
                state.observations.insert("dark", dark.rect);
                f.tooltip(&dark, "Dark / Everforest Medium");
                if dark.clicked() {
                    actions.push(GalleryAction::Theme(state.dark));
                }
                f.ui.checkbox(&mut policy.enabled, "Motion");
                f.ui.add(egui::Slider::new(&mut policy.seconds, 0.08..=0.8).text("seconds"));
            },
        );
        frame.ui.label(format!(
            "{:.2} ms UI / {} vertices / {} icon rasters",
            diag.draw_ms, diag.vertices, diag.icon_textures
        ));
        frame.ui.separator();
        frame.ui.heading("01  Controls");
        frame.scope(
            egui::UiBuilder::new().layout(egui::Layout::left_to_right(egui::Align::Min)),
            |mut frame| {
                let primary = frame.primary_button("Make something");
                state.observations.insert("primary", primary.rect);
                if primary.clicked() {
                    state.clicks += 1;
                }
                if frame.ui.button("Secondary").clicked() {
                    state.clicks += 1;
                }
                frame.ui.add_enabled(false, egui::Button::new("Disabled"));
            },
        );
        frame
            .ui
            .label(format!("{} button activations", state.clicks));
        let hover = frame.ui.button("Stay a little — animated tooltip");
        state.observations.insert("tooltip", hover.rect);
        state.tooltip_visible = frame.tooltip(
            &hover,
            "A tooltip expanding from its source, using Bedtime's motion policy.",
        );
        frame.ui.checkbox(&mut state.checked, "A familiar checkbox");
        frame.toggle(&mut state.toggled, "An animated toggle");
        frame.ui.label("Project");
        frame.text_edit(egui::TextEdit::singleline(&mut state.text).id_salt("project"));
        frame
            .ui
            .add(egui::Slider::new(&mut state.value, 0.0..=1.0).text("Intensity"));
        let combo = ComboBox::from_label("Behavior")
            .selected_text(["Comfortable", "Compact", "Spacious"][state.choice])
            .show_ui(frame.ui, |ui| {
                for (index, label) in ["Comfortable", "Compact", "Spacious"].iter().enumerate() {
                    let response = ui.selectable_value(&mut state.choice, index, *label);
                    if index == 1 {
                        state.observations.insert("compact", response.rect);
                    }
                }
            });
        state.observations.insert("selector", combo.response.rect);
        let disclosure = egui::CollapsingHeader::new("Details and disclosure")
            .open(Some(state.open))
            .show(frame.ui, |ui| {
                ui.label("Ordinary egui layout and widgets inherit the shared Bedtime theme.");
            });
        if disclosure.header_response.clicked() {
            state.open = !state.open;
        }
        frame.ui.horizontal(|ui| {
            if ui.button("Copy").clicked() {
                let _ = clipboard.write_text(&state.text);
            }
            if ui.button("Paste").clicked() {
                if let Ok(text) = clipboard.read_text() {
                    state.text = text;
                }
            }
            if ui.button("Browse icons").clicked() {
                state.icon_browser = !state.icon_browser;
            }
        });
        if frame.ui.button("Open dialog").clicked() {
            state.dialog = true;
        }
        let menu = frame.ui.button("Right-click for a menu");
        state.observations.insert("context", menu.rect);
        menu.context_menu(|ui| {
            if ui.button("Select this action").clicked() {
                state.clicks += 1;
                ui.close();
            }
            ui.separator();
            ui.add_enabled(false, egui::Button::new("Unavailable action"));
        });
        frame.ui.separator();
        frame.ui.heading("02  A clipped list");
        egui::ScrollArea::vertical()
            .id_salt("dense-list")
            .max_height(180.0)
            .show_rows(frame.ui, 24.0, 10_000, |ui, range| {
                for index in range {
                    if ui
                        .selectable_label(state.selected == index, format!("Row {:05}", index + 1))
                        .clicked()
                    {
                        state.selected = index;
                    }
                }
            });
        if state.dialog {
            let ctx = frame.ui.ctx().clone();
            egui::Modal::new(egui::Id::new((frame.view, "dialog"))).show(&ctx, |ui| {
                ui.heading("A little room to think");
                ui.label("A themed egui modal with Bedtime timing.");
                if ui.button("Done").clicked() {
                    state.dialog = false;
                }
            });
        }

        if state.icon_browser {
            frame.ui.separator();
            frame.ui.heading("03  Icons");
            frame.text_edit(
                egui::TextEdit::singleline(&mut state.icon_filter).hint_text("Filter icons"),
            );
            frame
                .ui
                .add(egui::Slider::new(&mut state.icon_size, 16.0..=64.0).text("Icon size"));
            frame.ui.horizontal(|ui| {
                if ui.button("Bundled pack").clicked() {
                    actions.push(GalleryAction::Icons(None));
                }
                if ui.button("Custom pack").clicked() {
                    actions.push(GalleryAction::Icons(Some(state.custom_icons.clone())));
                }
            });
            if let Some(error) = &status.error {
                frame
                    .ui
                    .colored_label(theme::color(frame.theme.palette.error), error);
            }
            for icon in BuiltinIcon::ALL
                .iter()
                .filter(|icon| icon.key().contains(&state.icon_filter))
                .take(80)
            {
                let (rect, _) = frame.ui.allocate_exact_size(
                    egui::vec2(state.icon_size + 180.0, state.icon_size + 8.0),
                    egui::Sense::hover(),
                );
                frame.icon_at(*icon, [rect.min.x, rect.min.y], state.icon_size);
                frame.ui.painter().text(
                    rect.min + egui::vec2(state.icon_size + 10.0, 4.0),
                    egui::Align2::LEFT_TOP,
                    icon.key(),
                    egui::FontId::proportional(frame.theme.metrics.font_size),
                    theme::color(frame.theme.palette.text),
                );
            }
        }
    };
    if embedded {
        let mut frame = frame;
        let padding = frame.panel_settings.padding;
        frame.scroll("gallery-panel", |mut frame| frame.padded(padding, draw));
    } else {
        let ctx = frame.ui.ctx().clone();
        egui::Window::new("Bedtime UI 2")
            .default_pos(egui::pos2(18.0, 18.0))
            .default_size(egui::vec2(1080.0, 820.0))
            .show(&ctx, |ui| draw(frame.for_view(frame.view, ui)));
    }
    actions
}
