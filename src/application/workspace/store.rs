//! bEd uses the optional workspace plugin's filesystem backend.
pub use bed_plugin_workspaces::filesystem::{
    WorkspaceStateStore, WorkspaceStore, WorkspaceStoreRepository,
};
pub use bed_plugin_workspaces::{WorkspaceSpec, WorkspaceTarget};
