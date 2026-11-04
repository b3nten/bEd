use super::*;
use crate::view::{Intent, apply};
use bedtime_document::TextBuffer;
use bedtime_workbench::{
    InArea, LayoutsWorldExt, Notifications, Panel, PanelInstance, SplitAxis, ViewersAppExt,
    WorkbenchCorePlugins,
};

/// A temporary folder, removed when dropped.
struct Project(PathBuf);
impl Project {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bedtime-filetree-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("src/nested")).unwrap();
        std::fs::create_dir_all(path.join("Docs")).unwrap();
        std::fs::write(path.join("b.txt"), "b").unwrap();
        std::fs::write(path.join("A.txt"), "a").unwrap();
        std::fs::write(path.join("src/main.rs"), "fn main() {}").unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[bedtime_ui::panel]
#[derive(Default)]
struct TextView;
impl Panel for TextView {
    const LABEL: &'static str = "Text";
    fn draw(_: &mut bedtime_ui::egui::Ui, _: &mut World, _: &bedtime_ui::UiContext<'_>, _: Entity) {
    }
}

/// A workbench showing `project` in a file tree, in a small area beside a big one.
fn app(project: &Project) -> (App, Entity) {
    let mut app = App::new();
    app.add_plugins((WorkbenchCorePlugins, FileTreePlugin))
        .insert_resource(Root(project.0.clone()))
        .register_viewer::<TextBuffer, TextView>("Text")
        .formats([bedtime_workbench::TEXT]);
    let world = app.world_mut();
    let layout = world.layouts_mut().create("Main").unwrap();
    world.layouts_mut().activate(layout).unwrap();
    let main = world.layouts().destination_area(layout).unwrap();
    let side = world
        .layouts_mut()
        .split_area(main, SplitAxis::Horizontal, 2500, [100, 100])
        .unwrap();
    let small = if world.layouts().destination_area(layout).unwrap() == main {
        side
    } else {
        main
    };
    let tree = world.spawn((FileTree::default(), InArea(small))).id();
    world.flush();
    world.layouts_mut().focus_panel(tree).unwrap();
    (app, tree)
}

/// The tree's rows once `done` holds, as indented names.
fn eventually(app: &mut App, tree: Entity, done: impl Fn(&[String]) -> bool) -> Vec<String> {
    let mut names = Vec::new();
    for _ in 0..400 {
        app.update();
        let world = app.world_mut();
        let root = root(world).unwrap();
        let mut view = world.entity_mut(tree).take::<TreeView>().unwrap();
        let filter = world
            .get::<FileTree>(tree)
            .unwrap()
            .filter(world.resource::<FileTreeDefaults>());
        refresh_rows(
            &root,
            world.get::<FileTree>(tree).unwrap(),
            world.resource::<Folders>(),
            filter,
            &mut view,
        );
        names = view
            .rows()
            .iter()
            .map(|row| format!("{}{}", "  ".repeat(row.depth), row.name))
            .collect();
        world.entity_mut(tree).insert(view);
        if done(&names) {
            return names;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("rows never got there: {names:?}");
}

#[test]
fn folders_are_listed_folders_first_and_open_folders_are_read() {
    let project = Project::new("rows");
    let (mut app, tree) = app(&project);
    let rows = eventually(&mut app, tree, |rows| rows.len() > 1);
    let root = project.0.file_name().unwrap().to_string_lossy();
    assert_eq!(rows, [&*root, "  Docs", "  src", "  A.txt", "  b.txt"]);

    apply(
        app.world_mut(),
        tree,
        &project.0,
        Intent::Toggle(project.0.join("src")),
    );
    let rows = eventually(&mut app, tree, |rows| rows.len() > 5);
    assert_eq!(rows[3..5], ["    nested", "    main.rs"]);
    assert!(
        app.world()
            .resource::<Folders>()
            .entries(&project.0.join("src"))
            .is_some()
    );

    // Closing the root lets its listing go.
    apply(
        app.world_mut(),
        tree,
        &project.0,
        Intent::Toggle(project.0.clone()),
    );
    apply(
        app.world_mut(),
        tree,
        &project.0,
        Intent::Toggle(project.0.join("src")),
    );
    let rows = eventually(&mut app, tree, |rows| rows.len() == 1);
    assert_eq!(rows, [&*root]);
    assert!(
        app.world()
            .resource::<Folders>()
            .entries(&project.0)
            .is_none()
    );
}

#[test]
fn hidden_and_ignored_files_show_dimmed_until_hidden() {
    let project = Project::new("ignored");
    std::fs::create_dir_all(project.0.join(".git")).unwrap();
    std::fs::create_dir_all(project.0.join("target/debug")).unwrap();
    std::fs::write(project.0.join(".gitignore"), "target/\n*.log\n").unwrap();
    std::fs::write(project.0.join("build.log"), "").unwrap();
    std::fs::write(project.0.join("Docs/.secret"), "").unwrap();
    let (mut app, tree) = app(&project);
    // Hidden files don't show by default: start with both shown.
    app.insert_resource(FileTreeDefaults {
        show_hidden: true,
        show_ignored: true,
    });
    apply(
        app.world_mut(),
        tree,
        &project.0,
        Intent::Toggle(project.0.join("target")),
    );
    apply(
        app.world_mut(),
        tree,
        &project.0,
        Intent::Toggle(project.0.join("Docs")),
    );
    let all = eventually(&mut app, tree, |rows| {
        rows.iter().any(|row| row == "    debug") && rows.iter().any(|row| row == "    .secret")
    });
    let view = app.world().get::<TreeView>(tree).unwrap();
    let row = |name: &str| view.rows().iter().find(|row| row.name == name).unwrap();
    assert!(row("build.log").ignored && !row("build.log").hidden);
    // What's in an ignored folder is ignored too.
    assert!(row("target").ignored && row("debug").ignored);
    assert!(row(".gitignore").hidden && !row(".gitignore").ignored);
    assert!(!row("A.txt").ignored && !row("src").ignored);
    assert!(all.iter().any(|row| row == "  .git"));

    apply(app.world_mut(), tree, &project.0, Intent::ToggleIgnored);
    let rows = eventually(&mut app, tree, |rows| {
        !rows.iter().any(|row| row == "  target")
    });
    assert!(
        !rows
            .iter()
            .any(|row| row.contains("build.log") || row.contains("debug"))
    );
    assert!(rows.iter().any(|row| row == "  .gitignore"));

    apply(app.world_mut(), tree, &project.0, Intent::ToggleHidden);
    let rows = eventually(&mut app, tree, |rows| {
        !rows.iter().any(|row| row == "  .gitignore")
    });
    assert!(
        !rows
            .iter()
            .any(|row| row.contains(".git") || row.contains(".secret"))
    );
    assert!(rows.iter().any(|row| row == "  A.txt"));
    let saved = app.world().get::<FileTree>(tree).unwrap();
    assert_eq!(
        (saved.show_hidden, saved.show_ignored),
        (Some(false), Some(false))
    );

    // Outside a Git repository nothing is ignored.
    std::fs::remove_dir_all(project.0.join(".git")).unwrap();
    apply(app.world_mut(), tree, &project.0, Intent::ToggleIgnored);
    app.world_mut().resource_mut::<Folders>().refresh();
    for _ in 0..400 {
        eventually(&mut app, tree, |rows| {
            rows.iter().any(|row| row == "  build.log")
        });
        let view = app.world().get::<TreeView>(tree).unwrap();
        if view.rows().iter().all(|row| !row.ignored) {
            return;
        }
    }
    panic!("ignored files stayed ignored without a repository");
}

#[test]
fn changes_on_disk_show_up() {
    let project = Project::new("watch");
    let (mut app, tree) = app(&project);
    eventually(&mut app, tree, |rows| rows.len() > 1);
    std::fs::write(project.0.join("new.txt"), "").unwrap();
    std::fs::remove_file(project.0.join("A.txt")).unwrap();
    eventually(&mut app, tree, |rows| {
        rows.iter().any(|row| row == "  new.txt") && !rows.iter().any(|row| row == "  A.txt")
    });
}

#[test]
fn a_preview_opens_beside_the_tree_and_leaves_it_focused() {
    let project = Project::new("open");
    let (mut app, tree) = app(&project);
    let world = app.world_mut();
    let open = |path: PathBuf, preview| Intent::Open {
        path,
        preview,
        viewer: None,
    };
    apply(world, tree, &project.0, open(project.0.join("A.txt"), true));
    let viewers: Vec<Entity> = world
        .query_filtered::<Entity, With<TextView>>()
        .iter(world)
        .collect();
    assert_eq!(viewers.len(), 1);
    let viewer = viewers[0];
    assert_ne!(
        world.get::<InArea>(viewer).unwrap().0,
        world.get::<InArea>(tree).unwrap().0
    );
    assert!(world.get::<PanelInstance>(viewer).unwrap().preview);
    let layout = world.layouts().active().unwrap();
    assert_eq!(world.layouts().focused_panel(layout), Some(tree));

    // Opening it for good pins it and moves focus to it.
    apply(
        world,
        tree,
        &project.0,
        open(project.0.join("A.txt"), false),
    );
    assert!(!world.get::<PanelInstance>(viewer).unwrap().preview);
    assert_eq!(world.layouts().focused_panel(layout), Some(viewer));

    // Nothing opens a binary file here (only text): the user is told.
    std::fs::write(project.0.join("blob.bin"), [0u8, 1]).unwrap();
    apply(
        world,
        tree,
        &project.0,
        open(project.0.join("blob.bin"), true),
    );
    let latest = world.resource::<Notifications>().latest().unwrap();
    assert!(
        latest.text.starts_with("Couldn't open blob.bin"),
        "{}",
        latest.text
    );
}
