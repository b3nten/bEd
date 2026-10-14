use super::*;

#[test]
fn new_window_accepts_a_configuration_directory_before_the_first_preference_write() {
    let directory = crate::test_support::TempDir::new();
    let config = directory.path("unwritten-config");
    let command = new_instance_command(Path::new("/tmp/bed"), &config, false, None).unwrap();
    let arguments = command.get_args().collect::<Vec<_>>();
    assert_eq!(arguments[2], config.as_os_str());
    assert!(!config.exists());
}

#[cfg(feature = "workspaces")]
#[test]
fn workspace_window_launch_passes_only_the_shared_config_and_selected_root() {
    let config = std::env::temp_dir().canonicalize().unwrap();
    let root = config.join("workspace with spaces");
    let mut command = new_instance_command(Path::new("/tmp/bed"), &config, false, None).unwrap();
    command.arg(&root);
    assert_eq!(command.get_program(), "/tmp/bed");
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        vec![
            std::ffi::OsStr::new("--new-window"),
            std::ffi::OsStr::new("--config-dir"),
            config.as_os_str(),
            root.as_os_str(),
        ]
    );
}

#[cfg(target_os = "macos")]
#[test]
fn bundled_workspace_window_launch_uses_launch_services_for_a_new_instance() {
    let config = std::env::temp_dir().canonicalize().unwrap();
    let command = new_instance_command(
        Path::new("/Applications/bEd.app/Contents/MacOS/bed"),
        &config,
        true,
        None,
    )
    .unwrap();
    assert_eq!(command.get_program(), "/usr/bin/open");
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        vec![
            std::ffi::OsStr::new("-n"),
            std::ffi::OsStr::new("-W"),
            std::ffi::OsStr::new("/Applications/bEd.app"),
            std::ffi::OsStr::new("--args"),
            std::ffi::OsStr::new("--new-window"),
            std::ffi::OsStr::new("--config-dir"),
            config.as_os_str(),
        ]
    );
}

#[cfg(feature = "workspaces")]
#[test]
fn workspace_window_launch_preserves_local_and_ssh_identity_as_one_argument() {
    use bedtime_plugin_workspaces::{WorkspaceSpec, WorkspaceTarget};
    let config = std::env::temp_dir().canonicalize().unwrap();
    for workspace in [
        WorkspaceSpec::local("/tmp/project with spaces"),
        WorkspaceSpec {
            name: "Named remote".into(),
            root: "/srv/project with spaces".into(),
            target: WorkspaceTarget::Ssh {
                host: "user@alias".into(),
            },
        },
    ] {
        let command =
            new_instance_command(Path::new("/tmp/bed"), &config, false, Some(&workspace)).unwrap();
        let args = command.get_args().collect::<Vec<_>>();
        assert_eq!(args[3], "--workspace");
        assert_eq!(args.len(), 5);
        let serialized: serde_json::Value =
            serde_json::from_str(args[4].to_str().unwrap()).unwrap();
        assert_eq!(WorkspaceSpec::from_value(&serialized), Some(workspace));
    }
}
