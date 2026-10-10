//! Await panel draft validation and acknowledged saves before service mutations.
use super::*;
use bed_workbench_api::{SaveToken, SavedDocument};

pub(super) struct PendingSave {
    recipient: String,
    token: SaveToken,
    documents: Vec<SavedDocument>,
    paths: Vec<String>,
}

impl Workbench {
    pub(super) fn module_save_result(
        &mut self,
        recipient: &str,
        token: SaveToken,
        result: Result<Vec<SavedDocument>, String>,
    ) {
        if let Some(module) = self
            .modules
            .instances
            .iter_mut()
            .find(|module| module.id() == recipient)
        {
            module.save_result(token, result);
        }
    }

    pub(super) fn begin_module_save(
        &mut self,
        recipient: String,
        token: SaveToken,
        mut documents: Vec<DocumentId>,
    ) {
        documents.sort_by_key(|document| document.0);
        documents.dedup();
        let result = (|| -> io::Result<(Vec<SavedDocument>, Vec<String>)> {
            if !self
                .modules
                .instances
                .iter()
                .any(|module| module.id() == recipient)
            {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "Save recipient is unavailable",
                ));
            }
            // Flush all drafts before any disk writes: a bad table draft must
            // never allow its dependent Git operation to proceed.
            for &document in &documents {
                self.ensure_file_operation_idle(document)?;
                if self.remote_ui.document_mutating(document) {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "Wait for the remote file action before saving",
                    ));
                }
                if self.session.is_snapshot_document(document) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "Comparison documents cannot be saved",
                    ));
                }
                self.commit_plugin_edits(document)?;
            }
            let mut saved = Vec::new();
            let mut paths = Vec::new();
            for &document in &documents {
                let snapshot = self.session.snapshot(document)?;
                if snapshot.path.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Save the document to a file first",
                    ));
                }
                if snapshot.disk_conflict.is_some() {
                    return Err(io::Error::other("Resolve the disk conflict before saving"));
                }
                saved.push(SavedDocument {
                    document,
                    revision: self.session.document_revision(document)?,
                });
                paths.push(snapshot.path);
            }
            for &document in &documents {
                self.session.save(document)?;
            }
            Ok((saved, paths))
        })();
        match result {
            Ok((documents, paths)) => {
                self.modules.pending_saves.push(PendingSave {
                    recipient,
                    token,
                    documents,
                    paths,
                });
                self.poll_module_saves(&bed_document_session::editor_session::TickReport::default());
            }
            Err(error) => self.module_save_result(&recipient, token, Err(error.to_string())),
        }
    }

    pub(super) fn poll_module_saves(
        &mut self,
        report: &bed_document_session::editor_session::TickReport,
    ) {
        let pending = std::mem::take(&mut self.modules.pending_saves);
        for save in pending {
            let result = (|| -> Result<bool, String> {
                let mut complete = true;
                for (document, path) in save.documents.iter().zip(&save.paths) {
                    if let Some(error) = report.errors.iter().find(|error| {
                        (error.document == Some(document.document) || error.document.is_none())
                            && matches!(error.service, "remote" | "save" | "autosave")
                    }) {
                        return Err(error.message.clone());
                    }
                    if self.session.is_remote() && !self.session.remote_connected() {
                        return Err("SSH disconnected before the save completed".into());
                    }
                    if self.remote_ui.document_mutating(document.document) {
                        return Err("A remote file action interrupted the save; try again after it finishes".into());
                    }
                    if self
                        .session
                        .document_revision(document.document)
                        .map_err(|error| error.to_string())?
                        != document.revision
                    {
                        return Err("Document changed while saving; save and try again".into());
                    }
                    if !self
                        .session
                        .with_document(document.document, |state| &state.path == path)
                        .map_err(|error| error.to_string())?
                    {
                        return Err("Document moved while saving; save and try again".into());
                    }
                    if self.session.save_pending(document.document) {
                        complete = false;
                    } else {
                        let snapshot = self
                            .session
                            .snapshot(document.document)
                            .map_err(|error| error.to_string())?;
                        if snapshot.dirty || snapshot.disk_conflict.is_some() {
                            return Err(
                                "Document was not saved; resolve the file error and try again"
                                    .into(),
                            );
                        }
                    }
                }
                Ok(complete)
            })();
            match result {
                Ok(false) => self.modules.pending_saves.push(save),
                Ok(true) => {
                    self.module_save_result(&save.recipient, save.token, Ok(save.documents))
                }
                Err(error) => self.module_save_result(&save.recipient, save.token, Err(error)),
            }
        }
    }
}
