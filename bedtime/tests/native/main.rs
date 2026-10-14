//! Native renderer and fixture-invariant coverage; no external-control transport.
use std::{
    path::PathBuf,
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
fn run(name: &str) {
    run_program(name, true, &[]);
}
fn run_program(name: &str, example: bool, args: &[&str]) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let directory = std::env::var_os(if example {
        "BED_NATIVE_FIXTURE_DIR"
    } else {
        "BED_NATIVE_APP_DIR"
    })
    .map(PathBuf::from)
    .unwrap_or_else(|| {
        root.join(if example {
            "target/debug/examples"
        } else {
            "target/debug"
        })
    });
    let mut fixture = Fixture(
        Command::new(directory.join(name))
            .args(args)
            .current_dir(root)
            .spawn()
            .expect("build native fixtures with scripts/test-native.py first"),
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(status) = fixture.0.try_wait().unwrap() {
            assert!(status.success(), "{name}: {status}");
            break;
        }
        assert!(Instant::now() < deadline, "{name} timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}
macro_rules! native {($($name:ident),* $(,)?)=>{$(#[test] #[ignore="requires native desktop/GPU and built fixture examples"] fn $name(){run(stringify!($name));})*};}
native!(
    native_smoke,
    canvas_smoke,
    automation_smoke,
    window_config_smoke,
    lifecycle_smoke,
    workbench_smoke,
    effects_smoke,
    git_smoke,
    viewers_smoke,
    terminal_smoke
);
#[test]
#[ignore = "requires desktop, Rust and LLDB"]
fn debugger_smoke() {
    run("debugger_smoke");
}

macro_rules! application {
    ($test:ident,$name:literal) => {
        #[test]
        #[ignore = "requires a native desktop and built fixture application"]
        fn $test() {
            run_program($name, false, &["--smoke-test"]);
        }
    };
}
application!(layout_identity, "bedtime-example-layouts");
application!(editor_shared_history, "bedtime-example-editor");
application!(typed_model_history, "bedtime-example-model");
application!(game_editor_composition, "bedtime-example-game-editor");
application!(game_runtime_composition, "bedtime-example-game-runtime");
