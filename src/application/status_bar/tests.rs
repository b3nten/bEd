use super::*;
use std::path::PathBuf;

#[test]
fn host_information_shows_names_and_keeps_paths_in_details() {
    let mut workspace = WorkspaceState {
        spec: None,
        project_root: String::new(),
        working_directory: PathBuf::from("/home/user/launcher"),
        error: None,
        close_requested: false,
        windows: vec![],
    };
    assert_eq!(
        host_information(&workspace),
        (
            "launcher".into(),
            "Working directory: /home/user/launcher".into()
        )
    );
    workspace.spec = Some(crate::application::WorkspaceSpec::local("/srv/code"));
    assert_eq!(host_information(&workspace).0, "code");
    workspace.spec = Some(crate::application::WorkspaceSpec {
        name: "Remote project".into(),
        target: bed_plugin_workspaces::WorkspaceTarget::Ssh {
            host: "devbox".into(),
        },
        root: "/srv/code".into(),
    });
    let (summary, detail) = host_information(&workspace);
    assert_eq!(summary, "Remote project");
    assert!(detail.contains("devbox:/srv/code"));
    assert!(!detail.contains("/launcher"));
}
