//! Actual transport coverage for watched-file filters and configured launch data.
#![cfg(unix)]
use bed_lsp::{lsp_client::LspClient, lsp_config::LspConfig};
use bed_remote::FilesystemChange;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "bed-file-watch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let script = root.join("server.py");
        fs::write(&script, format!("#!/usr/bin/env python3\n{SCRIPT}")).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        // Startup must use the supplied effective configuration, including its
        // settings and initialization options, even when this file disagrees.
        fs::write(root.join("lsp.json"), "{\"languages\":[]}").unwrap();
        Self(fs::canonicalize(root).unwrap())
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
    fn client(&self, observations: bool) -> LspClient {
        let config = LspConfig::from_json(&json!({
            "languages":[{"name":"fixture", "language_id":"fixture", "file_types":["rs"], "language_server":"fixture-server"}],
            "language_servers":{"fixture-server":{
                "command":self.path("server.py"), "args":["literal argument with spaces", "$(touch never-created)"],
                "environment":{"BED_LSP_LITERAL":"a'b $HOME `echo nope`"},
                "settings":{"fixture":{"authoritative":true}},
                "initialization_options":{"fixture":true}
            }}
        })).unwrap();
        let mut client = LspClient::with_config(self.0.join("lsp.json"), config);
        client.set_workspace(self.0.to_str().unwrap());
        client.set_file_observations_available(observations);
        assert!(client.start_server("fixture-server", "").unwrap());
        client
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn until(client: &mut LspClient, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        client.poll();
        assert!(
            Instant::now() < deadline,
            "Fixture stalled: {:?}\n{}",
            client.last_error(),
            client.stderr_text()
        );
        thread::sleep(Duration::from_millis(5));
    }
}

const SCRIPT: &str = r#"
import json, sys, os, pathlib

assert sys.argv[1:]==['literal argument with spaces', '$(touch never-created)']
assert os.environ['BED_LSP_LITERAL']=="a'b $HOME `echo nope`"
events=[]

def send(value):
    body=json.dumps(value).encode()
    sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(body)).encode()+body)
    sys.stdout.buffer.flush()

def read():
    length=None
    while True:
        line=sys.stdin.buffer.readline()
        if not line: return None
        if line in (b'\r\n',b'\n'): break
        if line.lower().startswith(b'content-length:'): length=int(line.split(b':',1)[1])
    return json.loads(sys.stdin.buffer.read(length))

while True:
    message=read()
    if message is None: break
    method=message.get('method')
    if method=='initialize':
        assert message['params']['initializationOptions']=={'fixture':True}
        available=message['params']['capabilities']['workspace'].get('didChangeWatchedFiles')
        if available: assert available=={'dynamicRegistration':True,'relativePatternSupport':True}
        pathlib.Path('initialize.json').write_text(json.dumps(message['params']))
        send({'jsonrpc':'2.0','id':message['id'],'result':{'capabilities':{'hoverProvider':False}}})
    elif method=='initialized':
        if available:
            send({'jsonrpc':'2.0','id':'register','method':'client/registerCapability','params':{'registrations':[{
                'id':'watch', 'method':'workspace/didChangeWatchedFiles','registerOptions':{'watchers':[
                    {'globPattern':{'baseUri':{'uri':pathlib.Path.cwd().as_uri()},'pattern':'**/*.rs'},'kind':5},
                    {'globPattern':'**/*.toml','kind':2}
                ]}
            }]}})
        else:
            pathlib.Path('ready').write_text('no observations')
    elif method=='workspace/didChangeConfiguration':
        assert message['params']['settings']=={'fixture':{'authoritative':True}}
        pathlib.Path('settings').write_text('configured')
    elif message.get('id')=='register' and 'result' in message:
        # A failed registration batch must retain the previous valid filters.
        send({'jsonrpc':'2.0','id':'invalid-register','method':'client/registerCapability','params':{'registrations':[
            {'id':'watch','method':'workspace/didChangeWatchedFiles','registerOptions':{'watchers':[{'globPattern':'**/*.py'}]}},
            {'id':'bad','method':'workspace/didChangeWatchedFiles','registerOptions':{'watchers':[{'globPattern':'**','kind':8}]}}
        ]}})
    elif message.get('id')=='invalid-register':
        assert message['error']['code']==-32602
        pathlib.Path('ready').write_text('registered')
    elif method=='workspace/didChangeWatchedFiles':
        events.append(message['params']['changes'])
        pathlib.Path('watched.json').write_text(json.dumps(events))
        send({'jsonrpc':'2.0','id':'unregister','method':'client/unregisterCapability','params':{'unregisterations':[{'id':'watch','method':'workspace/didChangeWatchedFiles'}]}})
    elif message.get('id')=='unregister' and 'result' in message:
        pathlib.Path('unregistered').write_text('done')
    elif method=='fixture/barrier':
        send({'jsonrpc':'2.0','id':message['id'],'result':len(events)})
    elif method=='shutdown':
        send({'jsonrpc':'2.0','id':message['id'],'result':None})
    elif method=='exit': break
"#;

#[test]
fn watched_file_registration_filters_renames_and_unregisters_over_stdio() {
    let fixture = Fixture::new();
    let mut client = fixture.client(true);
    until(&mut client, || {
        fixture.0.join("ready").exists() && fixture.0.join("settings").exists()
    });
    assert!(client.is_initialized());
    assert!(
        client
            .send_request("textDocument/hover", json!({}), |_| panic!(
                "unsupported hover callback"
            ))
            .is_err()
    );
    use FilesystemChange::*;
    client
        .observe_file_changes(&[
            Modified {
                path: fixture.path("main.rs"),
            },
            Created {
                path: fixture.path("main.rs"),
            },
            Created {
                path: fixture.path("main.rs"),
            },
            Removed {
                path: fixture.path("gone.rs"),
            },
            Renamed {
                from: fixture.path("old.rs"),
                to: fixture.path("new.rs"),
            },
            Modified {
                path: fixture.path("Cargo.toml"),
            },
            Created {
                path: "/outside/ignored.rs".into(),
            },
            Created {
                path: fixture.path("ignored.py"),
            },
        ])
        .unwrap();
    until(&mut client, || fixture.0.join("unregistered").exists());
    let batches: Value =
        serde_json::from_slice(&fs::read(fixture.0.join("watched.json")).unwrap()).unwrap();
    let uri = |name: &str| {
        bed_lsp::lsp_uri::LspUri::file_uri_from_path(&fixture.path(name))
            .unwrap()
            .to_string()
    };
    assert_eq!(
        batches,
        json!([[
            {"uri":uri("main.rs"), "type":1}, {"uri":uri("gone.rs"), "type":3},
            {"uri":uri("old.rs"), "type":3}, {"uri":uri("new.rs"), "type":1},
            {"uri":uri("Cargo.toml"), "type":2}
        ]])
    );
    client
        .observe_file_changes(&[Created {
            path: fixture.path("later.rs"),
        }])
        .unwrap();
    let reply = Rc::new(RefCell::new(None));
    let received = reply.clone();
    client
        .send_request("fixture/barrier", json!({}), move |value| {
            *received.borrow_mut() = Some(value.unwrap())
        })
        .unwrap();
    until(&mut client, || reply.borrow().is_some());
    assert_eq!(*reply.borrow(), Some(json!(1)));
    assert!(!fixture.0.join("never-created").exists());
    client.shutdown();
}

#[test]
fn clients_without_observations_omit_watcher_capabilities() {
    let fixture = Fixture::new();
    let mut client = fixture.client(false);
    until(&mut client, || {
        fixture.0.join("ready").exists() && fixture.0.join("settings").exists()
    });
    let params: Value =
        serde_json::from_slice(&fs::read(fixture.0.join("initialize.json")).unwrap()).unwrap();
    assert!(
        params["capabilities"]["workspace"]
            .get("didChangeWatchedFiles")
            .is_none()
    );
    client.shutdown();
}
