//! Bed highlight components. Attribution: workspace LICENSE and NOTICE.
pub mod capture_map;
mod grammars;
pub mod highlight_service;
pub mod outline;
pub mod span_map;
pub mod tree_sitter;

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
pub(crate) mod test_support;
