//! Human-facing failure presentation. Diagnostics remain in the owning state;
//! only new failure episodes enter the Workbench dialog queue.
use super::*;
use bedtime_workbench2::{ErrorDialog, request_error_dialog};
use std::io::ErrorKind;

#[derive(Default)]
pub(super) struct ReportedErrors {
    pub action: Option<String>,
    root: Option<String>,
    watcher: Option<String>,
    directories: HashMap<PathBuf, Arc<str>>,
    version: u64,
}

pub(super) fn operation_title(operation: &FileTreeOperation) -> &'static str {
    match operation {
        FileTreeOperation::CreateFile { .. } => "Couldn't create the file",
        FileTreeOperation::CreateDirectory { .. } => "Couldn't create the folder",
        FileTreeOperation::Rename { .. } => "Couldn't rename the item",
        FileTreeOperation::Copy { .. } => "Couldn't copy the items",
        FileTreeOperation::Move { .. } => "Couldn't move the items",
        FileTreeOperation::Duplicate { .. } => "Couldn't duplicate the items",
        FileTreeOperation::Trash { .. } => "Couldn't move the items to Trash",
    }
}

pub(super) fn action_title(action: &Action) -> &'static str {
    match action {
        Action::Open { .. } => "Couldn't open the item",
        Action::Copy { .. } => "Couldn't copy the items",
        Action::Cut { .. } => "Couldn't prepare the items to move",
        Action::Paste { .. } => "Couldn't paste the items",
        Action::NewFile { .. } => "Couldn't create the file",
        Action::NewFolder { .. } => "Couldn't create the folder",
        Action::Rename { .. } | Action::SubmitEdit => "Couldn't update the filename",
        Action::Duplicate { .. } => "Couldn't duplicate the items",
        Action::Trash { .. } => "Couldn't move the items to Trash",
        Action::Transfer { .. } => "Couldn't transfer the items",
        _ => "Couldn't update the file tree",
    }
}

pub(super) fn action_dialog(title: &str, details: String) -> ErrorDialog {
    ErrorDialog {
        title: title.into(),
        message: "The requested action couldn't be completed. Check that the items and destination are still available, then try again.".into(),
        details: Some(details),
    }
}

pub(super) fn operation_dialog(
    title: &str,
    result: &FileTreeOperationResult,
) -> Option<ErrorDialog> {
    let error = result.errors.first()?;
    let message = match error.kind {
        ErrorKind::AlreadyExists => {
            "An item with that name already exists in the destination. Choose a different name or folder."
        }
        ErrorKind::PermissionDenied => {
            "You don't have permission to change one or more of these items. Check the folder's permissions and try again."
        }
        ErrorKind::ReadOnlyFilesystem => {
            "This location is read-only. Choose a writable destination."
        }
        ErrorKind::NotFound | ErrorKind::NotADirectory => {
            "A file or folder is no longer available. Refresh the tree and try again."
        }
        ErrorKind::BrokenPipe => {
            "We couldn't confirm the result of the operation. Some files may already have changed. Check the source and destination before trying again."
        }
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => {
            "The destination doesn't have enough available space. Free some space and try again."
        }
        ErrorKind::InvalidInput | ErrorKind::InvalidFilename => {
            "Check the file or folder name and destination, then try again."
        }
        ErrorKind::Unsupported | ErrorKind::CrossesDevices => {
            "This operation isn't supported for these files or locations."
        }
        ErrorKind::DirectoryNotEmpty => {
            "That folder isn't empty, so the operation couldn't be completed."
        }
        _ => {
            "We couldn't finish the operation. Check the source and destination before trying again."
        }
    };
    let message = if result.completed.is_empty() {
        message.into()
    } else {
        format!(
            "{message}\n\n{} item(s) were changed before the operation stopped.",
            result.completed.len()
        )
    };
    let mut details = result
        .errors
        .iter()
        .take(8)
        .map(|error| format!("{}: {}", error.path.display(), error.message))
        .collect::<Vec<_>>()
        .join("\n");
    if result.errors.len() > 8 {
        details.push_str(&format!("\n… and {} more errors.", result.errors.len() - 8));
    }
    Some(ErrorDialog {
        title: if error.kind == ErrorKind::BrokenPipe {
            "Couldn't confirm the file operation".into()
        } else {
            title.into()
        },
        message,
        details: Some(details),
    })
}

pub(super) fn report(world: &mut World) {
    let mut dialogs = Vec::new();
    for mut state in world.query::<&mut FileTreePanel>().iter_mut(world) {
        if state.error != state.reported.action {
            state.reported.action = state.error.clone();
            if let Some(details) = &state.error {
                dialogs.push(ErrorDialog {
                    title: "Couldn't update the file tree".into(),
                    message: "The file tree couldn't finish the requested action. Try again, or restart the application if the problem continues.".into(),
                    details: Some(details.clone()),
                });
            }
        }
        if state.snapshot.error != state.reported.root {
            state.reported.root = state.snapshot.error.clone();
            // An operation failure already describes a concurrently lost root.
            if state.error.is_none()
                && let Some(details) = &state.snapshot.error
            {
                dialogs.push(ErrorDialog {
                    title: "Couldn't display the folder".into(),
                    message: format!("We couldn't display ‘{}’. Make sure the folder is available and that you can access it, then use Refresh to try again. If many folders are expanded, try collapsing some of them.", state.snapshot.root.file_name().unwrap_or(state.snapshot.root.as_os_str()).to_string_lossy()),
                    details: Some(format!("{}\n{details}", state.snapshot.root.display())),
                });
            }
        }
        if state.snapshot.watcher_error != state.reported.watcher {
            state.reported.watcher = state.snapshot.watcher_error.clone();
            if let Some(details) = &state.snapshot.watcher_error {
                dialogs.push(ErrorDialog {
                    title: "Live folder updates are unavailable".into(),
                    message: "We couldn't watch this folder for changes. The file tree will check for updates periodically instead.".into(),
                    details: Some(details.clone()),
                });
            }
        }
        if state.snapshot.version != state.reported.version {
            state.reported.version = state.snapshot.version;
            let previous = std::mem::take(&mut state.reported.directories);
            // This bounded list is built by the worker, not by scanning all rows
            // on the main thread. Resolved failures leave the acknowledgement set.
            for (path, message) in state.snapshot.directory_errors.iter() {
                if previous.get(path) != Some(message) {
                    dialogs.push(ErrorDialog {
                        title: "Couldn't read a folder".into(),
                        message: format!("We couldn't read ‘{}’. Check that the folder is available and that you can access it, then use Refresh to try again.", path.file_name().unwrap_or(path.as_os_str()).to_string_lossy()),
                        details: Some(format!("{}\n{message}", path.display())),
                    });
                }
            }
            state.reported.directories = state.snapshot.directory_errors.iter().cloned().collect();
        }
    }
    for dialog in dialogs {
        request_error_dialog(world, dialog);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acknowledged_persistent_failure_does_not_reopen_until_recovery_and_a_new_failure() {
        use crate::tests::{Fixture, finish, host, panel, ready};
        use bedtime_workbench2::ErrorDialogs;
        let fixture = Fixture::new();
        fixture.file("file.txt", b"content");
        let (mut app, workspace) = host();
        let panel = panel(&mut app, workspace, Some(&fixture.0));
        ready(&mut app, panel, &fixture.0);
        app.world_mut()
            .get_mut::<FileTreePanel>(panel)
            .unwrap()
            .error = Some("technical failure (os error 5)".into());
        app.update();
        assert_eq!(app.world().resource::<ErrorDialogs>().len(), 1);
        app.world_mut().remove_resource::<ErrorDialogs>(); // the UI acknowledged the queue
        for _ in 0..5 {
            app.update();
        }
        assert!(!app.world().contains_resource::<ErrorDialogs>());
        app.world_mut()
            .get_mut::<FileTreePanel>(panel)
            .unwrap()
            .error = None;
        app.update();
        app.world_mut()
            .get_mut::<FileTreePanel>(panel)
            .unwrap()
            .error = Some("technical failure (os error 5)".into());
        app.update();
        assert_eq!(app.world().resource::<ErrorDialogs>().len(), 1);
        finish(&mut app);
    }

    #[test]
    fn native_error_codes_stay_in_details_and_partial_effects_are_not_called_a_rollback() {
        let result = FileTreeOperationResult {
            id: 1,
            cancelled: false,
            completed: vec![FileTreeChange::Created {
                path: "created.txt".into(),
            }],
            errors: vec![FileTreeOperationError {
                path: "old.txt".into(),
                kind: ErrorKind::AlreadyExists,
                message: "File exists (os error 17)".into(),
            }],
        };
        let dialog = operation_dialog("Couldn't rename the item", &result).unwrap();
        assert!(dialog.message.contains("already exists"));
        assert!(dialog.message.contains("1 item(s) were changed"));
        assert!(!dialog.message.contains("os error"));
        assert!(dialog.details.unwrap().contains("os error 17"));
        assert!(
            operation_dialog(
                "Cancelled",
                &FileTreeOperationResult {
                    id: 2,
                    completed: Vec::new(),
                    errors: Vec::new(),
                    cancelled: true
                }
            )
            .is_none()
        );
    }
}
