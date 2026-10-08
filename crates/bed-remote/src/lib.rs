//! Headless workspace services and a bounded, versioned stdio protocol.
//!
//! Bed's custom documents and GUI remain in the consumer. This crate contains
//! no GUI, GPU, platform window, or editor document dependencies.
mod client;
mod deployment;
mod file_info;
mod filesystem;
mod protocol;

pub use client::{RemoteClient, SshTarget, remote_command, shell_quote};
pub use deployment::{expand_ssh_path, prepare_ssh_target};
pub use filesystem::{LocalBackend, git_ignored_paths, serve, serve_with};
pub use protocol::*;
