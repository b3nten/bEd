//! Bed session components. Attribution: workspace LICENSE and NOTICE.
pub mod document_runtime;
#[cfg(feature = "ide-services")]
pub mod project_diagnostics;
#[cfg(feature = "ide-services")]
pub use project_diagnostics::{CheckFormat, ProjectCheckConfig, ProjectDiagnosticsSnapshot};
pub mod document_api;
#[cfg(feature = "ide-services")]
pub mod git;
pub mod save_service;
pub mod session;
pub mod text_view_context;

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
pub(crate) mod test_support;

#[cfg(feature = "ide-services")]
pub use session::LspConfigMode;
pub use session::{
    ByteEdit, ClosePolicy, DocumentId, DocumentKind, DocumentSession, DocumentSnapshot,
    ServiceError, SessionEvent, SessionId, SessionOptions, TickReport, ViewId,
};
pub use text_view_context::TextViewContext;
