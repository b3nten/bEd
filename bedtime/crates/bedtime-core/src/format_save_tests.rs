//! Format-on-save behavior is shared editing policy, independent of bEd.
use super::*;
use bedtime_api::features::HostEffects;
use bedtime_documents::{ClosePolicy, DocumentId, SessionOptions};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

fn begin_save(world: &mut World, document: DocumentId) -> HostRequestId {
    let receipt = submit_host_request(world, None, HostAction::Save { document });
    resolve_documents(world);
    assert!(
        take_host_result(world, receipt).is_none(),
        "formatting saves complete after the language server replies"
    );
    receipt
}

const SERVER: &str = r#"#!/usr/bin/env python3
import json, sys, time
from pathlib import Path
mode, trace = sys.argv[1:]
def send(value):
    body = json.dumps(value).encode()
    sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n' % len(body)).encode() + body)
    sys.stdout.buffer.flush()
while True:
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if line == b'\r\n': break
        if line.lower().startswith(b'content-length:'): length = int(line.split(b':', 1)[1])
    msg = json.loads(sys.stdin.buffer.read(length))
    method = msg.get('method')
    if method == 'initialize':
        send({'jsonrpc':'2.0','id':msg['id'],'result':{'capabilities':{'textDocumentSync':2,'documentFormattingProvider':True}}})
    elif method == 'textDocument/formatting':
        with open(trace, 'a') as out: out.write('format\n')
        behavior = Path(mode).read_text()
        time.sleep(4 if behavior == 'timeout' else .15)
        if behavior == 'error':
            send({'jsonrpc':'2.0','id':msg['id'],'error':{'code':-32603,'message':'formatter unavailable'}})
        else:
            send({'jsonrpc':'2.0','id':msg['id'],'result':[{'range':{'start':{'line':0,'character':0},'end':{'line':0,'character':12}},'newText':'fn main() {}'}]})
    elif method == 'shutdown':
        send({'jsonrpc':'2.0','id':msg['id'],'result':None})
    elif method == 'exit': break
"#;

fn pump(app: &mut App, predicate: impl Fn(&World) -> bool) {
    let until = Instant::now() + Duration::from_secs(8);
    while !predicate(app.world()) {
        poll_document_services(app.world_mut());
        resolve_documents(app.world_mut());
        assert!(Instant::now() < until, "save or language server timed out");
        thread::sleep(Duration::from_millis(5));
    }
}

struct FormatterFixture {
    _dir: TempDir,
    app: App,
    path: PathBuf,
    mode: PathBuf,
    trace: PathBuf,
    panel: Entity,
    document: DocumentId,
}

fn formatter_fixture() -> FormatterFixture {
    let dir = TempDir::new();
    let server = dir.write("server.py", SERVER.as_bytes());
    fs::set_permissions(&server, fs::Permissions::from_mode(0o755)).unwrap();
    let mode = dir.write("mode", b"normal");
    let trace = dir.path("trace");
    let config = dir.write("config/lsp.json", &serde_json::to_vec(&json!({
        "languages":[{"name":"rust","language_id":"rust","file_types":["rs"],"language_server":"fixture"}],
        "language_servers":{"fixture":{"command":server,"args":[mode,trace]}}
    })).unwrap());
    let mut app = super::app();
    bedtime_api::settings::register(
        app.world_mut(),
        "bedtime.editor",
        vec![bedtime_api::settings::SettingDefinition::new(
            "lsp_format_on_save",
            "Format on save",
            true,
        )],
    )
    .unwrap();
    settings::initialize_settings(app.world_mut(), None).unwrap();
    settings::finalize_settings(app.world_mut()).unwrap();
    let session = DocumentSession::with_options(SessionOptions {
        project_root: Some(dir.root().into()),
        lsp_config: Some(config),
        ..Default::default()
    })
    .unwrap();
    app.world_mut().insert_non_send(session);
    settings::configure_document_settings(app.world_mut()).unwrap();
    let path = dir.write("main.rs", b"fn main( ){}\n");
    let document = app
        .world_mut()
        .non_send_mut::<DocumentSession>()
        .open_file(&path)
        .unwrap();
    let panel =
        open_document_panel(app.world_mut(), "test.panel", document, Value::Null, None).unwrap();
    let view = app
        .world_mut()
        .non_send_mut::<DocumentSession>()
        .create_view(document)
        .unwrap();
    app.world_mut().get_mut::<PanelBinding>(panel).unwrap().view = Some(view);
    pump(&mut app, |world| {
        world
            .non_send::<DocumentSession>()
            .lsp()
            .and_then(|pool| pool.client_for_document(document))
            .is_some_and(|client| client.borrow().is_initialized())
    });
    FormatterFixture {
        _dir: dir,
        app,
        path,
        mode,
        trace,
        panel,
        document,
    }
}

#[test]
fn pending_panel_draft_wins_over_formatting_reply() {
    let FormatterFixture {
        _dir,
        mut app,
        path,
        panel,
        document,
        ..
    } = formatter_fixture();
    let revision = app
        .world()
        .non_send::<DocumentSession>()
        .document_revision(document)
        .unwrap();
    let request = begin_save(app.world_mut(), document);

    #[derive(Component)]
    struct PendingDraft {
        document: DocumentId,
        revision: bedtime_api::documents::Revision,
    }
    let frontend = panel;
    let commit = app.world_mut().register_system(
        |In(invocation): In<bedtime_api::workbench::PanelActionInvocation>, world: &mut World| {
            if invocation.action == bedtime_api::workbench::PanelAction::CommitEdit
                && let Some(draft) = world.entity_mut(invocation.entity).take::<PendingDraft>()
            {
                world.resource_mut::<HostEffects>().push(
                    Some(invocation.entity),
                    HostAction::ApplyEdits {
                        document: draft.document,
                        revision: draft.revision,
                        edits: vec![ByteEdit {
                            range: 0..12,
                            bytes: b"fn draft(){}".to_vec(),
                        }],
                    },
                );
            }
            Ok(true)
        },
    );
    app.world_mut().entity_mut(frontend).insert((
        PendingDraft { document, revision },
        bedtime_api::workbench::PanelActions(commit),
    ));
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .document_revision(document)
            .unwrap(),
        revision,
        "the late draft has not reached the canonical document"
    );

    wait_host_result(app.world_mut(), request).unwrap();
    assert!(app.world().get::<PendingDraft>(frontend).is_none());
    assert_eq!(fs::read(&path).unwrap(), b"fn draft(){}\n");
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"fn draft(){}\n"
    );
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .shutdown(ClosePolicy::Discard)
        .unwrap();
}

#[test]
fn rejected_late_panel_draft_prevents_formatting_save() {
    let FormatterFixture {
        _dir,
        mut app,
        path,
        panel,
        document,
        ..
    } = formatter_fixture();
    let request = begin_save(app.world_mut(), document);
    let frontend = panel;
    #[derive(Component)]
    struct RejectedDraft;
    let commit = app.world_mut().register_system(
        |In(_): In<bedtime_api::workbench::PanelActionInvocation>| {
            Err::<bool, String>("draft rejected".into())
        },
    );
    app.world_mut()
        .entity_mut(frontend)
        .insert((RejectedDraft, bedtime_api::workbench::PanelActions(commit)));

    let error = wait_host_result(app.world_mut(), request).unwrap_err();
    assert_eq!(error.to_string(), "draft rejected");
    assert!(app.world().get::<RejectedDraft>(frontend).is_some());
    assert_eq!(fs::read(&path).unwrap(), b"fn main( ){}\n");
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"fn main( ){}\n"
    );
    assert!(
        !app.world()
            .non_send::<DocumentSession>()
            .autosave_paused(document)
            .unwrap()
    );
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .shutdown(ClosePolicy::Discard)
        .unwrap();
}

#[test]
fn explicit_save_formats_but_autosave_bypasses_and_newer_edits_win() {
    let FormatterFixture {
        _dir,
        mut app,
        path,
        mode,
        trace,
        document,
        ..
    } = formatter_fixture();

    let mut notices = bevy::ecs::message::MessageCursor::<CoreNotice>::default();
    let request = begin_save(app.world_mut(), document);
    let duplicate = begin_save(app.world_mut(), document);
    assert_eq!(fs::read(&path).unwrap(), b"fn main( ){}\n");
    wait_host_result(app.world_mut(), request).unwrap();
    assert!(wait_host_result(app.world_mut(), duplicate).is_ok());
    assert_eq!(
        fs::read_to_string(&trace).unwrap().lines().count(),
        1,
        "concurrent explicit save receipts must share one formatting request"
    );
    assert_eq!(fs::read(&path).unwrap(), b"fn main() {}\n");
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .undo_document(document)
        .unwrap();
    assert_eq!(
        app.world()
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .bytes,
        b"fn main( ){}\n"
    );

    let request = begin_save(app.world_mut(), document);
    {
        let mut session = app.world_mut().non_send_mut::<DocumentSession>();
        let revision = session.document_revision(document).unwrap();
        session
            .apply_edits(
                document,
                revision,
                &[ByteEdit {
                    range: 0..12,
                    bytes: b"fn other(){}".to_vec(),
                }],
            )
            .unwrap();
    }
    wait_host_result(app.world_mut(), request).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"fn other(){}\n");

    fs::write(&mode, b"error").unwrap();
    let request = begin_save(app.world_mut(), document);
    wait_host_result(app.world_mut(), request).unwrap();
    assert!(
        notices
            .read(app.world().resource::<Messages<CoreNotice>>())
            .any(|notice| notice.message.contains("Formatting failed"))
    );
    assert_eq!(fs::read(&path).unwrap(), b"fn other(){}\n");

    let count = fs::read_to_string(&trace).unwrap().lines().count();
    settings::apply_settings_request(
        app.world_mut(),
        &bedtime_api::settings::SettingsRequest::batch(vec![
            bedtime_api::settings::SettingsChange::set(
                "bedtime.editor",
                "lsp_format_on_save",
                json!(false),
            ),
        ]),
    )
    .unwrap();
    settings::configure_document_settings(app.world_mut()).unwrap();
    let request = submit_host_request(app.world_mut(), None, HostAction::Save { document });
    resolve_documents(app.world_mut());
    assert!(take_host_result(app.world_mut(), request).unwrap().is_ok());
    assert_eq!(fs::read_to_string(&trace).unwrap().lines().count(), count);
    settings::apply_settings_request(
        app.world_mut(),
        &bedtime_api::settings::SettingsRequest::batch(vec![
            bedtime_api::settings::SettingsChange::set(
                "bedtime.editor",
                "lsp_format_on_save",
                json!(true),
            ),
            bedtime_api::settings::SettingsChange::set(
                "bedtime.documents",
                "autosave_delay_ms",
                json!(0),
            ),
        ]),
    )
    .unwrap();
    settings::configure_document_settings(app.world_mut()).unwrap();
    {
        let mut session = app.world_mut().non_send_mut::<DocumentSession>();
        let revision = session.document_revision(document).unwrap();
        session
            .apply_edits(
                document,
                revision,
                &[ByteEdit {
                    range: 0..12,
                    bytes: b"fn auto( ){}".to_vec(),
                }],
            )
            .unwrap();
    }
    pump(&mut app, |world| {
        !world
            .non_send::<DocumentSession>()
            .snapshot(document)
            .unwrap()
            .dirty
    });
    assert_eq!(fs::read(&path).unwrap(), b"fn auto( ){}\n");
    assert_eq!(fs::read_to_string(&trace).unwrap().lines().count(), count);
    settings::apply_settings_request(
        app.world_mut(),
        &bedtime_api::settings::SettingsRequest::batch(vec![
            bedtime_api::settings::SettingsChange::set(
                "bedtime.documents",
                "autosave",
                json!(false),
            ),
        ]),
    )
    .unwrap();
    settings::configure_document_settings(app.world_mut()).unwrap();
    fs::write(&mode, b"timeout").unwrap();
    let request = begin_save(app.world_mut(), document);
    wait_host_result(app.world_mut(), request).unwrap();
    assert!(
        notices
            .read(app.world().resource::<Messages<CoreNotice>>())
            .any(|notice| notice.message.contains("timed out"))
    );
    assert_eq!(fs::read(&path).unwrap(), b"fn auto( ){}\n");
    app.world_mut()
        .non_send_mut::<DocumentSession>()
        .shutdown(ClosePolicy::Discard)
        .unwrap();
}
