//! Application explorer/finder composition; document lifecycle belongs to EditorSession.
use super::{file_finder::FileFinder, file_tree::FileTree};
pub struct FileExplorer {
    pub project_root: String,
    pub file_tree: FileTree,
    pub file_finder: FileFinder,
}
impl Default for FileExplorer {
    fn default() -> Self {
        let mut file_finder = FileFinder::new();
        file_finder.start_background_thread();
        Self {
            project_root: String::new(),
            file_tree: FileTree::default(),
            file_finder,
        }
    }
}
impl FileExplorer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn poll(&mut self) {
        self.file_finder.poll();
    }
    pub fn refresh_file_tree(&mut self) {
        if let Err(error) = self.file_tree.refresh_file_tree(&self.project_root) {
            eprintln!("Error accessing directory {}: {error}", self.project_root);
        }
    }
}
