//! The Files feature owns file jobs and their UI. Document drafts are committed
//! by the editor reducer before a worker may change an associated path.
use super::*;
use bed_api::{
    documents::{DocumentReply, SaveToken},
    workbench::FileClipboardService,
};
use bed_documents::{DocumentId, DocumentSession};
use bed_files::operations::{
    Conflict, ConflictChoice, ConflictDecision, Endpoint, OperationEvent, OperationJob,
    OperationKind, OperationRequest, OperationSummary, top_level_paths,
};
use bevy::prelude::*;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Default)]
pub struct FileOperations {
    target: ExplorerTarget,
    target_epoch: u64,
    active: Option<Active>,
    pending: Option<Pending>,
    simple: Option<SimpleJob>,
    cut: Option<(u64, Vec<String>)>,
    conflict: Option<Conflict>,
    conflict_appearing: bool,
    apply_all: bool,
    summary: Option<OperationSummary>,
    current: String,
    completed: usize,
    bytes: u64,
    dialog: Option<NameDialog>,
    delete: Option<Vec<String>>,
    delete_appearing: bool,
}

struct Active {
    job: OperationJob,
    session_id: Option<u64>,
    document_provider: bed_files::FileServices,
    started: Instant,
    paused: Vec<DocumentId>,
    export: bool,
    local_destination: bool,
    destination_provider: bed_files::FileServices,
    kind: OperationKind,
    refresh: Vec<String>,
    replacements: Vec<String>,
}

struct Pending {
    token: SaveToken,
    session_id: Option<u64>,
    document_provider: bed_files::FileServices,
    documents: Vec<DocumentId>,
    work: PendingWork,
}

enum PendingWork {
    Start(OperationRequest, bool),
    Replace {
        path: String,
        reply: mpsc::SyncSender<Result<(), String>>,
    },
    Simple(SimpleCommand),
}

struct NameDialog {
    action: FileTreeAction,
    name: String,
    appearing: bool,
    error: Option<String>,
}

struct SimpleJob {
    receiver: mpsc::Receiver<Result<SimpleResult, String>>,
    session_id: Option<u64>,
    document_provider: bed_files::FileServices,
    paused: Vec<DocumentId>,
}

struct SimpleCommand {
    request: bed_remote::Request,
    services: bed_files::FileServices,
    result: SimpleResult,
}

enum SimpleResult {
    Created(String, bool),
    Renamed(String, String),
}

impl FileOperations {
    /// The host's filesystem target owns transport and queued operation identity.
    pub fn bind_target(&mut self, target: &ExplorerTarget, session: &mut DocumentSession) -> bool {
        let same_connection = self.target.services.same_provider(&target.services);
        if self.target.root == target.root && same_connection {
            return false;
        }
        self.cancel(session);
        self.target_epoch = self
            .target_epoch
            .checked_add(1)
            .expect("Files target epoch overflow");
        self.target = target.clone();
        self.cut = None;
        self.summary = None;
        self.current.clear();
        self.completed = 0;
        self.bytes = 0;
        true
    }

    fn affected_documents(
        &self,
        session: &DocumentSession,
        path: &str,
    ) -> io::Result<Vec<DocumentId>> {
        if provider_matches(&self.target.services, session) {
            affected_documents(session, path)
        } else {
            Ok(Vec::new())
        }
    }

    pub fn busy(&self) -> bool {
        self.active.is_some() || self.pending.is_some() || self.simple.is_some()
    }

    pub fn modal_visible(&self) -> bool {
        self.dialog.is_some() || self.delete.is_some() || self.conflict.is_some()
    }

    pub fn cancel(&mut self, session: &mut DocumentSession) {
        if let Some(active) = self.active.take() {
            active.job.cancel();
            for document in active.paused {
                let _ = session.pause_autosave(document, false);
            }
        }
        if let Some(simple) = self.simple.take() {
            for document in simple.paused {
                let _ = session.pause_autosave(document, false);
            }
        }
        if let Some(Pending {
            work: PendingWork::Replace { reply, .. },
            ..
        }) = self.pending.take()
        {
            let _ = reply.send(Err("File operation canceled".into()));
        }
        self.conflict = None;
        self.dialog = None;
        self.delete = None;
        self.cut = None;
    }

    fn queue(
        &mut self,
        session: &DocumentSession,
        work: PendingWork,
        documents: Vec<DocumentId>,
        requests: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        if self.pending.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Wait for document preparation",
            ));
        }
        let token = SaveToken::next();
        let session_id = (!documents.is_empty()).then(|| session.session_id().0);
        if session_id.is_some() {
            requests.push(HostAction::CommitDocumentsWithResult {
                recipient: MODULE_ID.into(),
                token,
                documents: documents.clone(),
            });
        }
        self.pending = Some(Pending {
            token,
            session_id,
            document_provider: self.target.services.clone(),
            documents,
            work,
        });
        Ok(())
    }

    fn start(
        &mut self,
        request: OperationRequest,
        export: bool,
        session: &DocumentSession,
        requests: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        if self.busy() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Wait for the current file operation or cancel it",
            ));
        }
        let mut affected = Vec::new();
        if request.kind != OperationKind::Copy {
            for path in &request.paths {
                affected.extend(self.affected_documents(session, path)?);
            }
        }
        affected.sort();
        affected.dedup();
        self.queue(
            session,
            PendingWork::Start(request, export),
            affected,
            requests,
        )
    }

    pub fn reply(
        &mut self,
        reply: &DocumentReply,
        session: &mut DocumentSession,
        explorer: &ExplorerState,
    ) {
        let DocumentReply::Save {
            recipient,
            token,
            result,
        } = reply
        else {
            return;
        };
        if recipient != MODULE_ID
            || self
                .pending
                .as_ref()
                .is_none_or(|pending| pending.token != *token)
        {
            return;
        }
        let pending = self
            .pending
            .take()
            .expect("the matching document preparation is pending");
        let result = result.as_ref().map_err(Clone::clone).and_then(|saved| {
            if document_access_changed(pending.session_id, &pending.document_provider, session) {
                return Err("Workspace changed while preparing file operation".into());
            }
            if matches!(&pending.work, PendingWork::Replace { .. })
                && self
                    .active
                    .as_ref()
                    .is_none_or(|active| active.job.is_canceled())
            {
                return Err("File operation canceled".into());
            }
            for document in &pending.documents {
                let revision = session
                    .document_revision(*document)
                    .map_err(|error| error.to_string())?;
                if !saved
                    .iter()
                    .any(|saved| saved.document == *document && saved.revision == revision)
                {
                    return Err("Document changed while preparing file operation".into());
                }
            }
            Ok(())
        });
        if let Err(error) = result {
            if let PendingWork::Replace { reply, .. } = pending.work {
                let _ = reply.send(Err(error.clone()));
            }
            self.summary = Some(OperationSummary {
                errors: vec![error],
                ..Default::default()
            });
            return;
        }
        match pending.work {
            PendingWork::Start(request, export) => {
                let mut refresh = explorer.open_directories();
                refresh.push(request.directory.clone());
                refresh.extend(request.paths.iter().filter_map(|path| {
                    Path::new(path)
                        .parent()
                        .map(|parent| parent.to_string_lossy().into_owned())
                }));
                refresh.sort();
                refresh.dedup();
                for document in &pending.documents {
                    let _ = session.pause_autosave(*document, true);
                }
                self.summary = None;
                self.active = Some(Active {
                    session_id: pending.session_id,
                    document_provider: pending.document_provider,
                    started: Instant::now(),
                    paused: pending.documents,
                    export,
                    local_destination: matches!(request.destination, Endpoint::Local { .. }),
                    destination_provider: match &request.destination {
                        Endpoint::Remote { .. } => self.target.services.clone(),
                        Endpoint::Local { .. } => bed_files::FileServices::local(),
                    },
                    kind: request.kind,
                    refresh,
                    replacements: Vec::new(),
                    job: OperationJob::start(request),
                });
            }
            PendingWork::Replace { path, reply } => {
                let result = (|| -> io::Result<()> {
                    if let Some(active) = &mut self.active
                        && pending.session_id.is_some()
                    {
                        active.session_id = pending.session_id;
                        active.document_provider = pending.document_provider.clone();
                    }
                    let moving = self
                        .active
                        .as_ref()
                        .is_some_and(|active| active.kind == OperationKind::Move);
                    for document in pending.documents {
                        if moving || session.with_document(document, |state| state.dirty)? {
                            session.invalidate_removed_path(document)?;
                        } else {
                            session.pause_autosave(document, true)?;
                            if let Some(active) = &mut self.active {
                                active.paused.push(document);
                            }
                        }
                    }
                    if let Some(active) = &mut self.active {
                        active.replacements.push(path);
                    }
                    Ok(())
                })();
                let _ = reply.send(result.map_err(|error| error.to_string()));
            }
            PendingWork::Simple(command) => {
                for document in &pending.documents {
                    let _ = session.pause_autosave(*document, true);
                }
                self.start_simple(command, pending.session_id, pending.documents);
            }
        }
    }

    fn start_simple(
        &mut self,
        command: SimpleCommand,
        session_id: Option<u64>,
        paused: Vec<DocumentId>,
    ) {
        let document_provider = command.services.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let response = command.services.call(command.request);
            let _ = sender.send(
                response
                    .map(|_| command.result)
                    .map_err(|error| error.to_string()),
            );
        });
        self.simple = Some(SimpleJob {
            receiver,
            session_id,
            document_provider,
            paused,
        });
    }

    pub fn poll(
        &mut self,
        session: &mut DocumentSession,
        explorer: &mut ExplorerState,
        clipboard: Option<&mut dyn FileClipboardService>,
        protected: &[PathBuf],
        requests: &mut Vec<HostAction>,
    ) {
        if self.active.as_ref().is_some_and(|active| {
            document_access_changed(active.session_id, &active.document_provider, session)
        }) || self.pending.as_ref().is_some_and(|pending| {
            document_access_changed(pending.session_id, &pending.document_provider, session)
        }) || self.simple.as_ref().is_some_and(|job| {
            document_access_changed(job.session_id, &job.document_provider, session)
        }) {
            self.cancel(session);
        }
        if let Some(token) = self
            .pending
            .as_ref()
            .filter(|pending| pending.session_id.is_none())
            .map(|pending| pending.token)
        {
            self.reply(
                &DocumentReply::Save {
                    recipient: MODULE_ID.into(),
                    token,
                    result: Ok(Vec::new()),
                },
                session,
                explorer,
            );
        }
        let result = self
            .simple
            .as_ref()
            .and_then(|job| job.receiver.try_recv().ok());
        if let Some(result) = result {
            let job = self
                .simple
                .take()
                .expect("completed file command is owned by this feature");
            for document in job.paused {
                let _ = session.pause_autosave(document, false);
            }
            match result {
                Ok(SimpleResult::Created(path, file)) => {
                    if file && provider_matches(&self.target.services, session) {
                        requests.push(HostAction::OpenFileIn {
                            path,
                            viewer: None,
                            target: bed_api::workbench::PanelTarget::LargestArea,
                        });
                    }
                    explorer.file_explorer.refresh_file_tree();
                }
                Ok(SimpleResult::Renamed(from, to)) => {
                    if provider_matches(&self.target.services, session)
                        && let Err(error) = rebind(session, &from, &to)
                    {
                        requests.push(notify(error));
                    }
                    explorer.file_explorer.refresh_file_tree();
                }
                Err(message) => requests.push(HostAction::Notify { message }),
            }
            requests.push(HostAction::Invalidate);
        }
        let events = self
            .active
            .as_ref()
            .map(|active| active.job.events.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut clipboard = clipboard;
        for event in events {
            match event {
                OperationEvent::Progress {
                    path,
                    completed,
                    bytes,
                } => {
                    self.current = path;
                    self.completed = completed;
                    self.bytes = bytes;
                }
                OperationEvent::Conflict(conflict) => {
                    self.conflict = Some(conflict);
                    self.conflict_appearing = true;
                    self.apply_all = false;
                }
                OperationEvent::BeforeCommit { path, reply } => {
                    let result = if self
                        .active
                        .as_ref()
                        .is_none_or(|active| active.job.is_canceled())
                    {
                        Err("File operation canceled".into())
                    } else if self
                        .active
                        .as_ref()
                        .is_some_and(|active| active.local_destination)
                    {
                        ensure_unprotected(&path, protected).map_err(|error| error.to_string())
                    } else {
                        Ok(())
                    };
                    let _ = reply.send(result);
                }
                OperationEvent::BeforeReplace { path, reply } => {
                    let documents = if self.active.as_ref().is_some_and(|active| {
                        provider_matches(&active.destination_provider, session)
                    }) {
                        affected_documents(session, &path)
                    } else {
                        Ok(Vec::new())
                    };
                    match documents {
                        Ok(documents) => {
                            let work = PendingWork::Replace {
                                path,
                                reply: reply.clone(),
                            };
                            if let Err(error) = self.queue(session, work, documents, requests) {
                                let _ = reply.send(Err(error.to_string()));
                            }
                        }
                        Err(error) => {
                            let _ = reply.send(Err(error.to_string()));
                        }
                    }
                }
                OperationEvent::Moved {
                    source,
                    destination,
                    reply,
                } => {
                    let result = if provider_matches(&self.target.services, session) {
                        rebind(session, &source, &destination).map_err(|error| error.to_string())
                    } else {
                        Ok(())
                    };
                    let _ = reply.send(result);
                }
                OperationEvent::Deleted { path } => match self.affected_documents(session, &path) {
                    Ok(documents) => {
                        for document in documents {
                            if let Err(error) = session.invalidate_removed_path(document) {
                                requests.push(notify(error));
                            }
                        }
                    }
                    Err(error) => requests.push(notify(error)),
                },
                OperationEvent::Finished(mut summary) => {
                    if let Some(active) = self.active.take() {
                        summary.canceled |= active.job.is_canceled();
                        for document in active.paused {
                            let _ = session.pause_autosave(document, false);
                        }
                        for path in active.replacements {
                            if provider_matches(&active.destination_provider, session)
                                && let Ok(documents) = affected_documents(session, &path)
                            {
                                for document in documents {
                                    let _ = session.notify_disk_change(document);
                                }
                            }
                        }
                        if active.export && !summary.canceled && summary.errors.is_empty() {
                            if let Some(clipboard) = clipboard.as_deref_mut() {
                                if let Err(error) = clipboard.write_files(
                                    &summary
                                        .destinations
                                        .iter()
                                        .map(PathBuf::from)
                                        .collect::<Vec<_>>(),
                                ) {
                                    summary
                                        .errors
                                        .push(format!("Publishing file clipboard: {error}"));
                                }
                            } else {
                                summary
                                    .errors
                                    .push("Native file clipboard unavailable".into());
                            }
                        }
                        explorer
                            .file_explorer
                            .file_finder
                            .refresh_directories(active.refresh);
                    }
                    self.conflict = None;
                    self.summary = Some(summary);
                }
            }
            requests.push(HostAction::Invalidate);
        }
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "The file transition names its document, UI, native and protection inputs explicitly"
    )]
    pub fn action(
        &mut self,
        action: ExplorerAction,
        session: &mut DocumentSession,
        explorer: &mut ExplorerState,
        root: &str,
        clipboard: Option<&mut dyn FileClipboardService>,
        protected: &[PathBuf],
        requests: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        if root != self.target.root {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Files target changed before the operation was submitted",
            ));
        }
        if matches!(
            &action,
            ExplorerAction::Tree(
                FileTreeAction::Preview(_)
                    | FileTreeAction::Open(_)
                    | FileTreeAction::OpenWith { .. }
                    | FileTreeAction::OpenMany(_)
                    | FileTreeAction::OpenManyFromMenu(_)
            )
        ) && !provider_matches(&self.target.services, session)
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                DOCUMENT_PROVIDER_UNAVAILABLE,
            ));
        }
        let action = match action {
            ExplorerAction::ChooseDirectory => {
                requests.push(HostAction::ChooseDirectory);
                return Ok(());
            }
            ExplorerAction::PreferencesChanged(_) => {
                requests.push(HostAction::Invalidate);
                return Ok(());
            }
            ExplorerAction::RefreshRemote { force } => {
                if force {
                    explorer.file_explorer.refresh_file_tree();
                }
                return Ok(());
            }
            ExplorerAction::Import { paths, destination } => {
                let destination = validate_directory(&self.target, &destination)?;
                let paths = paths
                    .into_iter()
                    .map(|path| {
                        std::path::absolute(path).map(|path| path.to_string_lossy().into_owned())
                    })
                    .collect::<io::Result<Vec<_>>>()?;
                let destination_endpoint = endpoint(&self.target)?;
                return self.start(
                    OperationRequest {
                        kind: OperationKind::Copy,
                        source: Endpoint::local("/"),
                        destination: destination_endpoint,
                        paths,
                        directory: destination,
                        duplicate: false,
                        trash: None,
                    },
                    false,
                    session,
                    requests,
                );
            }
            ExplorerAction::Tree(action) => action,
        };
        match action {
            FileTreeAction::Preview(path) => requests.push(HostAction::PreviewFileIn {
                path,
                viewer: None,
                target: bed_api::workbench::PanelTarget::LargestArea,
            }),
            FileTreeAction::Command { command, context } => {
                requests.push(HostAction::Command { command, context })
            }
            FileTreeAction::Open(path) => requests.push(HostAction::OpenFileIn {
                path,
                viewer: None,
                target: bed_api::workbench::PanelTarget::LargestArea,
            }),
            FileTreeAction::OpenWith { path, viewer } => requests.push(HostAction::OpenFileIn {
                path,
                viewer: Some(viewer),
                target: bed_api::workbench::PanelTarget::LargestArea,
            }),
            FileTreeAction::OpenMany(paths) | FileTreeAction::OpenManyFromMenu(paths) => {
                for path in paths {
                    requests.push(HostAction::OpenFileIn {
                        path,
                        viewer: None,
                        target: bed_api::workbench::PanelTarget::LargestArea,
                    });
                }
            }
            action @ (FileTreeAction::NewFile(_)
            | FileTreeAction::NewFolder(_)
            | FileTreeAction::Rename(_)) => {
                let name = match &action {
                    FileTreeAction::Rename(path) => Path::new(path)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                self.dialog = Some(NameDialog {
                    action,
                    name,
                    appearing: true,
                    error: None,
                });
            }
            FileTreeAction::Trash(path) => {
                self.delete_paths(vec![path], session, protected, requests)?
            }
            FileTreeAction::TrashMany(paths) => {
                self.delete_paths(paths, session, protected, requests)?
            }
            FileTreeAction::Move { paths, destination } => self.project_job(
                OperationKind::Move,
                paths,
                destination,
                false,
                session,
                protected,
                requests,
            )?,
            FileTreeAction::Duplicate(paths) => {
                if let Some(first) = paths.first() {
                    let parent = Path::new(first)
                        .parent()
                        .unwrap_or(Path::new(root))
                        .to_string_lossy()
                        .into_owned();
                    self.project_job(
                        OperationKind::Copy,
                        paths,
                        parent,
                        true,
                        session,
                        protected,
                        requests,
                    )?;
                }
            }
            FileTreeAction::Copy(paths) => {
                self.cut = None;
                let clipboard = clipboard.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::Unsupported,
                        "Native file clipboard unavailable",
                    )
                })?;
                if self.target.services.is_remote() {
                    let directory = std::env::temp_dir().join("bed-file-exports").join(format!(
                        "{}-{}",
                        std::process::id(),
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    ));
                    fs::create_dir_all(&directory)?;
                    self.start(
                        OperationRequest {
                            kind: OperationKind::Copy,
                            source: endpoint(&self.target)?,
                            destination: Endpoint::local(&directory),
                            paths,
                            directory: directory.to_string_lossy().into_owned(),
                            duplicate: false,
                            trash: None,
                        },
                        true,
                        session,
                        requests,
                    )?;
                } else {
                    clipboard.write_files(&paths.iter().map(PathBuf::from).collect::<Vec<_>>())?;
                }
            }
            FileTreeAction::Cut(paths) => {
                let paths = top_level_paths(&paths);
                if !self.target.services.is_remote() {
                    clipboard
                        .ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::Unsupported,
                                "Native file clipboard unavailable",
                            )
                        })?
                        .write_files(&paths.iter().map(PathBuf::from).collect::<Vec<_>>())?;
                }
                self.cut = Some((self.target_epoch, paths));
            }
            FileTreeAction::Paste(directory) => {
                if let Some((session_id, paths)) = self
                    .cut
                    .take()
                    .filter(|(epoch, _)| *epoch == self.target_epoch)
                {
                    if let Err(error) = self.project_job(
                        OperationKind::Move,
                        paths.clone(),
                        directory,
                        false,
                        session,
                        protected,
                        requests,
                    ) {
                        self.cut = Some((session_id, paths));
                        return Err(error);
                    }
                } else {
                    let paths = clipboard
                        .ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::Unsupported,
                                "Native file clipboard unavailable",
                            )
                        })?
                        .read_files()?;
                    self.action(
                        ExplorerAction::Import {
                            paths,
                            destination: directory,
                        },
                        session,
                        explorer,
                        root,
                        None,
                        protected,
                        requests,
                    )?;
                }
            }
            FileTreeAction::Refresh => explorer.file_explorer.refresh_file_tree(),
            _ => {}
        }
        requests.push(HostAction::Invalidate);
        Ok(())
    }

    fn delete_paths(
        &mut self,
        paths: Vec<String>,
        session: &DocumentSession,
        protected: &[PathBuf],
        requests: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        if self.target.services.is_remote() {
            self.delete = Some(top_level_paths(&paths));
            self.delete_appearing = true;
            Ok(())
        } else {
            self.project_job(
                OperationKind::Delete,
                paths,
                String::new(),
                false,
                session,
                protected,
                requests,
            )
        }
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "One operation boundary validates the complete request against the current workspace and protected paths"
    )]
    fn project_job(
        &mut self,
        kind: OperationKind,
        paths: Vec<String>,
        directory: String,
        duplicate: bool,
        session: &DocumentSession,
        protected: &[PathBuf],
        requests: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        let paths = top_level_paths(&paths)
            .into_iter()
            .map(|path| validate_entry(&self.target, &path))
            .collect::<io::Result<Vec<_>>>()?;
        if kind != OperationKind::Copy && !self.target.services.is_remote() {
            for path in &paths {
                ensure_unprotected(path, protected)?;
            }
        }
        let directory = if kind == OperationKind::Delete {
            directory
        } else {
            validate_directory(&self.target, &directory)?
        };
        let endpoint = endpoint(&self.target)?;
        self.start(
            OperationRequest {
                kind,
                source: endpoint.clone(),
                destination: endpoint,
                paths,
                directory,
                duplicate,
                trash: Some(Arc::new(|path| {
                    trash::delete(path).map_err(io::Error::other)
                })),
            },
            false,
            session,
            requests,
        )
    }

    pub fn draw(
        &mut self,
        ui: &Ui,
        session: &DocumentSession,
        root: &str,
        protected: &[PathBuf],
        requests: &mut Vec<HostAction>,
    ) {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.started.elapsed() >= Duration::from_millis(500))
            || self
                .summary
                .as_ref()
                .is_some_and(|summary| !summary.errors.is_empty())
        {
            ui.window("File operations")
                .flags(
                    dear_imgui_rs::WindowFlags::ALWAYS_AUTO_RESIZE
                        | dear_imgui_rs::WindowFlags::NO_DOCKING
                        | dear_imgui_rs::WindowFlags::NO_SAVED_SETTINGS
                        | dear_imgui_rs::WindowFlags::NO_FOCUS_ON_APPEARING,
                )
                .build(|| {
                    if let Some(active) = &self.active {
                        ui.text_wrapped(&self.current);
                        ui.text(format!(
                            "{} items • {:.1} MiB",
                            self.completed,
                            self.bytes as f64 / 1048576.0
                        ));
                        if ui.button("Cancel") {
                            active.job.cancel();
                        }
                    } else if let Some(summary) = self.summary.clone() {
                        ui.text(format!(
                            "{} completed, {} skipped",
                            summary.completed, summary.skipped
                        ));
                        for error in &summary.errors {
                            ui.text_wrapped(error);
                        }
                        if ui.button("Dismiss") {
                            self.summary = None;
                        }
                    }
                });
        }
        if self.dialog.as_ref().is_some_and(|dialog| dialog.appearing) {
            ui.open_popup("File name");
            self.dialog.as_mut().expect("visible dialog").appearing = false;
        }
        let mut submit = false;
        let mut cancel = false;
        if let Some(_popup) = ui.begin_modal_popup("File name") {
            if let Some(dialog) = &mut self.dialog {
                let label = match dialog.action {
                    FileTreeAction::NewFile(_) => "New file",
                    FileTreeAction::NewFolder(_) => "New folder",
                    _ => "Rename",
                };
                ui.text(label);
                submit |= ui
                    .input_text("Name", &mut dialog.name)
                    .enter_returns_true(true)
                    .build();
                if let Some(error) = &dialog.error {
                    ui.text_wrapped(error);
                }
                cancel = ui.button("Cancel");
                ui.same_line();
                submit |= ui.button("OK");
            }
            if cancel || self.dialog.is_none() {
                ui.close_current_popup();
            }
        }
        if cancel {
            self.dialog = None;
        }
        if submit && let Some(dialog) = self.dialog.take() {
            match self.submit_name(&dialog, session, root, protected, requests) {
                Ok(()) => {}
                Err(error) => {
                    self.dialog = Some(NameDialog {
                        error: Some(error.to_string()),
                        ..dialog
                    });
                }
            }
        }
        if std::mem::take(&mut self.delete_appearing) {
            ui.open_popup("Delete permanently?");
        }
        let mut delete = false;
        if let Some(_popup) = ui.begin_modal_popup("Delete permanently?") {
            ui.text_wrapped(format!(
                "Permanently delete {} selected items from the remote project?",
                self.delete.as_ref().map_or(0, Vec::len)
            ));
            if ui.button("Cancel") {
                self.delete = None;
                ui.close_current_popup();
            }
            ui.same_line();
            if ui.button("Delete permanently") {
                delete = true;
                ui.close_current_popup();
            }
        }
        if delete
            && let Some(paths) = self.delete.take()
            && let Err(error) = self.project_job(
                OperationKind::Delete,
                paths,
                String::new(),
                false,
                session,
                protected,
                requests,
            )
        {
            requests.push(notify(error));
        }
        if std::mem::take(&mut self.conflict_appearing) {
            ui.open_popup("Name already exists");
        }
        let mut choice = None;
        if let Some(_popup) = ui.begin_modal_popup("Name already exists") {
            if let Some(conflict) = &self.conflict {
                ui.text_wrapped(&conflict.destination);
                ui.checkbox("Apply to all conflicts of this type", &mut self.apply_all);
                for (label, value, enabled) in [
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
                            choice = Some(value);
                            ui.close_current_popup();
                        }
                        ui.same_line();
                    }
                }
                if ui.button("Cancel operation") {
                    if let Some(active) = &self.active {
                        active.job.cancel();
                    }
                    ui.close_current_popup();
                }
            } else {
                ui.close_current_popup();
            }
        }
        if let Some(choice) = choice
            && let Some(conflict) = self.conflict.take()
        {
            let _ = conflict.reply.send(ConflictDecision {
                choice,
                apply_to_all: self.apply_all,
            });
        }
    }

    fn submit_name(
        &mut self,
        dialog: &NameDialog,
        session: &DocumentSession,
        root: &str,
        protected: &[PathBuf],
        requests: &mut Vec<HostAction>,
    ) -> io::Result<()> {
        if self.busy() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Wait for the current file operation",
            ));
        }
        bed_files::actions::validate_name(&dialog.name)?;
        let (request, result, documents) = match &dialog.action {
            FileTreeAction::NewFile(directory) | FileTreeAction::NewFolder(directory) => {
                let directory = validate_directory(&self.target, directory)?;
                let path = Path::new(&directory)
                    .join(&dialog.name)
                    .to_string_lossy()
                    .into_owned();
                let file = matches!(dialog.action, FileTreeAction::NewFile(_));
                let request = if file {
                    bed_remote::Request::CreateFile {
                        root: root.to_owned(),
                        path: path.clone(),
                    }
                } else {
                    bed_remote::Request::CreateDirectory {
                        root: root.to_owned(),
                        path: path.clone(),
                    }
                };
                (request, SimpleResult::Created(path, file), Vec::new())
            }
            FileTreeAction::Rename(from) => {
                let from = validate_entry(&self.target, from)?;
                if !self.target.services.is_remote() {
                    ensure_unprotected(&from, protected)?;
                }
                let to = Path::new(&from)
                    .with_file_name(&dialog.name)
                    .to_string_lossy()
                    .into_owned();
                let documents = self.affected_documents(session, &from)?;
                for document in &documents {
                    let current = session.with_document(*document, |state| state.path.clone())?;
                    let destination = Path::new(&to).join(
                        Path::new(&current)
                            .strip_prefix(&from)
                            .unwrap_or(Path::new("")),
                    );
                    session.check_rebind_path(*document, &destination)?;
                }
                (
                    bed_remote::Request::Rename {
                        root: root.to_owned(),
                        from: from.clone(),
                        to: to.clone(),
                    },
                    SimpleResult::Renamed(from, to),
                    documents,
                )
            }
            _ => unreachable!("only named actions create this dialog"),
        };
        self.queue(
            session,
            PendingWork::Simple(SimpleCommand {
                request,
                services: self.target.services.clone(),
                result,
            }),
            documents,
            requests,
        )
    }
}

fn notify(error: io::Error) -> HostAction {
    HostAction::Notify {
        message: error.to_string(),
    }
}

fn endpoint(target: &ExplorerTarget) -> io::Result<Endpoint> {
    let root = &target.root;
    if root.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Open Files to choose a directory first",
        ));
    }
    target.services.endpoint(&target.root)
}

fn remote_inside(root: &str, path: &str, allow_root: bool) -> bool {
    (allow_root && path == root)
        || path
            .strip_prefix(root.trim_end_matches('/'))
            .is_some_and(|suffix| {
                suffix.starts_with('/') && !suffix.split('/').any(|part| part == "..")
            })
}

fn validate_entry(target: &ExplorerTarget, path: &str) -> io::Result<String> {
    let root = &target.root;
    if target.services.is_remote() {
        if !remote_inside(root, path, false) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Choose an entry inside the project",
            ));
        }
        Ok(path.into())
    } else {
        Ok(
            bed_files::actions::validate_project_entry(Path::new(root), Path::new(path))?
                .to_string_lossy()
                .into_owned(),
        )
    }
}

fn validate_directory(target: &ExplorerTarget, path: &str) -> io::Result<String> {
    let root = &target.root;
    if target.services.is_remote() {
        if !remote_inside(root, path, true) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Choose a directory inside the project",
            ));
        }
        Ok(path.into())
    } else {
        let root = fs::canonicalize(root)?;
        let path = fs::canonicalize(path)?;
        if !path.starts_with(root) || !path.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Choose a directory inside the project",
            ));
        }
        Ok(path.to_string_lossy().into_owned())
    }
}

fn affected_documents(session: &DocumentSession, path: &str) -> io::Result<Vec<DocumentId>> {
    let mut result = Vec::new();
    for document in session.document_ids() {
        if session.with_document(document, |state| {
            !state.path.is_empty() && Path::new(&state.path).starts_with(path)
        })? {
            result.push(document);
        }
    }
    Ok(result)
}

pub(crate) fn provider_matches(
    services: &bed_files::FileServices,
    session: &DocumentSession,
) -> bool {
    session.file_provider_matches(services)
}

fn document_access_changed(
    session_id: Option<u64>,
    services: &bed_files::FileServices,
    session: &DocumentSession,
) -> bool {
    session_id.is_some_and(|session_id| {
        session_id != session.session_id().0 || !provider_matches(services, session)
    })
}

pub(crate) const DOCUMENT_PROVIDER_UNAVAILABLE: &str =
    "Configure document access for this Files location before opening its files";

pub(crate) fn guard_document_requests(
    target: &ExplorerTarget,
    session: &DocumentSession,
    requests: &mut Vec<HostAction>,
) {
    if provider_matches(&target.services, session) {
        return;
    }
    let count = requests.len();
    requests.retain(|request| {
        !matches!(
            request,
            HostAction::OpenFile { .. }
                | HostAction::OpenFileIn { .. }
                | HostAction::PreviewFileIn { .. }
                | HostAction::DropFile { .. }
        )
    });
    if requests.len() != count {
        requests.push(HostAction::Notify {
            message: DOCUMENT_PROVIDER_UNAVAILABLE.into(),
        });
    }
}

fn rebind(session: &mut DocumentSession, from: &str, to: &str) -> io::Result<()> {
    let mut changes = Vec::new();
    for document in affected_documents(session, from)? {
        let path = session.with_document(document, |state| state.path.clone())?;
        let tail = Path::new(&path)
            .strip_prefix(from)
            .expect("affected path is inside moved entry");
        let destination = if tail.as_os_str().is_empty() {
            PathBuf::from(to)
        } else {
            Path::new(to).join(tail)
        };
        session.check_rebind_path(document, &destination)?;
        changes.push((document, destination));
    }
    for (document, destination) in changes {
        session.rebind_path(document, &destination)?;
    }
    Ok(())
}

fn ensure_unprotected(path: &str, protected: &[PathBuf]) -> io::Result<()> {
    let entry = Path::new(path);
    let resolved = fs::symlink_metadata(entry)
        .ok()
        .filter(|metadata| !metadata.file_type().is_symlink())
        .and_then(|_| fs::canonicalize(entry).ok());
    if protected.iter().any(|protected| {
        protected.starts_with(entry)
            || resolved
                .as_ref()
                .is_some_and(|resolved| protected.starts_with(resolved))
    }) {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Close the file viewer before changing its path",
        ));
    }
    Ok(())
}

pub(crate) fn reconcile(
    session: &mut DocumentSession,
    explorer: &mut ExplorerState,
    target: &ExplorerTarget,
    requests: &mut Vec<HostAction>,
) {
    for update in explorer.take_filesystem_updates() {
        if update.root != target.root || !provider_matches(&target.services, session) {
            continue;
        }
        session.observe_project_file_changes(&update.changes);
        for change in update.changes {
            let result = match change {
                bed_remote::FilesystemChange::Renamed { from, to } => rebind(session, &from, &to),
                bed_remote::FilesystemChange::Created { path }
                | bed_remote::FilesystemChange::Modified { path }
                | bed_remote::FilesystemChange::Removed { path } => {
                    affected_documents(session, &path).and_then(|documents| {
                        for document in documents {
                            session.notify_disk_change(document)?;
                        }
                        Ok(())
                    })
                }
            };
            if let Err(error) = result {
                requests.push(notify(error));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_api::documents::SavedDocument;
    use bed_documents::ByteEdit;

    fn fixture() -> (
        crate::test_support::TempDir,
        DocumentSession,
        ExplorerState,
        String,
        DocumentId,
    ) {
        let temp = crate::test_support::TempDir::new();
        fs::create_dir_all(temp.path("destination")).unwrap();
        fs::write(temp.path("source.txt"), b"on disk").unwrap();
        let mut session = DocumentSession::new();
        let document = session.open_file(&temp.path("source.txt")).unwrap();
        let mut explorer = ExplorerState::default();
        let root_directory = temp.root().to_string_lossy().into_owned();
        explorer.file_explorer.project_root = root_directory.clone();
        (temp, session, explorer, root_directory, document)
    }

    fn local_operations(session: &mut DocumentSession, root: &str) -> FileOperations {
        let mut operations = FileOperations::default();
        operations.bind_target(
            &ExplorerTarget {
                root: root.into(),
                services: bed_files::FileServices::local(),
            },
            session,
        );
        operations
    }

    fn request_move(
        operations: &mut FileOperations,
        session: &mut DocumentSession,
        explorer: &mut ExplorerState,
        root: &str,
        temp: &crate::test_support::TempDir,
    ) -> SaveToken {
        let mut requests = Vec::new();
        operations
            .action(
                ExplorerAction::Tree(FileTreeAction::Move {
                    paths: vec![temp.path("source.txt").to_string_lossy().into_owned()],
                    destination: temp.path("destination").to_string_lossy().into_owned(),
                }),
                session,
                explorer,
                root,
                None,
                &[],
                &mut requests,
            )
            .unwrap();
        requests
            .into_iter()
            .find_map(|request| match request {
                HostAction::CommitDocumentsWithResult { token, .. } => Some(token),
                _ => None,
            })
            .expect("move must wait for document preparation")
    }

    #[test]
    fn rejected_draft_prevents_file_mutation_and_leaves_the_buffer_attached() {
        let (temp, mut session, mut explorer, root_directory, document) = fixture();
        let mut operations = local_operations(&mut session, &root_directory);
        let token = request_move(
            &mut operations,
            &mut session,
            &mut explorer,
            &root_directory,
            &temp,
        );
        assert!(operations.active.is_none());
        assert!(temp.path("source.txt").exists());
        operations.reply(
            &DocumentReply::Save {
                recipient: MODULE_ID.into(),
                token,
                result: Err("CSV draft is invalid".into()),
            },
            &mut session,
            &explorer,
        );
        assert!(!operations.busy());
        assert!(temp.path("source.txt").exists());
        assert!(!temp.path("destination/source.txt").exists());
        assert_eq!(
            session
                .with_document(document, |state| state.path.clone())
                .unwrap(),
            temp.path("source.txt")
                .canonicalize()
                .unwrap()
                .to_string_lossy()
        );
        assert_eq!(operations.summary.unwrap().errors, ["CSV draft is invalid"]);
    }

    #[test]
    fn move_rebinds_dirty_buffer_without_writing_its_unsaved_bytes() {
        let (temp, mut session, mut explorer, root_directory, document) = fixture();
        let revision = session.document_revision(document).unwrap();
        session
            .apply_edits(
                document,
                revision,
                &[ByteEdit {
                    range: 0..7,
                    bytes: b"unsaved".to_vec(),
                }],
            )
            .unwrap();
        let revision = session.document_revision(document).unwrap();
        let mut operations = local_operations(&mut session, &root_directory);
        let token = request_move(
            &mut operations,
            &mut session,
            &mut explorer,
            &root_directory,
            &temp,
        );
        operations.reply(
            &DocumentReply::Save {
                recipient: MODULE_ID.into(),
                token,
                result: Ok(vec![SavedDocument { document, revision }]),
            },
            &mut session,
            &explorer,
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while operations.busy() && Instant::now() < deadline {
            operations.poll(&mut session, &mut explorer, None, &[], &mut Vec::new());
            thread::sleep(Duration::from_millis(2));
        }
        assert!(!operations.busy());
        assert!(
            operations.summary.as_ref().unwrap().errors.is_empty(),
            "{:?}",
            operations.summary.as_ref().unwrap().errors
        );
        assert!(!temp.path("source.txt").exists());
        assert_eq!(
            fs::read(temp.path("destination/source.txt")).unwrap(),
            b"on disk"
        );
        assert_eq!(session.snapshot(document).unwrap().bytes, b"unsaved");
        assert!(
            session
                .with_document(document, |state| state.dirty)
                .unwrap()
        );
        assert_eq!(
            session
                .with_document(document, |state| state.path.clone())
                .unwrap(),
            temp.path("destination/source.txt")
                .canonicalize()
                .unwrap()
                .to_string_lossy()
        );
    }

    #[test]
    fn stale_reply_and_workspace_replacement_cannot_start_an_old_file_job() {
        let (temp, mut session, mut explorer, root_directory, document) = fixture();
        let revision = session.document_revision(document).unwrap();
        let mut operations = local_operations(&mut session, &root_directory);
        let token = request_move(
            &mut operations,
            &mut session,
            &mut explorer,
            &root_directory,
            &temp,
        );
        operations.reply(
            &DocumentReply::Save {
                recipient: MODULE_ID.into(),
                token: SaveToken::next(),
                result: Ok(vec![]),
            },
            &mut session,
            &explorer,
        );
        assert!(operations.pending.is_some());
        let mut replacement = DocumentSession::new();
        operations.reply(
            &DocumentReply::Save {
                recipient: MODULE_ID.into(),
                token,
                result: Ok(vec![SavedDocument { document, revision }]),
            },
            &mut replacement,
            &explorer,
        );
        assert!(!operations.busy());
        assert!(temp.path("source.txt").exists());
        assert!(!temp.path("destination/source.txt").exists());
    }

    #[test]
    fn changing_target_cancels_preflight_cut_and_dialogs_before_rebinding() {
        let (temp, mut session, mut explorer, root_directory, document) = fixture();
        let mut operations = local_operations(&mut session, &root_directory);
        let token = request_move(
            &mut operations,
            &mut session,
            &mut explorer,
            &root_directory,
            &temp,
        );
        operations.cut = Some((
            operations.target_epoch,
            vec![temp.path("source.txt").to_string_lossy().into_owned()],
        ));
        operations.dialog = Some(NameDialog {
            action: FileTreeAction::Rename("source.txt".into()),
            name: "new.txt".into(),
            appearing: true,
            error: None,
        });
        let replacement = ExplorerTarget {
            root: temp.path("destination").to_string_lossy().into_owned(),
            services: bed_files::FileServices::local(),
        };
        assert!(operations.bind_target(&replacement, &mut session));
        assert!(!operations.busy());
        assert!(operations.cut.is_none() && operations.dialog.is_none());
        operations.reply(
            &DocumentReply::Save {
                recipient: MODULE_ID.into(),
                token,
                result: Ok(vec![SavedDocument {
                    document,
                    revision: session.document_revision(document).unwrap(),
                }]),
            },
            &mut session,
            &explorer,
        );
        assert!(!operations.busy());
        assert!(temp.path("source.txt").exists());
        let error = operations
            .action(
                ExplorerAction::Tree(FileTreeAction::Refresh),
                &mut session,
                &mut explorer,
                &root_directory,
                None,
                &[],
                &mut Vec::new(),
            )
            .unwrap_err();
        assert!(error.to_string().contains("target changed"));
    }

    #[cfg(unix)]
    #[test]
    fn replacing_only_the_remote_connection_cancels_queued_work_and_cut() {
        let connected = crate::test_remote_client;
        let (temp, mut session, mut explorer, root_directory, _) = fixture();
        let target = ExplorerTarget {
            root: root_directory.clone(),
            services: bed_files::FileServices::remote("explorer-fixture", connected()),
        };
        let mut operations = FileOperations::default();
        operations.bind_target(&target, &mut session);
        let mut requests = Vec::new();
        for action in [
            FileTreeAction::Move {
                paths: vec![temp.path("source.txt").to_string_lossy().into_owned()],
                destination: temp.path("destination").to_string_lossy().into_owned(),
            },
            FileTreeAction::Cut(vec!["captured.txt".into()]),
        ] {
            operations
                .action(
                    ExplorerAction::Tree(action),
                    &mut session,
                    &mut explorer,
                    &root_directory,
                    None,
                    &[],
                    &mut requests,
                )
                .unwrap();
        }
        assert!(operations.pending.is_some() && operations.cut.is_some());
        assert!(
            requests
                .iter()
                .all(|request| !matches!(request, HostAction::CommitDocumentsWithResult { .. }))
        );
        assert!(!operations.bind_target(&target.clone(), &mut session));
        assert!(operations.pending.is_some());
        assert!(operations.bind_target(
            &ExplorerTarget {
                root: root_directory,
                services: bed_files::FileServices::remote("explorer-fixture", connected())
            },
            &mut session
        ));
        assert!(!operations.busy() && operations.cut.is_none());
        operations.poll(&mut session, &mut explorer, None, &[], &mut Vec::new());
        assert!(temp.path("source.txt").exists());
    }

    fn wait_for_replacement(
        operations: &mut FileOperations,
        session: &mut DocumentSession,
        explorer: &mut ExplorerState,
    ) -> SaveToken {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let mut requests = Vec::new();
            operations.poll(session, explorer, None, &[], &mut requests);
            if let Some(conflict) = operations.conflict.take() {
                conflict
                    .reply
                    .send(ConflictDecision {
                        choice: ConflictChoice::Replace,
                        apply_to_all: false,
                    })
                    .unwrap();
            }
            if let Some(token) = requests.into_iter().find_map(|request| match request {
                HostAction::CommitDocumentsWithResult { token, .. } => Some(token),
                _ => None,
            }) {
                return token;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("replacement did not reach document preparation");
    }

    #[test]
    fn overwrite_waits_for_destination_draft_and_rejection_preserves_both_files() {
        let (temp, mut session, mut explorer, root_directory, source) = fixture();
        let destination = temp.write("destination/source.txt", b"destination disk");
        let document = session.open_file(&destination).unwrap();
        let revision = session.document_revision(document).unwrap();
        session
            .apply_edits(
                document,
                revision,
                &[ByteEdit {
                    range: 0..16,
                    bytes: b"destination draft".to_vec(),
                }],
            )
            .unwrap();
        let mut operations = local_operations(&mut session, &root_directory);
        let token = request_move(
            &mut operations,
            &mut session,
            &mut explorer,
            &root_directory,
            &temp,
        );
        let revision = session.document_revision(source).unwrap();
        operations.reply(
            &DocumentReply::Save {
                recipient: MODULE_ID.into(),
                token,
                result: Ok(vec![SavedDocument {
                    document: source,
                    revision,
                }]),
            },
            &mut session,
            &explorer,
        );
        let replacement = wait_for_replacement(&mut operations, &mut session, &mut explorer);
        assert_eq!(operations.pending.as_ref().unwrap().documents, [document]);
        assert_eq!(fs::read(&destination).unwrap(), b"destination disk");
        operations.reply(
            &DocumentReply::Save {
                recipient: MODULE_ID.into(),
                token: replacement,
                result: Err("destination CSV draft is invalid".into()),
            },
            &mut session,
            &explorer,
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while operations.busy() && Instant::now() < deadline {
            operations.poll(&mut session, &mut explorer, None, &[], &mut Vec::new());
            thread::sleep(Duration::from_millis(2));
        }
        assert!(!operations.busy());
        assert_eq!(fs::read(&destination).unwrap(), b"destination disk");
        assert_eq!(fs::read(temp.path("source.txt")).unwrap(), b"on disk");
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            b"destination draft"
        );
        assert!(
            session
                .with_document(document, |state| state.dirty)
                .unwrap()
        );
    }

    #[test]
    fn canceled_replacement_acknowledgement_cannot_invalidate_destination() {
        let (temp, mut session, mut explorer, root_directory, source) = fixture();
        let destination = temp.write("destination/source.txt", b"destination disk");
        let document = session.open_file(&destination).unwrap();
        let mut operations = local_operations(&mut session, &root_directory);
        let token = request_move(
            &mut operations,
            &mut session,
            &mut explorer,
            &root_directory,
            &temp,
        );
        let revision = session.document_revision(source).unwrap();
        operations.reply(
            &DocumentReply::Save {
                recipient: MODULE_ID.into(),
                token,
                result: Ok(vec![SavedDocument {
                    document: source,
                    revision,
                }]),
            },
            &mut session,
            &explorer,
        );
        let replacement = wait_for_replacement(&mut operations, &mut session, &mut explorer);
        operations.active.as_ref().unwrap().job.cancel();
        let revision = session.document_revision(document).unwrap();
        operations.reply(
            &DocumentReply::Save {
                recipient: MODULE_ID.into(),
                token: replacement,
                result: Ok(vec![SavedDocument { document, revision }]),
            },
            &mut session,
            &explorer,
        );
        assert_eq!(fs::read(&destination).unwrap(), b"destination disk");
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            b"destination disk"
        );
        assert!(session.take_events().iter().all(|event|!matches!(event,bed_documents::SessionEvent::Removed{document:removed,..} if *removed==document)));
        operations.cancel(&mut session);
    }

    #[test]
    fn source_revision_changed_after_preflight_acknowledgement_blocks_job() {
        let (temp, mut session, mut explorer, root_directory, document) = fixture();
        let mut operations = local_operations(&mut session, &root_directory);
        let token = request_move(
            &mut operations,
            &mut session,
            &mut explorer,
            &root_directory,
            &temp,
        );
        let revision = session.document_revision(document).unwrap();
        session
            .apply_edits(
                document,
                revision,
                &[ByteEdit {
                    range: 0..0,
                    bytes: b"edit ".to_vec(),
                }],
            )
            .unwrap();
        operations.reply(
            &DocumentReply::Save {
                recipient: MODULE_ID.into(),
                token,
                result: Ok(vec![SavedDocument { document, revision }]),
            },
            &mut session,
            &explorer,
        );
        assert!(!operations.busy());
        assert_eq!(fs::read(temp.path("source.txt")).unwrap(), b"on disk");
        assert!(!temp.path("destination/source.txt").exists());
    }

    #[derive(Default)]
    struct Clipboard {
        paths: Vec<PathBuf>,
        reads: usize,
        writes: usize,
    }

    impl FileClipboardService for Clipboard {
        fn read_files(&mut self) -> io::Result<Vec<PathBuf>> {
            self.reads += 1;
            Ok(self.paths.clone())
        }

        fn write_files(&mut self, paths: &[PathBuf]) -> io::Result<()> {
            self.writes += 1;
            self.paths = paths.to_vec();
            Ok(())
        }
    }

    fn finish_clipboard(
        operations: &mut FileOperations,
        session: &mut DocumentSession,
        explorer: &mut ExplorerState,
        clipboard: &mut Clipboard,
        choice: ConflictChoice,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while operations.busy() {
            if let Some(pending) = &operations.pending {
                let token = pending.token;
                let documents = pending
                    .documents
                    .iter()
                    .map(|document| SavedDocument {
                        document: *document,
                        revision: session.document_revision(*document).unwrap(),
                    })
                    .collect();
                operations.reply(
                    &DocumentReply::Save {
                        recipient: MODULE_ID.into(),
                        token,
                        result: Ok(documents),
                    },
                    session,
                    explorer,
                );
            }
            operations.poll(session, explorer, Some(clipboard), &[], &mut Vec::new());
            if let Some(conflict) = operations.conflict.take() {
                conflict
                    .reply
                    .send(ConflictDecision {
                        choice,
                        apply_to_all: false,
                    })
                    .unwrap();
            }
            if operations
                .active
                .as_ref()
                .is_some_and(|active| active.export)
            {
                assert_eq!(
                    clipboard.writes, 0,
                    "partial exports cannot replace the OS clipboard"
                );
            }
            assert!(Instant::now() < deadline, "clipboard operation stalled");
            thread::sleep(Duration::from_millis(2));
        }
        assert!(
            operations.summary.as_ref().unwrap().errors.is_empty(),
            "{:?}",
            operations.summary.as_ref().unwrap().errors
        );
    }

    #[test]
    fn os_clipboard_paste_copies_and_internal_cut_keeps_dirty_view_identity() {
        let (temp, mut session, mut explorer, root_directory, document) = fixture();
        let upload = temp.write("upload.bin", &[0, 255, b'\r', b'\n']);
        let mut clipboard = Clipboard {
            paths: vec![upload.clone()],
            ..Default::default()
        };
        let mut operations = local_operations(&mut session, &root_directory);
        operations
            .action(
                ExplorerAction::Tree(FileTreeAction::Paste(
                    temp.path("destination").to_string_lossy().into_owned(),
                )),
                &mut session,
                &mut explorer,
                &root_directory,
                Some(&mut clipboard),
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        finish_clipboard(
            &mut operations,
            &mut session,
            &mut explorer,
            &mut clipboard,
            ConflictChoice::KeepBoth,
        );
        assert_eq!(
            fs::read(temp.path("destination/upload.bin")).unwrap(),
            [0, 255, b'\r', b'\n']
        );
        assert!(upload.exists());
        assert_eq!(clipboard.reads, 1);
        assert_eq!(clipboard.writes, 0);
        let view = session.create_view(document).unwrap();
        session
            .with_commands(view, |commands| commands.type_text(b"dirty "))
            .unwrap();
        operations
            .action(
                ExplorerAction::Tree(FileTreeAction::Cut(vec![
                    temp.path("source.txt").to_string_lossy().into_owned(),
                ])),
                &mut session,
                &mut explorer,
                &root_directory,
                Some(&mut clipboard),
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        operations
            .action(
                ExplorerAction::Tree(FileTreeAction::Paste(
                    temp.path("destination").to_string_lossy().into_owned(),
                )),
                &mut session,
                &mut explorer,
                &root_directory,
                Some(&mut clipboard),
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        finish_clipboard(
            &mut operations,
            &mut session,
            &mut explorer,
            &mut clipboard,
            ConflictChoice::KeepBoth,
        );
        assert_eq!(
            session.document_for_path(&temp.path("destination/source.txt")),
            Some(document)
        );
        assert_eq!(session.document_for_view(view), Some(document));
        assert_eq!(session.snapshot(document).unwrap().bytes, b"dirty on disk");
        assert_eq!(
            fs::read(temp.path("destination/source.txt")).unwrap(),
            b"on disk"
        );
        assert_eq!(
            clipboard.reads, 1,
            "internal cut bypasses OS clipboard reads"
        );
        session
            .with_commands(view, |commands| commands.undo())
            .unwrap();
        assert_eq!(session.snapshot(document).unwrap().bytes, b"on disk");
    }

    #[test]
    fn clipboard_keep_both_preserves_source_destination_and_previous_clipboard() {
        let (temp, mut session, mut explorer, root_directory, _) = fixture();
        let upload = temp.write("outside/source.txt", b"imported");
        let original = temp.write("destination/source.txt", b"original");
        let mut clipboard = Clipboard {
            paths: vec![upload.clone()],
            ..Default::default()
        };
        let mut operations = local_operations(&mut session, &root_directory);
        operations
            .action(
                ExplorerAction::Tree(FileTreeAction::Paste(
                    temp.path("destination").to_string_lossy().into_owned(),
                )),
                &mut session,
                &mut explorer,
                &root_directory,
                Some(&mut clipboard),
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        finish_clipboard(
            &mut operations,
            &mut session,
            &mut explorer,
            &mut clipboard,
            ConflictChoice::KeepBoth,
        );
        assert_eq!(fs::read(original).unwrap(), b"original");
        assert_eq!(fs::read(&upload).unwrap(), b"imported");
        assert_eq!(
            fs::read(temp.path("destination/source copy.txt")).unwrap(),
            b"imported"
        );
        assert_eq!(clipboard.paths, [upload]);
        assert_eq!(clipboard.writes, 0);
    }

    #[cfg(unix)]
    #[test]
    fn remote_clipboard_exports_publish_complete_binary_files_and_paste_and_cut_preserve_identity()
    {
        let temp = crate::test_support::TempDir::new();
        let bytes = [0, 255, 0xef, 0xbb, 0xbf, b'\r', b'\n'].repeat(170_000);
        let source = temp
            .write("remote/source.bin", &bytes)
            .canonicalize()
            .unwrap();
        let text = temp
            .write("remote/source.txt", b"disk")
            .canonicalize()
            .unwrap();
        let upload = temp
            .write("local/upload.bin", &bytes)
            .canonicalize()
            .unwrap();
        let previous = temp.write("local/previous.txt", b"previous");
        fs::create_dir(temp.path("remote/destination")).unwrap();
        let root = temp.path("remote").canonicalize().unwrap();
        let client = crate::test_remote_client();
        let mut session = DocumentSession::new();
        let root_directory = root.to_string_lossy().into_owned();
        let mut explorer = ExplorerState::default();
        explorer.file_explorer.project_root = root_directory.clone();
        let mut clipboard = Clipboard {
            paths: vec![previous.clone()],
            ..Default::default()
        };
        let mut operations = local_operations(&mut session, &root_directory);
        let target = ExplorerTarget {
            root: root_directory.clone(),
            services: bed_files::FileServices::remote("explorer-fixture", client.clone()),
        };
        operations.bind_target(&target, &mut session);
        assert!(
            !session.is_remote(),
            "filesystem transport is independent of documents"
        );
        for action in [
            FileTreeAction::Open(text.to_string_lossy().into_owned()),
            FileTreeAction::Preview(text.to_string_lossy().into_owned()),
            FileTreeAction::OpenWith {
                path: text.to_string_lossy().into_owned(),
                viewer: "text".into(),
            },
            FileTreeAction::OpenMany(vec![text.to_string_lossy().into_owned()]),
            FileTreeAction::OpenManyFromMenu(vec![text.to_string_lossy().into_owned()]),
        ] {
            let mut requests = Vec::new();
            let error = operations
                .action(
                    ExplorerAction::Tree(action),
                    &mut session,
                    &mut explorer,
                    &root_directory,
                    None,
                    &[],
                    &mut requests,
                )
                .unwrap_err();
            assert!(error.to_string().contains("document access"));
            assert!(requests.is_empty());
        }
        let mut finder_requests = vec![HostAction::OpenFile {
            path: text.to_string_lossy().into_owned(),
            viewer: None,
        }];
        guard_document_requests(&target, &session, &mut finder_requests);
        assert!(
            matches!(&finder_requests[..], [HostAction::Notify { message }] if message.contains("document access"))
        );
        let mut requests = Vec::new();
        operations
            .action(
                ExplorerAction::Tree(FileTreeAction::Copy(vec![
                    source.to_string_lossy().into_owned(),
                ])),
                &mut session,
                &mut explorer,
                &root_directory,
                Some(&mut clipboard),
                &[],
                &mut requests,
            )
            .unwrap();
        assert!(
            requests
                .iter()
                .all(|request| !matches!(request, HostAction::CommitDocumentsWithResult { .. }))
        );
        assert_eq!(clipboard.paths, [previous]);
        assert_eq!(clipboard.writes, 0);
        finish_clipboard(
            &mut operations,
            &mut session,
            &mut explorer,
            &mut clipboard,
            ConflictChoice::KeepBoth,
        );
        let staged = clipboard.paths[0].clone();
        assert_ne!(staged, source);
        assert_eq!(fs::read(&staged).unwrap(), bytes);
        assert_eq!(clipboard.writes, 1);
        assert!(source.exists());
        clipboard.paths = vec![upload.clone()];
        let destination = root.join("destination");
        operations
            .action(
                ExplorerAction::Tree(FileTreeAction::Paste(
                    destination.to_string_lossy().into_owned(),
                )),
                &mut session,
                &mut explorer,
                &root_directory,
                Some(&mut clipboard),
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        finish_clipboard(
            &mut operations,
            &mut session,
            &mut explorer,
            &mut clipboard,
            ConflictChoice::KeepBoth,
        );
        assert_eq!(fs::read(destination.join("upload.bin")).unwrap(), bytes);
        assert_eq!(fs::read(upload).unwrap(), bytes);
        assert_eq!(clipboard.writes, 1);
        // A same-named local document is unrelated to the remote filesystem.
        let unrelated = temp
            .write("remote/unrelated.txt", b"local disk")
            .canonicalize()
            .unwrap();
        let local_document = session.open_file(&unrelated).unwrap();
        let revision = session.document_revision(local_document).unwrap();
        session
            .apply_edits(
                local_document,
                revision,
                &[ByteEdit {
                    range: 0..0,
                    bytes: b"local draft ".to_vec(),
                }],
            )
            .unwrap();
        let mut requests = Vec::new();
        for action in [
            FileTreeAction::Cut(vec![unrelated.to_string_lossy().into_owned()]),
            FileTreeAction::Paste(destination.to_string_lossy().into_owned()),
        ] {
            operations
                .action(
                    ExplorerAction::Tree(action),
                    &mut session,
                    &mut explorer,
                    &root_directory,
                    Some(&mut clipboard),
                    &[],
                    &mut requests,
                )
                .unwrap();
        }
        assert!(
            requests
                .iter()
                .all(|request| !matches!(request, HostAction::CommitDocumentsWithResult { .. }))
        );
        finish_clipboard(
            &mut operations,
            &mut session,
            &mut explorer,
            &mut clipboard,
            ConflictChoice::KeepBoth,
        );
        assert_eq!(session.document_for_path(&unrelated), Some(local_document));
        assert_eq!(
            session.snapshot(local_document).unwrap().bytes,
            b"local draft local disk"
        );
        // The same provider's documents still receive preflight and identity updates.
        session = DocumentSession::with_remote_options(
            bed_documents::SessionOptions {
                autosave: None,
                ..Default::default()
            },
            bed_remote::SshTarget::new("explorer-fixture"),
            client,
            root_directory.clone(),
        )
        .unwrap();
        session.request_open_file(&text).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while session.document_for_path(&text).is_none() {
            session.tick();
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        }
        let document = session.document_for_path(&text).unwrap();
        let view = session.create_view(document).unwrap();
        session
            .with_commands(view, |commands| commands.type_text(b"dirty "))
            .unwrap();
        let reads = clipboard.reads;
        operations
            .action(
                ExplorerAction::Tree(FileTreeAction::Cut(vec![
                    text.to_string_lossy().into_owned(),
                ])),
                &mut session,
                &mut explorer,
                &root_directory,
                Some(&mut clipboard),
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        operations
            .action(
                ExplorerAction::Tree(FileTreeAction::Paste(
                    destination.to_string_lossy().into_owned(),
                )),
                &mut session,
                &mut explorer,
                &root_directory,
                Some(&mut clipboard),
                &[],
                &mut Vec::new(),
            )
            .unwrap();
        finish_clipboard(
            &mut operations,
            &mut session,
            &mut explorer,
            &mut clipboard,
            ConflictChoice::KeepBoth,
        );
        assert_eq!(session.document_for_view(view), Some(document));
        assert_eq!(
            session.document_for_path(&destination.join("source.txt")),
            Some(document)
        );
        assert_eq!(session.snapshot(document).unwrap().bytes, b"dirty disk");
        assert_eq!(fs::read(destination.join("source.txt")).unwrap(), b"disk");
        assert!(!text.exists());
        assert_eq!(clipboard.reads, reads);
        assert_eq!(clipboard.writes, 1);
        session
            .with_commands(view, |commands| commands.undo())
            .unwrap();
        assert_eq!(session.snapshot(document).unwrap().bytes, b"disk");
        operations.cancel(&mut session);
        drop(explorer);
        drop(session);
        assert_eq!(
            fs::read(&staged).unwrap(),
            bytes,
            "OS clipboard exports outlive the editor workspace"
        );
        fs::remove_dir_all(staged.parent().unwrap()).unwrap();
    }

    #[test]
    fn actual_plugin_drains_tree_open_and_preferences_without_a_host_module() {
        let mut app = App::new();
        app.add_plugins(super::super::FileExplorerPlugin);
        app.update(); // Bind the initial live directory before admitting panel actions.
        let state = &mut *app.world_mut().non_send_mut::<ExplorerState>();
        state
            .actions
            .push(ExplorerAction::Tree(FileTreeAction::Open(
                "/captured/file.txt".into(),
            )));
        state
            .actions
            .push(ExplorerAction::PreferencesChanged(Default::default()));
        state
            .actions
            .push(ExplorerAction::Tree(FileTreeAction::Preview(
                "/captured/preview.txt".into(),
            )));
        app.update();
        let intents = app
            .world_mut()
            .resource_mut::<bed_api::features::HostEffects>()
            .take();
        assert!(intents.iter().any(|intent| matches!(&intent.action, HostAction::OpenFileIn { path, target: bed_api::workbench::PanelTarget::LargestArea, .. } if path == "/captured/file.txt")));
        assert!(intents.iter().any(|intent| matches!(&intent.action, HostAction::PreviewFileIn { path, target: bed_api::workbench::PanelTarget::LargestArea, .. } if path == "/captured/preview.txt")));
        let changes = app
            .world()
            .resource::<Messages<bed_api::storage::FeatureStateChanged>>();
        assert!(
            changes
                .get_cursor()
                .read(changes)
                .any(|change| change.owner == crate::MODULE_ID)
        );
        assert!(app.world().contains_non_send::<DocumentSession>());
        assert!(app.world().contains_non_send::<FileOperations>());
    }
}
