//! Application policy and panel contributions. All layouts and panels use public workbench APIs.
use bedtime_ui2::{UiFrame, egui, gallery, register_view};
use bedtime_workbench2::*;
use bevy::prelude::*;

#[derive(Resource)]
pub(crate) struct DemoWorkbench {
    pub drag_source: Rect,
    pub light: Entity,
    pub dark: Entity,
    pub home: Entity,
    pub split: Entity,
    pub reading: Entity,
    pub scratch: Entity,
}
#[derive(Component)]
pub(crate) struct Notes {
    pub text: String,
}

#[derive(Resource, Default)]
struct DemoRequests(Vec<DemoRequest>);
#[derive(Clone, Copy)]
enum DemoRequest {
    Controls,
    Gallery,
    Notes,
    Cube,
    ThemeEditor,
    NewLayout,
    DeleteLayout,
    NewWorkspace,
    DeleteWorkspace,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<DemoRequests>()
        .add_systems(
            PreUpdate,
            sync_availability.before(bedtime_ui2::UiSystems::Draw),
        )
        .add_systems(
            PreUpdate,
            apply
                .after(ContextCommandSystems::Dispatch)
                .before(WorkbenchSystems::Apply),
        );
}

pub(crate) fn setup(
    world: &mut World,
    surface: Entity,
    menu: Entity,
    light: Entity,
    dark: Entity,
) -> Entity {
    let (home, split, reading, scratch, scratch_layout) = {
        let mut w = Workbench::new(world);
        let home = w.active_workspace();
        let split = w.active_layout();
        let reading = w.create_layout(home, "Reading").unwrap();
        let scratch = w.create_workspace("Scratch", false);
        w.activate_workspace(scratch).unwrap();
        let scratch_layout = w.active_layout();
        w.activate_workspace(home).unwrap();
        (home, split, reading, scratch, scratch_layout)
    };
    world.get_mut::<Workspace>(home).unwrap().name = "Playground".into();
    world.get_mut::<Layout>(split).unwrap().name = "Split".into();
    world.insert_resource(DemoWorkbench {
        drag_source: Rect::default(),
        light,
        dark,
        home,
        split,
        reading,
        scratch,
    });
    setup_menus(world);
    let host = attach_surface(world, surface);
    world.get_mut::<WorkbenchHost>(host).unwrap().menu = Some(menu);
    let status_bar = world.get::<WorkbenchHost>(host).unwrap().status_bar();
    register_view(world, surface, crate::viewport_demo::draw_overlay);
    show_controls(world, split);
    let gallery = add_gallery(world, split);
    let cube = crate::viewport_demo::spawn(world, split);
    Workbench::new(world)
        .move_panel(cube, gallery, Some(SplitAxis::Horizontal))
        .unwrap();
    Workbench::new(world)
        .resize_border(split, SplitAxis::Horizontal, 5000, [0, 7400], 5900, 1)
        .unwrap();
    Workbench::new(world)
        .register_panel_type::<GalleryPanel, _>(gallery_factory)
        .unwrap();
    Workbench::new(world)
        .register_panel_type::<NotesPanel, _>(notes_factory)
        .unwrap();
    Workbench::new(world)
        .register_panel_type::<ControlsPanel, _>(controls_factory)
        .unwrap();
    Workbench::new(world)
        .register_panel_type::<crate::theme_editor::ThemeEditor, _>(crate::theme_editor::factory)
        .unwrap();
    show_controls(world, reading);
    add_gallery(world, reading);
    show_controls(world, scratch_layout);
    let area = Workbench::new(world)
        .destination_area(scratch_layout)
        .unwrap();
    let notes = notes_factory(In(area), world).unwrap();
    world.get_mut::<Panel>(notes).unwrap().title = "Scratch notes".into();
    world.get_mut::<Notes>(notes).unwrap().text = "This is panel-local state. Switch workspaces and come back.\n\nFocus this panel to reveal its Notes menu.".into();
    crate::status_demo::setup(world, status_bar);
    gallery
}

#[derive(Component)]
pub(crate) struct Controls;

fn show_controls(world: &mut World, layout: Entity) {
    let existing = world
        .query_filtered::<(Entity, &InArea), With<Controls>>()
        .iter(world)
        .find_map(|(entity, owner)| {
            (world.get::<InLayout>(owner.0).unwrap().0 == layout).then_some(entity)
        });
    if let Some(panel) = existing {
        Workbench::new(world).focus_panel(panel).unwrap();
        return;
    }
    let areas = world.get::<LayoutAreas>(layout).unwrap().entities();
    let empty = areas
        .iter()
        .copied()
        .find(|&a| areas.len() > 1 && panels_in_area(world, a).is_empty());
    let area = empty.unwrap_or_else(|| {
        let area = Workbench::new(world).destination_area(layout).unwrap();
        let rect = world.get::<Area>(area).unwrap().rect();
        let at = rect.min[1] + (rect.max[1] - rect.min[1]) * 74 / 100;
        Workbench::new(world)
            .split_area(area, SplitAxis::Vertical, at, [1, 1])
            .unwrap()
    });
    controls_factory(In(area), world).unwrap();
}

fn add_gallery(world: &mut World, layout: Entity) -> Entity {
    let context = world.resource::<DemoWorkbench>();
    let (light, dark) = (context.light, context.dark);
    let mut state = gallery::create_state(world, light, dark);
    state.embedded = true;
    let panel = register_panel(world, layout, "Widget gallery", gallery::draw_gallery).unwrap();
    world.entity_mut(panel).insert((
        state,
        bedtime_ui2::IconSource::from(bedtime_ui2::BuiltinIcon::Folder),
    ));
    panel
}

fn draw_notes(mut frame: UiFrame, mut notes: Query<&mut Notes>) {
    let mut notes = notes.get_mut(frame.view).unwrap();
    let padding = frame.panel_settings.padding;
    frame.scroll("notes", |mut frame| {
        frame.padded(padding, |mut frame| {
            frame.ui.heading("A workspace with its own panels");
            frame
                .ui
                .weak("Layout switches retain instances; closing a panel retires it.");
            frame.text_edit(
                egui::TextEdit::multiline(&mut notes.text)
                    .desired_width(f32::INFINITY)
                    .desired_rows(18),
            );
        })
    });
}
fn draw_controls(mut frame: UiFrame, world: &mut World) {
    let active_workspace = Workbench::new(world).active_workspace();
    let active_layout = Workbench::new(world).active_layout();
    let workspaces: Vec<_> = world
        .query::<(Entity, &Workspace)>()
        .iter(world)
        .map(|(e, w)| (e, w.name.clone()))
        .collect();
    let layouts: Vec<_> = world
        .query::<(Entity, &Layout, &InWorkspace)>()
        .iter(world)
        .filter(|(_, _, w)| w.0 == active_workspace)
        .map(|(e, l, _)| (e, l.name.clone()))
        .collect();
    let padding = frame.panel_settings.padding;
    frame.scroll("controls", |mut frame| {
        frame.padded(padding, |frame| {
            frame.ui.horizontal_wrapped(|ui| {
                bedtime_ui2::ComboBox::from_label("Workspace")
                    .selected_text(&world.get::<Workspace>(active_workspace).unwrap().name)
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for (entity, name) in &workspaces {
                            if ui
                                .selectable_label(*entity == active_workspace, name)
                                .clicked()
                            {
                                world
                                    .resource_mut::<WorkbenchRequests>()
                                    .submit(WorkbenchRequest::ActivateWorkspace(*entity));
                            }
                        }
                    });
                bedtime_ui2::ComboBox::from_label("Layout")
                    .selected_text(&world.get::<Layout>(active_layout).unwrap().name)
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for (entity, name) in &layouts {
                            if ui
                                .selectable_label(*entity == active_layout, name)
                                .clicked()
                            {
                                world
                                    .resource_mut::<WorkbenchRequests>()
                                    .submit(WorkbenchRequest::ActivateLayout(*entity));
                            }
                        }
                    });
            });
            let mut settings = frame.panel_settings;
            let settings_changed = frame
                .ui
                .horizontal_wrapped(|ui| {
                    let root = world
                        .query_filtered::<Entity, With<MenuBar>>()
                        .iter(world)
                        .next();
                    if let Some(root) = root {
                        let mut native = world.get::<NativeMenuBar>(root).is_some();
                        let supported = world
                            .get_resource::<NativeCapabilities>()
                            .is_some_and(|c| c.native_menus);
                        if ui
                            .add_enabled(supported, egui::Checkbox::new(&mut native, "Native menu"))
                            .changed()
                        {
                            if native {
                                world.entity_mut(root).insert(NativeMenuBar);
                            } else {
                                world.entity_mut(root).remove::<NativeMenuBar>();
                            }
                        }
                    }
                    let mut blur = world.get::<WindowBlur>(frame.surface).is_some_and(|b| b.0);
                    if ui
                        .add_enabled(
                            cfg!(target_os = "macos"),
                            egui::Checkbox::new(&mut blur, "Blur"),
                        )
                        .changed()
                    {
                        world.entity_mut(frame.surface).insert(WindowBlur(blur));
                    }
                    let mut opacity = frame.theme.palette.background[3];
                    if ui
                        .add(egui::Slider::new(&mut opacity, 0.25..=1.0).text("Opacity"))
                        .changed()
                    {
                        let context = world.resource::<DemoWorkbench>();
                        let themes = [context.light, context.dark];
                        for theme in themes {
                            world
                                .get_mut::<bedtime_ui2::Theme>(theme)
                                .unwrap()
                                .palette
                                .background[3] = opacity;
                        }
                    }
                    let debug_changed = ui
                        .checkbox(&mut settings.debug, "Show corner targets")
                        .changed();
                    let size_changed = ui
                        .add(
                            egui::Slider::new(&mut settings.hit_size, 0.0..=64.0)
                                .text("Corner size"),
                        )
                        .changed();
                    let rounding_changed = ui
                        .add(
                            egui::DragValue::new(&mut settings.rounding)
                                .prefix("Panel rounding ")
                                .speed(0.25)
                                .range(0.0..=64.0),
                        )
                        .changed();
                    debug_changed || size_changed || rounding_changed
                })
                .inner;
            let spacing_changed = frame
                .ui
                .horizontal(|ui| {
                    ui.label("Panel padding");
                    let x_changed = ui
                        .add(
                            egui::DragValue::new(&mut settings.padding[0])
                                .prefix("X ")
                                .speed(0.25)
                                .range(0.0..=128.0),
                        )
                        .changed();
                    let y_changed = ui
                        .add(
                            egui::DragValue::new(&mut settings.padding[1])
                                .prefix("Y ")
                                .speed(0.25)
                                .range(0.0..=128.0),
                        )
                        .changed();
                    let gap_changed = ui
                        .add(
                            egui::DragValue::new(&mut settings.panel_gap)
                                .prefix("Panel gap ")
                                .speed(0.25)
                                .range(0.0..=64.0),
                        )
                        .changed();
                    x_changed || y_changed || gap_changed
                })
                .inner;
            if settings_changed || spacing_changed {
                world.entity_mut(frame.surface).insert(settings);
            }
            frame.ui.horizontal_wrapped(|ui| {
                for (label, request, enabled) in [
                    ("New layout", DemoRequest::NewLayout, true),
                    (
                        "Delete layout",
                        DemoRequest::DeleteLayout,
                        layouts.len() > 1,
                    ),
                    ("Open gallery", DemoRequest::Gallery, true),
                    ("Open 3D viewport", DemoRequest::Cube, true),
                    ("Open notes", DemoRequest::Notes, true),
                    ("Theme editor", DemoRequest::ThemeEditor, true),
                    ("New workspace", DemoRequest::NewWorkspace, true),
                    (
                        "Delete workspace",
                        DemoRequest::DeleteWorkspace,
                        workspaces.len() > 1,
                    ),
                ] {
                    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                        world.resource_mut::<DemoRequests>().0.push(request);
                    }
                }
                let response =
                    ui.add(egui::Button::new("Drag notes").sense(egui::Sense::click_and_drag()));
                world.resource_mut::<DemoWorkbench>().drag_source = Rect::from_corners(
                    Vec2::new(response.rect.min.x, response.rect.min.y),
                    Vec2::new(response.rect.max.x, response.rect.max.y),
                );
                if let Some(kind) = world
                    .query::<(Entity, &PanelType)>()
                    .iter(world)
                    .find_map(|(e, p)| (p.key == NotesPanel::KEY).then_some(e))
                {
                    panel_drag_source(&response, kind, "Notes");
                }
            });
            frame.ui.weak(concat!(
                "Drag corners to split/join. Drag tabs to move or split. ",
                "Focus Notes to reveal its menu."
            ));
        })
    });
}

fn apply(world: &mut World) {
    let requests = std::mem::take(&mut world.resource_mut::<DemoRequests>().0);
    for request in requests {
        let workspace = Workbench::new(world).active_workspace();
        let layout = Workbench::new(world).active_layout();
        match request {
            DemoRequest::Controls => show_controls(world, layout),
            DemoRequest::Gallery => {
                let panel = world
                    .query_filtered::<(Entity, &InArea), With<gallery::GalleryState>>()
                    .iter(world)
                    .find_map(|(e, l)| {
                        (world.get::<InLayout>(l.0).unwrap().0 == layout).then_some(e)
                    });
                if let Some(panel) = panel {
                    Workbench::new(world).focus_panel(panel).unwrap();
                } else {
                    add_gallery(world, layout);
                }
            }
            DemoRequest::Notes => {
                let area = Workbench::new(world).destination_area(layout).unwrap();
                notes_factory(In(area), world).unwrap();
            }
            DemoRequest::Cube => crate::viewport_demo::show(world),
            DemoRequest::ThemeEditor => {
                let panel = world
                    .query_filtered::<(Entity, &InArea), With<crate::theme_editor::ThemeEditor>>()
                    .iter(world)
                    .find_map(|(entity, area)| {
                        (world.get::<InLayout>(area.0).unwrap().0 == layout).then_some(entity)
                    });
                if let Some(panel) = panel {
                    Workbench::new(world).focus_panel(panel).unwrap();
                } else {
                    let area = Workbench::new(world).destination_area(layout).unwrap();
                    crate::theme_editor::factory(In(area), world).unwrap();
                }
            }
            DemoRequest::NewLayout => {
                let n = world.query::<&Layout>().iter(world).count();
                let layout = Workbench::new(world)
                    .create_layout(workspace, format!("Layout {}", n + 1))
                    .unwrap();
                Workbench::new(world).activate_layout(layout).unwrap();
                show_controls(world, layout);
            }
            DemoRequest::DeleteLayout => {
                Workbench::new(world).delete_layout(layout).unwrap();
            }
            DemoRequest::NewWorkspace => {
                let n = world.query::<&Workspace>().iter(world).count();
                let workspace =
                    Workbench::new(world).create_workspace(format!("Workspace {}", n + 1), false);
                Workbench::new(world).activate_workspace(workspace).unwrap();
                let layout = Workbench::new(world).active_layout();
                show_controls(world, layout);
            }
            DemoRequest::DeleteWorkspace => {
                let replacement = world
                    .query_filtered::<Entity, With<Workspace>>()
                    .iter(world)
                    .find(|&e| e != workspace)
                    .unwrap();
                let mut w = Workbench::new(world);
                w.activate_workspace(replacement).unwrap();
                w.delete_workspace(workspace).unwrap();
            }
        }
    }
}

struct OpenControls;
impl ContextCommand for OpenControls {
    const KEY: &'static str = "bed2.open_controls";
    const LABEL: &'static str = "Open Controls";
}
struct OpenGallery;
impl ContextCommand for OpenGallery {
    const KEY: &'static str = "bed2.open_gallery";
    const LABEL: &'static str = "Open Widget Gallery";
}
struct OpenNotes;
impl ContextCommand for OpenNotes {
    const KEY: &'static str = "bed2.open_notes";
    const LABEL: &'static str = "Open Notes";
}
struct OpenThemeEditor;
impl ContextCommand for OpenThemeEditor {
    const KEY: &'static str = "bed2.open_theme_editor";
    const LABEL: &'static str = "Open Theme Editor";
}
struct NewLayout;
impl ContextCommand for NewLayout {
    const KEY: &'static str = "bed2.new_layout";
    const LABEL: &'static str = "New Layout";
}
struct DeleteLayout;
impl ContextCommand for DeleteLayout {
    const KEY: &'static str = "bed2.delete_layout";
    const LABEL: &'static str = "Delete Layout";
}
struct NewWorkspace;
impl ContextCommand for NewWorkspace {
    const KEY: &'static str = "bed2.new_workspace";
    const LABEL: &'static str = "New Workspace";
}
struct DeleteWorkspace;
impl ContextCommand for DeleteWorkspace {
    const KEY: &'static str = "bed2.delete_workspace";
    const LABEL: &'static str = "Delete Workspace";
}
struct CopyNotes;
impl ContextCommand for CopyNotes {
    const KEY: &'static str = "bed2.copy_notes";
    const LABEL: &'static str = "Copy Note Text";
}
struct ClearNotes;
impl ContextCommand for ClearNotes {
    const KEY: &'static str = "bed2.clear_notes";
    const LABEL: &'static str = "Clear Note Text";
}
struct InsertSample;
impl ContextCommand for InsertSample {
    const KEY: &'static str = "bed2.insert_sample";
    const LABEL: &'static str = "Insert Sample Note";
}

fn register_request<C: ContextCommand>(world: &mut World, request: DemoRequest) {
    Workbench::new(world)
        .register_handler::<C, _>(
            move |_: In<ContextCommandInvocation>,
                  mut requests: ResMut<DemoRequests>|
                  -> ContextCommandResult {
                requests.0.push(request);
                Ok(())
            },
        )
        .unwrap();
}

fn setup_menus(world: &mut World) {
    register_request::<OpenControls>(world, DemoRequest::Controls);
    register_request::<OpenGallery>(world, DemoRequest::Gallery);
    register_request::<OpenNotes>(world, DemoRequest::Notes);
    register_request::<OpenThemeEditor>(world, DemoRequest::ThemeEditor);
    register_request::<NewLayout>(world, DemoRequest::NewLayout);
    register_request::<DeleteLayout>(world, DemoRequest::DeleteLayout);
    register_request::<NewWorkspace>(world, DemoRequest::NewWorkspace);
    register_request::<DeleteWorkspace>(world, DemoRequest::DeleteWorkspace);
    world.spawn(Binding::new::<OpenNotes>(KeyChord::new(
        Modifiers::PRIMARY,
        KeyCode::KeyN,
    )));
    world.spawn(Binding::new::<NewLayout>(KeyChord::new(
        Modifiers::PRIMARY | Modifiers::SHIFT,
        KeyCode::KeyN,
    )));
    let root = world
        .query_filtered::<Entity, With<MenuBar>>()
        .single(world)
        .unwrap();
    for (label, items) in [
        (
            "View",
            vec![
                (OpenControls::LABEL, ContextCommandId::of::<OpenControls>()),
                (OpenGallery::LABEL, ContextCommandId::of::<OpenGallery>()),
                (OpenNotes::LABEL, ContextCommandId::of::<OpenNotes>()),
                (
                    OpenThemeEditor::LABEL,
                    ContextCommandId::of::<OpenThemeEditor>(),
                ),
                (
                    crate::OpenViewport::LABEL,
                    ContextCommandId::of::<crate::OpenViewport>(),
                ),
            ],
        ),
        (
            "Layout",
            vec![
                (NewLayout::LABEL, ContextCommandId::of::<NewLayout>()),
                (DeleteLayout::LABEL, ContextCommandId::of::<DeleteLayout>()),
            ],
        ),
        (
            "Workspace",
            vec![
                (NewWorkspace::LABEL, ContextCommandId::of::<NewWorkspace>()),
                (
                    DeleteWorkspace::LABEL,
                    ContextCommandId::of::<DeleteWorkspace>(),
                ),
            ],
        ),
    ] {
        let menu = world
            .spawn((
                Menu {
                    label: label.into(),
                },
                InMenu(root),
            ))
            .id();
        for (label, command) in items {
            world.spawn((MenuItem::new(label), InMenu(menu), Invokes(command)));
        }
    }
}

fn sync_availability(world: &mut World) {
    // The demo is initialized in Startup, before the first UI pass.
    let workspace = Workbench::new(world).active_workspace();
    let layouts = world
        .query::<&InWorkspace>()
        .iter(world)
        .filter(|w| w.0 == workspace)
        .count();
    let workspaces = world.query::<&Workspace>().iter(world).count();
    for (command, enabled) in [
        (ContextCommandId::of::<DeleteLayout>(), layouts > 1),
        (ContextCommandId::of::<DeleteWorkspace>(), workspaces > 1),
    ] {
        let handler = Workbench::new(world)
            .handler(command, None)
            .expect("Demo handler");
        world.get_mut::<ActionEnabled>(handler).unwrap().0 = enabled;
    }
}

struct ControlsPanel;
impl PanelDefinition for ControlsPanel {
    const KEY: &'static str = "bed2.controls";
    const LABEL: &'static str = "Controls";
}
struct GalleryPanel;
impl PanelDefinition for GalleryPanel {
    const KEY: &'static str = "bed2.gallery";
    const LABEL: &'static str = "Widget gallery";
}
struct NotesPanel;
impl PanelDefinition for NotesPanel {
    const KEY: &'static str = "bed2.notes";
    const LABEL: &'static str = "Notes";
}
fn gallery_factory(In(area): In<Entity>, world: &mut World) -> Result<Entity, String> {
    let context = world.resource::<DemoWorkbench>();
    let (light, dark) = (context.light, context.dark);
    let mut state = gallery::create_state(world, light, dark);
    state.embedded = true;
    let panel = register_panel_in_area(world, area, "Widget gallery", gallery::draw_gallery)?;
    world.entity_mut(panel).insert((
        state,
        bedtime_ui2::IconSource::from(bedtime_ui2::BuiltinIcon::Folder),
    ));
    Ok(panel)
}
fn notes_factory(In(area): In<Entity>, world: &mut World) -> Result<Entity, String> {
    let panel = register_panel_in_area(world, area, "Notes", draw_notes)?;
    world.entity_mut(panel).insert(Notes {
        text: "New panel. Drag its tab or split its area.".into(),
    });
    Workbench::new(world).register_panel_handler::<CopyNotes, _>(panel, copy_notes)?;
    Workbench::new(world).register_panel_handler::<ClearNotes, _>(panel, clear_notes)?;
    Workbench::new(world).register_panel_handler::<InsertSample, _>(panel, insert_sample)?;
    world.spawn((
        Binding::new::<CopyNotes>(KeyChord::new(
            Modifiers::PRIMARY | Modifiers::SHIFT,
            KeyCode::KeyC,
        )),
        InPanel(panel),
    ));
    let root = world
        .query_filtered::<Entity, With<MenuBar>>()
        .single(world)
        .unwrap();
    let menu = world
        .spawn((
            Menu {
                label: "Notes".into(),
            },
            InMenu(root),
            InPanel(panel),
        ))
        .id();
    world.spawn((
        MenuItem::new(CopyNotes::LABEL),
        InMenu(menu),
        Invokes::of::<CopyNotes>(),
    ));
    world.spawn((MenuSeparator, InMenu(menu)));
    world.spawn((
        MenuItem::new(InsertSample::LABEL),
        InMenu(menu),
        Invokes::of::<InsertSample>(),
    ));
    world.spawn((
        MenuItem::new(ClearNotes::LABEL),
        InMenu(menu),
        Invokes::of::<ClearNotes>(),
    ));
    Ok(panel)
}
fn controls_factory(In(area): In<Entity>, world: &mut World) -> Result<Entity, String> {
    let panel = register_panel_in_area(world, area, "Controls", draw_controls)?;
    world.entity_mut(panel).insert(Controls);
    Ok(panel)
}
fn copy_notes(
    In(invocation): In<ContextCommandInvocation>,
    notes: Query<&Notes>,
    mut clipboard: ResMut<Clipboard>,
) -> ContextCommandResult {
    let notes = notes
        .get(invocation.panel.expect("Panel handler"))
        .map_err(|e| e.to_string())?;
    clipboard.write_text(&notes.text).map_err(|e| e.to_string())
}
fn clear_notes(
    In(invocation): In<ContextCommandInvocation>,
    mut notes: Query<&mut Notes>,
) -> ContextCommandResult {
    notes
        .get_mut(invocation.panel.expect("Panel handler"))
        .map_err(|e| e.to_string())?
        .text
        .clear();
    Ok(())
}
fn insert_sample(
    In(invocation): In<ContextCommandInvocation>,
    mut notes: Query<&mut Notes>,
) -> ContextCommandResult {
    let mut notes = notes
        .get_mut(invocation.panel.expect("Panel handler"))
        .map_err(|e| e.to_string())?;
    notes.text.push_str("\nA little room for a new idea.");
    Ok(())
}
