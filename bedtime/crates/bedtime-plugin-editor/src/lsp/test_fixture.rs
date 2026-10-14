//! One deterministic transport fixture exercises the editor's actual RPC/UI path.
use crate::test_support::TempDir;
use bedtime_documents::{DocumentId, DocumentSession, SessionOptions};
use serde_json::json;
use std::{
    thread,
    time::{Duration, Instant},
};
const SERVER: &str = r#"import json, sys, time

def send(value):
    body=json.dumps(value).encode()
    sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(body)).encode()+body)
    sys.stdout.buffer.flush()
mode=sys.argv[2]
trace=sys.argv[3]
completion_count=0
command_id=None
while True:
    length=None
    while True:
        line=sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if line==b'\r\n': break
        if line.lower().startswith(b'content-length:'): length=int(line.split(b':',1)[1])
    msg=json.loads(sys.stdin.buffer.read(length))
    method=msg.get('method')
    with open(trace,'a') as out: out.write('%s\n'%method)
    if msg.get('id')=='fixture-apply':
        send({'jsonrpc':'2.0','id':command_id,'result':None})
        continue
    if 'id' not in msg: continue
    result=None
    if method=='initialize':
        result={'capabilities':{'textDocumentSync':2,'hoverProvider':mode.startswith('hover'),'completionProvider':{'resolveProvider':True,'triggerCharacters':['.']},'documentFormattingProvider':True,'renameProvider':{'prepareProvider':True},'codeActionProvider':{'resolveProvider':True},'executeCommandProvider':{'commands':['fixture.fix']}}}
    elif method=='textDocument/hover':
        if mode=='hover-delayed': time.sleep(.1)
        position=msg['params']['position']
        with open(trace,'a') as out: out.write('hover:%s:%s\n'%(position['line'],position['character']))
        if position['character']==12:
            result={'contents':{'kind':'markdown','value':'```rust\npub fn St()\n```\n\nA **Unicode** `string`.'}}
        elif position['character']==13:
            result={'contents':{'kind':'plaintext','value':'Literal **text** with `punctuation`.'}}
    elif method=='textDocument/completion':
        completion_count+=1
        with open(trace,'a') as out: out.write('completion:%s\n'%msg['params']['context']['triggerKind'])
        if mode=='delayed' or (mode=='incomplete' and completion_count>1): time.sleep(.1)
        position=msg['params']['position']
        end=position['character']
        result={'isIncomplete':mode=='incomplete' and completion_count==1,'itemDefaults':{'editRange':{'start':{'line':0,'character':12},'end':{'line':0,'character':end}}},'items':[{'label':'String','sortText':'a','data':'string'},{'label':'Strange','sortText':'b'}]}
        if mode=='commands': result['items'][0]['command']={'title':'Fix completion','command':'fixture.fix'}
    elif method=='completionItem/resolve':
        result=msg['params']
        result['documentation']={'kind':'markdown','value':'A **Unicode** string. Documentation wraps across styled runs and remains readable in narrow views. '*8}
        if result['label']=='String':
            result['additionalTextEdits']=[{'range':{'start':{'line':0,'character':0},'end':{'line':0,'character':0}},'newText':'use String;\n'}]
    elif method=='workspace/executeCommand':
        command_id=msg['id']
        send({'jsonrpc':'2.0','id':'fixture-apply','method':'workspace/applyEdit','params':{'edit':{'changes':{sys.argv[1]:[{'range':{'start':{'line':1,'character':18},'end':{'line':1,'character':18}},'newText':'!'}]}}}})
        continue
    elif method=='textDocument/formatting':
        result=[{'range':{'start':{'line':0,'character':0},'end':{'line':0,'character':14}},'newText':'let value = St;'}]
    elif method=='textDocument/prepareRename':
        result={'range':{'start':{'line':0,'character':12},'end':{'line':0,'character':14}},'placeholder':'St'}
    elif method=='textDocument/rename':
        result={'changes':{msg['params']['textDocument']['uri']:[{'range':{'start':{'line':0,'character':12},'end':{'line':0,'character':14}},'newText':msg['params']['newName']}]}}
    elif method=='textDocument/codeAction':
        result=[{'title':'Fix value','kind':'quickfix','isPreferred':True,'data':1},{'title':'Disabled','kind':'quickfix','disabled':{'reason':'Unavailable here'}}]
    elif method=='codeAction/resolve':
        result=msg['params']
        result['edit']={'changes':{sys.argv[1]:[{'range':{'start':{'line':0,'character':12},'end':{'line':0,'character':14}},'newText':'Fixed'}]}}
    send({'jsonrpc':'2.0','id':msg['id'],'result':result})
"#;
pub(super) fn session() -> (TempDir, DocumentSession, DocumentId) {
    session_mode("normal")
}

pub(crate) fn session_mode(mode: &str) -> (TempDir, DocumentSession, DocumentId) {
    let directory = TempDir::new();
    let path = directory
        .write("test.rs", b"let value = St")
        .canonicalize()
        .unwrap();
    let uri = bedtime_lsp::lsp_uri::LspUri::file_uri_from_path(path.to_str().unwrap())
        .unwrap()
        .to_string();
    let script = directory.write("server.py", SERVER.as_bytes());
    let config = directory.write("lsp.json",&serde_json::to_vec(&json!({
        "languages":[{"name":"rust","language_id":"rust","file_types":["rs"],"language_server":"fixture"}],
        "language_servers":{"fixture":{"command":"/usr/bin/env","args":["python3","-u",script,uri,mode,directory.path("trace")]}}
    })).unwrap());
    let mut session = DocumentSession::with_options(SessionOptions {
        project_root: Some(directory.root().to_owned()),
        lsp_config: Some(config),
        ..Default::default()
    })
    .unwrap();
    let document = session.open_file(&path).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !session
        .lsp()
        .unwrap()
        .client_for_document(document)
        .is_some_and(|client| client.borrow().is_initialized())
    {
        session.tick();
        assert!(
            Instant::now() < deadline,
            "Fixture server did not initialize"
        );
        thread::sleep(Duration::from_millis(5));
    }
    (directory, session, document)
}
