//! Opt-in acceptance against an installed headless agent and Rust language server.
use bed_lsp::{
    lsp_client::LspClient,
    lsp_config::{LanguageServerInfo, LspConfig},
    lsp_uri::LspUri,
};
use bed_remote::SshTarget;
use serde_json::json;
use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[test]
#[ignore = "requires BED_TEST_SSH_HOST, BED_TEST_SSH_AGENT, and BED_TEST_SSH_ROOT"]
fn remote_rust_server_initializes_and_reads_local_unsaved_document() {
    let host = std::env::var("BED_TEST_SSH_HOST").unwrap();
    let agent = std::env::var("BED_TEST_SSH_AGENT").unwrap();
    let root = std::env::var("BED_TEST_SSH_ROOT").unwrap();
    let path = format!("{}/src/lib.rs", root.trim_end_matches('/'));
    let mut client = LspClient::with_config(
        "/intentionally/no/local/lsp.json",
        LspConfig {
            language_servers: vec![LanguageServerInfo {
                language: "rust".into(),
                file_extensions: vec![".rs".into()],
                server_paths: vec!["rust-analyzer".into()],
                server_args: Vec::new(),
            }],
        },
    );
    client.set_ssh_target(Some(SshTarget { host, agent }));
    client.set_workspace(&root);
    assert!(client.init(&path).unwrap());
    client
        .did_open(
            &path,
            b"pub fn unsaved_remote_symbol() -> u32 { 42 }\n",
            1,
            "rust",
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(45);
    while !client.is_initialized() {
        client.poll();
        assert!(
            client.is_process_started(),
            "{:?}\n{}",
            client.last_error(),
            client.stderr_text()
        );
        assert!(
            Instant::now() < deadline,
            "initialization timed out: {:?}\n{}",
            client.last_error(),
            client.stderr_text()
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(client.is_document_open(&path));
    let (sender, receiver) = mpsc::channel();
    client
        .send_request(
            "textDocument/documentSymbol",
            json!({"textDocument":{"uri":LspUri::file_uri_from_remote_path(&path).unwrap().to_string()}}),
            move |result| {
                let _ = sender.send(result);
            },
        )
        .unwrap();
    let result = loop {
        client.poll();
        if let Ok(result) = receiver.try_recv() {
            break result;
        }
        assert!(Instant::now() < deadline, "symbol response timed out");
        thread::sleep(Duration::from_millis(10));
    };
    let result = result.unwrap();
    assert!(
        result.to_string().contains("unsaved_remote_symbol"),
        "{result}"
    );
    client.shutdown();
}
