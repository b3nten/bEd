//! Exercise viewer choices through the native tree and document context menus.
#![cfg(all(feature = "editor", feature = "file-explorer"))]

#[path = "support/editor_host.rs"]
mod editor_host;
#[path = "support/temp_dir.rs"]
mod temp_dir;

use bed_api::{
    application::WorkspaceAction,
    documents::PanelBinding,
    features::{HostAction, HostEffects},
};
use bed_app::{AppOptions, AppRuntimePlugin, WorkspaceState};
use bed_documents::DocumentSession;
use bevy::prelude::*;
use dear_imgui_rs::{Context, MouseButton, sys};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use {
    bed_workbench::PanelPresentation, bed_workbench::WorkbenchLayout,
    bed_workbench::workspace::tiling::Layout, bed_workbench::workspace::tiling_state::TilingState,
};

struct Fixture {
    app: App,
    context: Context,
    files: Entity,
    _directory: temp_dir::TempDir,
}

impl Fixture {
    fn frame(
        &mut self,
    ) -> (
        Vec<bed_plugin_file_explorer::file_tree::FileTreeDropTarget>,
        String,
    ) {
        self.app.update();
        let ui = self.context.frame();
        ui.binding()
            .with_bound_context(|| unsafe { sys::igLogToBuffer(-1) });
        bed_app::draw_ui(self.app.world_mut(), ui).unwrap();
        let id = self
            .app
            .world()
            .get::<PanelPresentation>(self.files)
            .unwrap()
            .id;
        let mut rows = Vec::new();
        ui.window(format!("Files###bed_tab_{id}")).build(|| {
            rows = self
                .app
                .world()
                .non_send::<bed_plugin_file_explorer::ExplorerState>()
                .explorer()
                .file_tree
                .drop_targets(ui);
        });
        let log = ui.binding().with_bound_context(|| unsafe {
            let native = &*sys::igGetCurrentContext();
            let bytes = &native.LogBuffer.Buf;
            let text = if bytes.Size > 0 {
                std::ffi::CStr::from_ptr(bytes.Data)
                    .to_string_lossy()
                    .into_owned()
            } else {
                String::new()
            };
            sys::igLogFinish();
            text
        });
        drop(self.context.render_legacy());
        bed_app::process_app_requests(self.app.world_mut()).unwrap();
        assert!(
            self.app
                .world()
                .resource::<WorkspaceState>()
                .error
                .is_none(),
            "menu request failed: {:?}",
            self.app.world().resource::<WorkspaceState>().error
        );
        (rows, log)
    }

    fn settle(&mut self) {
        for _ in 0..6 {
            self.frame();
        }
    }

    fn click(&mut self, point: [f32; 2], button: MouseButton) -> String {
        self.context.io_mut().add_mouse_pos_event(point);
        self.frame();
        self.context.io_mut().add_mouse_button_event(button, true);
        self.frame();
        self.context.io_mut().add_mouse_button_event(button, false);
        self.frame();
        self.settle();
        self.frame().1
    }

    fn popup_item(&self, level: usize, row: Option<usize>) -> [f32; 2] {
        self.context.binding().with_bound_context(|| unsafe {
            let native = &*sys::igGetCurrentContext();
            assert!(
                native.OpenPopupStack.Size > level as i32,
                "context menu did not open"
            );
            let window = (*native.OpenPopupStack.Data.add(level)).Window;
            assert!(!window.is_null(), "menu was not rendered");
            let popup = &*window;
            let y = row.map_or(popup.DC.CursorPosPrevLine.y, |row| {
                popup.DC.CursorStartPos.y + row as f32 * native.FontSize * 1.4
            });
            [popup.DC.CursorStartPos.x + 18., y + native.FontSize * 0.5]
        })
    }

    fn active_frontend(&self, panel: Entity) -> Entity {
        bed_app::panel_presentation(self.app.world(), panel).unwrap()
    }
}

#[test]
fn markdown_menus_open_and_switch_the_shared_document_without_sqlite_choices() {
    let directory = temp_dir::TempDir::new();
    let root = directory.root().join("project");
    std::fs::create_dir(&root).unwrap();
    let markdown = root.join("README.md");
    let disk = b"# Saved heading\n\nSaved paragraph.\n";
    directory.write("project/README.md", disk);
    std::fs::write(root.join("data.sqlite"), b"SQLite format 3\0").unwrap();
    directory.write("config/settings.json", br#"{"autosave":false}"#);
    let mut app = App::new();
    editor_host::install(
        &mut app,
        bed_app_config::Settings::with_paths(
            directory.path("config"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap(),
    )
    .unwrap();
    app.add_plugins((
        bed_runtime::BedRuntimePlugin,
        AppRuntimePlugin {
            options: AppOptions {
                working_directory: Some(root.clone()),
                ..Default::default()
            },
        },
        bed_plugin_editor::EditorPlugin,
        bed_plugin_file_explorer::FileExplorerPlugin,
        bed_plugin_markdown_preview::MarkdownPreviewPlugin,
        bed_plugin_sqlite_browser::SqliteBrowserPlugin,
    ));
    app.update();
    app.world_mut().resource_mut::<HostEffects>().push(
        None,
        HostAction::Workspace(WorkspaceAction::OpenLocalWorkspace(root.clone())),
    );
    app.update();
    app.world_mut().resource_mut::<HostEffects>().push(
        None,
        HostAction::OpenPanel {
            panel_type: bed_plugin_file_explorer::PANEL_ID.into(),
            document: None,
            state: serde_json::Value::Null,
        },
    );
    app.update();
    let files_frontend = *app
        .world()
        .non_send::<bed_plugin_file_explorer::ExplorerPanels>()
        .0
        .keys()
        .last()
        .unwrap();
    let files = bed_workbench::api::panel_owner(app.world(), files_frontend).unwrap();
    let panel = bed_app::open_file(
        app.world_mut(),
        &markdown,
        Some(bed_plugin_editor::TEXT_VIEWER),
        false,
        None,
    )
    .unwrap();
    let text_frontend = bed_app::panel_presentation(app.world(), panel).unwrap();
    let binding = app
        .world()
        .get::<PanelBinding>(text_frontend)
        .unwrap()
        .clone();
    let document = binding.document.unwrap();
    let view = binding.view.unwrap();
    let unsaved = b"# Unsaved heading\n\nThis edit must survive viewer switches.\n";
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .with_commands(view, |commands| {
            commands.select_all();
            commands.paste(unsaved);
        })
        .unwrap();
    let mut layout = Layout::default();
    let editor_area = layout.split(1, 0, 3500, 1).unwrap();
    let mut tiling = TilingState::new(layout);
    let panels = app
        .world_mut()
        .query::<(Entity, &PanelPresentation)>()
        .iter(app.world())
        .map(|(entity, presentation)| (entity, presentation.id))
        .collect::<Vec<_>>();
    for (entity, id) in panels {
        tiling.assign(id, if entity == files { 1 } else { editor_area });
        app.world_mut()
            .get_mut::<PanelPresentation>(entity)
            .unwrap()
            .dock_next = true;
    }
    app.world_mut().resource_mut::<WorkbenchLayout>().tiling = tiling;
    app.world_mut()
        .resource_mut::<bed_workbench::WorkbenchAppearance>()
        .animations = false;
    app.world_mut()
        .resource_mut::<bed_plugin_file_explorer::ExplorerPresentation>()
        .tree
        .animations = false;
    app.world_mut()
        .non_send_mut::<bed_plugin_file_explorer::ExplorerState>()
        .explorer_mut()
        .file_tree
        .preferences
        .hide_hidden = true;
    bed_app::focus_panel(app.world_mut(), panel, true).unwrap();
    bed_app::focus_panel(app.world_mut(), files, true).unwrap();
    let mut context = Context::create();
    editor_host::prepare_ui(app.world_mut(), &mut context).unwrap();
    context.io_mut().set_display_size([1200., 800.]);
    context.io_mut().set_delta_time(1. / 60.);
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    let mut fixture = Fixture {
        app,
        context,
        files,
        _directory: directory,
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    let rows = loop {
        let (rows, _) = fixture.frame();
        if rows.len() == 3 {
            break rows;
        }
        assert!(
            Instant::now() < deadline,
            "workspace tree never populated: {rows:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    fixture.settle();
    let row = &rows[1];
    let menu = fixture.click(
        [row.min[0] + 40., (row.min[1] + row.max[1]) * 0.5],
        MouseButton::Right,
    );
    assert!(
        menu.contains("Open With"),
        "file menu lacks Open With: {menu}"
    );
    assert!(
        !menu.contains("Open in SQLite Viewer"),
        "SQLite action leaked into Markdown file menu: {menu}"
    );
    let open_with = fixture.popup_item(0, None);
    fixture.click(open_with, MouseButton::Left);
    let menu = fixture.frame().1;
    assert!(
        menu.contains("Markdown Preview"),
        "Markdown choice missing: {menu}"
    );
    assert!(
        !menu.contains("SQLite Viewer"),
        "SQLite viewer offered for Markdown: {menu}"
    );
    // Text remains the default; Markdown is the next compatible presentation.
    fixture.click(fixture.popup_item(1, Some(1)), MouseButton::Left);
    assert_eq!(
        fixture.app.world().resource::<WorkbenchLayout>().focused,
        Some(panel),
        "Open With duplicated the document's tab"
    );
    assert_eq!(
        fixture
            .app
            .world()
            .resource::<WorkbenchLayout>()
            .tiling
            .area_for(
                fixture
                    .app
                    .world()
                    .get::<PanelPresentation>(panel)
                    .unwrap()
                    .id
            ),
        Some(editor_area)
    );
    let preview_frontend = fixture.active_frontend(panel);
    assert_ne!(preview_frontend, text_frontend);
    assert_eq!(
        fixture
            .app
            .world()
            .get::<bed_plugin_markdown_preview::MarkdownDocument>(preview_frontend)
            .unwrap()
            .0,
        document
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture
        .app
        .world()
        .non_send::<bed_plugin_markdown_preview::MarkdownStates>()
        .get(preview_frontend)
        .unwrap()
        .is_ready()
    {
        fixture.frame();
        assert!(
            Instant::now() < deadline,
            "Markdown preview never parsed the shared document"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    fixture.settle();
    let geometry = fixture
        .app
        .world()
        .get::<bed_workbench::api::PanelGeometry>(panel)
        .unwrap();
    let preview_point = [geometry.content_min[0] + 80., geometry.content_min[1] + 80.];
    let menu = fixture.click(preview_point, MouseButton::Right);
    assert!(
        menu.contains("Edit as Text"),
        "preview lacks text switch: {menu}"
    );
    fixture.click(fixture.popup_item(0, Some(0)), MouseButton::Left);
    assert_eq!(
        fixture.active_frontend(panel),
        text_frontend,
        "switch did not retain the text frontend"
    );
    let menu = fixture
        .app
        .world()
        .non_send::<bed_plugin_editor::EditorViews>()
        .text(text_frontend)
        .unwrap();
    let text_point = [
        menu.presentation().layout.text_pos[0] + 20.,
        menu.presentation().layout.text_pos[1] + 5.,
    ];
    let menu = fixture.click(text_point, MouseButton::Right);
    assert!(
        menu.contains("View as Markdown Preview"),
        "text editor lacks Markdown switch: {menu}"
    );
    assert!(
        !menu.contains("SQLite Viewer"),
        "text editor offered SQLite for Markdown: {menu}"
    );
    fixture.click(fixture.popup_item(0, Some(0)), MouseButton::Left);
    assert_eq!(
        fixture.active_frontend(panel),
        preview_frontend,
        "switch did not retain the preview frontend"
    );
    let snapshot = fixture
        .app
        .world()
        .non_send::<DocumentSession>()
        .snapshot(document)
        .unwrap();
    assert_eq!(snapshot.bytes, unsaved);
    assert!(snapshot.dirty);
    assert_eq!(std::fs::read(&markdown).unwrap(), disk);
    assert_eq!(
        fixture
            .app
            .world()
            .non_send::<DocumentSession>()
            .view_count(document),
        1
    );
    let registry = fixture
        .app
        .world()
        .resource::<bed_api::features::FeatureRegistry>();
    assert!(
        registry
            .compatible_viewers(
                root.join("data.sqlite").to_str().unwrap(),
                Some(bed_api::documents::DocumentKind::Bytes)
            )
            .iter()
            .any(|viewer| viewer.id == bed_plugin_sqlite_browser::VIEWER_ID),
        "SQLite viewer should remain available for matching files"
    );
}
