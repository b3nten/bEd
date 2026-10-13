//! bEd chooses format-on-save policy; core owns formatting, writes and receipts.
use super::*;
#[cfg(test)]
use std::time::Duration;

pub(super) struct PendingFormatSave {
    ids: Vec<Option<AppRequestId>>,
    document: DocumentId,
    save: bed_core::DocumentSaveId,
}

pub(super) fn begin(
    world: &mut World,
    document: DocumentId,
    id: Option<AppRequestId>,
) -> io::Result<Option<AppRequestOutcome>> {
    if let Some(pending) = world
        .resource_mut::<PendingOperations>()
        .formats
        .iter_mut()
        .find(|pending| pending.document == document)
    {
        pending.ids.push(id);
        return Ok(None);
    }
    let format = world.non_send::<Settings>().lsp_format_on_save_enabled();
    let save = bed_core::begin_explicit_document_save(world, document, format)
        .map_err(io::Error::other)?;
    match bed_core::poll_document_save(world, save) {
        Some(result) => {
            if let Some(warning) = bed_core::take_document_save_warning(world, save) {
                world.resource_mut::<WorkspaceState>().error = Some(warning);
            }
            result
                .map(|documents| Some(AppRequestOutcome::Saved { documents }))
                .map_err(io::Error::other)
        }
        None => {
            world
                .resource_mut::<PendingOperations>()
                .formats
                .push(PendingFormatSave {
                    ids: vec![id],
                    document,
                    save,
                });
            Ok(None)
        }
    }
}

pub(super) fn poll(world: &mut World) {
    let requests = std::mem::take(&mut world.resource_mut::<PendingOperations>().formats);
    for pending in requests {
        let Some(result) = bed_core::poll_document_save(world, pending.save) else {
            world
                .resource_mut::<PendingOperations>()
                .formats
                .push(pending);
            continue;
        };
        if let Some(warning) = bed_core::take_document_save_warning(world, pending.save) {
            world.resource_mut::<WorkspaceState>().error = Some(warning);
        }
        let result = result
            .map(|documents| AppRequestOutcome::Saved { documents })
            .map_err(AppRequestError::from);
        for id in pending.ids {
            complete(world, id, result.clone());
        }
    }
}

pub(super) fn cancel(world: &mut World) {
    bed_core::cancel_explicit_document_saves(world);
    let requests = std::mem::take(&mut world.resource_mut::<PendingOperations>().formats);
    for pending in requests {
        for id in pending.ids {
            complete(world, id, Err("Workspace changed while formatting".into()));
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::application::test_support::TempDir;
    use std::{fs, os::unix::fs::PermissionsExt, thread};

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
            tick_app_services(app.world_mut()).unwrap();
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
        let mut app = App::new();
        install_test_infrastructure(&mut app, &dir.path("config"));
        app.add_plugins((
            (bevy::MinimalPlugins, bevy::input::InputPlugin),
            AppRuntimePlugin {
                options: AppOptions {
                    working_directory: Some(dir.root().into()),
                    ..Default::default()
                },
            },
            bed_plugin_editor::EditorPlugin,
        ));
        app.update();
        let session = DocumentSession::with_options(SessionOptions {
            project_root: Some(dir.root().into()),
            lsp_config: Some(config),
            ..Default::default()
        })
        .unwrap();
        let session_id = session.session_id().0;
        app.world_mut().insert_non_send(session);
        app.world_mut().insert_resource(ActiveSessionId(session_id));
        let path = dir.write("main.rs", b"fn main( ){}\n");
        let panel = open_file(app.world_mut(), &path, None, false, None).unwrap();
        let document = app
            .world()
            .get::<PanelBinding>(panel)
            .unwrap()
            .document
            .unwrap();
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
        let request = internal_request_id(app.world_mut());
        assert!(
            begin(app.world_mut(), document, Some(request))
                .unwrap()
                .is_none()
        );

        #[derive(Component)]
        struct PendingDraft {
            document: DocumentId,
            revision: bed_api::documents::Revision,
        }
        let frontend = panel_presentation(app.world(), panel).unwrap();
        let commit = app.world_mut().register_system(
            |In(invocation): In<bed_api::workbench::PanelActionInvocation>, world: &mut World| {
                if invocation.action == bed_api::workbench::PanelAction::CommitEdit
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
            bed_api::workbench::PanelActions(commit),
        ));
        assert_eq!(
            app.world()
                .non_send::<DocumentSession>()
                .document_revision(document)
                .unwrap(),
            revision,
            "the late draft has not reached the canonical document"
        );

        pump(&mut app, |world| {
            world.resource::<PendingOperations>().formats.is_empty()
        });
        poll_app_request(app.world_mut(), request).unwrap().unwrap();
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
        let request = internal_request_id(app.world_mut());
        assert!(
            begin(app.world_mut(), document, Some(request))
                .unwrap()
                .is_none()
        );
        let frontend = panel_presentation(app.world(), panel).unwrap();
        #[derive(Component)]
        struct RejectedDraft;
        let commit = app.world_mut().register_system(
            |In(_): In<bed_api::workbench::PanelActionInvocation>| {
                Err::<bool, String>("draft rejected".into())
            },
        );
        app.world_mut()
            .entity_mut(frontend)
            .insert((RejectedDraft, bed_api::workbench::PanelActions(commit)));

        pump(&mut app, |world| {
            world.resource::<PendingOperations>().formats.is_empty()
        });
        let error = poll_app_request(app.world_mut(), request)
            .unwrap()
            .unwrap_err();
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

        let request = internal_request_id(app.world_mut());
        assert!(
            begin(app.world_mut(), document, Some(request))
                .unwrap()
                .is_none()
        );
        let duplicate = internal_request_id(app.world_mut());
        assert!(
            begin(app.world_mut(), document, Some(duplicate))
                .unwrap()
                .is_none()
        );
        assert_eq!(fs::read(&path).unwrap(), b"fn main( ){}\n");
        pump(&mut app, |world| {
            world.resource::<PendingOperations>().formats.is_empty()
        });
        poll_app_request(app.world_mut(), request).unwrap().unwrap();
        assert!(
            poll_app_request(app.world_mut(), duplicate)
                .unwrap()
                .is_ok()
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

        let request = internal_request_id(app.world_mut());
        assert!(
            begin(app.world_mut(), document, Some(request))
                .unwrap()
                .is_none()
        );
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
        pump(&mut app, |world| {
            world.resource::<PendingOperations>().formats.is_empty()
        });
        poll_app_request(app.world_mut(), request).unwrap().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"fn other(){}\n");

        fs::write(&mode, b"error").unwrap();
        let request = internal_request_id(app.world_mut());
        assert!(
            begin(app.world_mut(), document, Some(request))
                .unwrap()
                .is_none()
        );
        pump(&mut app, |world| {
            world.resource::<PendingOperations>().formats.is_empty()
        });
        poll_app_request(app.world_mut(), request).unwrap().unwrap();
        assert!(
            app.world()
                .resource::<WorkspaceState>()
                .error
                .as_ref()
                .unwrap()
                .contains("Formatting failed")
        );
        assert_eq!(fs::read(&path).unwrap(), b"fn other(){}\n");

        let count = fs::read_to_string(&trace).unwrap().lines().count();
        bed_core::settings::apply_settings_request(
            app.world_mut(),
            &bed_api::settings::SettingsRequest::batch(vec![
                bed_api::settings::SettingsChange::set(
                    "bed.editor",
                    "lsp_format_on_save",
                    json!(false),
                ),
            ]),
        )
        .unwrap();
        crate::config::sync_view(app.world_mut()).unwrap();
        bed_core::settings::configure_document_settings(app.world_mut()).unwrap();
        assert!(begin(app.world_mut(), document, None).unwrap().is_some());
        assert_eq!(fs::read_to_string(&trace).unwrap().lines().count(), count);
        bed_core::settings::apply_settings_request(
            app.world_mut(),
            &bed_api::settings::SettingsRequest::batch(vec![
                bed_api::settings::SettingsChange::set(
                    "bed.editor",
                    "lsp_format_on_save",
                    json!(true),
                ),
                bed_api::settings::SettingsChange::set(
                    "bed.documents",
                    "autosave_delay_ms",
                    json!(0),
                ),
            ]),
        )
        .unwrap();
        crate::config::sync_view(app.world_mut()).unwrap();
        bed_core::settings::configure_document_settings(app.world_mut()).unwrap();
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
        bed_core::settings::apply_settings_request(
            app.world_mut(),
            &bed_api::settings::SettingsRequest::batch(vec![
                bed_api::settings::SettingsChange::set("bed.documents", "autosave", json!(false)),
            ]),
        )
        .unwrap();
        crate::config::sync_view(app.world_mut()).unwrap();
        bed_core::settings::configure_document_settings(app.world_mut()).unwrap();
        fs::write(&mode, b"timeout").unwrap();
        let request = internal_request_id(app.world_mut());
        assert!(
            begin(app.world_mut(), document, Some(request))
                .unwrap()
                .is_none()
        );
        pump(&mut app, |world| {
            world.resource::<PendingOperations>().formats.is_empty()
        });
        poll_app_request(app.world_mut(), request).unwrap().unwrap();
        assert!(
            app.world()
                .resource::<WorkspaceState>()
                .error
                .as_ref()
                .unwrap()
                .contains("timed out")
        );
        assert_eq!(fs::read(&path).unwrap(), b"fn auto( ){}\n");
        app.world_mut()
            .non_send_mut::<DocumentSession>()
            .shutdown(ClosePolicy::Discard)
            .unwrap();
    }
}
