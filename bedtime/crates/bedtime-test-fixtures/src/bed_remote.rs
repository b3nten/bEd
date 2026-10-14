use bedtime_remote::{LocalBackend, Request, serve_with};
use std::{thread, time::Duration};
pub fn run() {
    serve_with(
        std::io::stdin().lock(),
        std::io::stdout().lock(),
        |request| {
            if matches!(
                request,
                Request::ReadFile { .. } | Request::WriteFile { .. }
            ) {
                thread::sleep(Duration::from_millis(120));
            }
            LocalBackend.call(request)
        },
    )
    .unwrap();
}
