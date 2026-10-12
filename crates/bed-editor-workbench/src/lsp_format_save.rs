//! Explicit saves wait for formatting; background and lifecycle saves bypass it.
use super::*;
use bed_lsp::{jsonrpc::RpcId, lsp_uri::LspUri};
use std::{
    sync::{Mutex, mpsc},
    time::Duration,
};

pub(super) struct PendingFormatSave {
    ids: Vec<Option<EditorRequestId>>,
    origin: LspRequestOrigin,
    revision: Revision,
    rpc: RpcId,
    response: Mutex<mpsc::Receiver<Result<Value, String>>>,
    started: Instant,
    autosave_was_paused: bool,
    temporary_view: bool,
}

pub(super) fn begin(
    world: &mut World,
    document: DocumentId,
    id: Option<EditorRequestId>,
) -> io::Result<Option<EditorRequestOutcome>> {
    if let Some(pending) = world
        .resource_mut::<PendingOperations>()
        .formats
        .iter_mut()
        .find(|pending| pending.origin.document_id == document)
    {
        pending.ids.push(id);
        return Ok(None);
    }
    let enabled = world.non_send::<Settings>().lsp_format_on_save_enabled();
    let ready = {
        let session = world.non_send::<EditorSession>();
        session.document_kind(document)? == bed_document_session::DocumentKind::Text
            && !session.is_snapshot_document(document)
            && session
                .lsp()
                .and_then(|pool| pool.client_for_document(document))
                .is_some_and(|client| {
                    let client = client.borrow();
                    client.is_initialized() && client.supports_method("textDocument/formatting")
                })
    };
    if !enabled || !ready {
        return begin_save(world, vec![document], None, None, id);
    }
    commit_documents(world, &HashSet::from([document]))?;
    let mut session = world.non_send_mut::<EditorSession>();
    let snapshot = session.snapshot(document)?;
    if snapshot.path.is_empty() || snapshot.disk_conflict.is_some() {
        drop(session);
        return begin_save(world, vec![document], None, None, id);
    }
    let existing = session.view_ids(document).into_iter().next();
    let view = match existing {
        Some(view) => view,
        None => session.create_view(document)?,
    };
    let revision = session.document_revision(document)?;
    let origin = session
        .lsp()
        .and_then(|pool| pool.request_origin(document, view, snapshot.generation, 0))
        .ok_or_else(|| io::Error::other("Formatting document is no longer bound to a server"))?;
    let uri = if session.is_remote() {
        LspUri::file_uri_from_remote_path(&snapshot.path)?
    } else {
        LspUri::file_uri_from_path(&snapshot.path)?
    };
    // The editor currently indents using four spaces.
    let params =
        json!({"textDocument":{"uri":uri.to_string()},"options":{"tabSize":4,"insertSpaces":true}});
    let (sender, response) = mpsc::channel();
    let request = session.lsp_mut().unwrap().send_request(
        origin,
        "textDocument/formatting",
        params,
        move |reply| {
            let _ = sender.send(reply.result.map_err(|error| error.to_string()));
        },
    );
    let rpc = match request {
        Ok(rpc) => rpc,
        Err(error) => {
            if existing.is_none() {
                session.detach_view(view);
            }
            drop(session);
            world.resource_mut::<EditorWorkspace>().error =
                Some(format!("Formatting failed; saving current text: {error}"));
            return begin_save(world, vec![document], None, None, id);
        }
    };
    let autosave_was_paused = session.autosave_paused(document)?;
    session.pause_autosave(document, true)?;
    drop(session);
    world
        .resource_mut::<PendingOperations>()
        .formats
        .push(PendingFormatSave {
            ids: vec![id],
            origin,
            revision,
            rpc,
            response: Mutex::new(response),
            started: Instant::now(),
            autosave_was_paused,
            temporary_view: existing.is_none(),
        });
    Ok(None)
}

pub(super) fn poll(world: &mut World) {
    let requests = std::mem::take(&mut world.resource_mut::<PendingOperations>().formats);
    for pending in requests {
        let current = world
            .non_send::<EditorSession>()
            .accepts_lsp_origin(&pending.origin);
        let reply = match pending.response.lock().unwrap().try_recv() {
            Ok(reply) => Some(reply),
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("Server disconnected".into())),
            Err(mpsc::TryRecvError::Empty)
                if pending.started.elapsed() >= Duration::from_secs(3) =>
            {
                Some(Err("Formatting timed out".into()))
            }
            Err(mpsc::TryRecvError::Empty) => None,
        };
        if reply.is_none() && current {
            world
                .resource_mut::<PendingOperations>()
                .formats
                .push(pending);
            continue;
        }
        let document = pending.origin.document_id;
        let workspace_matches =
            world.non_send::<EditorSession>().workspace_id() == pending.origin.workspace_id;
        let result = if current {
            match reply.unwrap() {
                Ok(edits) => world.non_send_mut::<EditorSession>().apply_lsp_text_edits(
                    document,
                    pending.revision,
                    &edits,
                ),
                Err(error) => Err(io::Error::other(error)),
            }
        } else {
            Ok(()) // A newer revision wins; the pending save writes that revision.
        };
        release(world, &pending);
        if let Err(error) = result {
            world.resource_mut::<EditorWorkspace>().error =
                Some(format!("Formatting failed; saving current text: {error}"));
        }
        for id in pending.ids {
            let outcome = if workspace_matches {
                begin_save(world, vec![document], None, None, id)
            } else {
                Err(io::Error::other("Workspace changed while formatting"))
            };
            match outcome {
                Ok(Some(outcome)) => complete(world, id, Ok(outcome)),
                Ok(None) => {}
                Err(error) => complete(world, id, Err(error.into())),
            }
        }
    }
}

fn release(world: &mut World, pending: &PendingFormatSave) {
    let mut session = world.non_send_mut::<EditorSession>();
    if session.workspace_id() != pending.origin.workspace_id {
        return;
    }
    if let Some(client) = session
        .lsp()
        .and_then(|pool| pool.client_for_document(pending.origin.document_id))
    {
        let _ = client.borrow_mut().cancel_request(&pending.rpc);
    }
    let _ = session.pause_autosave(pending.origin.document_id, pending.autosave_was_paused);
    if pending.temporary_view {
        session.detach_view(pending.origin.view_id);
    }
}

pub(super) fn cancel(world: &mut World) {
    let requests = std::mem::take(&mut world.resource_mut::<PendingOperations>().formats);
    for pending in requests {
        release(world, &pending);
        for id in pending.ids {
            complete(world, id, Err("Workspace changed while formatting".into()));
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
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
            tick_editor_services(app.world_mut()).unwrap();
            assert!(Instant::now() < until, "save or language server timed out");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn explicit_save_formats_but_autosave_bypasses_and_newer_edits_win() {
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
            bed_runtime::BedRuntimePlugin,
            EditorWorkbenchPlugin {
                options: EditorWorkbenchOptions {
                    working_directory: Some(dir.root().into()),
                    ..Default::default()
                },
            },
            bed_editor::EditorPlugin,
        ));
        app.update();
        let session = EditorSession::with_options(SessionOptions {
            project_root: Some(dir.root().into()),
            lsp_config: Some(config),
            ..Default::default()
        })
        .unwrap();
        let session_id = session.workspace_id().0;
        app.world_mut().insert_non_send(session);
        app.world_mut().insert_resource(EditorSessionId(session_id));
        let path = dir.write("main.rs", b"fn main( ){}\n");
        let panel = open_file(app.world_mut(), &path, None, false, None).unwrap();
        let document = app
            .world()
            .get::<EditorPanelBinding>(panel)
            .unwrap()
            .document
            .unwrap();
        pump(&mut app, |world| {
            world
                .non_send::<EditorSession>()
                .lsp()
                .and_then(|pool| pool.client_for_document(document))
                .is_some_and(|client| client.borrow().is_initialized())
        });

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
        assert!(
            poll_editor_request(app.world_mut(), request)
                .unwrap()
                .is_ok()
        );
        assert!(
            poll_editor_request(app.world_mut(), duplicate)
                .unwrap()
                .is_ok()
        );
        assert_eq!(fs::read(&path).unwrap(), b"fn main() {}\n");
        app.world_mut()
            .non_send_mut::<EditorSession>()
            .undo_document(document)
            .unwrap();
        assert_eq!(
            app.world()
                .non_send::<EditorSession>()
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
        let mut session = app.world_mut().non_send_mut::<EditorSession>();
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
        drop(session);
        pump(&mut app, |world| {
            world.resource::<PendingOperations>().formats.is_empty()
        });
        assert!(
            poll_editor_request(app.world_mut(), request)
                .unwrap()
                .is_ok()
        );
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
        assert!(
            poll_editor_request(app.world_mut(), request)
                .unwrap()
                .is_ok()
        );
        assert!(
            app.world()
                .resource::<EditorWorkspace>()
                .error
                .as_ref()
                .unwrap()
                .contains("Formatting failed")
        );
        assert_eq!(fs::read(&path).unwrap(), b"fn other(){}\n");

        let count = fs::read_to_string(&trace).unwrap().lines().count();
        app.world_mut().non_send_mut::<Settings>().settings["lsp_format_on_save"] = json!(false);
        assert!(begin(app.world_mut(), document, None).unwrap().is_some());
        assert_eq!(fs::read_to_string(&trace).unwrap().lines().count(), count);
        app.world_mut().non_send_mut::<Settings>().settings["lsp_format_on_save"] = json!(true);
        app.world_mut().non_send_mut::<Settings>().settings["autosave_delay_ms"] = json!(0);
        let mut session = app.world_mut().non_send_mut::<EditorSession>();
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
        drop(session);
        pump(&mut app, |world| {
            !world
                .non_send::<EditorSession>()
                .snapshot(document)
                .unwrap()
                .dirty
        });
        assert_eq!(fs::read(&path).unwrap(), b"fn auto( ){}\n");
        assert_eq!(fs::read_to_string(&trace).unwrap().lines().count(), count);
        app.world_mut().non_send_mut::<Settings>().settings["autosave"] = json!(false);
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
        assert!(
            poll_editor_request(app.world_mut(), request)
                .unwrap()
                .is_ok()
        );
        assert!(
            app.world()
                .resource::<EditorWorkspace>()
                .error
                .as_ref()
                .unwrap()
                .contains("timed out")
        );
        assert_eq!(fs::read(&path).unwrap(), b"fn auto( ){}\n");
        app.world_mut()
            .non_send_mut::<EditorSession>()
            .shutdown(ClosePolicy::Discard)
            .unwrap();
    }
}
