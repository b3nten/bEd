#![cfg(unix)]
//! Optional local-toolchain characterization; no network or project dependencies.
use bedtime_editor::TextBuffer;
use bedtime_lsp::*;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[test]
#[ignore = "requires installed rust-analyzer, gopls and their local toolchains"]
fn real_rust_and_go_hover_completion_and_resolve() {
    for language in ["rust", "go"] {
        let root = std::env::temp_dir().join(format!(
            "bedtime-intelligence-{}-{language}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let (path, text, program, settings) = if language == "rust" {
            std::fs::create_dir_all(root.join("src")).unwrap();
            std::fs::write(root.join("Cargo.toml"),"[package]\nname='intelligence_fixture'\nversion='0.1.0'\nedition='2024'\n[workspace]\n").unwrap();
            (
                root.join("src/main.rs"),
                "fn answer() -> u32 { 42 }\nfn main() { let _ = answer(); let _v: Vec<u8> = Vec::new(); }\n",
                "rust-analyzer".to_owned(),
                serde_json::json!({"rust-analyzer":{"checkOnSave":false,"procMacro":{"enable":false}}}),
            )
        } else {
            std::fs::write(
                root.join("go.mod"),
                "module example.com/intelligence\n\ngo 1.23\n",
            )
            .unwrap();
            (
                root.join("main.go"),
                "package main\nfunc answer() int { return 42 }\nfunc main() { _ = answer() }\n",
                home.join("go/bin/gopls").to_string_lossy().into_owned(),
                serde_json::json!({}),
            )
        };
        std::fs::write(&path, text).unwrap();
        let config = ServerConfig {
            program,
            settings,
            environment: [
                (
                    "PATH".into(),
                    format!(
                        "/usr/local/go/bin:{}",
                        std::env::var("PATH").unwrap_or_default()
                    ),
                ),
                ("CARGO_NET_OFFLINE".into(), "true".into()),
                ("GOPROXY".into(), "off".into()),
                ("GOSUMDB".into(), "off".into()),
                ("GOTOOLCHAIN".into(), "local".into()),
                (
                    "GOCACHE".into(),
                    root.join("go-cache").to_string_lossy().into_owned(),
                ),
                (
                    "XDG_CACHE_HOME".into(),
                    root.join("cache").to_string_lossy().into_owned(),
                ),
            ]
            .into(),
            ..Default::default()
        };
        let (handle, worker) = ServerHandle::prepare(Launch {
            root: root.clone(),
            config,
        })
        .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let worker = thread::spawn(move || worker.run(|| stop.load(Ordering::Acquire)));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let key = DocumentKey {
                id: 1,
                generation: 1,
            };
            handle
                .update(DocumentUpdate {
                    key,
                    uri: FileUri::from_path(&path).unwrap(),
                    language: language.into(),
                    revision: 0,
                    text: text.into(),
                    changes: None,
                })
                .unwrap();
            let until = Instant::now() + Duration::from_secs(45);
            while handle.status() != ServerStatus::Ready {
                assert!(Instant::now() < until, "{language}: {:?}", handle.status());
                thread::sleep(Duration::from_millis(10));
            }
            let byte = text.rfind("answer").unwrap();
            let mut ticket = 0;
            let mut query = |request: LanguageQuery| {
                ticket += 1;
                let kind = request.kind();
                handle
                    .query(
                        RequestOrigin {
                            document: key,
                            revision: 0,
                            panel: 1,
                            ticket,
                        },
                        request,
                    )
                    .unwrap();
                let deadline = Instant::now() + Duration::from_secs(20);
                loop {
                    if let Ok(Event::Intelligence {
                        kind: k, result, ..
                    }) = handle.try_recv()
                    {
                        if k == kind {
                            return result.unwrap();
                        }
                    }
                    assert!(
                        Instant::now() < deadline,
                        "{language}: request timed out {:?}",
                        handle.status()
                    );
                    thread::sleep(Duration::from_millis(10));
                }
            };
            loop {
                if let Intelligence::Hover(Some(hover)) = query(LanguageQuery::Hover { byte }) {
                    assert!(
                        hover.contents.text.contains("answer"),
                        "{}",
                        hover.contents.text
                    );
                    break;
                }
                assert!(Instant::now() < until, "{language}: no hover");
                thread::sleep(Duration::from_millis(100));
            }
            let Intelligence::Completion(list) = query(LanguageQuery::Completion {
                byte: byte + 3,
                context: completion_context(None, false),
            }) else {
                panic!("no completions")
            };
            let item = list
                .items
                .iter()
                .find(|item| item.label.starts_with("answer"))
                .unwrap_or_else(|| {
                    panic!(
                        "{language}: {:?}",
                        list.items.iter().map(|i| &i.label).collect::<Vec<_>>()
                    )
                });
            let prepared = item.edit.prepare(&TextBuffer::from(text)).unwrap();
            let mut accepted = TextBuffer::from(text);
            prepared.edit.apply(&mut accepted).unwrap();
            assert!(accepted.to_string().contains("answer"));
            if handle.activity().supports_completion_resolve {
                let Intelligence::Resolved(item) = query(LanguageQuery::Resolve {
                    byte: byte + 3,
                    item: item.raw.clone(),
                }) else {
                    panic!("no resolved completion")
                };
                item.edit.prepare(&TextBuffer::from(text)).unwrap();
            }
            if language == "rust" {
                for (symbol, position) in [
                    ("Vec", text.rfind("Vec::").unwrap()),
                    ("new", text.rfind("new()").unwrap()),
                ] {
                    let deadline = Instant::now() + Duration::from_secs(30);
                    loop {
                        if let Intelligence::Hover(Some(hover)) =
                            query(LanguageQuery::Hover { byte: position })
                        {
                            assert!(
                                hover.contents.text.contains(symbol),
                                "{symbol}: {}",
                                hover.contents.text
                            );
                            break;
                        }
                        assert!(
                            Instant::now() < deadline,
                            "No standard-library hover for {symbol}"
                        );
                        thread::sleep(Duration::from_millis(100));
                    }
                    handle
                        .definition(
                            RequestOrigin {
                                document: key,
                                revision: 0,
                                panel: 1,
                                ticket: 1000,
                            },
                            position,
                        )
                        .unwrap();
                    loop {
                        if let Ok(Event::Definition { result, .. }) = handle.try_recv() {
                            let locations = result.unwrap();
                            assert!(!locations.is_empty(), "No definition for {symbol}");
                            assert!(
                                locations.iter().all(|l| l.uri.path().is_file()),
                                "Standard-library definition is not a readable file"
                            );
                            eprintln!(
                                "rust: {symbol} hover and definition: {}",
                                locations[0].uri.path().display()
                            );
                            break;
                        }
                        assert!(
                            Instant::now() < deadline,
                            "No definition response for {symbol}"
                        );
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            }
            assert_eq!(handle.status(), ServerStatus::Ready);
            eprintln!(
                "{language}: hover, completion and supported resolution passed ({:?})",
                handle
                    .activity()
                    .server_version
                    .as_deref()
                    .map(|s| s.chars().take(80).collect::<String>())
            );
        }));
        cancel.store(true, Ordering::Release);
        worker.join().unwrap();
        let _ = std::fs::remove_dir_all(root);
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}
