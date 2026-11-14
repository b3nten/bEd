//! Project search and replace. `engine` finds a query's matches in a folder's
//! files (and in open documents' unsaved text); with feature `workbench`,
//! `SearchPanel` is the panel to search and replace from.
mod engine;
pub use engine::*;
#[cfg(feature = "workbench")]
mod workbench;
#[cfg(feature = "workbench")]
pub use workbench::{
    NextResult, PreviousResult, SearchPanel, SearchPlugin, SearchState, SearchSystems,
    SearchWorkspace, seed,
};
