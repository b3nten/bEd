//! Bed files components. Attribution: workspace LICENSE, NOTICE and UPSTREAM_REVISION.
pub mod content_search;
pub mod file_finder;
pub mod file_monitor;
pub mod files;
pub mod search_files;
pub mod tree_ignore;

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
pub(crate) mod test_support;
