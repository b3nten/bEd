//! Fake rustup/proxy and a minimal LSP server. Never touches real toolchains.
#![allow(dead_code)]
#[path = "temp_dir.rs"]
mod temp_dir;
use bedtime_lsp::lsp_config::LspConfig;
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
};

pub const TOOLCHAIN: &str = "1.99.0-aarch64-apple-darwin";

pub struct RustupFixture {
    pub dir: temp_dir::TempDir,
    pub config: PathBuf,
}

impl RustupFixture {
    pub fn new() -> Self {
        let dir = temp_dir::TempDir::new();
        let program = dir.write(
            "rustup",
            format!("#!/usr/bin/env python3\n{SCRIPT}").as_bytes(),
        );
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        symlink("rustup", dir.path("rust-analyzer")).unwrap();
        let config = dir.write("lsp.json", serde_json::to_vec(&json!({
            "languages":[{"name":"rust","language_id":"rust","file_types":["rs"],"language_server":"rust-analyzer"}],
            "language_servers":{"rust-analyzer":{
                "command":dir.path("rust-analyzer"),
                "environment":{"BED_FIXTURE_LITERAL":"a'b $HOME $(touch never-created) `echo nope`", "RUSTUP_TOOLCHAIN":""}
            }}
        })).unwrap().as_slice());
        Self { dir, config }
    }

    pub fn root(&self, name: &str) -> PathBuf {
        let root = self.dir.path(name);
        fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }

    pub fn config(&self) -> LspConfig {
        LspConfig::load(&self.config).unwrap()
    }

    pub fn touch(&self, name: &str) {
        self.dir.write(name, b"");
    }

    pub fn calls(&self) -> Vec<Value> {
        fs::read_to_string(self.dir.path("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    pub fn messages(&self, root: &Path) -> Vec<Value> {
        fs::read_to_string(root.join("messages.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

const SCRIPT: &str = r#"
import json, os, pathlib, sys, time
base=pathlib.Path(__file__).resolve().parent
root=pathlib.Path.cwd()
selected=os.environ.get('RUSTUP_TOOLCHAIN') or ((root/'selection').read_text() if (root/'selection').exists() else '1.99.0-aarch64-apple-darwin')
args=sys.argv[1:]
installed=(base/'installed').exists()

def failure():
    if (root/'unrelated').exists(): return "error: rust-analyzer crashed for an unrelated reason"
    if (root/'missing-toolchain').exists(): return "error: override toolchain '%s' is not installed: the RUSTUP_TOOLCHAIN environment variable specifies an uninstalled toolchain"%selected
    return "error: Unknown binary 'rust-analyzer' in official toolchain '%s'."%selected

if pathlib.Path(sys.argv[0]).name=='rust-analyzer':
    if not installed:
        print(failure(), file=sys.stderr, flush=True)
        sys.exit(1)
    while True:
        length=None
        while True:
            line=sys.stdin.buffer.readline()
            if not line: sys.exit(0)
            if line in (b'\r\n',b'\n'): break
            if line.lower().startswith(b'content-length:'): length=int(line.split(b':',1)[1])
        message=json.loads(sys.stdin.buffer.read(length))
        with (root/'messages.jsonl').open('a') as stream: stream.write(json.dumps(message)+'\n')
        method=message.get('method')
        if method=='exit': sys.exit(0)
        if 'id' in message:
            result={'capabilities':{'textDocumentSync':{'openClose':True,'change':1}}} if method=='initialize' else None
            body=json.dumps({'jsonrpc':'2.0','id':message['id'],'result':result}).encode()
            sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(body)).encode()+body)
            sys.stdout.buffer.flush()
    sys.exit(0)

with (base/'calls.jsonl').open('a') as stream:
    stream.write(json.dumps({'args':args, 'cwd':str(root), 'auto_install':os.environ.get('RUSTUP_AUTO_INSTALL'), 'literal':os.environ.get('BED_FIXTURE_LITERAL')})+'\n')
if args==['show','active-toolchain']:
    if (base/'hang-query').exists(): time.sleep(60)
    if (root/'missing-toolchain').exists() and not installed:
        print(failure(),file=sys.stderr); sys.exit(1)
    print(selected+" (overridden by 'fixture')")
elif args==['toolchain','list']:
    print('stable-aarch64-apple-darwin (default)')
elif args[:2]==['component','list']:
    if (base/'unavailable').exists():
        print('rust-src')
    else:
        print('rust-analyzer-aarch64-apple-darwin'+(' (installed)' if installed else ''))
        print('rust-src')
elif args[:2] in (['component','add'],['toolchain','install']):
    print('info: downloading component rust-analyzer',file=sys.stderr,flush=True)
    (base/'install-started').touch()
    deadline=time.monotonic()+15
    while (base/'wait-install').exists() and not (base/'allow-install').exists():
        if time.monotonic()>deadline: sys.exit(43)
        time.sleep(.01)
    if (base/'install-fails').exists():
        print('error: download failed (fixture)',file=sys.stderr); sys.exit(42)
    (base/'installed').touch()
    if (base/'burst-output').exists(): print('x'*(128*1024),file=sys.stderr)
    print('info: installation complete',file=sys.stderr)
else:
    print('unexpected arguments: '+repr(args),file=sys.stderr); sys.exit(90)
"#;
