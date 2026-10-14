use super::*;
use std::path::PathBuf;

#[test]
fn host_information_shows_names_and_keeps_paths_in_details() {
    let directory = PathBuf::from("/home/user/launcher");
    assert_eq!(
        host_information(None, &directory),
        (
            "launcher".into(),
            "Working directory: /home/user/launcher".into()
        )
    );
    let local = crate::application::WorkspaceSpec::local("/srv/code");
    assert_eq!(host_information(Some(&local), &directory).0, "code");
    let remote = crate::application::WorkspaceSpec {
        name: "Remote project".into(),
        target: bedtime_plugin_workspaces::WorkspaceTarget::Ssh {
            host: "devbox".into(),
        },
        root: "/srv/code".into(),
    };
    let (summary, detail) = host_information(Some(&remote), &directory);
    assert_eq!(summary, "Remote project");
    assert!(detail.contains("devbox:/srv/code"));
    assert!(!detail.contains("/launcher"));
}
