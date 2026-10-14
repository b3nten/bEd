//! Exercise service workers against a real framed subprocess without SSH setup.
use crate::temp_dir as test_support;
use bedtime_files::{
    content_search::{ContentSearch, SearchBuffer},
    file_finder::FileFinder,
};
use bedtime_remote::RemoteClient;
use std::{
    thread,
    time::{Duration, Instant},
};

#[test]
fn remote_files_search_and_workspace_updates() {
    let client = RemoteClient::launch_command(crate::fixtures::command("remote-services")).unwrap();
    let mut finder = FileFinder::new();
    finder.set_file_services(bedtime_files::FileServices::remote(
        "fixture",
        client.clone(),
    ));
    finder.set_project_dir("/remote/project");
    finder.start_background_thread();
    wait_until(|| finder.poll());
    finder.toggle_window();
    assert_eq!(finder.filtered_list[0].full_path, "/remote/project/file.rs");
    assert_eq!(finder.filtered_list[0].relative_path, "file.rs");

    let mut search = ContentSearch::new_lazy();
    search.set_file_services(bedtime_files::FileServices::remote(
        "fixture",
        client.clone(),
    ));
    search.include_ignored = true;
    search.start("/remote/project", "needle", false);
    wait_until(|| {
        search.poll();
        !search.searching
    });
    assert!(search.error.is_none(), "{:?}", search.error);
    assert_eq!(search.results.len(), 1);
    assert_eq!(search.progress.scanned_files, 3);
    assert_eq!(search.progress.discovered_files, 3);
    assert_eq!(search.progress.ignored_paths, 2);
    assert_eq!(search.skipped_files, 1);
    let found = &search.results[0];
    assert_eq!((found.source_row, found.row, found.column), (1, 4, 2));
    assert_eq!(&*found.line, b"\xff needle");
    assert_eq!(found.file.relative_path, "ignored/file.rs");

    let temp = test_support::TempDir::new();
    let root = std::fs::canonicalize(temp.root()).unwrap();
    let context = bedtime_files::FileContext::remote("fixture", root.clone()).unwrap();
    let services = bedtime_files::FileServices::remote("fixture", client.clone());
    let location = context.location("facade.bin");
    let baseline = services
        .write_atomic(&root, &location, b"\0initial\xff", None)
        .unwrap();
    assert_eq!(services.read(&root, &location).unwrap().0, b"\0initial\xff");
    assert_eq!(
        services.canonicalize(&root, &location, false).unwrap(),
        location
    );
    std::fs::write(&location.path, b"external").unwrap();
    assert!(
        services
            .write_atomic(&root, &location, b"ours", Some(&baseline))
            .is_err()
    );
    assert_eq!(std::fs::read(&location.path).unwrap(), b"external");
    let outside = test_support::TempDir::new();
    let outside_file = std::fs::canonicalize(outside.write("outside", b"outside")).unwrap();
    assert!(
        services
            .read(&root, &context.location(outside_file))
            .is_err()
    );
    assert!(
        services
            .read(
                &root,
                &bedtime_files::FileContext::local(&root)
                    .unwrap()
                    .location("facade.bin")
            )
            .is_err()
    );
    git2::Repository::init(&root).unwrap();
    temp.write(".gitignore", b"ignored.rs\n");
    let path = temp.write("src/main.rs", b"value=1");
    temp.write("other.rs", b"value=22");
    temp.write("skip.rs", b"value=33");
    temp.write("ignored.rs", b"value=99");
    let buffers = vec![SearchBuffer {
        path: std::fs::canonicalize(path)
            .unwrap()
            .to_str()
            .unwrap()
            .into(),
        bytes: Some(std::sync::Arc::from(b"value=42".as_slice())),
    }];
    search.include_ignored = false;
    search.regex = true;
    search.include = "*.rs".into();
    search.exclude = "skip.rs".into();
    search.start_with_buffers(root.to_str().unwrap(), r"value=(\d+)", buffers.clone());
    wait_until(|| {
        search.poll();
        !search.searching
    });
    assert!(search.error.is_none(), "{:?}", search.error);
    assert_eq!(search.results.len(), 2);
    let mut local = ContentSearch::new_lazy();
    local.regex = true;
    local.include = "*.rs".into();
    local.exclude = "skip.rs".into();
    local.start_with_buffers(root.to_str().unwrap(), r"value=(\d+)", buffers);
    wait_until(|| {
        local.poll();
        !local.searching
    });
    assert_eq!(local.results, search.results);
    assert!(
        search
            .results
            .iter()
            .any(|found| &*found.line == b"value=42")
    );
    search.start(root.to_str().unwrap(), "[", false);
    wait_until(|| {
        search.poll();
        !search.searching
    });
    assert!(search.error.is_some());
    search.start("/remote/project", "slow", true);
    thread::sleep(Duration::from_millis(50));
    search.cancel();
    assert!(!search.searching);
    let started = Instant::now();
    drop(search);
    drop(finder);
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "remote workers blocked consumer teardown"
    );
    client.disconnect();
    println!(
        "remote finder, byte positions, ignored-file routing, cancellation and teardown passed"
    );
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
}
