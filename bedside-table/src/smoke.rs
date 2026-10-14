//! Seeded native proof for the independent application, including a fresh restart.
use super::*;
use bedtime::desktop::winit::event::MouseButton;
use bedtime::desktop::winit::keyboard::{Key, KeyCode, NamedKey};
use bedtime::desktop::{
    DesktopCaptureId, DesktopCaptureRequests, DesktopCaptureResults, DesktopCaptureTarget,
    DesktopFrameInfo, DesktopInjectedEvent, DesktopInputId, DesktopInputRequests,
    DesktopInputResults,
};
use bedtime_workbench::WorkbenchRequests;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

pub const SMOKE_SQL: &str = "SELECT count(*) FROM items";

/// Fixtures are deliberately writable seed databases; runtime inspection uses
/// the read-only service. Never call this for a user-supplied database.
pub fn seed_databases(root: &Path) -> io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(root)?;
    let mut paths = Vec::new();
    for (name, count) in [("first.sqlite", 625), ("second.sqlite", 3)] {
        let path = root.join(name);
        let mut connection = rusqlite::Connection::open(&path).map_err(io::Error::other)?;
        let transaction = connection.transaction().map_err(io::Error::other)?;
        transaction.execute_batch("CREATE TABLE IF NOT EXISTS categories(id INTEGER PRIMARY KEY, name TEXT); CREATE TABLE IF NOT EXISTS items(id INTEGER PRIMARY KEY, category INTEGER REFERENCES categories(id), name TEXT, note TEXT, payload BLOB); CREATE VIEW IF NOT EXISTS names AS SELECT id, name FROM items; DELETE FROM items; DELETE FROM categories; INSERT INTO categories VALUES(1, 'Things');").map_err(io::Error::other)?;
        for row in 1..=count {
            transaction
                .execute(
                    "INSERT INTO items VALUES (?1, 1, ?2, NULL, x'0001FF')",
                    rusqlite::params![row, format!("Item {row} · café")],
                )
                .map_err(io::Error::other)?;
        }
        transaction.commit().map_err(io::Error::other)?;
        paths.push(path);
    }
    Ok(paths)
}

#[derive(Resource)]
struct NativeSmoke {
    reopen: bool,
    phase: u8,
    deadline: Instant,
    query: Option<Entity>,
    input: Option<DesktopInputId>,
    capture: Option<DesktopCaptureId>,
    input_center: Option<[f32; 2]>,
    capture_path: Option<PathBuf>,
    verified: Arc<AtomicBool>,
}

pub fn install(app: &mut App, reopen: bool, capture_path: Option<PathBuf>) -> Arc<AtomicBool> {
    let verified = Arc::new(AtomicBool::new(false));
    app.insert_resource(NativeSmoke {
        reopen,
        phase: 0,
        deadline: Instant::now() + Duration::from_secs(60),
        query: None,
        input: None,
        capture: None,
        input_center: None,
        capture_path,
        verified: Arc::clone(&verified),
    })
    .add_systems(Last, advance.before(autosave_session));
    verified
}

fn submit(world: &mut World, command: &str, arguments: Value, source: Option<Entity>) {
    let mut invocation = bedtime_workbench::capture_command(world, command, arguments);
    invocation.source = source;
    world
        .resource_mut::<WorkbenchRequests>()
        .push(WorkbenchAction::Invoke(invocation));
}

fn shortcut(world: &mut World, physical: KeyCode, logical: Key, shift: bool) -> DesktopInputId {
    use bedtime::desktop::DesktopModifiers;
    let modifiers = DesktopModifiers {
        shift,
        super_key: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        ..Default::default()
    };
    world.resource_mut::<DesktopInputRequests>().submit(vec![
        vec![DesktopInjectedEvent::Modifiers(modifiers)].into(),
        vec![DesktopInjectedEvent::Key {
            physical,
            logical: logical.clone(),
            pressed: true,
        }]
        .into(),
        vec![
            DesktopInjectedEvent::Key {
                physical,
                logical,
                pressed: false,
            },
            DesktopInjectedEvent::Modifiers(Default::default()),
        ]
        .into(),
    ])
}

fn advance(world: &mut World) {
    let mut smoke = world.remove_resource::<NativeSmoke>().unwrap();
    assert!(
        Instant::now() < smoke.deadline,
        "Bedside native smoke timed out at phase {}",
        smoke.phase
    );
    let frame = world.resource::<DesktopFrameInfo>().clone();
    match smoke.phase {
        0 if frame.frame.0 >= 8
            && world.resource::<Connections>().entries.len() == 2
            && world
                .resource::<Connections>()
                .entries
                .iter()
                .all(|connection| connection.schema.is_some()) =>
        {
            if smoke.reopen {
                assert_eq!(world.resource::<DataPanels>().0.len(), 2);
                assert_eq!(world.resource::<QueryPanels>().0.len(), 1);
                let state = world.resource::<QueryPanels>().0.values().next().unwrap();
                assert_eq!(
                    bedtime_core::model::<QueryDocument>(world, state.document)
                        .unwrap()
                        .sql,
                    SMOKE_SQL
                );
                assert!(
                    state.serial.is_none() && state.result.is_none(),
                    "Restored SQL executed automatically"
                );
                smoke.query = Some(*world.resource::<QueryPanels>().0.keys().next().unwrap());
                smoke.phase = 6;
            } else {
                let ids = world
                    .resource::<Connections>()
                    .entries
                    .iter()
                    .map(|connection| connection.record.id)
                    .collect::<Vec<_>>();
                for (id, command) in [
                    (ids[0], "bedside.open-data"),
                    (ids[0], "bedside.open-schema"),
                    (ids[1], "bedside.open-data"),
                ] {
                    submit(
                        world,
                        command,
                        json!({"connection":id.0,"table":"items"}),
                        None,
                    );
                }
                submit(
                    world,
                    "bedside.new-query",
                    json!({"connection":ids[0].0}),
                    None,
                );
                smoke.phase = 1;
            }
        }
        1 => {
            if let Some((&entity, state)) = world.resource::<QueryPanels>().0.iter().next()
                && let Some(center) = state.input_center
            {
                smoke.query = Some(entity);
                smoke.input_center = Some(center);
                smoke.capture = Some(world.resource_mut::<DesktopCaptureRequests>().submit(
                    vec![DesktopCaptureTarget::Window],
                    smoke.deadline,
                    Some(frame.frame),
                ));
                smoke.phase = 8;
            }
        }
        8 => {
            if let Some(result) = world
                .resource_mut::<DesktopCaptureResults>()
                .take(smoke.capture.unwrap())
            {
                let captured = result.expect("Bedside input target capture failed");
                if let Some(path) = &smoke.capture_path {
                    captured.images[0]
                        .rgba
                        .write_png(path.with_file_name("bedside-before-input.png"))
                        .unwrap();
                }
                smoke.input = Some(world.resource_mut::<DesktopInputRequests>().submit(vec![
                        vec![DesktopInjectedEvent::PointerMoved(
                            smoke.input_center.unwrap(),
                        )]
                        .into(),
                        vec![DesktopInjectedEvent::PointerButton {
                            button: MouseButton::Left,
                            pressed: true,
                        }]
                        .into(),
                        vec![DesktopInjectedEvent::PointerButton {
                            button: MouseButton::Left,
                            pressed: false,
                        }]
                        .into(),
                    ]));
                smoke.phase = 2;
            }
        }
        2 => {
            if let Some(result) = world
                .resource_mut::<DesktopInputResults>()
                .take(smoke.input.unwrap())
            {
                result.expect("Native query click failed");
                let entity = smoke.query.unwrap();
                assert!(
                    world.resource::<QueryPanels>().0[&entity].input_active,
                    "Query input did not focus at {:?}",
                    smoke.input_center
                );
                smoke.input = Some(world.resource_mut::<DesktopInputRequests>().submit(vec![
                    vec![DesktopInjectedEvent::Text(SMOKE_SQL.into())].into(),
                ]));
                smoke.phase = 9;
            }
        }
        9 => {
            if let Some(result) = world
                .resource_mut::<DesktopInputResults>()
                .take(smoke.input.unwrap())
            {
                result.expect("Native query text failed");
                let entity = smoke.query.unwrap();
                let document = world.get::<DocumentBinding>(entity).unwrap().0;
                assert_eq!(
                    bedtime_core::model::<QueryDocument>(world, document)
                        .unwrap()
                        .sql,
                    SMOKE_SQL
                );
                assert!(bedtime_core::document_dirty(world, document).unwrap());
                smoke.input = Some(shortcut(
                    world,
                    KeyCode::KeyZ,
                    Key::Character("z".into()),
                    false,
                ));
                smoke.phase = 3;
            }
        }
        3 => {
            let entity = smoke.query.unwrap();
            let state = &world.resource::<QueryPanels>().0[&entity];
            let document = state.document;
            if let Some(result) = world
                .resource_mut::<DesktopInputResults>()
                .take(smoke.input.unwrap())
            {
                result.expect("Native Undo shortcut failed");
                assert!(
                    bedtime_core::model::<QueryDocument>(world, document)
                        .unwrap()
                        .sql
                        .is_empty()
                );
                assert!(world.resource::<QueryPanels>().0[&entity].buffer.is_empty());
                smoke.input = Some(shortcut(
                    world,
                    KeyCode::KeyZ,
                    Key::Character("z".into()),
                    true,
                ));
                smoke.phase = 4;
            }
        }
        4 => {
            let entity = smoke.query.unwrap();
            let document = world.resource::<QueryPanels>().0[&entity].document;
            if let Some(result) = world
                .resource_mut::<DesktopInputResults>()
                .take(smoke.input.unwrap())
            {
                result.expect("Native Redo shortcut failed");
                assert_eq!(
                    bedtime_core::model::<QueryDocument>(world, document)
                        .unwrap()
                        .sql,
                    SMOKE_SQL
                );
                assert_eq!(world.resource::<QueryPanels>().0[&entity].buffer, SMOKE_SQL);
                smoke.input = Some(shortcut(
                    world,
                    KeyCode::Enter,
                    Key::Named(NamedKey::Enter),
                    false,
                ));
                smoke.phase = 5;
            }
        }
        5 => {
            if let Some(input) = smoke.input
                && let Some(result) = world.resource_mut::<DesktopInputResults>().take(input)
            {
                result.expect("Native Run Query shortcut failed");
                smoke.input = None;
            }
            let entity = smoke.query.unwrap();
            let state = &world.resource::<QueryPanels>().0[&entity];
            let document = state.document;
            if let Some(result) = &state.result
                && smoke.input.is_none()
            {
                assert_eq!(result.rows[0][0], bedtime_sqlite::Cell::Integer(625));
                submit(
                    world,
                    "bedtime.documents.save",
                    json!({"document":document.0}),
                    Some(entity),
                );
                smoke.phase = 6;
            }
        }
        6 => {
            let entity = smoke.query.unwrap();
            let state = &world.resource::<QueryPanels>().0[&entity];
            if !bedtime_core::document_dirty(world, state.document).unwrap()
                && world
                    .resource::<DataPanels>()
                    .0
                    .values()
                    .all(|state| state.result.is_some())
            {
                assert_eq!(
                    world.resource::<MetadataStore>().queries().unwrap()[0].sql,
                    SMOKE_SQL
                );
                smoke.capture = Some(world.resource_mut::<DesktopCaptureRequests>().submit(
                    vec![DesktopCaptureTarget::Window],
                    smoke.deadline,
                    Some(frame.frame),
                ));
                smoke.phase = 7;
            }
        }
        7 => {
            if let Some(result) = world
                .resource_mut::<DesktopCaptureResults>()
                .take(smoke.capture.unwrap())
            {
                let captured = result.expect("Bedside GPU capture failed");
                let image = &captured.images[0].rgba;
                assert!(image.size.iter().all(|size| *size > 0));
                if let Some(path) = &smoke.capture_path {
                    image.write_png(path).unwrap();
                }
                smoke.verified.store(true, Ordering::SeqCst);
                submit(world, "bedtime.core.quit", Value::Null, None);
                smoke.phase = 8;
            }
        }
        _ => {}
    }
    world.insert_resource(smoke);
}
