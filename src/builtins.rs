use bed_workbench::WorkbenchModules;
use std::{cell::RefCell, rc::Rc};

pub fn modules() -> WorkbenchModules {
    let extensions = bed_editor_ui::extensions::EditorExtensions::default();
    let mut config = bed_module_editor::EditorConfig::default();
    config.options.extensions = extensions.clone();
    let editor_config = Rc::new(RefCell::new(config));
    let editor = bed_module_editor::EditorModule::new(editor_config.clone());
    let editor_runtime = editor.runtime().clone();
    let explorer = bed_module_explorer::ExplorerHandle::default();
    let search = bed_module_search::SearchHandle::default();
    let projects = Rc::new(RefCell::new(bed_module_projects::ProjectsState::default()));
    let instances: Vec<Box<dyn bed_workbench_api::Module>> = vec![
        Box::new(editor),
        Box::new(bed_module_explorer::ExplorerModule::new(explorer.clone())),
        Box::new(bed_module_search::SearchModule::new(search.clone())),
        Box::new(bed_module_projects::ProjectsModule::new(projects.clone())),
        Box::new(bed_module_settings::SettingsModule),
        Box::new(bed_module_terminal::TerminalModule),
        Box::new(bed_module_debug::DebugModule::new(&extensions)),
        Box::new(bed_plugin_structure::StructurePlugin::default()),
        Box::new(bed_plugin_image::ImagePlugin::default()),
        Box::new(bed_plugin_gltf::GltfPlugin),
        Box::new(bed_plugin_font::FontPlugin),
        Box::new(bed_plugin_audio::AudioPlugin),
        Box::new(bed_plugin_csv::CsvPlugin),
    ];
    WorkbenchModules {
        editor_config,
        editor_runtime,
        explorer,
        search,
        projects,
        instances,
    }
}
