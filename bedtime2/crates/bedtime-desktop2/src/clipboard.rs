use std::{io, path::PathBuf};

// TODO: look into https://docs.rs/bevy/latest/bevy/clipboard/index.html

/// Main-thread clipboard service, installed once a native window exists.
/// An unavailable clipboard retains its initialization error; reads and writes
/// return that error rather than substituting empty contents.
pub struct DesktopClipboard(Result<arboard::Clipboard, arboard::Error>);

impl DesktopClipboard {
    pub(crate) fn open() -> Self {
        Self(arboard::Clipboard::new())
    }

    pub fn available(&self) -> bool {
        self.0.is_ok()
    }

    pub fn read_text(&mut self) -> io::Result<String> {
        self.backend()?.get_text().map_err(io::Error::other)
    }

    pub fn write_text(&mut self, text: &str) -> io::Result<()> {
        self.backend()?.set_text(text).map_err(io::Error::other)
    }

    pub fn read_files(&mut self) -> io::Result<Vec<PathBuf>> {
        self.backend()?.get().file_list().map_err(io::Error::other)
    }

    pub fn write_files(&mut self, files: &[PathBuf]) -> io::Result<()> {
        self.backend()?
            .set()
            .file_list(files)
            .map_err(io::Error::other)
    }

    fn backend(&mut self) -> io::Result<&mut arboard::Clipboard> {
        self.0
            .as_mut()
            .map_err(|error| io::Error::other(error.to_string()))
    }
}
