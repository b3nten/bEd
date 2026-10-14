use bedtime_remote::{LocalBackend, Request, serve_with};
use std::{thread, time::Duration};
pub fn run() {
    serve_with(
            std::io::stdin().lock(),
            std::io::stdout().lock(),
            |request| {
                if std::env::var_os("BED_TEST_FAIL_WORKSPACE_WRITE").is_some()
                    && matches!(&request, Request::WriteFile { path, bytes, .. } if path.ends_with("workspace-failing.rs") && bytes == b"new")
                {
                    return Err(bedtime_remote::RemoteError::new(bedtime_remote::ErrorKind::PermissionDenied, "fixture workspace write denied"));
                }
                if std::env::var_os("BED_TEST_DELAY_WRITE").is_some()
                    && matches!(request, Request::WriteFile { .. })
                {
                    thread::sleep(Duration::from_millis(150));
                }
                if std::env::var_os("BED_TEST_DELAY_READ").is_some()
                    && matches!(request, Request::ReadFile { .. })
                {
                    thread::sleep(Duration::from_millis(150));
                }
                LocalBackend.call(request)
            },
        )
        .unwrap();
}
