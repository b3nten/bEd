//! Resolve adapter source paths without depending on the editor process directory.

use std::path::{Path, PathBuf};

/// Give LLDB the same workspace-relative mapping destinations as the host,
/// even when the adapter runs in a different launch directory.
pub(crate) fn absolute_source_map(
    workspace: &Path,
    source_map: &[[String; 2]],
) -> Vec<[String; 2]> {
    source_map
        .iter()
        .map(|[from, to]| {
            [
                from.clone(),
                workspace.join(to).to_string_lossy().into_owned(),
            ]
        })
        .collect()
}

/// Find an existing source file, applying explicit mappings before local fallbacks.
///
/// Mapping prefixes match whole path components; the most specific prefix wins
/// when more than one mapping leads to an existing file. Relative mapping
/// destinations and a relative launch directory are anchored to the workspace.
/// The returned path is canonical so navigation and source decorations share the
/// same identity, including when a workspace is opened through a symlink.
pub(crate) fn resolve_source_path(
    source: &str,
    workspace: &Path,
    cwd: &Path,
    source_map: &[[String; 2]],
) -> Option<PathBuf> {
    if source.is_empty() {
        return None;
    }
    let source = Path::new(source);
    let mut mappings: Vec<_> = source_map
        .iter()
        .filter_map(|[from, to]| {
            if from.is_empty() {
                return None;
            }
            let prefix = Path::new(from);
            let suffix = source.strip_prefix(prefix).ok()?;
            Some((prefix.components().count(), Path::new(to), suffix))
        })
        .collect();
    mappings.sort_by_key(|(length, _, _)| std::cmp::Reverse(*length));
    for (_, destination, suffix) in mappings {
        if let Some(path) = canonical_file(workspace.join(destination).join(suffix)) {
            return Some(path);
        }
    }
    if source.is_absolute() {
        return canonical_file(source.to_path_buf());
    }
    canonical_file(workspace.join(cwd).join(source))
        .or_else(|| canonical_file(workspace.join(source)))
}

fn canonical_file(path: PathBuf) -> Option<PathBuf> {
    let path = path.canonicalize().ok()?;
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "bed-debugger-paths-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }

        fn write(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"source\n").unwrap();
            path.canonicalize().unwrap()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn adapter_mapping_destinations_are_anchored_to_workspace() {
        let temp = TempDir::new();
        let maps = [["/remote/build/source".into(), "local sources".into()]];
        assert_eq!(
            absolute_source_map(&temp.0, &maps),
            vec![[
                "/remote/build/source".into(),
                temp.0.join("local sources").to_string_lossy().into_owned(),
            ]]
        );
    }

    #[test]
    fn adapter_mappings_preserve_absolute_destinations_and_remote_prefixes() {
        let temp = TempDir::new();
        let maps = [[
            "remote/unchanged/../source".into(),
            temp.0
                .join("absolute sources")
                .to_string_lossy()
                .into_owned(),
        ]];
        assert_eq!(absolute_source_map(&temp.0.join("workspace"), &maps), maps);
    }

    #[test]
    fn relative_sources_prefer_launch_directory_then_workspace() {
        let temp = TempDir::new();
        let launch_file = temp.write("launch/src/main.rs");
        let workspace_file = temp.write("src/main.rs");
        let fallback = temp.write("src/lib.rs");
        let cwd = temp.0.join("launch");
        assert_eq!(
            resolve_source_path("src/main.rs", &temp.0, &cwd, &[]),
            Some(launch_file)
        );
        assert_eq!(
            resolve_source_path("src/lib.rs", &temp.0, &cwd, &[]),
            Some(fallback)
        );
        assert_eq!(
            resolve_source_path("src/main.rs", &temp.0, Path::new("missing"), &[]),
            Some(workspace_file)
        );
    }

    #[test]
    fn relative_launch_directory_is_anchored_to_workspace() {
        let temp = TempDir::new();
        let file = temp.write("launch/src/main.cpp");
        assert_eq!(
            resolve_source_path("src/main.cpp", &temp.0, Path::new("launch"), &[]),
            Some(file)
        );
    }

    #[test]
    fn mappings_prefer_longest_component_prefix_and_relative_destinations() {
        let temp = TempDir::new();
        temp.write("broad/project/src/main.rs");
        let specific = temp.write("specific/src/main.rs");
        let maps = [
            ["/build".into(), "broad".into()],
            ["/build/project".into(), "specific".into()],
        ];
        assert_eq!(
            resolve_source_path("/build/project/src/main.rs", &temp.0, &temp.0, &maps),
            Some(specific)
        );
    }

    #[test]
    fn mapping_prefixes_match_whole_components() {
        let temp = TempDir::new();
        temp.write("mapped/src/main.rs");
        let maps = [["/build/project".into(), "mapped".into()]];
        assert!(
            resolve_source_path("/build/project-other/src/main.rs", &temp.0, &temp.0, &maps)
                .is_none()
        );
    }

    #[test]
    fn existing_explicit_mapping_precedes_existing_original_file() {
        let temp = TempDir::new();
        let original = temp.write("original/main.cpp");
        let mapped = temp.write("mapped/main.cpp");
        let maps = [[
            temp.0.join("original").to_string_lossy().into_owned(),
            temp.0.join("mapped").to_string_lossy().into_owned(),
        ]];
        assert_eq!(
            resolve_source_path(&original.to_string_lossy(), &temp.0, &temp.0, &maps),
            Some(mapped)
        );
    }

    #[test]
    fn absolute_sources_and_failed_mappings_fall_back_to_existing_file() {
        let temp = TempDir::new();
        let file = temp.write("src/main.rs");
        let maps = [[temp.0.to_string_lossy().into_owned(), "missing".into()]];
        assert_eq!(
            resolve_source_path(&file.to_string_lossy(), &temp.0, &temp.0, &maps),
            Some(file.clone())
        );
        assert_eq!(
            resolve_source_path(&file.to_string_lossy(), &temp.0, &temp.0, &[]),
            Some(file)
        );
    }

    #[test]
    fn unavailable_paths_and_directories_do_not_guess_source_files() {
        let temp = TempDir::new();
        temp.write("src/main.rs");
        for source in [
            "",
            "src",
            "main.rs",
            "missing/main.rs",
            "/usr/lib/dyld`_dyld_start",
        ] {
            assert!(resolve_source_path(source, &temp.0, &temp.0, &[]).is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn mapped_and_relative_symlink_paths_share_canonical_identity() {
        let temp = TempDir::new();
        let file = temp.write("real/src/main.rs");
        std::os::unix::fs::symlink(temp.0.join("real"), temp.0.join("linked")).unwrap();
        let maps = [["/remote/project".into(), "linked".into()]];
        assert_eq!(
            resolve_source_path("src/main.rs", &temp.0.join("linked"), Path::new("."), &[]),
            Some(file.clone())
        );
        assert_eq!(
            resolve_source_path("/remote/project/src/main.rs", &temp.0, &temp.0, &maps),
            Some(file)
        );
    }
}
