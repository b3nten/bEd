//! Locate prebuilt target helpers without requiring a toolchain on the SSH host.
use std::path::{Path, PathBuf};

/// Packaged helpers take priority over development artifacts. An explicit
/// override supports local builds without modifying the installed application.
pub fn helpers_directory() -> PathBuf {
    if let Some(directory) = std::env::var_os("BED_REMOTE_HELPERS_DIR")
        && !directory.is_empty()
    {
        return PathBuf::from(directory);
    }
    let development = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("remote-helpers");
    find_helpers(std::env::current_exe().ok().as_deref(), &development)
}

fn find_helpers(executable: Option<&Path>, development: &Path) -> PathBuf {
    let mut candidates = Vec::new();
    if let Some(directory) = executable.and_then(Path::parent) {
        #[cfg(target_os = "macos")]
        candidates.push(directory.join("../Resources/remote-helpers"));
        candidates.extend([
            directory.join("remote-helpers"),
            directory.join("../share/Bed/remote-helpers"),
            directory.join("../remote-helpers"),
        ]);
    }
    candidates.push(development.to_owned());
    candidates
        .into_iter()
        .find(|candidate| candidate.is_dir())
        .unwrap_or_else(|| development.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_helpers_take_priority_over_development_builds() {
        let root = std::env::temp_dir().join(format!("bed-helpers-{}", std::process::id()));
        let portable = root.join("app/remote-helpers");
        let development = root.join("target/remote-helpers");
        std::fs::create_dir_all(&portable).unwrap();
        std::fs::create_dir_all(&development).unwrap();
        assert_eq!(
            find_helpers(Some(&root.join("app/bed")), &development),
            portable
        );
        assert_eq!(find_helpers(None, &development), development);
        std::fs::remove_dir_all(root).unwrap();
    }
}
