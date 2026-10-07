//! Bed highlight components. Attribution: workspace LICENSE, NOTICE and UPSTREAM_REVISION.
pub mod capture_map;
pub mod highlight_service;
pub mod span_map;
pub mod tree_sitter;

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
pub(crate) mod test_support;
