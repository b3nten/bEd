//! External definition files share the requesting analysis context; ordinary
//! file opens keep their own configured root. These exercise real stdio I/O.
#[cfg(unix)]
mod unix {
    use bed_document_session::{
        ClosePolicy, DocumentId, EditorSession, LspConfigMode, SessionOptions, ViewId,
    };
    use bed_lsp::workspace_lsp::{LspRequestOrigin, ServerInstanceId};
    use bed_remote::{LocalBackend, RemoteClient, Request, SshTarget, serve_with};
    use serde_json::json;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        process::Command,
        rc::Rc,
        sync::atomic::{AtomicU64, Ordering},
        thread,
        time::{Duration, Instant},
    };

    const SERVER: &str = r#"#!/usr/bin/env python3
import json, os, sys
trace = sys.argv[1]
def record(value):
    with open(trace, 'a') as log:
        log.write(json.dumps({'pid':os.getpid(), **value}) + '\n')
def send(value):
    body = json.dumps(value).encode()
    sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n' % len(body)).encode() + body)
    sys.stdout.buffer.flush()
record({'start':os.getcwd()})
while True:
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if line == b'\r\n': break
        if line.lower().startswith(b'content-length:'): length = int(line.split(b':', 1)[1])
    msg = json.loads(sys.stdin.buffer.read(length))
    record(msg)
    if msg.get('method') == 'initialize':
        send({'jsonrpc':'2.0', 'id':msg['id'], 'result':{'capabilities':{'textDocumentSync':2, 'definitionProvider':True}}})
    elif msg.get('method') == 'shutdown':
        send({'jsonrpc':'2.0', 'id':msg['id'], 'result':None})
    elif msg.get('method') == 'exit': break
"#;

    struct Fixture {
        root: PathBuf,
        workspace: PathBuf,
        config: PathBuf,
        trace: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "bed-navigation-context-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("workspace/.bed")).unwrap();
            let root = fs::canonicalize(root).unwrap();
            let workspace = root.join("workspace");
            fs::write(workspace.join("project.marker"), b"").unwrap();
            let server = root.join("server");
            fs::write(&server, SERVER).unwrap();
            fs::set_permissions(&server, fs::Permissions::from_mode(0o755)).unwrap();
            let trace = root.join("trace.jsonl");
            let config = root.join("lsp.json");
            fs::write(&config, serde_json::to_vec(&json!({
            "languages":[{"name":"bed-navigation", "language_id":"bed-navigation", "file_types":["nav"], "roots":["project.marker"], "workspace_lsp_roots":["nested"], "language_server":"bed-navigation"}],
            "language_servers":{"bed-navigation":{"command":server,"args":[trace],"settings":{"flavor":"base"}}}
        })).unwrap()).unwrap();
            fs::write(
                workspace.join(".bed/lsp.json"),
                br#"{"language_servers":{"bed-navigation":{"settings":{"flavor":"project"}}}}"#,
            )
            .unwrap();
            Self {
                root,
                workspace,
                config,
                trace,
            }
        }

        fn file(&self, relative: &str) -> PathBuf {
            let path = self.root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"definition\n").unwrap();
            path
        }

        fn session(&self) -> EditorSession {
            EditorSession::with_options(SessionOptions {
                project_root: Some(self.workspace.clone()),
                lsp_config: Some(self.config.clone()),
                lsp_config_mode: LspConfigMode::Layered,
                ..Default::default()
            })
            .unwrap()
        }

        fn trace(&self) -> Vec<serde_json::Value> {
            fs::read_to_string(&self.trace)
                .unwrap_or_default()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn wait(session: &mut EditorSession, predicate: impl Fn(&EditorSession) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !predicate(session) {
            let report = session.tick();
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            assert!(
                Instant::now() < deadline,
                "LSP timed out: {:?}",
                session.lsp().unwrap().server_statuses()
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn ready(session: &mut EditorSession, document: DocumentId) {
        wait(session, |session| {
            session
                .lsp()
                .unwrap()
                .client_for_document(document)
                .is_some_and(|client| {
                    let client = client.borrow();
                    client.is_initialized()
                        && client.is_document_open(&session.snapshot(document).unwrap().path)
                })
        });
    }

    fn origin(session: &EditorSession, document: DocumentId, view: ViewId) -> LspRequestOrigin {
        session
            .lsp()
            .unwrap()
            .request_origin(
                document,
                view,
                session.snapshot(document).unwrap().generation,
                1,
            )
            .unwrap()
    }

    fn instance(session: &EditorSession, document: DocumentId) -> ServerInstanceId {
        let client = session
            .lsp()
            .unwrap()
            .client_for_document(document)
            .unwrap();
        session
            .lsp()
            .unwrap()
            .server_statuses()
            .into_iter()
            .find(|status| status.process_id == client.borrow().process_id())
            .unwrap()
            .instance
    }

    fn external_definition_chains_reuse_one_process_and_preserve_context_on_reload_and_retry() {
        let fixture = Fixture::new();
        let source = fixture.file("workspace/source.nav");
        let target = fixture.file("external/one/target.nav");
        let next = fixture.file("external/two/next.nav");
        let mut session = fixture.session();
        let source_doc = session.open_file(&source).unwrap();
        let source_view = session.create_view(source_doc).unwrap();
        ready(&mut session, source_doc);
        let source_client = session
            .lsp()
            .unwrap()
            .client_for_document(source_doc)
            .unwrap();
        let context = instance(&session, source_doc);
        let target_doc = session
            .open_file_from_lsp(&target, origin(&session, source_doc, source_view))
            .unwrap();
        let target_view = session.create_view(target_doc).unwrap();
        assert!(Rc::ptr_eq(
            &source_client,
            &session
                .lsp()
                .unwrap()
                .client_for_document(target_doc)
                .unwrap()
        ));
        let next_doc = session
            .open_file_from_lsp(&next, origin(&session, target_doc, target_view))
            .unwrap();
        ready(&mut session, next_doc);
        assert_eq!(instance(&session, next_doc), context);
        assert_eq!(
            fixture
                .trace()
                .iter()
                .filter(|message| message.get("start").is_some())
                .count(),
            1
        );

        fs::write(&target, b"changed definition\n").unwrap();
        session.reload_from_disk(target_doc).unwrap();
        ready(&mut session, target_doc);
        assert_eq!(instance(&session, target_doc), context);
        assert_eq!(
            session.snapshot(target_doc).unwrap().bytes,
            b"changed definition\n"
        );
        assert_eq!(
            fixture
                .trace()
                .iter()
                .filter(|message| message.get("start").is_some())
                .count(),
            1
        );

        session.retry_server(&context).unwrap();
        ready(&mut session, next_doc);
        assert_eq!(instance(&session, target_doc), context);
        assert_ne!(source_client.borrow().process_id(), None);
        session.reload_lsp_config().unwrap();
        wait(&mut session, |session| {
            !session.lsp().unwrap().is_discovering()
        });
        ready(&mut session, source_doc);
        ready(&mut session, target_doc);
        assert_eq!(instance(&session, target_doc), context);
        assert_eq!(instance(&session, next_doc), context);
        session.shutdown(ClosePolicy::Discard).unwrap();
        let transcript = fixture.trace();
        let starts: Vec<_> = transcript
            .iter()
            .filter(|message| message.get("start").is_some())
            .collect();
        assert_eq!(
            starts.len(),
            3,
            "Only initial launch, explicit retry and config reload may launch servers"
        );
        assert!(
            starts
                .iter()
                .all(|message| message["start"] == fixture.workspace.to_str().unwrap())
        );
        assert!(
            transcript
                .iter()
                .filter(|message| message["method"] == "workspace/didChangeConfiguration")
                .all(|message| message["params"]["settings"]["flavor"] == "project")
        );
    }

    fn navigation_preserves_workspace_boundaries_independent_files_and_existing_bindings() {
        let fixture = Fixture::new();
        let source = fixture.file("workspace/source.nav");
        let nested = fixture.file("workspace/nested/source.nav");
        let independent = fixture.file("external/independent.nav");
        let incompatible = fixture.file("external/plain.txt");
        let mut session = fixture.session();
        let doc = session.open_file(&source).unwrap();
        let view = session.create_view(doc).unwrap();
        ready(&mut session, doc);
        let nested_doc = session
            .open_file_from_lsp(&nested, origin(&session, doc, view))
            .unwrap();
        ready(&mut session, nested_doc);
        assert_eq!(
            instance(&session, nested_doc).root,
            fixture.workspace.join("nested")
        );
        let independent_doc = session.open_file(&independent).unwrap();
        ready(&mut session, independent_doc);
        let independent_context = instance(&session, independent_doc);
        assert_eq!(independent_context.root, independent.parent().unwrap());
        assert_eq!(
            session
                .open_file_from_lsp(&independent, origin(&session, doc, view))
                .unwrap(),
            independent_doc
        );
        assert_eq!(instance(&session, independent_doc), independent_context);
        let plain_doc = session
            .open_file_from_lsp(&incompatible, origin(&session, doc, view))
            .unwrap();
        wait(&mut session, |session| {
            !session.lsp().unwrap().is_discovering()
        });
        assert!(
            session
                .lsp()
                .unwrap()
                .client_for_document(plain_doc)
                .is_none()
        );
        session.shutdown(ClosePolicy::Discard).unwrap();
        let transcript = fixture.trace();
        let external_pid = transcript
            .iter()
            .find(|message| message["start"] == independent_context.root.to_str().unwrap())
            .unwrap()["pid"]
            .clone();
        assert!(
            transcript
                .iter()
                .filter(|message| message["pid"] == external_pid
                    && message["method"] == "workspace/didChangeConfiguration")
                .all(|message| message["params"]["settings"]["flavor"] == "base")
        );
    }

    fn stale_navigation_origin_cannot_open_or_register_a_target() {
        let fixture = Fixture::new();
        let source = fixture.file("workspace/source.nav");
        let target = fixture.file("external/target.nav");
        let mut session = fixture.session();
        let doc = session.open_file(&source).unwrap();
        let view = session.create_view(doc).unwrap();
        ready(&mut session, doc);
        let request = origin(&session, doc, view);
        session
            .with_commands(view, |commands| commands.type_text(b"edit"))
            .unwrap();
        assert_eq!(
            session
                .open_file_from_lsp(&target, request)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert!(session.document_for_path(&target).is_none());
        assert_eq!(session.document_ids(), vec![doc]);
        session.shutdown(ClosePolicy::Discard).unwrap();
        assert_eq!(
            fixture
                .trace()
                .iter()
                .filter(|message| message.get("start").is_some())
                .count(),
            1
        );
    }

    fn project_specific_server_mapping_applies_to_inherited_targets() {
        let fixture = Fixture::new();
        fs::write(fixture.workspace.join(".bed/lsp.json"), serde_json::to_vec(&json!({
        "languages":[{"name":"bed-navigation", "language_server":"project-server"}],
        "language_servers":{"project-server":{"command":fixture.root.join("server"),"args":[fixture.trace],"settings":{"flavor":"project"}}}
    })).unwrap()).unwrap();
        let source = fixture.file("workspace/source.nav");
        let target = fixture.file("external/target.nav");
        let mut session = fixture.session();
        let source_doc = session.open_file(&source).unwrap();
        let source_view = session.create_view(source_doc).unwrap();
        ready(&mut session, source_doc);
        let target_doc = session
            .open_file_from_lsp(&target, origin(&session, source_doc, source_view))
            .unwrap();
        let target_view = session.create_view(target_doc).unwrap();
        ready(&mut session, target_doc);
        let context = instance(&session, source_doc);
        assert_eq!(context.server, "project-server");
        assert_eq!(instance(&session, target_doc), context);
        session
            .with_commands(target_view, |commands| commands.type_text(b"edit"))
            .unwrap();
        session.tick();
        assert_eq!(instance(&session, target_doc), context);
        session
            .replace_content(target_doc, b"replacement\n")
            .unwrap();
        ready(&mut session, target_doc);
        assert_eq!(instance(&session, target_doc), context);
        session.reload_lsp_config().unwrap();
        wait(&mut session, |session| {
            !session.lsp().unwrap().is_discovering()
        });
        ready(&mut session, target_doc);
        assert_eq!(instance(&session, target_doc), context);
        session
            .rebind_path(target_doc, &fixture.root.join("external/renamed.txt"))
            .unwrap();
        wait(&mut session, |session| {
            !session.lsp().unwrap().is_discovering()
        });
        assert!(
            session
                .lsp()
                .unwrap()
                .client_for_document(target_doc)
                .is_none()
        );
        session
            .rebind_path(target_doc, &fixture.root.join("external/renamed.nav"))
            .unwrap();
        ready(&mut session, target_doc);
        assert_eq!(
            instance(&session, target_doc).root,
            fixture.root.join("external")
        );
        assert_eq!(instance(&session, target_doc).server, "bed-navigation");
        session.shutdown(ClosePolicy::Discard).unwrap();
    }

    fn language_changes_clear_the_inherited_analysis_context() {
        let fixture = Fixture::new();
        let mut config: serde_json::Value =
            serde_json::from_slice(&fs::read(&fixture.config).unwrap()).unwrap();
        config["languages"][0]["shebangs"] = json!(["bed-nav"]);
        config["languages"].as_array_mut().unwrap().push(json!({
            "name":"other-navigation", "language_id":"other-navigation",
            "shebangs":["bed-other"], "language_server":"other-server"
        }));
        config["language_servers"]["other-server"] = json!({
            "command":fixture.root.join("server"), "args":[fixture.trace]
        });
        fs::write(&fixture.config, serde_json::to_vec(&config).unwrap()).unwrap();
        let source = fixture.file("workspace/source.nav");
        let target = fixture.file("external/dependency");
        fs::write(&target, b"#!/usr/bin/env bed-nav\ndefinition\n").unwrap();
        let mut session = fixture.session();
        let source_doc = session.open_file(&source).unwrap();
        let source_view = session.create_view(source_doc).unwrap();
        ready(&mut session, source_doc);
        let target_doc = session
            .open_file_from_lsp(&target, origin(&session, source_doc, source_view))
            .unwrap();
        ready(&mut session, target_doc);
        assert_eq!(
            instance(&session, target_doc),
            instance(&session, source_doc)
        );
        session
            .replace_content(target_doc, b"#!/usr/bin/env bed-other\nchanged\n")
            .unwrap();
        ready(&mut session, target_doc);
        assert_eq!(instance(&session, target_doc).server, "other-server");
        assert_eq!(
            instance(&session, target_doc).root,
            fixture.root.join("external")
        );
        session.shutdown(ClosePolicy::Discard).unwrap();
    }

    fn remote_navigation() {
        let fixture = Fixture::new();
        let source = fixture.file("workspace/source.nav");
        let target = fixture.file("external/target.nav");
        let stale_target = fixture.file("external/stale.nav");
        let stale_failure = fixture.file("external/stale-failure.nav");
        let agent = fixture.root.join("agent");
        fs::write(
            &agent,
            r#"#!/usr/bin/env python3
import os, sys
args = sys.argv[2:]
cwd = None
candidates = []
environment = os.environ.copy()
while args[0] != '--':
    option = args.pop(0)
    if option == '--cwd': cwd = args.pop(0)
    elif option == '--candidate': candidates.append(args.pop(0))
    elif option == '--env':
        key = args.pop(0)
        environment[key] = args.pop(0)
    else: raise Exception(option)
os.chdir(cwd)
os.execve(candidates[0], [candidates[0]] + args[1:], environment)
"#,
        )
        .unwrap();
        fs::set_permissions(&agent, fs::Permissions::from_mode(0o755)).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("--remote-transport")
            .env("BED_FIXTURE_CAPABILITY_ROOT", &fixture.root);
        let transport = RemoteClient::launch_command(command).unwrap();
        let mut ssh = SshTarget::new("fixture");
        ssh.agent = agent.to_str().unwrap().to_owned();
        let mut session = EditorSession::with_remote_options(
            SessionOptions {
                lsp_config: Some(fixture.config.clone()),
                lsp_config_mode: LspConfigMode::Layered,
                ..Default::default()
            },
            ssh,
            transport.clone(),
            fixture.workspace.to_str().unwrap().to_owned(),
        )
        .unwrap();
        assert_eq!(
            session.open_file(&source).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        wait(&mut session, |session| !session.open_pending(&source));
        let source_doc = session.document_for_path(&source).unwrap();
        let source_view = session.create_view(source_doc).unwrap();
        ready(&mut session, source_doc);
        let context = instance(&session, source_doc);
        assert_eq!(
            session
                .open_file_from_lsp(&target, origin(&session, source_doc, source_view))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(session.open_pending(&target));
        wait(&mut session, |session| !session.open_pending(&target));
        let target_doc = session.document_for_path(&target).unwrap();
        ready(&mut session, target_doc);
        assert_eq!(instance(&session, target_doc), context);
        assert!(Rc::ptr_eq(
            &session
                .lsp()
                .unwrap()
                .client_for_document(source_doc)
                .unwrap(),
            &session
                .lsp()
                .unwrap()
                .client_for_document(target_doc)
                .unwrap()
        ));
        fs::write(&target, b"remote changed\n").unwrap();
        session.reload_from_disk(target_doc).unwrap();
        wait(&mut session, |session| {
            session.snapshot(target_doc).unwrap().bytes == b"remote changed\n"
        });
        ready(&mut session, target_doc);
        assert_eq!(instance(&session, target_doc), context);
        assert_eq!(
            session
                .open_file_from_lsp(&stale_target, origin(&session, source_doc, source_view))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::WouldBlock
        );
        session
            .with_commands(source_view, |commands| commands.type_text(b"edit"))
            .unwrap();
        wait(&mut session, |session| !session.open_pending(&stale_target));
        assert!(session.document_for_path(&stale_target).is_none());
        assert_eq!(
            session
                .open_file_from_lsp(&stale_failure, origin(&session, source_doc, source_view))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::WouldBlock
        );
        fs::remove_file(&stale_failure).unwrap();
        session
            .with_commands(source_view, |commands| commands.type_text(b"another edit"))
            .unwrap();
        wait(&mut session, |session| {
            !session.open_pending(&stale_failure)
        });
        assert!(session.document_for_path(&stale_failure).is_none());
        session.shutdown(ClosePolicy::Discard).unwrap();
        transport.disconnect();
        assert_eq!(
            fixture
                .trace()
                .iter()
                .filter(|message| message.get("start").is_some())
                .count(),
            1
        );
    }

    pub fn run() {
        if std::env::args().any(|arg| arg == "--remote-transport") {
            let root = std::env::var("BED_FIXTURE_CAPABILITY_ROOT").unwrap();
            serve_with(
                std::io::stdin().lock(),
                std::io::stdout().lock(),
                |mut request| {
                    if let Request::ReadFile {
                        root: capability, ..
                    } = &mut request
                    {
                        // The fixture agent explicitly grants a capability covering
                        // both project and dependencies; the session's workspace stays narrower.
                        capability.clone_from(&root);
                        thread::sleep(Duration::from_millis(100));
                    }
                    LocalBackend.call(request)
                },
            )
            .unwrap();
            return;
        }
        if std::env::args().any(|arg| arg == "--remote-navigation") {
            remote_navigation();
            return;
        }
        external_definition_chains_reuse_one_process_and_preserve_context_on_reload_and_retry();
        navigation_preserves_workspace_boundaries_independent_files_and_existing_bindings();
        stale_navigation_origin_cannot_open_or_register_a_target();
        project_specific_server_mapping_applies_to_inherited_targets();
        language_changes_clear_the_inherited_analysis_context();
        let fixture = Fixture::new();
        let bin = fixture.root.join("bin");
        fs::create_dir(&bin).unwrap();
        let ssh = bin.join("ssh");
        fs::write(&ssh, "#!/bin/sh\nexec /bin/sh -c \"$4\"\n").unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o755)).unwrap();
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        let status = Command::new(std::env::current_exe().unwrap())
            .arg("--remote-navigation")
            .env("PATH", std::env::join_paths(paths).unwrap())
            .status()
            .unwrap();
        assert!(
            status.success(),
            "deferred navigation fixture failed: {status}"
        );
        println!("6 LSP navigation context scenarios passed");
    }
}

#[cfg(unix)]
fn main() {
    unix::run();
}

#[cfg(not(unix))]
fn main() {
    println!("LSP subprocess navigation fixtures require a Unix host");
}
