//! Native policy and document acknowledgements around asynchronous file jobs.
use super::*;
use bed_files::operations::{
    Conflict, ConflictChoice, ConflictDecision, Endpoint, OperationEvent, OperationJob,
    OperationKind, OperationRequest, OperationSummary, top_level_paths,
};
use bed_workbench_api::{
    ExternalFileDrag, ExternalFileDragPhase, ExternalFileDropResponse, FileClipboardService,
};
use std::{
    fs,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Default)]
pub(super) struct FileOperations {
    clipboard: Option<Box<dyn FileClipboardService>>,
    cut: Option<(String, Vec<String>)>,
    active: Option<ActiveOperation>,
    pending_delete: Option<Vec<String>>,
    delete_appearing: bool,
    conflict: Option<Conflict>,
    conflict_appearing: bool,
    apply_all: bool,
    summary: Option<OperationSummary>,
    current: String,
    completed: usize,
    bytes: u64,
    drag_panel: Option<u64>,
    tiling_drops: Vec<(u32, Vec<String>)>,
    draining: bool,
}
struct ActiveOperation {
    job: OperationJob,
    started: Instant,
    workspace: String,
    paused: Vec<DocumentId>,
    export: bool,
    local_destination: bool,
    kind: OperationKind,
    refresh_directories: Vec<String>,
    replacement_paths: Vec<String>,
}
impl FileOperations {
    pub(super) fn modal_visible(&self) -> bool {
        self.conflict.is_some() || self.pending_delete.is_some()
    }
}

fn sqlite_related_paths(database: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    ["", "-wal", "-shm"].into_iter().map(|suffix| {
        let mut path = database.as_os_str().to_os_string();
        path.push(suffix);
        PathBuf::from(path)
    })
}

fn same_local_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let (Ok(a), Ok(b)) = (fs::metadata(a), fs::metadata(b)) else {
            return false;
        };
        a.dev() == b.dev() && a.ino() == b.ino()
    }
    #[cfg(not(unix))]
    {
        fs::canonicalize(a)
            .ok()
            .zip(fs::canonicalize(b).ok())
            .is_some_and(|(a, b)| a == b)
    }
}

impl Workbench {
    /// Keep editor I/O protected even while a panel is temporarily borrowed out
    /// of `tabs` for drawing, state serialization, or a service callback.
    pub(super) fn sync_local_file_protections(&mut self) {
        let paths = self
            .tabs
            .iter()
            .filter_map(|tab| tab.panel.input.local_file())
            .flat_map(sqlite_related_paths)
            .collect();
        self.session.set_read_only_local_paths(paths);
    }

    pub fn set_file_clipboard(&mut self, clipboard: Box<dyn FileClipboardService>) {
        self.file_operations.clipboard = Some(clipboard);
        self.cleanup_file_exports();
    }
    fn files_identity(&self) -> String {
        self.workspace_spec
            .as_ref()
            .map(|spec| spec.identity())
            .unwrap_or_else(|| self.files_root())
    }
    fn files_endpoint(&self) -> io::Result<Endpoint> {
        let root = self.files_root();
        if root.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Open Files to choose a directory first",
            ));
        }
        Ok(match self.session.remote_client() {
            Some(client) => Endpoint::Remote { root, client },
            None => Endpoint::local(&root),
        })
    }
    /// File-backed panels retain a live association with their on-disk file.
    /// SQLite's sidecars belong to that association even when currently absent.
    pub(super) fn ensure_local_files_unaffected(&self, path: &Path) -> io::Result<()> {
        let Some(name) = path.file_name() else {
            return Ok(());
        };
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let entry = fs::canonicalize(parent)?.join(name);
        // Preserve symlink-entry semantics for rename/delete, while recognizing
        // alternate casing of ordinary entries on case-insensitive filesystems.
        let resolved = fs::symlink_metadata(&entry)
            .ok()
            .filter(|metadata| !metadata.file_type().is_symlink())
            .and_then(|_| fs::canonicalize(&entry).ok());
        for tab in &self.tabs {
            let Some(database) = tab.panel.input.local_file() else {
                continue;
            };
            for protected in sqlite_related_paths(database) {
                if protected.starts_with(&entry)
                    || resolved
                        .as_ref()
                        .is_some_and(|path| protected.starts_with(path))
                {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        format!(
                            "Close the file viewer for {} before changing this path",
                            database.display()
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
    pub(super) fn ensure_no_editable_local_file(&self, database: &Path) -> io::Result<()> {
        for document in self.session.document_ids() {
            let path = self
                .session
                .with_document(document, |state| state.path.clone())?;
            if !path.is_empty()
                && sqlite_related_paths(database)
                    .any(|related| same_local_file(Path::new(&path), &related))
            {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Close the editable views of this database and its sidecars before opening the file viewer",
                ));
            }
        }
        Ok(())
    }
    pub(super) fn ensure_local_file_open_idle(&self) -> io::Result<()> {
        if self.file_operations.active.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Wait for the file operation before opening a file viewer",
            ));
        }
        Ok(())
    }
    fn start_file_job(&mut self, request: OperationRequest, export: bool) -> io::Result<()> {
        if self.file_operations.active.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Wait for the current file operation or cancel it",
            ));
        }
        if request.kind != OperationKind::Copy && matches!(request.source, Endpoint::Local { .. }) {
            for path in &request.paths {
                self.ensure_local_files_unaffected(Path::new(path))?;
            }
        }
        let mut affected = Vec::new();
        if request.kind != OperationKind::Copy {
            for path in &request.paths {
                affected.extend(
                    self.affected_documents(Path::new(path))
                        .into_iter()
                        .map(|(id, _)| id),
                );
            }
        }
        affected.sort();
        affected.dedup();
        for id in &affected {
            if request.kind != OperationKind::Delete {
                self.commit_plugin_edits(*id)?;
            }
            if self.session.save_pending(*id) {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Wait for the pending save before changing its path",
                ));
            }
        }
        for id in &affected {
            self.session.pause_autosave(*id, true)?;
        }
        self.file_operations.summary = None;
        self.file_operations.current.clear();
        self.file_operations.completed = 0;
        self.file_operations.bytes = 0;
        let kind = request.kind;
        let mut refresh_directories = Vec::new();
        if !export {
            if kind != OperationKind::Delete {
                refresh_directories.push(request.directory.clone());
            }
            if kind != OperationKind::Copy || request.duplicate {
                refresh_directories.extend(request.paths.iter().filter_map(|path| {
                    Path::new(path)
                        .parent()
                        .map(|parent| parent.to_string_lossy().into_owned())
                }));
            }
            // Merging into existing folders can change already visible descendants.
            // Refresh those listings too without rescanning the project index.
            if kind != OperationKind::Delete && !request.directory.is_empty() {
                let visible = self.modules.explorer.open_directories();
                refresh_directories.extend(
                    visible
                        .into_iter()
                        .filter(|path| Path::new(path).starts_with(&request.directory)),
                );
            }
            refresh_directories.sort();
            refresh_directories.dedup();
        }
        self.file_operations.active = Some(ActiveOperation {
            local_destination: matches!(request.destination, Endpoint::Local { .. }),
            job: OperationJob::start(request),
            started: Instant::now(),
            workspace: self.files_identity(),
            paused: affected,
            export,
            kind,
            refresh_directories,
            replacement_paths: Vec::new(),
        });
        self.scene += 1;
        Ok(())
    }
    fn project_file_job(
        &mut self,
        kind: OperationKind,
        paths: Vec<String>,
        directory: String,
        duplicate: bool,
    ) -> io::Result<()> {
        let endpoint = self.files_endpoint()?;
        let root = self.files_root();
        let mut paths = top_level_paths(&paths);
        if paths.is_empty() {
            return Ok(());
        }
        for path in &mut paths {
            if self.session.is_remote() {
                if path == &root
                    || !path
                        .strip_prefix(root.trim_end_matches('/'))
                        .is_some_and(|suffix| suffix.starts_with('/'))
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "Choose entries inside this project",
                    ));
                }
            } else {
                *path = file_actions::validate_project_entry(Path::new(&root), Path::new(path))?
                    .to_string_lossy()
                    .into_owned();
            }
        }
        let directory = if !self.session.is_remote() && kind != OperationKind::Delete {
            self.validate_directory(&directory)?;
            fs::canonicalize(directory)?.to_string_lossy().into_owned()
        } else {
            directory
        };
        self.start_file_job(
            OperationRequest {
                kind,
                source: endpoint.clone(),
                destination: endpoint,
                paths,
                directory,
                duplicate,
                trash: Some(Arc::new(file_actions::move_to_trash)),
            },
            false,
        )
    }
    pub(super) fn import_files(
        &mut self,
        paths: Vec<PathBuf>,
        destination: String,
    ) -> io::Result<()> {
        let endpoint = self.files_endpoint()?;
        let destination = if !self.session.is_remote() {
            self.validate_directory(&destination)?;
            fs::canonicalize(&destination)?
                .to_string_lossy()
                .into_owned()
        } else {
            destination
        };
        let paths = paths
            .into_iter()
            .map(|path| std::path::absolute(path).map(|path| path.to_string_lossy().into_owned()))
            .collect::<io::Result<Vec<_>>>()?;
        self.start_file_job(
            OperationRequest {
                kind: OperationKind::Copy,
                source: Endpoint::local("/"),
                destination: endpoint,
                paths,
                directory: destination,
                duplicate: false,
                trash: None,
            },
            false,
        )
    }
    pub(super) fn handle_batch_file_action(&mut self, action: &FileTreeAction) -> io::Result<bool> {
        match action {
            FileTreeAction::OpenMany(paths) | FileTreeAction::OpenManyFromMenu(paths) => {
                let area = Some(self.panel_target_area(PanelTarget::PreviousFocused));
                for path in paths {
                    self.open_file_from_menu(Path::new(path), None, false, area)?;
                }
            }
            FileTreeAction::TrashMany(paths) => self.request_file_deletion(paths.clone())?,
            FileTreeAction::Trash(path) => self.request_file_deletion(vec![path.clone()])?,
            FileTreeAction::Move { paths, destination } => self.project_file_job(
                OperationKind::Move,
                paths.clone(),
                destination.clone(),
                false,
            )?,
            FileTreeAction::Duplicate(paths) => {
                // Each selection item duplicates alongside itself, even across folders.
                if let Some(first) = paths.first() {
                    let root = self.files_root();
                    let parent = first
                        .rsplit_once('/')
                        .map(|(parent, _)| parent)
                        .unwrap_or(&root);
                    let parent = parent.to_owned();
                    self.project_file_job(OperationKind::Copy, paths.clone(), parent, true)?;
                }
            }
            FileTreeAction::Copy(paths) => self.copy_files_to_clipboard(paths.clone())?,
            FileTreeAction::Cut(paths) => {
                let paths = top_level_paths(paths);
                // Finder needs native file references too. The workbench owns
                // move intent; remote paths are meaningful only in this workspace.
                if !self.session.is_remote() {
                    self.copy_files_to_clipboard(paths.clone())?;
                }
                self.file_operations.cut = Some((self.files_identity(), paths));
            }
            FileTreeAction::Paste(directory) => {
                if let Some((workspace, paths)) = self
                    .file_operations
                    .cut
                    .take()
                    .filter(|(workspace, _)| workspace == &self.files_identity())
                {
                    if let Err(error) = self.project_file_job(
                        OperationKind::Move,
                        paths.clone(),
                        directory.clone(),
                        false,
                    ) {
                        self.file_operations.cut = Some((workspace, paths));
                        return Err(error);
                    }
                } else {
                    let paths = self
                        .file_operations
                        .clipboard
                        .as_mut()
                        .ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::Unsupported,
                                "Native file clipboard unavailable",
                            )
                        })?
                        .read_files()?;
                    self.import_files(paths, directory.clone())?;
                }
            }
            FileTreeAction::Refresh => self.refresh_files()?,
            _ => return Ok(false),
        }
        Ok(true)
    }
    fn request_file_deletion(&mut self, paths: Vec<String>) -> io::Result<()> {
        if self.session.is_remote() {
            self.file_operations.pending_delete = Some(top_level_paths(&paths));
            self.file_operations.delete_appearing = true;
        } else {
            self.project_file_job(OperationKind::Delete, paths, String::new(), false)?;
        }
        Ok(())
    }
    fn copy_files_to_clipboard(&mut self, paths: Vec<String>) -> io::Result<()> {
        self.file_operations.cut = None;
        if self.file_operations.clipboard.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Native file clipboard unavailable",
            ));
        }
        if self.session.is_remote() {
            let root = std::env::temp_dir().join("bed-file-exports");
            let directory = root.join(format!(
                "{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&directory)?;
            self.start_file_job(
                OperationRequest {
                    kind: OperationKind::Copy,
                    source: self.files_endpoint()?,
                    destination: Endpoint::local(&directory),
                    paths,
                    directory: directory.to_string_lossy().into_owned(),
                    duplicate: false,
                    trash: None,
                },
                true,
            )?;
        } else {
            self.file_operations
                .clipboard
                .as_mut()
                .unwrap()
                .write_files(&paths.iter().map(PathBuf::from).collect::<Vec<_>>())?;
        }
        Ok(())
    }
    fn cleanup_file_exports(&mut self) {
        let Some(Ok(referenced)) = self
            .file_operations
            .clipboard
            .as_mut()
            .map(|clipboard| clipboard.read_files())
        else {
            return;
        };
        let referenced = referenced
            .into_iter()
            .map(|path| {
                path.parent()
                    .and_then(|parent| fs::canonicalize(parent).ok())
                    .map(|parent| parent.join(path.file_name().unwrap_or_default()))
                    .unwrap_or(path)
            })
            .collect::<Vec<_>>();
        let root = std::env::temp_dir().join("bed-file-exports");
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                let old = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age > Duration::from_secs(7 * 24 * 60 * 60));
                let entry_path = fs::canonicalize(entry.path()).unwrap_or_else(|_| entry.path());
                if old && !referenced.iter().any(|path| path.starts_with(&entry_path)) {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
    }
    pub(super) fn poll_file_operations(&mut self) -> io::Result<()> {
        if !self.file_operations.draining
            && self
                .file_operations
                .active
                .as_ref()
                .is_some_and(|active| active.workspace != self.files_identity())
        {
            self.cancel_file_operations();
        }
        let events = self
            .file_operations
            .active
            .as_ref()
            .map(|active| active.job.events.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in events {
            match event {
                OperationEvent::Progress {
                    path,
                    completed,
                    bytes,
                } => {
                    self.file_operations.current = path;
                    self.file_operations.completed = completed;
                    self.file_operations.bytes = bytes;
                }
                OperationEvent::Conflict(conflict) => {
                    self.file_operations.conflict = Some(conflict);
                    self.file_operations.conflict_appearing = true;
                    self.file_operations.apply_all = false;
                }
                OperationEvent::BeforeCommit { path, reply } => {
                    let result = if self.file_operations.draining
                        || self
                            .file_operations
                            .active
                            .as_ref()
                            .is_none_or(|active| active.job.is_canceled())
                    {
                        Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "File operation canceled",
                        ))
                    } else if self
                        .file_operations
                        .active
                        .as_ref()
                        .is_some_and(|active| active.local_destination)
                    {
                        self.ensure_local_files_unaffected(Path::new(&path))
                    } else {
                        Ok(())
                    };
                    let _ = reply.send(result.map_err(|error| error.to_string()));
                }
                OperationEvent::BeforeReplace { path, reply } => {
                    if self.file_operations.draining
                        || self
                            .file_operations
                            .active
                            .as_ref()
                            .is_none_or(|active| active.job.is_canceled())
                    {
                        let _ = reply.send(Err("File operation canceled".into()));
                        continue;
                    }
                    if let Some(active) = &mut self.file_operations.active {
                        active.replacement_paths.push(path.clone());
                    }
                    let result = (|| {
                        for (id, _) in self.affected_documents(Path::new(&path)) {
                            let committed = self.commit_plugin_edits(id).is_ok();
                            if self.session.save_pending(id) {
                                return Err(io::Error::new(
                                    io::ErrorKind::WouldBlock,
                                    "Destination save is pending",
                                ));
                            }
                            let moving = self
                                .file_operations
                                .active
                                .as_ref()
                                .is_some_and(|active| active.kind == OperationKind::Move);
                            if moving
                                || !committed
                                || self.session.with_document(id, |state| state.dirty)?
                            {
                                self.session.invalidate_removed_path(id)?;
                                self.handle_removed_document(id, &path)?;
                            } else {
                                self.session.pause_autosave(id, true)?;
                                if let Some(active) = &mut self.file_operations.active
                                    && !active.paused.contains(&id)
                                {
                                    active.paused.push(id);
                                }
                            }
                        }
                        Ok(())
                    })();
                    let _ = reply.send(result.map_err(|error: io::Error| error.to_string()));
                }
                OperationEvent::Moved {
                    source,
                    destination,
                    reply,
                } => {
                    let result = self.rebind_moved_files(&source, &destination);
                    let _ = reply.send(result.map_err(|error| error.to_string()));
                }
                OperationEvent::Deleted { path } => {
                    for (id, _) in self.affected_documents(Path::new(&path)) {
                        self.session.invalidate_removed_path(id)?;
                        self.handle_removed_document(id, &path)?;
                    }
                }
                OperationEvent::Finished(mut summary) => {
                    if let Some(active) = self.file_operations.active.take() {
                        summary.canceled |=
                            active.job.is_canceled() || self.file_operations.draining;
                        for id in active.paused {
                            let _ = self.session.pause_autosave(id, false);
                        }
                        for path in active.replacement_paths {
                            if let Err(error) = self.notify_path_changed(&path) {
                                summary.errors.push(format!("Reloading {path}: {error}"));
                            }
                        }
                        if active.export
                            && !summary.canceled
                            && summary.errors.is_empty()
                            && let Some(clipboard) = &mut self.file_operations.clipboard
                            && let Err(error) = clipboard.write_files(
                                &summary
                                    .destinations
                                    .iter()
                                    .map(PathBuf::from)
                                    .collect::<Vec<_>>(),
                            )
                        {
                            summary
                                .errors
                                .push(format!("Publishing file clipboard: {error}"));
                        }
                        self.refresh_file_directories(active.refresh_directories);
                    }
                    self.file_operations.conflict = None;
                    self.file_operations.conflict_appearing = false;
                    self.file_operations.summary = Some(summary);
                }
            }
            self.scene += 1;
        }
        Ok(())
    }
    pub(super) fn poll_workspace_filesystem(&mut self) -> io::Result<()> {
        for update in self.modules.explorer.take_filesystem_updates() {
            if update.root != self.files_root() {
                continue;
            }
            self.session.observe_project_file_changes(&update.changes);
            for change in update.changes {
                match change {
                    bed_remote::FilesystemChange::Renamed { from, to } => {
                        if let Err(error) = self.rebind_moved_files(&from, &to) {
                            self.error = Some(format!("Following moved file: {error}"));
                        }
                        self.notify_path_changed(&to)?;
                    }
                    bed_remote::FilesystemChange::Removed { path } => {
                        for (id, _) in self.affected_documents(Path::new(&path)) {
                            // Confirm absence after the session's grace period so
                            // atomic replacement and paired rename events settle.
                            self.session.notify_disk_change(id)?;
                        }
                    }
                    bed_remote::FilesystemChange::Created { path }
                    | bed_remote::FilesystemChange::Modified { path } => {
                        self.notify_path_changed(&path)?;
                    }
                }
            }
            self.scene += 1;
        }
        Ok(())
    }
    fn notify_path_changed(&mut self, path: &str) -> io::Result<()> {
        for id in self.session.document_ids() {
            if self.session.with_document(id, |state| state.path == path)? {
                self.session.notify_disk_change(id)?;
            }
        }
        Ok(())
    }
    pub(super) fn rebind_moved_files(&mut self, source: &str, destination: &str) -> io::Result<()> {
        for (id, path) in self.affected_documents(Path::new(source)) {
            let path = path.to_string_lossy();
            let target = format!(
                "{destination}{}",
                path.strip_prefix(source).unwrap_or_default()
            );
            self.session.rebind_path(id, Path::new(&target))?;
        }
        Ok(())
    }
    pub(super) fn cancel_file_operations(&mut self) {
        if let Some(active) = &self.file_operations.active {
            active.job.cancel();
        }
        // Drain already committed moves/deletions before releasing document
        // bindings. Blocked I/O still cannot make window shutdown unbounded.
        self.file_operations.draining = true;
        let deadline = Instant::now() + Duration::from_millis(100);
        while self.file_operations.active.is_some() && Instant::now() < deadline {
            if let Err(error) = self.poll_file_operations() {
                self.error = Some(error.to_string());
                break;
            }
            if self.file_operations.active.is_some() {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        self.file_operations.draining = false;
        if let Some(active) = self.file_operations.active.take() {
            active.job.cancel();
            for id in active.paused {
                let _ = self.session.pause_autosave(id, false);
            }
        }
        self.file_operations.conflict = None;
        self.file_operations.conflict_appearing = false;
        self.file_operations.pending_delete = None;
        self.file_operations.delete_appearing = false;
        self.file_operations.cut = None;
        self.file_operations.tiling_drops.clear();
    }
    pub(super) fn ensure_file_operation_idle(&self, document: DocumentId) -> io::Result<()> {
        if self
            .file_operations
            .active
            .as_ref()
            .is_some_and(|active| active.paused.contains(&document))
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Wait for the file operation before saving this document",
            ));
        }
        Ok(())
    }
    pub(super) fn draw_file_operations(&mut self, ui: &Ui) -> io::Result<()> {
        // Quick operations finish quietly. Longer jobs retain cancellation,
        // while only failures need a result window after completion.
        let progress = self
            .file_operations
            .active
            .as_ref()
            .is_some_and(|active| active.started.elapsed() >= Duration::from_millis(500));
        let failed = self
            .file_operations
            .summary
            .as_ref()
            .is_some_and(|summary| !summary.errors.is_empty());
        if progress || failed {
            ui.window("File operations")
                .flags(
                    WindowFlags::ALWAYS_AUTO_RESIZE
                        | WindowFlags::NO_DOCKING
                        | WindowFlags::NO_SAVED_SETTINGS
                        | WindowFlags::NO_FOCUS_ON_APPEARING,
                )
                .build(|| {
                    if self.file_operations.active.is_some() {
                        ui.text_wrapped(&self.file_operations.current);
                        ui.text(format!(
                            "{} items • {:.1} MiB",
                            self.file_operations.completed,
                            self.file_operations.bytes as f64 / 1048576.0
                        ));
                        if ui.button("Cancel")
                            && let Some(active) = &self.file_operations.active
                        {
                            active.job.cancel();
                        }
                    } else if let Some(summary) = self.file_operations.summary.clone() {
                        ui.text(format!(
                            "{} completed, {} skipped{}",
                            summary.completed,
                            summary.skipped,
                            if summary.canceled {
                                " • canceled"
                            } else {
                                ""
                            }
                        ));
                        for error in &summary.errors {
                            ui.text_wrapped(error);
                        }
                        if ui.button("Dismiss") {
                            self.file_operations.summary = None;
                        }
                    }
                });
        }
        if std::mem::take(&mut self.file_operations.delete_appearing) {
            ui.open_popup("Delete permanently?");
        }
        let mut delete = false;
        let mut cancel = false;
        if let Some(_popup) = ui.begin_modal_popup("Delete permanently?") {
            let paths = self
                .file_operations
                .pending_delete
                .as_deref()
                .unwrap_or_default();
            ui.text_wrapped(format!(
                "Permanently delete {} selected items from the remote project?",
                paths.len()
            ));
            for path in paths.iter().take(8) {
                ui.text_wrapped(path);
            }
            if ui.button("Cancel") {
                cancel = true;
                ui.close_current_popup();
            }
            ui.same_line();
            if ui.button("Delete permanently") {
                delete = true;
                ui.close_current_popup();
            }
        }
        if cancel {
            self.file_operations.pending_delete = None;
        }
        if delete && let Some(paths) = self.file_operations.pending_delete.take() {
            self.project_file_job(OperationKind::Delete, paths, String::new(), false)?;
        }
        if std::mem::take(&mut self.file_operations.conflict_appearing) {
            ui.open_popup("Name already exists");
        }
        let mut decision = None;
        if let Some(_popup) = ui.begin_modal_popup("Name already exists") {
            if let Some(conflict) = &self.file_operations.conflict {
                ui.text_wrapped(&conflict.destination);
                ui.checkbox(
                    "Apply to all conflicts of this type",
                    &mut self.file_operations.apply_all,
                );
                for (label, choice, enabled) in [
                    ("Skip", ConflictChoice::Skip, true),
                    ("Keep Both", ConflictChoice::KeepBoth, true),
                    (
                        "Replace",
                        ConflictChoice::Replace,
                        !conflict.source_directory && !conflict.destination_directory,
                    ),
                    (
                        "Merge",
                        ConflictChoice::Merge,
                        conflict.source_directory && conflict.destination_directory,
                    ),
                ] {
                    if enabled {
                        if ui.button(label) {
                            decision = Some(choice);
                            ui.close_current_popup();
                        }
                        ui.same_line();
                    }
                }
                if ui.button("Cancel operation") {
                    if let Some(active) = &self.file_operations.active {
                        active.job.cancel();
                    }
                    ui.close_current_popup();
                }
            } else {
                ui.close_current_popup();
            }
        }
        if let Some(choice) = decision
            && let Some(conflict) = self.file_operations.conflict.take()
        {
            let _ = conflict.reply.send(ConflictDecision {
                choice,
                apply_to_all: self.file_operations.apply_all,
            });
        }
        Ok(())
    }

    /// Open a dropped file in the requested area. Remote files keep this intent
    /// until their asynchronous opening creates the corresponding panel.
    pub(super) fn open_file_in_area(&mut self, path: &Path, area: u32) -> io::Result<()> {
        let area = self
            .current_tiling()
            .layout
            .areas
            .iter()
            .any(|value| value.id == area)
            .then_some(area);
        self.open_file_with_viewer_at(path, None, false, area)
            .map(|_| ())
    }

    /// File-tree rows retain their own directory move targets; panel contents
    /// and empty areas additionally accept files as viewers.
    pub(super) fn accept_tiling_file_drop(&mut self, ui: &Ui) -> io::Result<()> {
        let paths = {
            let explorer = self.modules.explorer.borrow();
            let tree = &explorer.file_tree;
            let Some(paths) = tree.dragged_paths() else {
                return Ok(());
            };
            paths
                .iter()
                .filter(|path| tree.is_directory(path) != Some(true))
                .cloned()
                .collect::<Vec<_>>()
        };
        if paths.is_empty() {
            return Ok(());
        }
        let viewport = ui.with_bound_context(|| unsafe { (*sys::igGetWindowViewport()).ID });
        let Some(area) = self.tiling_area_at(ui.io().mouse_pos(), viewport) else {
            return Ok(());
        };
        let target = ui.get_id("##bed_area_file_drop").raw();
        // SAFETY: The current window and payload belong to this bound frame.
        // Begin/end pair locally, and only the delivery flag is copied out.
        let delivered = ui.with_bound_context(|| unsafe {
            let rect = (*sys::igGetCurrentWindow()).InnerRect;
            if !sys::igBeginDragDropTargetCustom(rect, target) {
                return false;
            }
            let payload = sys::igAcceptDragDropPayload(
                c"BED_FILES".as_ptr(),
                sys::ImGuiDragDropFlags_AcceptBeforeDelivery,
            );
            let delivered = !payload.is_null() && (*payload).Delivery;
            sys::igEndDragDropTarget();
            delivered
        });
        if delivered {
            self.file_operations.tiling_drops.push((area, paths));
        }
        Ok(())
    }

    /// Resolve file-tree drops after drawing restores all borrowed panels.
    pub(super) fn finish_tiling_file_drops(&mut self) -> io::Result<()> {
        for (area, paths) in std::mem::take(&mut self.file_operations.tiling_drops) {
            for path in paths {
                if let Err(error) = self.open_file_in_area(Path::new(&path), area) {
                    self.error = Some(error.to_string());
                }
            }
        }
        Ok(())
    }

    /// Offer OS drops to the actual hovered panel before opening in its area.
    pub fn external_file_drag(&mut self, event: ExternalFileDrag) -> io::Result<bool> {
        let target = self
            .composition
            .iter()
            .rev()
            .find(|(_, viewport, _, visible, rect)| {
                let x = f32::from_bits(rect[0]);
                let y = f32::from_bits(rect[1]);
                *viewport == event.viewport
                    && *visible
                    && event.position[0] >= x
                    && event.position[1] >= y
                    && event.position[0] < x + f32::from_bits(rect[2])
                    && event.position[1] < y + f32::from_bits(rect[3])
            })
            .map(|(id, ..)| *id);
        if self.file_operations.drag_panel != target
            && let Some(previous) = self.file_operations.drag_panel.take()
        {
            let mut cancel = event.clone();
            cancel.phase = ExternalFileDragPhase::Cancel;
            self.dispatch_external_drag(previous, &cancel)?;
        }
        self.file_operations.drag_panel = target;
        let accepted = if let Some(id) = target {
            self.dispatch_external_drag(id, &event)? == ExternalFileDropResponse::Accepted
        } else {
            false
        };
        if event.phase == ExternalFileDragPhase::Drop {
            self.file_operations.drag_panel = None;
            if !accepted {
                let area = self.tiling_area_at(event.position, event.viewport);
                for path in &event.paths {
                    if path.is_dir() {
                        self.set_project(path)?;
                    } else if let Some(area) = area {
                        self.open_file_in_area(path, area)?;
                    } else {
                        self.open_or_focus(path)?;
                    }
                }
            }
        }
        if event.phase == ExternalFileDragPhase::Cancel {
            self.file_operations.drag_panel = None;
        }
        self.scene += 1;
        Ok(accepted)
    }
    fn dispatch_external_drag(
        &mut self,
        id: u64,
        event: &ExternalFileDrag,
    ) -> io::Result<ExternalFileDropResponse> {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return Ok(ExternalFileDropResponse::Ignored);
        };
        let mut tab = self.tabs.remove(index);
        let mut requests = Vec::new();
        let result = self.with_module_services(|modules, services| {
            tab.panel.instance.external_files_with_services(
                event,
                &modules.frame.context(),
                services,
                &mut requests,
            )
        });
        self.tabs.insert(index, tab);
        self.modules.requests.extend(requests);
        result
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/file_operations_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "local_file_tests.rs"]
mod local_file_tests;
