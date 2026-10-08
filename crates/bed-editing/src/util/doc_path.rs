//! Translated from ned editor/util/doc_path.h; see LICENSE and NOTICE.
use std::path::{Component, Path, PathBuf};

/// Match `weakly_canonical`, then absolute, then the original spelling.
/// Nonexistent tail components are normalized after resolving the existing
/// prefix, so a symlink in that prefix has the same key as an opened file.
pub fn normalize(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let input = Path::new(path);
    let absolute = if input.is_absolute() {
        input.to_path_buf()
    } else {
        let Ok(current) = std::env::current_dir() else {
            return path.to_owned();
        };
        current.join(input)
    };
    let mut prefix = absolute.clone();
    let mut tail = Vec::new();
    loop {
        match std::fs::canonicalize(&prefix) {
            Ok(mut canonical) => {
                for component in tail.into_iter().rev() {
                    canonical.push(component);
                }
                return lexical_normalize(&canonical).to_string_lossy().into_owned();
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) => {}
            Err(_) => return absolute.to_string_lossy().into_owned(),
        }
        let Some(component) = prefix.components().next_back() else {
            return absolute.to_string_lossy().into_owned();
        };
        tail.push(component.as_os_str().to_owned());
        if !prefix.pop() {
            return absolute.to_string_lossy().into_owned();
        }
    }
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            _ => result.push(component.as_os_str()),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_and_nonexistent_spellings_share_canonical_keys() {
        assert_eq!(normalize(""), "");
        let root = std::fs::canonicalize(env!("CARGO_MANIFEST_DIR")).unwrap();
        assert_eq!(normalize("src/../Cargo.toml"), normalize("Cargo.toml"));
        assert_eq!(
            normalize("bed-no-such-path/../a.cpp"),
            root.join("a.cpp").to_string_lossy()
        );
    }

    #[cfg(unix)]
    #[test]
    fn missing_tail_resolves_existing_symlink_prefix() {
        let directory = std::env::temp_dir().join(format!("bed-doc-path-{}", std::process::id()));
        std::fs::create_dir_all(directory.join("real")).unwrap();
        let link = directory.join("link");
        std::os::unix::fs::symlink(directory.join("real"), &link).unwrap();
        assert_eq!(
            normalize(link.join("missing/../a.rs").to_str().unwrap()),
            std::fs::canonicalize(directory.join("real"))
                .unwrap()
                .join("a.rs")
                .to_string_lossy()
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
