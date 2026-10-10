//! Bed session components. Attribution: workspace LICENSE and NOTICE.
pub mod editor;
pub mod project_diagnostics;
pub use project_diagnostics::{CheckFormat, ProjectCheckConfig, ProjectDiagnosticsSnapshot};
pub mod editor_api;
pub mod editor_session;
pub mod git;
pub mod save_service;
pub mod view_context;

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
pub(crate) mod test_support;

pub use editor_session::{
    ByteEdit, ClosePolicy, DocumentId, DocumentKind, DocumentSnapshot, EditorSession, ServiceError,
    SessionEvent, SessionOptions, TickReport, ViewId, WorkspaceId,
};
pub use view_context::ViewContext;
