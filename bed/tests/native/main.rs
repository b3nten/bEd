//! Real desktop fixtures, explicit via cargo bed-test --test native -- --ignored.
use std::{
    process::{Child, Command},
    time::{Duration, Instant},
};
struct Fixture(Child);
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
fn run(scenario: &str) {
    let capture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/native-captures");
    std::fs::create_dir_all(&capture).unwrap();
    let mut fixture = Fixture(
        Command::new(env!("CARGO_BIN_EXE_bed-native-fixture"))
            .args(["--new-window", scenario, "--capture-frame"])
            .arg(capture.join(format!("{}.png", scenario.trim_start_matches('-'))))
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(status) = fixture.0.try_wait().unwrap() {
            assert!(status.success(), "{scenario} failed: {status}");
            break;
        }
        assert!(Instant::now() < deadline, "{scenario} timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[test]
#[ignore = "requires a desktop"]
fn window_composition() {
    run("--main-only-smoke");
}
#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires a desktop"]
fn terminal_promotion_preserves_process_and_window() {
    run("--term-promotion-smoke");
}
#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires macOS desktop"]
fn native_menu_and_layout_identity() {
    run("--menu-smoke");
}
#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires macOS desktop"]
fn native_chrome_toolbar_and_material() {
    run("--platform-smoke");
}
