//! Opt-in compatibility probes: cargo test -p bed-lsp --test real_servers -- --ignored.
//! These require the named server, its compiler/toolchain, and no network dependencies.
use bed_editing::editor_events::DocumentChange;
use bed_lsp::{lsp_client::LspClient, lsp_config::LspConfig, lsp_uri::LspUri};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    fs,
    path::PathBuf,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "bed-real-lsp-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self(fs::canonicalize(root).unwrap())
    }
    fn write(&self, name: &str, text: &str) -> String {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path.to_string_lossy().into_owned()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn wait(
    client: &mut LspClient,
    ready: impl Fn(&LspClient) -> bool,
    stage: &str,
    deadline: Instant,
) {
    loop {
        client.poll();
        if ready(client) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{stage}: {:?}\n{}",
            client.last_error(),
            client.stderr_text()
        );
        assert!(
            client.is_process_started(),
            "{stage}: server exited: {:?}\n{}",
            client.last_error(),
            client.stderr_text()
        );
        thread::sleep(Duration::from_millis(10));
    }
}
fn semantic_reply(client: &mut LspClient, method: &str, params: &Value) -> Value {
    assert!(
        client.supports_method(method),
        "server does not advertise {method}"
    );
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let result = Rc::new(RefCell::new(None));
        let received = result.clone();
        client
            .send_request(method, params.clone(), move |value| {
                *received.borrow_mut() = Some(value)
            })
            .unwrap();
        wait(client, |_| result.borrow().is_some(), method, deadline);
        let reply = result.borrow_mut().take().unwrap();
        if let Ok(value) = &reply {
            if !value.is_null() && !matches!(value, Value::Array(values) if values.is_empty()) {
                return value.clone();
            }
        }
        assert!(
            Instant::now() < deadline,
            "{method} never resolved: {reply:?}\n{}",
            client.stderr_text()
        );
        thread::sleep(Duration::from_millis(50));
    }
}
fn exercise(
    project: &Project,
    path: &str,
    source: &str,
    language_id: &str,
    server: &str,
    row: u32,
    bad_source: Option<&str>,
) {
    let config = LspConfig::from_layers(None, None).unwrap();
    let detected = config
        .detect_language_with_content(path, source.as_bytes())
        .unwrap();
    assert_eq!(detected.language_id, language_id);
    assert_eq!(detected.language_server.as_deref(), Some(server));
    assert!(
        config.find_server_path(server).is_some(),
        "Install {server} and its toolchain before running this ignored test"
    );
    let mut client = LspClient::with_config(project.0.join("no-user-config.json"), config);
    client.set_workspace(project.0.to_str().unwrap());
    client.set_project_check_owner(true);
    assert!(client.start_server(server, "").unwrap());
    client
        .did_open(path, source.as_bytes(), 0, language_id)
        .unwrap();
    wait(
        &mut client,
        LspClient::is_initialized,
        "initialization",
        Instant::now() + Duration::from_secs(45),
    );
    assert!(client.is_document_open(path));
    let line = source.lines().nth(row as usize).unwrap();
    let character = line.find("square").unwrap() as u32 + 2;
    let params = json!({"textDocument":{"uri":LspUri::file_uri_from_path(path).unwrap().to_string()},"position":{"line":row,"character":character}});
    let hover = semantic_reply(&mut client, "textDocument/hover", &params);
    assert!(
        hover
            .get("contents")
            .is_some_and(|contents| contents.to_string().contains("square")),
        "invalid hover: {hover}"
    );
    if client.supports_method("textDocument/definition") {
        let definition = semantic_reply(&mut client, "textDocument/definition", &params);
        assert!(
            definition.is_object() || definition.is_array(),
            "invalid definition: {definition}"
        );
        let location = definition
            .as_array()
            .map(|locations| &locations[0])
            .unwrap_or(&definition);
        let uri = location.get("uri").or_else(|| location.get("targetUri"));
        assert_eq!(
            uri,
            Some(&params["textDocument"]["uri"]),
            "definition points outside the fixture: {definition}"
        );
        let range = location
            .get("range")
            .or_else(|| location.get("targetSelectionRange"))
            .unwrap();
        assert_eq!(
            range["start"]["line"], 0,
            "definition does not point to square: {definition}"
        );
    }
    if let Some(bad_source) = bad_source {
        client
            .did_change(
                path,
                1,
                &[DocumentChange {
                    start_line: 0,
                    start_character: 0,
                    end_line: source.bytes().filter(|byte| *byte == b'\n').count() as i32,
                    end_character: 0,
                    text: bad_source.as_bytes().to_vec(),
                }],
                || Ok(bad_source.to_owned()),
            )
            .unwrap();
        assert_eq!(fs::read(path).unwrap(), source.as_bytes());
        wait(
            &mut client,
            |client| {
                client
                    .diagnostics()
                    .for_document(path)
                    .iter()
                    .any(|diagnostic| diagnostic.severity == 1)
            },
            "edited-document diagnostics",
            Instant::now() + Duration::from_secs(30),
        );
        client
            .did_change(
                path,
                2,
                &[DocumentChange {
                    start_line: 0,
                    start_character: 0,
                    end_line: bad_source.bytes().filter(|byte| *byte == b'\n').count() as i32,
                    end_character: 0,
                    text: source.as_bytes().to_vec(),
                }],
                || Ok(source.to_owned()),
            )
            .unwrap();
        wait(
            &mut client,
            |client| {
                !client
                    .diagnostics()
                    .for_document(path)
                    .iter()
                    .any(|diagnostic| diagnostic.severity == 1)
            },
            "corrected-document diagnostics",
            Instant::now() + Duration::from_secs(30),
        );
    }
    client.did_close(path).unwrap();
    client.shutdown();
}

#[test]
#[ignore = "requires clangd and a C compiler toolchain"]
fn clangd_initializes_and_serves_c_hover_navigation_and_edit_diagnostics() {
    let project = Project::new();
    project.write("compile_flags.txt", "-std=c11\n");
    let source =
        "int square(int value) { return value * value; }\nint main(void) { return square(3); }\n";
    let path = project.write("main.c", source);
    exercise(
        &project,
        &path,
        source,
        "c",
        "clangd",
        1,
        Some("int main(void) { return unknown_bed_symbol; }\n"),
    );
}

#[test]
#[ignore = "requires rust-analyzer, Cargo and the Rust toolchain"]
fn rust_analyzer_serves_hover_navigation_and_unsaved_syntax_diagnostics() {
    let project = Project::new();
    project.write(
        "Cargo.toml",
        "[package]\nname=\"bed_lsp_probe\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    );
    let source = "fn square(value: i32) -> i32 { value * value }\nfn main() { let answer = square(3); let _ = answer; }\n";
    let path = project.write("src/main.rs", source);
    exercise(
        &project,
        &path,
        source,
        "rust",
        "rust-analyzer",
        1,
        Some("fn main() { let broken = ; }\n"),
    );
}

#[test]
#[ignore = "requires sourcekit-lsp and the Swift toolchain"]
fn sourcekit_initializes_and_serves_swift_hover_and_navigation() {
    let project = Project::new();
    project.write("Package.swift", "// swift-tools-version:6.0\nimport PackageDescription\nlet package = Package(name: \"BedLspProbe\", targets: [.executableTarget(name: \"BedLspProbe\")])\n");
    let source = "func square(_ value: Int) -> Int { value * value }\nlet answer = square(3)\n";
    let path = project.write("Sources/BedLspProbe/main.swift", source);
    exercise(&project, &path, source, "swift", "sourcekit-lsp", 1, None);
}
