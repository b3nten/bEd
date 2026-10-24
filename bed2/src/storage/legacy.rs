//! Read-only migration of the former tool/geometry defaults. Live reconstruction
//! uses reflected layouts; this enum exists only at the old-format boundary.
use super::*;
use crate::persistence::ReflectGlobalPersist;
use bedtime_workbench2::{Area, AreaRect, InArea, InLayout, Layout, WorkbenchCorePlugin};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) enum Tool {
    LanguageServers,
    Git,
    Files,
    Structure,
    Terminal,
    Viewport,
    Bed,
    Ducky,
    Tasks,
    Settings,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Template {
    pub areas: Vec<TemplateArea>,
    pub focused_area: Option<usize>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TemplateArea {
    pub rect: AreaRect,
    pub tools: Vec<Tool>,
    pub selected: Option<usize>,
}
#[derive(Resource, Reflect, Clone, Serialize, Deserialize)]
#[type_path = "bed2::layouts"]
#[type_name = "LayoutDefaults"]
// The old type deliberately shares the current type path. Register it only in
// the isolated legacy decoder; Bevy's automatic registration must never see it.
#[reflect(opaque, no_auto_register)]
#[reflect(Resource, Serialize, Deserialize, GlobalPersist)]
#[serde(rename = "LayoutDefaults")]
struct OldDefaults([Template; 5]);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OldDocument {
    version: u32,
    settings: Settings,
    #[serde(default)]
    startup: StartupPreferences,
    layouts: [Template; 5],
    #[serde(default)]
    bindings: crate::keybindings::GlobalBindings,
}

pub(super) fn read_values(bytes: &[u8]) -> Result<Document, String> {
    let old: OldDocument = ron::de::from_bytes(bytes).map_err(|e| e.to_string())?;
    convert(old)
}
pub(super) fn read_reflected(
    snapshot: &bedtime_workbench2::persistence::Snapshot,
) -> Result<Document, String> {
    let mut app = App::new();
    bedtime_workbench2::persistence::register(&mut app);
    app.register_type::<GlobalPreferences>()
        .register_type::<StartupPreferences>()
        .register_type::<OldDefaults>()
        .register_type::<crate::keybindings::GlobalBindings>();
    let prepared =
        bedtime_workbench2::persistence::CapturedWorld::<ReflectGlobalPersist>::deserialize(
            snapshot,
            app.world().resource::<AppTypeRegistry>(),
        )?;
    if prepared.has_entities() {
        return Err("Global preferences contain entities".into());
    }
    convert(OldDocument {
        version: 1,
        settings: prepared
            .resource::<GlobalPreferences>()
            .ok_or("Missing global settings")?
            .0,
        startup: prepared
            .resource::<StartupPreferences>()
            .ok_or("Missing startup settings")?,
        layouts: prepared
            .resource::<OldDefaults>()
            .ok_or("Missing layout defaults")?
            .0,
        bindings: prepared
            .resource::<crate::keybindings::GlobalBindings>()
            .ok_or("Missing global bindings")?,
    })
}
fn convert(old: OldDocument) -> Result<Document, String> {
    if old.version != 1 {
        return Err("Unsupported preferences version".into());
    }
    let layouts = old
        .layouts
        .iter()
        .map(capture_template)
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| "Expected five layout defaults")?;
    let document = Document {
        version: 1,
        settings: old.settings,
        startup: old.startup,
        layouts,
        bindings: old.bindings,
    };
    document.validate()?;
    Ok(document)
}
fn capture_template(
    template: &Template,
) -> Result<Option<bedtime_workbench2::persistence::Snapshot>, String> {
    if template.areas.is_empty()
        || template.areas.len() > 64
        || template
            .focused_area
            .is_some_and(|i| i >= template.areas.len())
    {
        return Err("Invalid legacy layout areas/focus".into());
    }
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin);
    crate::persistence::register(&mut app);
    let world = app.world_mut();
    let layout = world.spawn(Layout::default()).id();
    let mut areas = Vec::new();
    for recipe in &template.areas {
        if recipe.tools.len() > 256 || recipe.selected.is_some_and(|i| i >= recipe.tools.len()) {
            return Err("Invalid legacy layout tabs".into());
        }
        let area = world
            .spawn((
                Area {
                    rect: recipe.rect,
                    selected: None,
                },
                InLayout(layout),
            ))
            .id();
        areas.push(area);
        let mut panels = Vec::new();
        for tool in &recipe.tools {
            let panel = match tool {
                Tool::LanguageServers => {
                    world.spawn((crate::lsp_panel::LspPanel, InArea(area))).id()
                }
                Tool::Git => world
                    .spawn((crate::git::GitPanel::default(), InArea(area)))
                    .id(),
                Tool::Files => world
                    .spawn((bedtime_filetree2::FileTreePanel::default(), InArea(area)))
                    .id(),
                Tool::Structure => world
                    .spawn((bed_structure2::StructurePanel::default(), InArea(area)))
                    .id(),
                Tool::Terminal => world
                    .spawn((bedtime_terminal2::TerminalPanel::default(), InArea(area)))
                    .id(),
                Tool::Viewport => world.spawn((crate::cube::CubeViewport, InArea(area))).id(),
                Tool::Bed => world.spawn((bedtime_mascots2::BedPanel, InArea(area))).id(),
                Tool::Ducky => world
                    .spawn((bedtime_mascots2::DuckPanel, InArea(area)))
                    .id(),
                Tool::Tasks => world
                    .spawn((crate::tasks::TasksPanel::default(), InArea(area)))
                    .id(),
                Tool::Settings => world
                    .spawn((crate::settings::SettingsPanel, InArea(area)))
                    .id(),
            };
            panels.push(panel);
        }
        world.flush();
        world.get_mut::<Area>(area).unwrap().selected = recipe.selected.map(|i| panels[i]);
    }
    world.get_mut::<Layout>(layout).unwrap().focused_area = template.focused_area.map(|i| areas[i]);
    let capture = crate::persistence::capture_features(world)?
        .into_layout(layout, world.resource::<AppTypeRegistry>())?;
    capture
        .serialize(world.resource::<AppTypeRegistry>())
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedtime_workbench2::Workbench;
    fn defaults() -> [Template; 5] {
        std::array::from_fn(|_| Template {
            areas: vec![TemplateArea {
                rect: AreaRect {
                    min: [0, 0],
                    max: [10000, 10000],
                },
                tools: vec![Tool::Files, Tool::Settings],
                selected: Some(1),
            }],
            focused_area: Some(0),
        })
    }
    #[test]
    fn legacy_global_files_are_read_without_rewriting_and_upgrade_on_a_real_edit() {
        let original = OldDocument {
            version: 1,
            settings: Settings::default(),
            startup: StartupPreferences::default(),
            layouts: defaults(),
            bindings: default(),
        };
        let values = encode(&original).unwrap();
        let mut legacy_app = App::new();
        bedtime_workbench2::persistence::register(&mut legacy_app);
        legacy_app
            .register_type::<GlobalPreferences>()
            .register_type::<StartupPreferences>()
            .register_type::<OldDefaults>()
            .register_type::<crate::keybindings::GlobalBindings>();
        legacy_app
            .insert_resource(GlobalPreferences(original.settings.clone()))
            .insert_resource(original.startup.clone())
            .insert_resource(OldDefaults(defaults()))
            .insert_resource(original.bindings.clone());
        let reflected = Workbench::capture_world::<ReflectGlobalPersist>(legacy_app.world_mut())
            .unwrap()
            .serialize(legacy_app.world().resource::<AppTypeRegistry>())
            .unwrap();
        let reflected = encode(&StoredPreferences {
            version: 2,
            values: reflected,
        })
        .unwrap();
        for bytes in [values, reflected] {
            let (mut app, root) = crate::storage::tests::host();
            let path = root.join("preferences.bed");
            atomic_write(&path, &bytes).unwrap();
            configure_directory(&mut app, root.clone());
            crate::storage::tests::settle(&mut app).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert!(
                app.world()
                    .resource::<LayoutDefaults>()
                    .0
                    .iter()
                    .all(Option::is_some)
            );
            app.world_mut()
                .resource_mut::<GlobalPreferences>()
                .0
                .font_size = 25.0;
            crate::storage::tests::settle(&mut app).unwrap();
            assert_ne!(std::fs::read(&path).unwrap(), bytes);
            let restored = read(&path).unwrap().unwrap();
            assert_eq!(restored.settings.font_size, 25.0);
            assert!(restored.layouts.iter().all(Option::is_some));
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn reflected_recipe_preferences_migrate_without_losing_settings_or_panel_choices() {
        let mut app = App::new();
        bedtime_workbench2::persistence::register(&mut app);
        app.register_type::<GlobalPreferences>()
            .register_type::<StartupPreferences>()
            .register_type::<OldDefaults>()
            .register_type::<crate::keybindings::GlobalBindings>();
        let mut settings = Settings::default();
        settings.font_size = 23.0;
        app.insert_resource(GlobalPreferences(settings))
            .insert_resource(StartupPreferences::default())
            .insert_resource(OldDefaults(defaults()))
            .init_resource::<crate::keybindings::GlobalBindings>();
        let snapshot = Workbench::capture_world::<ReflectGlobalPersist>(app.world_mut())
            .unwrap()
            .serialize(app.world().resource::<AppTypeRegistry>())
            .unwrap();
        let migrated = read_reflected(&snapshot).unwrap();
        assert_eq!(migrated.settings.font_size, 23.0);
        let capture = bedtime_workbench2::persistence::CapturedLayout::<
            bedtime_workbench2::persistence::ReflectWorkbenchPersist,
        >::deserialize(
            migrated.layouts[0].as_ref().unwrap(),
            &preference_registry(),
        )
        .unwrap();
        assert_eq!(capture.components::<crate::files::PersistedTree>().len(), 1);
        assert_eq!(
            capture
                .components::<crate::settings::PersistedSettings>()
                .len(),
            1
        );
        assert!(capture.components::<Area>()[0].1.selected.is_some());
    }
}
