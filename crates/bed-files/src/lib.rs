//! Bed files components. Attribution: workspace LICENSE and NOTICE.
pub mod actions;
pub mod context;
#[cfg(feature = "file-services")]
pub use bed_remote::FileBaseline;
pub use context::{FileAvailability, FileContext, FileLocation, FileProviderId, FileServices};
#[cfg(feature = "workspace-services")]
pub mod content_search;
#[cfg(feature = "workspace-services")]
pub mod file_finder;
pub mod file_monitor;
pub mod files;
#[cfg(feature = "workspace-services")]
pub mod operations;
#[cfg(feature = "workspace-services")]
pub mod search_files;
#[cfg(feature = "workspace-services")]
pub mod tree_ignore;

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../tests/support/temp_dir.rs"]
pub(crate) mod test_support;
