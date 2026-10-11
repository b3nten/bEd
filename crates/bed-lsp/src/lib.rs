//! Bed lsp components. Attribution: workspace LICENSE and NOTICE.
pub mod connection;
pub mod diagnostics;
mod document_diagnostics;
pub mod jsonrpc;
pub mod lsp_client;
pub mod lsp_config;
pub mod lsp_document_sync;
pub mod lsp_locations;
mod lsp_project;
pub mod lsp_request;
pub mod lsp_uri;
pub mod message_handler;
pub mod process;
mod workspace_diagnostics;
pub mod workspace_lsp;

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
pub(crate) mod test_support;
