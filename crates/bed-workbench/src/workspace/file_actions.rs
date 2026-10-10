//! Desktop filesystem policy around reusable file operations.
pub use bed_files::actions::*;
pub fn move_to_trash(path: &std::path::Path) -> std::io::Result<()> {
    trash::delete(path).map_err(std::io::Error::other)
}
