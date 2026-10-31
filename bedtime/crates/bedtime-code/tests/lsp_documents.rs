//! Language servers for workbench documents, against the fake server in
//! `lsp_server.py`.
#![cfg(unix)]
use bedtime_code::{
    CodePlugin, EditorPanel,
    lsp::{
        DocumentServer, Intelligence, LanguageQuery, LanguageServer, LanguageServers, QueryKind,
        ServerConfig, ServerStatus, completion_context, go_to_definition, query_is_current,
        request_query, take_query_answers,
    },
};
use bedtime_document::{BufferEdit, TextBuffer};
use bedtime_workbench::{
    DocumentReference, EditOptions, LayoutsWorldExt, PathOptions, Root, ViewerFile,
    WorkbenchCorePlugins, WorkbenchWorldExt,
};
use bevy::prelude::*;
use std::{path::PathBuf, time::Duration};

fn until(app: &mut App, what: &str, mut done: impl FnMut(&mut World) -> bool) {
    for _ in 0..2000 {
        app.update();
        if done(app.world_mut()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("never: {what}");
}

fn server(world: &World, panel: Entity) -> Option<&DocumentServer> {
    let document = world.get::<DocumentReference>(panel)?.document();
    world.get::<DocumentServer>(document.entity())
}

#[test]
fn shown_files_sync_get_diagnostics_and_go_to_definitions() {
    let root = std::env::temp_dir().join(format!("bedtime-code-lsp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    std::fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join("target.rs"), "fn target() {}\n").unwrap();

    let mut app = App::new();
    app.add_plugins((WorkbenchCorePlugins, CodePlugin))
        .insert_resource(Root(root.clone()))
        .insert_resource(LanguageServers(
            [(
                "rust".to_owned(),
                ServerConfig {
                    program: "python3".into(),
                    arguments: vec![
                        format!("{}/tests/lsp_server.py", env!("CARGO_MANIFEST_DIR")),
                        "cross-file".into(),
                        root.join("transcript.jsonl").to_string_lossy().into(),
                    ],
                    ..default()
                },
            )]
            .into(),
        ));
    let world = app.world_mut();
    let layout = world.layouts_mut().create("Test").unwrap();
    world.layouts_mut().activate(layout).unwrap();
    let panel = world
        .workbench_mut()
        .open_path(&root.join("main.rs"), PathOptions::default())
        .unwrap()
        .unwrap();

    // Opened: the server reports a diagnostic for the first revision.
    until(&mut app, "a diagnostic", |world| {
        server(world, panel)
            .and_then(|server| server.diagnostics.as_ref())
            .is_some_and(|report| report.items().len() == 1)
    });
    let connection = server(app.world(), panel).unwrap();
    let status = app
        .world()
        .get::<LanguageServer>(connection.server)
        .unwrap()
        .status();
    assert_eq!(status, ServerStatus::Ready);

    // An edit is sent; the server clears its diagnostics.
    let world = app.world_mut();
    let document = world.get::<DocumentReference>(panel).unwrap().document();
    let handle = world
        .workbench()
        .cast_document::<TextBuffer>(document)
        .unwrap();
    world
        .workbench_mut()
        .edit_document(
            handle,
            0,
            BufferEdit::new(0..0, "// x\n"),
            EditOptions::new("Typing"),
        )
        .unwrap();
    until(&mut app, "diagnostics cleared", |world| {
        server(world, panel)
            .and_then(|server| server.diagnostics.as_ref())
            .is_some_and(|report| report.items().is_empty())
    });

    // Hover and completion answer for the current revision. (This fixture's
    // first hover is empty.)
    let ask = |app: &mut App, query: LanguageQuery| {
        let kind = query.kind();
        let origin = request_query(app.world_mut(), panel, query).unwrap();
        let mut answer = None;
        until(app, "an answer", |world| {
            answer = take_query_answers(world)
                .into_iter()
                .find(|answer| answer.origin == origin && answer.kind == kind)
                .map(|answer| answer.result.unwrap());
            answer.is_some()
        });
        answer.unwrap()
    };
    ask(&mut app, LanguageQuery::Hover { byte: 6 });
    let Intelligence::Hover(Some(hover)) = ask(&mut app, LanguageQuery::Hover { byte: 6 }) else {
        panic!("no hover");
    };
    assert!(hover.contents.text.contains("fn first()"));
    let completion = LanguageQuery::Completion {
        byte: 5,
        context: completion_context(None, false),
    };
    let Intelligence::Completion(list) = ask(&mut app, completion) else {
        panic!("no completions");
    };
    assert_eq!(list.items[0].label, "first");
    // A newer query of the same kind replaces an older one.
    let older = request_query(app.world_mut(), panel, LanguageQuery::Hover { byte: 6 }).unwrap();
    request_query(app.world_mut(), panel, LanguageQuery::Hover { byte: 7 }).unwrap();
    assert!(!query_is_current(app.world(), &older, QueryKind::Hover));

    // The definition is in another file: it opens.
    go_to_definition(app.world_mut(), panel, 0).unwrap();
    let target: PathBuf = root.join("target.rs");
    until(&mut app, "the definition opens", |world| {
        world
            .query_filtered::<&ViewerFile, With<EditorPanel>>()
            .iter(world)
            .any(|file| file.0 == target)
    });

    // Closed (and saved, so nothing unsaved keeps it): the document leaves
    // the server.
    app.world_mut()
        .workbench_mut()
        .save_document(document)
        .unwrap();
    until(&mut app, "saved", |world| {
        world
            .workbench()
            .inspect_document(document)
            .is_ok_and(|info| !info.dirty)
    });
    app.world_mut().layouts_mut().close_panel(panel).unwrap();
    until(&mut app, "the document is closed", |world| {
        world.get::<DocumentServer>(document.entity()).is_none()
    });
    let _ = std::fs::remove_dir_all(&root);
}
