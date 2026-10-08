use bed_debug::{
    BuildEvent, BuildJob, BuildRequest, CargoDiscovery, CargoLaunch, CargoTargetKind, DebugProfile,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "bed build jobs {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn write(&self, name: &str, text: &str) {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn finish(job: &mut BuildJob) -> Vec<BuildEvent> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut events = Vec::new();
    while !job.is_finished() {
        events.extend(job.poll());
        assert!(Instant::now() < deadline, "Build did not finish");
        thread::sleep(Duration::from_millis(5));
    }
    events
}
#[test]
fn cargo_discovers_and_builds_exact_binary_example_and_test_artifacts() {
    let fixture = Fixture::new();
    fixture.write("Cargo.toml", "[package]\nname='bed_debug_build_fixture'\nversion='0.1.0'\nedition='2024'\n[features]\nextra=[]\n[[example]]\nname='demo'\nrequired-features=['extra']\n[workspace]\n");
    fixture.write(
        ".cargo/config.toml",
        "[build]\ntarget-dir='custom artifacts'\n",
    );
    fixture.write("src/main.rs", "fn main() {}\n");
    fixture.write(
        "src/lib.rs",
        "pub fn answer()->u32 {42}\n#[test] fn works(){assert_eq!(answer(),42);}\n",
    );
    fixture.write(
        "tests/integration.rs",
        "#[test] fn works(){assert_eq!(bed_debug_build_fixture::answer(),42);}\n",
    );
    fixture.write("examples/demo.rs", "fn main() {println!(\"demo\");}\n");
    let mut discovery = CargoDiscovery::start(&fixture.0.join("Cargo.toml")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while discovery.outcome().is_none() {
        discovery.poll();
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    let workspace = discovery.outcome().unwrap().as_ref().unwrap();
    assert_eq!(workspace.targets.len(), 5);
    assert!(workspace.target_directory.ends_with("custom artifacts"));
    for kind in [
        CargoTargetKind::Binary,
        CargoTargetKind::Example,
        CargoTargetKind::LibraryTests,
        CargoTargetKind::BinaryTests,
        CargoTargetKind::IntegrationTest,
    ] {
        let selected = workspace
            .targets
            .iter()
            .find(|t| t.target.kind == kind)
            .unwrap();
        let profile = DebugProfile {
            cargo: Some(CargoLaunch {
                manifest_path: "Cargo.toml".into(),
                package: selected.package.clone(),
                target: selected.target.clone(),
                features: vec!["extra".into()],
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut job =
            BuildJob::start(BuildRequest::for_profile(&profile, &fixture.0).unwrap()).unwrap();
        finish(&mut job);
        let artifact = job.outcome().unwrap().as_ref().unwrap();
        assert!(artifact.program.is_file());
        assert!(artifact.program.starts_with(&workspace.target_directory));
        if kind.is_test() {
            assert!(artifact.program.parent().unwrap().ends_with("deps"));
        }
        if kind == CargoTargetKind::Example {
            assert!(artifact.program.parent().unwrap().ends_with("examples"));
        }
    }
}
#[cfg(unix)]
#[test]
fn failed_manual_build_never_returns_an_existing_old_program() {
    let fixture = Fixture::new();
    fixture.write("build-marker.txt", "EXPECTED BUILD FAILURE\n");
    let profile = DebugProfile {
        program: std::env::current_exe().unwrap().to_string_lossy().into(),
        // Program cwd is independent of the build command's workspace cwd.
        cwd: "runtime-directory".into(),
        build_command: "cat build-marker.txt; exit 17".into(),
        ..Default::default()
    };
    let mut job =
        BuildJob::start(BuildRequest::for_profile(&profile, &fixture.0).unwrap()).unwrap();
    let events = finish(&mut job);
    assert!(job.outcome().unwrap().is_err());
    assert!(
        events.iter().any(
            |e| matches!(e,BuildEvent::Output(text) if text.contains("EXPECTED BUILD FAILURE"))
        )
    );
}
#[cfg(unix)]
#[test]
fn cancelling_a_running_build_terminates_it_promptly() {
    let fixture = Fixture::new();
    let profile = DebugProfile {
        program: std::env::current_exe().unwrap().to_string_lossy().into(),
        build_command: "printf 'RUNNING\\n'; sleep 30".into(),
        ..Default::default()
    };
    let mut job =
        BuildJob::start(BuildRequest::for_profile(&profile, &fixture.0).unwrap()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !job
        .poll()
        .iter()
        .any(|e| matches!(e,BuildEvent::Output(text) if text=="RUNNING\n"))
    {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    let start = Instant::now();
    job.cancel();
    finish(&mut job);
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(
        job.outcome()
            .unwrap()
            .as_ref()
            .unwrap_err()
            .contains("cancelled")
    );
}
#[test]
fn omitted_profile_fields_preserve_safe_defaults() {
    let profile: DebugProfile =
        serde_json::from_str("{\"name\":\"Old profile\",\"program\":\"app\"}").unwrap();
    assert!(profile.stop_on_entry);
    assert!(profile.test_filter.is_empty());
    assert_eq!(profile.program, "app");
}
