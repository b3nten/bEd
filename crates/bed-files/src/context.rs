//! Live filesystem identity and access, independent of saved workspace policy.
use bevy::prelude::Resource;
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum FileProviderId {
    #[default]
    Local,
    Ssh {
        host: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileLocation {
    pub provider: FileProviderId,
    pub path: PathBuf,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileAvailability {
    #[default]
    Available,
    Disconnected,
}

/// Selected directory, rather than the process cwd or a saved workspace root.
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct FileContext {
    pub provider: FileProviderId,
    pub directory: PathBuf,
    pub availability: FileAvailability,
    pub generation: u64,
}

impl Default for FileContext {
    fn default() -> Self {
        Self::local(std::env::current_dir().expect("launch directory is available"))
            .expect("launch directory is a readable directory")
    }
}

impl FileContext {
    pub fn local(directory: impl AsRef<Path>) -> io::Result<Self> {
        let directory = std::fs::canonicalize(directory)?;
        if !directory.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "File context requires a directory",
            ));
        }
        Ok(Self {
            provider: FileProviderId::Local,
            directory,
            availability: FileAvailability::Available,
            generation: 1,
        })
    }

    /// The authenticated provider validates existence and confinement remotely.
    pub fn remote(host: impl Into<String>, directory: impl Into<PathBuf>) -> io::Result<Self> {
        let host = host.into();
        let directory = directory.into();
        if host.is_empty() || !directory.to_str().is_some_and(|path| path.starts_with('/')) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SSH context requires a host and absolute directory",
            ));
        }
        Ok(Self {
            provider: FileProviderId::Ssh { host },
            directory,
            availability: FileAvailability::Available,
            generation: 1,
        })
    }

    pub fn directory_string(&self) -> String {
        self.directory.to_string_lossy().into_owned()
    }

    pub fn location(&self, path: impl AsRef<Path>) -> FileLocation {
        let path = path.as_ref();
        let path = if matches!(self.provider, FileProviderId::Ssh { .. }) {
            // SSH agent paths use Unix separators, even on a Windows client.
            let path = path.as_os_str().to_string_lossy();
            if path.starts_with('/') {
                PathBuf::from(path.as_ref())
            } else {
                PathBuf::from(format!(
                    "{}/{}",
                    self.directory.to_string_lossy().trim_end_matches('/'),
                    path
                ))
            }
        } else {
            self.directory.join(path)
        };
        FileLocation {
            provider: self.provider.clone(),
            path,
        }
    }

    /// One explicit context transition invalidates results from the previous directory.
    pub fn replace(&mut self, mut next: Self) {
        if self.provider != next.provider
            || self.directory != next.directory
            || self.availability != next.availability
        {
            next.generation = self
                .generation
                .checked_add(1)
                .expect("file context generation overflow");
            *self = next;
        }
    }
}

/// Clone into workers to retain the provider admitted with their request.
#[derive(Resource, Clone, Debug)]
pub struct FileServices {
    provider: FileProviderId,
    available: bool,
    #[cfg(feature = "file-services")]
    remote: Option<bed_remote::RemoteClient>,
    #[cfg(feature = "file-services")]
    ssh_target: Option<bed_remote::SshTarget>,
}

impl Default for FileServices {
    fn default() -> Self {
        Self {
            provider: FileProviderId::Local,
            available: true,
            #[cfg(feature = "file-services")]
            remote: None,
            #[cfg(feature = "file-services")]
            ssh_target: None,
        }
    }
}

impl FileServices {
    pub fn local() -> Self {
        Self::default()
    }
    pub fn provider_id(&self) -> &FileProviderId {
        &self.provider
    }
    pub fn is_remote(&self) -> bool {
        matches!(self.provider, FileProviderId::Ssh { .. })
    }
    pub fn connected(&self) -> bool {
        if !self.available {
            return false;
        }
        #[cfg(feature = "file-services")]
        if let Some(client) = &self.remote {
            return client.is_connected();
        }
        !self.is_remote()
    }

    pub fn same_provider(&self, other: &Self) -> bool {
        if self.provider != other.provider || self.available != other.available {
            return false;
        }
        #[cfg(feature = "file-services")]
        return match (&self.remote, &other.remote) {
            (Some(a), Some(b)) => a.same_connection(b),
            (None, None) => true,
            _ => false,
        };
        #[cfg(not(feature = "file-services"))]
        true
    }

    #[cfg(feature = "file-services")]
    pub fn remote(host: impl Into<String>, client: bed_remote::RemoteClient) -> Self {
        Self::remote_target(bed_remote::SshTarget::new(host), client)
    }

    /// Retain the resolved agent path used by processes such as language servers.
    #[cfg(feature = "file-services")]
    pub fn remote_target(target: bed_remote::SshTarget, client: bed_remote::RemoteClient) -> Self {
        Self {
            available: true,
            provider: FileProviderId::Ssh {
                host: target.host.clone(),
            },
            remote: Some(client),
            ssh_target: Some(target),
        }
    }

    /// Inconsistent context/provider configuration must never fall back to a
    /// coincident local path. The unavailable handle retains its identity.
    pub fn for_context(&self, context: &FileContext) -> Self {
        if &context.provider == self.provider_id() {
            let mut services = self.clone();
            services.available &= context.availability == FileAvailability::Available;
            return services;
        }
        Self {
            provider: context.provider.clone(),
            available: false,
            #[cfg(feature = "file-services")]
            remote: None,
            #[cfg(feature = "file-services")]
            ssh_target: None,
        }
    }

    #[cfg(feature = "file-services")]
    pub fn ssh_target(&self) -> Option<&bed_remote::SshTarget> {
        self.ssh_target.as_ref()
    }

    /// Backend integration uses this transport; ordinary plugins use the facade.
    #[cfg(feature = "file-services")]
    pub fn remote_client(&self) -> Option<bed_remote::RemoteClient> {
        self.remote.clone()
    }

    #[cfg(feature = "file-services")]
    pub fn call(&self, request: bed_remote::Request) -> io::Result<bed_remote::Response> {
        self.ensure_available()?;
        match &self.remote {
            Some(client) => client.call(request),
            None => bed_remote::LocalBackend
                .call(request)
                .map_err(|error| error.into_io()),
        }
    }

    #[cfg(feature = "file-services")]
    pub fn file_info(&self, root: &str, path: &str) -> io::Result<bed_remote::FileInfo> {
        match self.call(bed_remote::Request::FileInfo {
            root: root.into(),
            path: path.into(),
        })? {
            bed_remote::Response::FileInfo { info } => Ok(info),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unexpected file information response",
            )),
        }
    }

    #[cfg(feature = "file-services")]
    pub fn read_prefix(&self, root: &str, path: &str) -> io::Result<(String, Vec<u8>)> {
        match self.call(bed_remote::Request::ReadFilePrefix {
            root: root.into(),
            path: path.into(),
        })? {
            bed_remote::Response::FilePrefix { path, bytes } => Ok((path, bytes)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unexpected file prefix response",
            )),
        }
    }

    #[cfg(feature = "file-services")]
    pub fn canonicalize(
        &self,
        root: &Path,
        location: &FileLocation,
        allow_missing: bool,
    ) -> io::Result<FileLocation> {
        self.validate_provider(location)?;
        let path = if self.is_remote() {
            match self.call(bed_remote::Request::Canonicalize {
                root: utf8(root)?.into(),
                path: utf8(&location.path)?.into(),
                allow_missing,
            })? {
                bed_remote::Response::Path { path } => PathBuf::from(path),
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Unexpected canonical path response",
                    ));
                }
            }
        } else {
            match std::fs::canonicalize(&location.path) {
                Ok(path) => path,
                Err(error) if allow_missing && error.kind() == io::ErrorKind::NotFound => {
                    let parent = location.path.parent().ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "Path requires a parent")
                    })?;
                    let name = location.path.file_name().ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "Path requires a filename")
                    })?;
                    std::fs::canonicalize(parent)?.join(name)
                }
                Err(error) => return Err(error),
            }
        };
        Ok(FileLocation {
            provider: location.provider.clone(),
            path,
        })
    }

    #[cfg(feature = "file-services")]
    pub fn read_directory(
        &self,
        root: &str,
        path: &str,
        classify_gitignored: bool,
    ) -> io::Result<bed_remote::DirectoryListing> {
        match self.call(bed_remote::Request::ReadDirectory {
            root: root.into(),
            path: path.into(),
            classify_gitignored,
        })? {
            bed_remote::Response::Directory { entries, warning } => {
                Ok(bed_remote::DirectoryListing {
                    path: path.into(),
                    entries,
                    warning,
                })
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unexpected directory response",
            )),
        }
    }

    #[cfg(feature = "file-services")]
    pub fn read(
        &self,
        root: &Path,
        location: &FileLocation,
    ) -> io::Result<(Vec<u8>, bed_remote::FileBaseline)> {
        self.validate_provider(location)?;
        if !self.is_remote() {
            return bed_remote::LocalBackend.read_file(&location.path);
        }
        match self.call(bed_remote::Request::ReadFile {
            root: utf8(root)?.into(),
            path: utf8(&location.path)?.into(),
        })? {
            bed_remote::Response::File {
                bytes, baseline, ..
            } => Ok((bytes, baseline)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unexpected file read response",
            )),
        }
    }

    /// None admits creation only. Existing files require the exact read baseline.
    #[cfg(feature = "file-services")]
    pub fn write_atomic(
        &self,
        root: &Path,
        location: &FileLocation,
        bytes: &[u8],
        expected: Option<&bed_remote::FileBaseline>,
    ) -> io::Result<bed_remote::FileBaseline> {
        self.validate_provider(location)?;
        if !self.is_remote() {
            return bed_remote::LocalBackend.write_atomic(&location.path, bytes, expected);
        }
        match self.call(bed_remote::Request::WriteFile {
            root: utf8(root)?.into(),
            path: utf8(&location.path)?.into(),
            bytes: bytes.to_vec(),
            baseline: expected.cloned(),
        })? {
            bed_remote::Response::Written { baseline } => Ok(baseline),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unexpected file write response",
            )),
        }
    }

    #[cfg(feature = "file-services")]
    fn validate_provider(&self, location: &FileLocation) -> io::Result<()> {
        self.ensure_available()?;
        if self.provider == location.provider {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "File location belongs to a different provider",
            ))
        }
    }

    #[cfg(feature = "file-services")]
    fn ensure_available(&self) -> io::Result<()> {
        if self.connected() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "File provider is unavailable",
            ))
        }
    }

    #[cfg(feature = "workspace-services")]
    pub fn watch(
        &self,
        root: String,
        include_ignored: bool,
    ) -> io::Result<bed_remote::WorkspaceFilesystem> {
        self.ensure_available()?;
        Ok(match &self.remote {
            Some(client) => {
                bed_remote::WorkspaceFilesystem::remote(root, include_ignored, client.clone())
            }
            None => bed_remote::WorkspaceFilesystem::local(root, include_ignored),
        })
    }

    #[cfg(feature = "workspace-services")]
    pub fn endpoint(&self, root: &str) -> io::Result<crate::operations::Endpoint> {
        self.ensure_available()?;
        Ok(match &self.remote {
            Some(client) => crate::operations::Endpoint::Remote {
                root: root.into(),
                client: client.clone(),
            },
            None => crate::operations::Endpoint::local(root),
        })
    }
}

#[cfg(feature = "file-services")]
fn utf8(path: &Path) -> io::Result<&str> {
    path.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Remote paths must be valid UTF-8",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn local_locations_retain_native_filename_bytes() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let temp = crate::test_support::TempDir::new();
        let context = FileContext::local(temp.root()).unwrap();
        let location = context.location(std::ffi::OsString::from_vec(b"document-\xff".to_vec()));
        assert_eq!(
            location.path.file_name().unwrap().as_bytes(),
            b"document-\xff"
        );
    }

    #[test]
    fn context_transitions_retain_process_cwd_and_advance_generation() {
        let cwd = std::env::current_dir().unwrap();
        let temp = crate::test_support::TempDir::new();
        let mut context = FileContext::local(&cwd).unwrap();
        context.replace(FileContext::local(temp.root()).unwrap());
        assert_eq!(context.generation, 2);
        assert_eq!(std::env::current_dir().unwrap(), cwd);
        context.replace(FileContext::local(temp.root()).unwrap());
        assert_eq!(context.generation, 2);
        context.replace(FileContext::remote("example", "/project").unwrap());
        assert_eq!(context.generation, 3);
        assert_ne!(context.location("file").provider, FileProviderId::Local);
        assert_eq!(
            context.location("nested/file").path,
            PathBuf::from("/project/nested/file")
        );
        assert_eq!(
            context.location("/other/file").path,
            PathBuf::from("/other/file")
        );
    }

    #[cfg(feature = "file-services")]
    #[test]
    fn atomic_writes_require_the_observed_baseline_and_retain_conflicting_bytes() {
        let temp = crate::test_support::TempDir::new();
        let context = FileContext::local(temp.root()).unwrap();
        let services = FileServices::local();
        let file = context.location("file");
        let baseline = services
            .write_atomic(&context.directory, &file, b"initial", None)
            .unwrap();
        std::fs::write(&file.path, b"external").unwrap();
        assert!(
            services
                .write_atomic(&context.directory, &file, b"ours", Some(&baseline))
                .is_err()
        );
        assert_eq!(std::fs::read(&file.path).unwrap(), b"external");
        let (_, current) = services.read(&context.directory, &file).unwrap();
        services
            .write_atomic(&context.directory, &file, b"ours", Some(&current))
            .unwrap();
        assert_eq!(std::fs::read(&file.path).unwrap(), b"ours");
        let foreign = FileLocation {
            provider: FileProviderId::Ssh {
                host: "example".into(),
            },
            path: file.path,
        };
        assert!(services.read(&context.directory, &foreign).is_err());
        let remote = FileContext::remote("unconfigured", context.directory.clone()).unwrap();
        let unavailable = services.for_context(&remote);
        assert!(!unavailable.connected());
        assert!(
            unavailable
                .read(&remote.directory, &remote.location("file"))
                .is_err()
        );
        assert!(
            unavailable
                .canonicalize(&remote.directory, &remote.location("file"), false)
                .is_err()
        );
        #[cfg(feature = "workspace-services")]
        assert!(unavailable.watch(remote.directory_string(), false).is_err());
    }

    #[cfg(all(unix, not(target_os = "macos"), feature = "file-services"))]
    #[test]
    fn native_local_paths_retain_non_utf8_bytes_through_atomic_services() {
        use std::os::unix::ffi::OsStringExt;
        let temp = crate::test_support::TempDir::new();
        let context = FileContext::local(temp.root()).unwrap();
        let location = context.location(std::ffi::OsString::from_vec(b"document-\xff".to_vec()));
        let services = FileServices::local();
        let baseline = services
            .write_atomic(&context.directory, &location, b"\0bytes\xff", None)
            .unwrap();
        assert_eq!(
            services
                .canonicalize(&context.directory, &location, false)
                .unwrap(),
            location
        );
        let (bytes, current) = services.read(&context.directory, &location).unwrap();
        assert_eq!(bytes, b"\0bytes\xff");
        assert_eq!(current, baseline);
    }
}
