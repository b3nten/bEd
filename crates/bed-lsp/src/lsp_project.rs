//! Filesystem discovery produces facts; WorkspaceLsp owns binding transitions.
use crate::lsp_config::{LanguageConfiguration, LspConfig};
use bed_remote::{RemoteClient, Request, Response};
use globset::{Glob, GlobSetBuilder};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

pub(crate) fn layered_config(
    user_path: &Path,
    root: &Path,
    remote: Option<&RemoteClient>,
) -> io::Result<(LspConfig, LspConfig)> {
    let user = optional_json(user_path, None, root)?;
    let project_path = root.join(".bed/lsp.json");
    let project = optional_json(&project_path, remote, root)?;
    let base = LspConfig::from_layers(user.as_ref(), None).map_err(|error| {
        io::Error::new(error.kind(), format!("{}: {error}", user_path.display()))
    })?;
    let effective = LspConfig::from_layers(user.as_ref(), project.as_ref()).map_err(|error| {
        io::Error::new(error.kind(), format!("{}: {error}", project_path.display()))
    })?;
    Ok((effective, base))
}

fn optional_json(
    path: &Path,
    remote: Option<&RemoteClient>,
    root: &Path,
) -> io::Result<Option<Value>> {
    let read = if let Some(remote) = remote {
        remote
            .call(Request::ReadFile {
                root: root.to_string_lossy().into_owned(),
                path: path.to_string_lossy().into_owned(),
            })
            .and_then(|response| match response {
                Response::File { bytes, .. } => Ok(bytes),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Expected remote configuration file",
                )),
            })
    } else {
        std::fs::read(path)
    };
    match read {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {error}", path.display()),
            )
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("{}: {error}", path.display()),
        )),
    }
}

/// The highest marker below the workspace ceiling wins, as in Helix. Explicit
/// sub-workspace roots lower that ceiling without looking beyond the project.
pub(crate) fn resolve_root(
    parent: &Path,
    workspace: &Path,
    language: &LanguageConfiguration,
    remote: Option<&RemoteClient>,
    listings: &mut BTreeMap<PathBuf, Vec<OsString>>,
) -> io::Result<PathBuf> {
    if !parent.starts_with(workspace) {
        return Ok(parent.to_owned());
    }
    let mut ceiling = workspace.to_owned();
    for boundary in &language.workspace_lsp_roots {
        let candidate = workspace.join(boundary);
        if parent.starts_with(&candidate)
            && candidate.components().count() > ceiling.components().count()
        {
            ceiling = candidate;
        }
    }
    let mut builder = GlobSetBuilder::new();
    for marker in &language.roots {
        builder.add(
            Glob::new(marker).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        );
    }
    let markers = builder
        .build()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut highest = None;
    for ancestor in parent.ancestors() {
        if !listings.contains_key(ancestor) {
            let read = if let Some(remote) = remote {
                remote
                    .call(Request::ReadDirectory {
                        root: workspace.to_string_lossy().into_owned(),
                        path: ancestor.to_string_lossy().into_owned(),
                        classify_gitignored: false,
                    })
                    .and_then(|response| match response {
                        Response::Directory { entries, .. } => Ok(entries
                            .into_iter()
                            .map(|entry| OsString::from(entry.name))
                            .collect()),
                        _ => Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Expected remote root directory",
                        )),
                    })
            } else {
                std::fs::read_dir(ancestor).and_then(|entries| {
                    entries
                        .map(|entry| entry.map(|entry| entry.file_name()))
                        .collect()
                })
            };
            match read {
                Ok(names) => {
                    listings.insert(ancestor.to_owned(), names);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    listings.insert(ancestor.to_owned(), Vec::new());
                }
                Err(error) => return Err(error),
            }
        }
        if listings[ancestor].iter().any(|name| markers.is_match(name)) {
            highest = Some(ancestor.to_owned());
        }
        if ancestor == ceiling {
            return Ok(highest.unwrap_or(ceiling));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "Document has no workspace ancestor",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn highest_marker_boundaries_globs_and_outside_files() {
        let temp = TempDir::new();
        temp.write("Cargo.toml", b"");
        temp.write("packages/a/Cargo.toml", b"");
        temp.write("packages/a/src/lib.rs", b"");
        let mut language = LanguageConfiguration {
            roots: vec!["Cargo.toml".into()],
            ..Default::default()
        };
        let parent = temp.path("packages/a/src");
        assert_eq!(
            resolve_root(&parent, temp.root(), &language, None, &mut BTreeMap::new()).unwrap(),
            temp.root()
        );
        language.workspace_lsp_roots = vec![PathBuf::from("packages/a")];
        assert_eq!(
            resolve_root(&parent, temp.root(), &language, None, &mut BTreeMap::new()).unwrap(),
            temp.path("packages/a")
        );
        temp.write("packages/a/a.csproj", b"");
        language.roots = vec!["*.csproj".into()];
        assert_eq!(
            resolve_root(&parent, temp.root(), &language, None, &mut BTreeMap::new()).unwrap(),
            temp.path("packages/a")
        );
        assert_eq!(
            resolve_root(
                Path::new("/outside"),
                temp.root(),
                &language,
                None,
                &mut BTreeMap::new()
            )
            .unwrap(),
            Path::new("/outside")
        );
        language.roots.clear();
        assert_eq!(
            resolve_root(&parent, temp.root(), &language, None, &mut BTreeMap::new()).unwrap(),
            temp.path("packages/a")
        );
    }
}
