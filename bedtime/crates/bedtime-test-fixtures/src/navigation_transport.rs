use bedtime_remote::{LocalBackend, Request, serve_with};
use std::{thread, time::Duration};
pub fn run() {
    let root = std::env::var("BED_FIXTURE_CAPABILITY_ROOT").unwrap();
    serve_with(
        std::io::stdin().lock(),
        std::io::stdout().lock(),
        |mut request| {
            if let Request::ReadFile {
                root: capability, ..
            } = &mut request
            {
                // The fixture agent explicitly grants a capability covering
                // both project and dependencies; the session's workspace stays narrower.
                capability.clone_from(&root);
                thread::sleep(Duration::from_millis(100));
            }
            LocalBackend.call(request)
        },
    )
    .unwrap();
}
