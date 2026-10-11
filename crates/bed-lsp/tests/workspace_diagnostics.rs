//! Exercise workspace pulls with no open documents, including partial reports,
//! dynamic registration, identifiers, refresh and cancellation of old requests.
#![cfg(unix)]
use bed_lsp::{lsp_client::LspClient, lsp_config::LspConfig};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new(script: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "bed-workspace-lsp-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("server.py");
        fs::write(&file, format!("#!/usr/bin/env python3\n{script}")).unwrap();
        fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
        Self(root)
    }
    fn client(&self) -> LspClient {
        let mut client = LspClient::with_config(
            self.0.join("missing.json"),
            LspConfig::from_json(&serde_json::json!({
                "languages":[{"name":"python","language_id":"python","file_types":["py"],"language_server":"python"}],
                "language_servers":{"python":{
                    "command":self.0.join("server.py"),"args":[],
                    "settings":{"python":{"analysis":{"diagnosticMode":"workspace"}}}
                }}
            }))
            .unwrap(),
        );
        client.set_workspace(self.0.to_str().unwrap());
        assert!(client.start_server("python", "").unwrap());
        client
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn until(client: &mut LspClient, ready: impl Fn(&LspClient) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        client.poll();
        if ready(client) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Workspace diagnostics stalled: {:?}\n{}",
            client.last_error(),
            client.stderr_text()
        );
        thread::sleep(Duration::from_millis(5));
    }
}
const SCRIPT: &str = r#"
import json, sys, pathlib

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

def item(text):
    return {'range':{'start':{'line':2,'character':1},'end':{'line':2,'character':4}},'message':text,'severity':1}

first=None
count=0
while True:
    msg=read()
    if msg is None: break
    method=msg.get('method')
    if method=='initialize':
        send({'jsonrpc':'2.0','id':msg['id'],'result':{'capabilities':{}}})
    elif method=='initialized':
        send({'jsonrpc':'2.0','id':'register','method':'client/registerCapability','params':{'registrations':[{'id':'workspace','method':'textDocument/diagnostic','registerOptions':{'workspaceDiagnostics':True,'identifier':'fixture'}}]}})
        send({'jsonrpc':'2.0','id':'config','method':'workspace/configuration','params':{'items':[{'section':'python.analysis'}]}})
    elif msg.get('id')=='config' and 'result' in msg:
        assert msg['result'][0]['diagnosticMode']=='workspace'
    elif method=='workspace/diagnostic':
        assert msg['params']['identifier']=='fixture'
        count+=1
        if count==1:
            first=msg['id']
            pathlib.Path('requested').write_text('ready')
        else:
            send({'jsonrpc':'2.0','id':first,'result':{'items':[{'uri':'file:///tmp/stale.py','kind':'full','items':[item('obsolete')]}]}})
            send({'jsonrpc':'2.0','method':'$/progress','params':{'token':msg['params']['partialResultToken'],'value':{'items':[{'uri':'file:///tmp/closed.py','kind':'full','resultId':'closed-v1','items':[item('closed error')]}]}}})
            send({'jsonrpc':'2.0','id':msg['id'],'result':{'items':[]}})
    elif method=='shutdown':
        send({'jsonrpc':'2.0','id':msg['id'],'result':None})
    elif method=='exit': break
"#;
#[test]
fn workspace_pulls_report_closed_files_and_discard_canceled_replies() {
    let fixture = Fixture::new(SCRIPT);
    let mut client = fixture.client();
    until(&mut client, |_| fixture.0.join("requested").exists());
    assert!(!client.workspace_diagnostics_complete());
    client.refresh_workspace_diagnostics();
    until(&mut client, LspClient::workspace_diagnostics_complete);
    let diagnostics = client.diagnostics();
    assert!(diagnostics.for_document("/tmp/stale.py").is_empty());
    assert_eq!(
        diagnostics.for_document("/tmp/closed.py")[0].message,
        "closed error"
    );
    assert!(!client.is_document_open("/tmp/closed.py"));
    client.shutdown();
    assert!(diagnostics.snapshot().is_empty());
}
