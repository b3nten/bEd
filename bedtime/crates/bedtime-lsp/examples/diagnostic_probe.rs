//! Characterize native diagnostic latency without autosave or cargo check.
//! Runs against the installed rust-analyzer, using only temporary offline crates.
#[cfg(unix)]
use bedtime_lsp::*;
#[cfg(unix)]
use serde_json::json;
#[cfg(unix)]
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[cfg(not(unix))]
fn main() {
    eprintln!("The local stdio diagnostic probe requires Unix.");
}

#[cfg(unix)]
fn main() {
    for experimental in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "bedtime-lsp-probe-{}-{experimental}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='diagnostic_probe'\nversion='0.1.0'\nedition='2024'\n[workspace]\n",
        )
        .unwrap();
        let initial = "pub fn probe() -> String { String::new() }\n";
        std::fs::write(root.join("src/lib.rs"), initial).unwrap();
        let options = json!({"checkOnSave":false,"cargo":{"buildScripts":{"enable":false}},"procMacro":{"enable":false},"diagnostics":{"experimental":{"enable":experimental}}});
        let (handle, worker) = ServerHandle::prepare(Launch {
            root: root.clone(),
            config: ServerConfig {
                environment: [("CARGO_NET_OFFLINE".into(), "true".into())].into(),
                initialization_options: options.clone(),
                settings: json!({"rust-analyzer":options}),
                ..Default::default()
            },
        })
        .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation = cancelled.clone();
        let worker = thread::spawn(move || worker.run(|| cancellation.load(Ordering::Acquire)));
        let key = DocumentKey {
            id: 1,
            generation: 1,
        };
        let uri = FileUri::from_path(&root.join("src/lib.rs")).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let update = |text: &str, revision| {
                handle
                    .update(DocumentUpdate {
                        key,
                        uri: uri.clone(),
                        language: "rust".into(),
                        revision,
                        text: text.into(),
                        changes: None,
                    })
                    .unwrap()
            };
            update(initial, 0);
            let startup = Instant::now();
            let mut ready = false;
            while startup.elapsed() < Duration::from_secs(45) && !ready {
                while let Ok(event) = handle.try_recv() {
                    if let Event::Diagnostics { report, .. } = event {
                        ready = report.revision == 0;
                    }
                }
                assert!(
                    !matches!(handle.status(), ServerStatus::Failed(_)),
                    "{:?}",
                    handle.status()
                );
                thread::sleep(Duration::from_millis(5));
            }
            assert!(ready, "No initial diagnostic report: {:?}", handle.status());
            println!(
                "{}",
                json!({"experimental":experimental,"startup_ms":startup.elapsed().as_millis(),"server":handle.activity().server_name,"version":handle.activity().server_version})
            );
            // No saved notifications or disk writes follow these updates.
            for (revision, case, text, needle, seconds) in [
                (
                    1,
                    "associated item",
                    "pub fn probe() { String::foo(); }\n",
                    "associated item",
                    5,
                ),
                (2, "syntax", "pub fn broken( {\n", "Syntax Error", 5),
            ] {
                let sent = Instant::now();
                update(text, revision);
                let mut found = None;
                let mut messages = Vec::new();
                while sent.elapsed() < Duration::from_secs(seconds) && found.is_none() {
                    while let Ok(event) = handle.try_recv() {
                        if let Event::Diagnostics { report, .. } = event {
                            if report.revision == revision {
                                messages = report
                                    .items()
                                    .iter()
                                    .map(|d| d.message.to_string())
                                    .collect();
                                if messages
                                    .iter()
                                    .any(|m| m.to_lowercase().contains(&needle.to_lowercase()))
                                {
                                    found = Some(sent.elapsed().as_millis());
                                }
                            }
                        }
                    }
                    assert!(
                        !matches!(handle.status(), ServerStatus::Failed(_)),
                        "{:?}",
                        handle.status()
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                println!(
                    "{}",
                    json!({"experimental":experimental,"case":case,"latency_ms":found,"observation_window_s":seconds,"messages":messages})
                );
            }
            assert_eq!(
                std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
                initial
            );
        }));
        cancelled.store(true, Ordering::Release);
        worker.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}
