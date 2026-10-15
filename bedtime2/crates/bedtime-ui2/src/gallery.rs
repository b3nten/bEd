//! A reusable gallery view. Its state is ordinary ECS data, not a global singleton.
use crate::*;
use bedtime_desktop2::{ActionEnabled, Activates, DesktopAction};
use imgui::{Condition, StyleVar, TableOptions, TableSizingPolicy, WindowFlags};

#[derive(Component)]
pub struct GalleryState {
    pub visible: bool,
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
    pub icon_filter: String,
    pub icon_size: f32,
    pub custom_icons: Handle<IconSet>,
    pub custom_icon: Handle<Icon>,
}

/// Optional action placement; application command entities remain independently owned.
#[derive(Component)]
pub struct GalleryAction {
    pub label: String,
}

pub fn spawn(world: &mut World, surface: Entity, light: Entity, dark: Entity) -> Entity {
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
    let view = register_view(world, surface, draw_gallery);
    world.entity_mut(view).insert(GalleryState {
        visible: true,
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
        icon_filter: String::new(),
        icon_size: 32.0,
        custom_icons,
        custom_icon,
    });
    view
}

fn draw_gallery(
    frame: UiFrame,
    mut states: Query<&mut GalleryState>,
    mut surfaces: Query<(&mut MotionPolicy, &UiDiagnostics, &UiIconStatus)>,
    mut commands: Commands,
    placements: Query<(Entity, &GalleryAction, &Activates, &InView)>,
    availability: Query<&ActionEnabled>,
    mut actions: MessageWriter<DesktopAction>,
) {
    let mut state = states
        .get_mut(frame.view)
        .expect("Gallery view has GalleryState");
    if !state.visible {
        return;
    }
    let (mut policy, diag, icon_status) = surfaces.get_mut(frame.surface).unwrap();
    let ui = frame.ui;
    let c = frame.controls();
    let p = &frame.theme.palette;
    let window_style = ui.clone_style();
    let surface_rounding = ui.push_style_var(StyleVar::WindowRounding(0.0));
    let surface_border = ui.push_style_var(StyleVar::WindowBorderSize(0.0));
    ui.window("Bedtime widget gallery")
        .position([0.0, 0.0], Condition::Always)
        .size(ui.io().display_size(), Condition::Always)
        .flags(WindowFlags::NO_DECORATION | WindowFlags::NO_MOVE | WindowFlags::NO_SAVED_SETTINGS)
        .build(|| {
            // The surface has captured its flat style. Nested windows inherit
            // the theme again, with every style token kept in its Begin scope.
            let _rounding =
                ui.push_style_var(StyleVar::WindowRounding(window_style.window_rounding()));
            let _border = ui.push_style_var(StyleVar::WindowBorderSize(
                window_style.window_border_size(),
            ));
            c.muted("B E D T I M E   /   U I   2");
            ui.text_colored(p.text, "A place for every little interaction.");
            c.muted("Built-in controls, shared themes, and motion. All running through Bevy.");
            ui.spacing();
            if c.icon_button("light", BuiltinIcon::Sun) {
                commands
                    .entity(frame.surface)
                    .insert(UsesTheme(state.light));
            }
            c.tooltip("Light / Everforest Medium");
            c.same_line();
            if c.icon_button("dark", BuiltinIcon::Moon) {
                commands.entity(frame.surface).insert(UsesTheme(state.dark));
            }
            c.tooltip("Dark / Everforest Medium");
            c.same_line();
            c.checkbox("Motion", &mut policy.enabled);
            c.same_line();
            ui.set_next_item_width(110.0);
            ui.slider("seconds", 0.08, 0.8, &mut policy.seconds);
            c.same_line();
            c.muted(format!(
                "{:.2} ms UI  /  {} vertices",
                diag.draw_ms, diag.vertices
            ));
            ui.separator();

            let Some(_columns) = ui.begin_table_with_flags(
                "gallery-columns",
                2,
                TableOptions::new().sizing_policy(TableSizingPolicy::StretchSame),
            ) else {
                return;
            };
            ui.table_next_column();
            c.heading("01   Controls");
            if c.primary_button("Make something") {
                state.clicks += 1;
            }
            c.same_line();
            if c.button("Secondary") {
                state.clicks += 1;
            }
            c.same_line();
            c.disabled(true, || {
                c.button("Disabled");
            });
            c.muted(format!("{} button activations", state.clicks));
            if c.button("Stay a little — animated hover") {
                state.clicks += 1;
            }
            c.tooltip("Motion is handled automatically by the control.");
            c.checkbox("A familiar checkbox", &mut state.checked);
            c.toggle("An animated toggle", &mut state.toggled);
            c.row("Project", || {
                c.input("##project", &mut state.text);
            });
            c.row("Intensity", || {
                c.slider("##intensity", &mut state.value, 0.0, 1.0);
            });
            c.row("Behavior", || {
                c.select(
                    "##behavior",
                    &mut state.choice,
                    &["Comfortable", "Compact", "Quiet"],
                );
            });

            c.heading("02   Icons & feedback");
            c.icon_button("add", BuiltinIcon::Plus);
            c.tooltip("Add an item");
            c.same_line();
            c.icon_button("folder", BuiltinIcon::Folder);
            c.tooltip("Open folder");
            c.same_line();
            if c.button("Browse icons") {
                state.icon_browser = true;
            }
            ui.text_colored(p.success, "Ready to go");
            ui.same_line();
            ui.text_colored(p.warning, "Needs attention");
            ui.text_colored(p.error, "Something needs fixing");
            c.muted("Muted text and keyboard focus use the same theme.");

            c.heading("03   Popups & dialogs");
            c.button("Right-click me");
            c.context_menu("gallery-context", || {
                if ui.menu_item("Add one more") {
                    state.clicks += 1;
                }
                if ui.menu_item("Reset counter") {
                    state.clicks = 0;
                }
            });
            c.same_line();
            if c.button("Open dialog") {
                c.open_dialog("A small decision");
            }
            c.dialog("A small decision", || {
                c.text("Dialogs inherit the active theme, too.");
                c.muted("Escape closes this modal.");
                if c.primary_button("Looks good") {
                    c.close_popup();
                }
            });
            c.heading("04   Shared actions");
            let mut any = false;
            for (source, placement, target, owner) in &placements {
                if owner.0 != frame.view {
                    continue;
                }
                any = true;
                let enabled = availability.get(target.0).is_ok_and(|v| v.0);
                c.disabled(!enabled, || {
                    if c.button(&placement.label) {
                        actions.write(DesktopAction::Activated {
                            source,
                            target: target.0,
                        });
                    }
                });
            }
            if !any {
                c.muted("Applications can attach command buttons to this view.");
            }

            ui.table_next_column();
            c.heading("05   Palette");
            for (name, color) in [
                ("Background", p.background),
                ("Surface", p.surface),
                ("Elevated", p.elevated),
                ("Text", p.text),
                ("Accent", p.accent),
                ("Selection", p.selection),
            ] {
                let _id = ui.push_id(name);
                ui.color_button(name, color);
                c.same_line();
                c.text(name);
            }
            c.heading("06   Disclosure & fade");
            c.disclosure("There is more here", &mut state.open, || {
                c.text("Smooth expansion and fading.");
                c.muted("Try reversing direction while it is moving.");
                c.text("Turn motion off to settle immediately.");
            });
            c.heading("07   A list with 10,000 rows");
            c.muted("Only visible rows are submitted. Right-click any row.");
            // Fixed row spacing makes clipping predictable, regardless of popup contents.
            ui.child_window("dense-list")
                .size([0.0, 240.0])
                .build(ui, || {
                    let _spacing = ui.push_style_var(StyleVar::ItemSpacing([8.0, 4.0]));
                    c.list(10_000, |index| {
                        let _id = ui.push_id(index as i32);
                        if ui
                            .selectable_config(format!("Item {index:05}  /  a quiet little row"))
                            .selected(state.selected == index)
                            .build()
                        {
                            state.selected = index;
                        }
                        c.context_menu("row-menu", || {
                            if ui.menu_item("Select this row") {
                                state.selected = index;
                            }
                        });
                    });
                });
            c.muted(format!("Selected row: {}", state.selected));
        });
    drop(surface_border);
    drop(surface_rounding);
    let mut browser_open = state.icon_browser;
    if browser_open {
        ui.window("Icon browser")
            .opened(&mut browser_open)
            .position([130.0, 120.0], Condition::FirstUseEver)
            .size([720.0, 640.0], Condition::FirstUseEver)
            .build(|| {
                c.text(format!("{} bundled SVG icons", BuiltinIcon::ALL.len()));
                c.muted("Replace a pack at runtime; omitted slots inherit bundled artwork.");
                if c.button("Bundled icons") {
                    commands.entity(frame.surface).remove::<UiIcons>();
                }
                c.same_line();
                if c.button("Custom pack") {
                    commands
                        .entity(frame.surface)
                        .insert(UiIcons(state.custom_icons.clone()));
                }
                c.same_line();
                c.muted(if icon_status.pending {
                    "Loading replacement…"
                } else if icon_status.active.is_some() {
                    "Custom pack active"
                } else {
                    "Bundled set active"
                });
                if let Some(error) = &icon_status.error {
                    ui.text_colored(p.error, error);
                }
                c.muted("Folder and Plus follow the pack; the explicit asset stays the same:");
                c.icon_sized(BuiltinIcon::Folder, 32.0);
                c.same_line();
                c.icon_sized(BuiltinIcon::Plus, 32.0);
                c.same_line();
                c.icon_sized(&state.custom_icon, 32.0);
                c.same_line();
                c.muted(format!(
                    "{} textures / {} rasterizations",
                    diag.icon_textures, diag.icon_rasterizations
                ));
                c.input("Filter", &mut state.icon_filter);
                c.slider("Preview size", &mut state.icon_size, 16.0, 64.0);
                let filter = state.icon_filter.to_lowercase();
                let icons: Vec<_> = BuiltinIcon::ALL
                    .iter()
                    .copied()
                    .filter(|icon| icon.key().contains(&filter))
                    .collect();
                ui.separator();
                ui.child_window("icon-catalog")
                    .size([0.0, 0.0])
                    .build(ui, || {
                        c.list(icons.len(), |index| {
                            let icon = icons[index];
                            ui.group(|| {
                                c.icon_sized(icon, state.icon_size);
                                c.same_line();
                                c.text(icon.key());
                            });
                        });
                    });
            });
        state.icon_browser = browser_open;
    }
}
