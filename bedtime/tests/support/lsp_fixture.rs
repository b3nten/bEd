// Client fixtures shared by the LSP layers of the integration suite.
use bedtime_editing::identity::SessionId;
use bedtime_lsp::{
    jsonrpc::ResponseError, lsp_client::LspClient, lsp_uri::LspUri, message_handler::RpcSession,
    workspace_lsp::WorkspaceLsp,
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    fs,
    path::PathBuf,
    rc::Rc,
    sync::atomic::Ordering,
    thread,
    time::{Duration, Instant},
};

use crate::LSP_FIXTURE_ID as TEMP_ID;
#[allow(dead_code)] // Only the transport-layer tests inspect the stderr sentinel.
const STDERR_DRAIN_SENTINEL: &str = "\n[stderr complete]\n";

struct Fixture {
    root: PathBuf,
    client: LspClient,
    document: String,
}

#[allow(dead_code)] // This shared fixture serves targets exercising different layers.
impl Fixture {
    fn new(scenario: &str) -> Self {
        Self::with_server(scenario, "mock", json!({}))
    }

    fn with_server(scenario: &str, server: &str, settings: Value) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bed-lsp-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let config = root.join("lsp.json");
        fs::write(&config, serde_json::to_vec(&json!({
            "languages":[{"name":"mock","language_id":"mock","file_types":["rs"],"language_server":server}],
            "language_servers":{(server):{"command":crate::fixtures::executable(),"args":["--mock-server",scenario],"settings":settings}}
        })).unwrap()).unwrap();
        let document = root.join("sp ace😀.rs").to_string_lossy().into_owned();
        let mut client = LspClient::new(config);
        client.set_workspace(root.to_str().unwrap());
        assert!(client.init(&document).unwrap());
        Self {
            root,
            client,
            document,
        }
    }

    fn ready(&mut self) {
        wait_until(|| {
            self.client.poll();
            self.client.is_initialized()
        });
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, ResponseError> {
        let result = Rc::new(RefCell::new(None));
        let delivered = result.clone();
        self.client
            .send_request(method, params, move |reply| {
                *delivered.borrow_mut() = Some(reply);
            })
            .unwrap();
        wait_until(|| {
            self.client.poll();
            result.borrow().is_some()
        });
        result.borrow_mut().take().unwrap()
    }

    fn transcript(&mut self) -> Vec<Value> {
        self.request("test/transcript", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.client.shutdown();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[allow(dead_code)] // This shared fixture serves targets exercising different layers.
fn session(scenario: &str, extra: &[String]) -> RpcSession {
    let mut arguments = vec!["--mock-server".into(), scenario.into()];
    arguments.extend_from_slice(extra);
    RpcSession::start(&crate::fixtures::executable(), &arguments).unwrap()
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline, "mock language server timed out");
        thread::sleep(Duration::from_millis(2));
    }
}

#[allow(dead_code)] // This shared fixture serves targets exercising different layers.
fn rpc_request(session: &mut RpcSession, method: &str) -> Result<Value, ResponseError> {
    let result = Rc::new(RefCell::new(None));
    let delivered = result.clone();
    session
        .send_request(method, Some(json!({})), move |reply| {
            *delivered.borrow_mut() = Some(reply);
        })
        .unwrap();
    wait_until(|| {
        session.poll();
        result.borrow().is_some()
    });
    result.borrow_mut().take().unwrap()
}

fn shared_request(client: &Rc<RefCell<LspClient>>, method: &str) -> Value {
    shared_request_params(client, method, json!({}))
}

fn shared_request_params(client: &Rc<RefCell<LspClient>>, method: &str, params: Value) -> Value {
    let reply = Rc::new(RefCell::new(None));
    let delivered = reply.clone();
    client
        .borrow_mut()
        .send_request(method, params, move |result| {
            *delivered.borrow_mut() = Some(result);
        })
        .unwrap();
    wait_until(|| {
        client.borrow_mut().poll();
        reply.borrow().is_some()
    });
    reply.borrow_mut().take().unwrap().unwrap()
}

struct PoolFixture {
    root: PathBuf,
    pool: WorkspaceLsp,
}

#[allow(dead_code)] // This shared fixture serves targets exercising different layers.
impl PoolFixture {
    fn new(id: u64, scenario: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bed-lsp-pool-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let config = root.join("lsp.json");
        fs::write(&config, serde_json::to_vec(&json!({
            "languages":[
                {"name":"cpp","language_id":"cpp","file_types":["cpp"],"language_server":"cpp"},
                {"name":"rust","language_id":"rust","file_types":["rs"],"language_server":"rust"}
            ],
            "language_servers":{
                "cpp":{"command":crate::fixtures::executable(),"args":["--mock-server",scenario]},
                "rust":{"command":crate::fixtures::executable(),"args":["--mock-server",scenario]}
            }
        })).unwrap()).unwrap();
        let pool = WorkspaceLsp::new(SessionId(id), config, root.clone());
        Self { root, pool }
    }

    fn path(&self, name: &str) -> String {
        self.root.join(name).to_string_lossy().into_owned()
    }

    fn ready(&mut self) {
        wait_until(|| {
            self.pool.poll();
            self.pool
                .server_statuses()
                .iter()
                .filter(|status| status.process_id.is_some())
                .all(|status| status.initialized)
        });
    }

    fn transcript(&self, language: &str) -> Vec<Value> {
        shared_request(
            &self.pool.client_for_language(language).unwrap(),
            "test/transcript",
        )
        .as_array()
        .unwrap()
        .clone()
    }
}

impl Drop for PoolFixture {
    fn drop(&mut self) {
        self.pool.shutdown();
        let _ = fs::remove_dir_all(&self.root);
    }
}
