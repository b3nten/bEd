//! A clipboard usable by games, UI and workbench independently: text, file
//! lists and images.
//!
//! Adapted from Bevy 0.20's `bevy_clipboard` resource/plugin design, distributed
//! under MIT OR Apache-2.0 (see the accompanying licenses). File lists, images, shared
//! ownership and fallback after native initialization failure are Bedtime additions.
//! `system_clipboard` opts into OS access on desktop targets. Otherwise the same
//! API uses an in-process clipboard. An operation on a working native backend
//! reports its error; it never silently substitutes stale in-process contents.
use bevy_app::{App, Plugin};
use bevy_ecs::resource::Resource;
use std::{
    io,
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
};

/// Installs one shared clipboard. A resource supplied by the application is kept.
pub struct ClipboardPlugin;
impl Plugin for ClipboardPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Clipboard>();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardMode {
    System,
    Memory,
}

enum Contents {
    Empty,
    Text(String),
    Files(Vec<PathBuf>),
    Image(ClipboardImage),
}

/// Pixels on the clipboard: `rgba` holds `width * height` 8-bit RGBA pixels,
/// row-major without padding. Encoding (PNG, …) is left to the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}
enum Backend {
    #[cfg(all(
        feature = "system_clipboard",
        any(unix, windows),
        not(any(target_os = "android", target_os = "ios"))
    ))]
    System(arboard::Clipboard),
    Memory(Contents),
}
struct State {
    backend: Backend,
    initialization_error: Option<String>,
    local_write_generation: u64,
}

/// File contents observed together with the shared handle's local write generation.
/// This is not a native clipboard ownership token: external writes are not counted.
#[derive(Clone)]
pub struct ClipboardFilesSnapshot {
    source: Weak<Mutex<State>>,
    local_write_generation: u64,
    files: Vec<PathBuf>,
}
impl ClipboardFilesSnapshot {
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }
    /// Counts successful writes/clears through this service, not external OS writes.
    pub fn local_write_generation(&self) -> u64 {
        self.local_write_generation
    }
}

/// Cloneable handles share one backend and lock, including the egui adapter.
/// Cloning does not open another OS clipboard or duplicate its contents.
#[derive(Resource, Clone)]
pub struct Clipboard(Arc<Mutex<State>>);

impl Default for Clipboard {
    fn default() -> Self {
        #[cfg(all(
            feature = "system_clipboard",
            any(unix, windows),
            not(any(target_os = "android", target_os = "ios"))
        ))]
        {
            let (backend, initialization_error) = match arboard::Clipboard::new() {
                Ok(clipboard) => (Backend::System(clipboard), None),
                Err(error) => (
                    Backend::Memory(Contents::Text(String::new())),
                    Some(error.to_string()),
                ),
            };
            Self(Arc::new(Mutex::new(State {
                backend,
                initialization_error,
                local_write_generation: 0,
            })))
        }
        #[cfg(not(all(
            feature = "system_clipboard",
            any(unix, windows),
            not(any(target_os = "android", target_os = "ios"))
        )))]
        {
            Self::memory()
        }
    }
}
impl Clipboard {
    /// Explicitly request local copy/paste, even in a build with native support.
    pub fn memory() -> Self {
        Self(Arc::new(Mutex::new(State {
            backend: Backend::Memory(Contents::Text(String::new())),
            initialization_error: None,
            local_write_generation: 0,
        })))
    }
    pub fn mode(&self) -> ClipboardMode {
        match &self.0.lock().expect("Clipboard lock poisoned").backend {
            Backend::Memory(_) => ClipboardMode::Memory,
            #[cfg(all(
                feature = "system_clipboard",
                any(unix, windows),
                not(any(target_os = "android", target_os = "ios"))
            ))]
            Backend::System(_) => ClipboardMode::System,
        }
    }
    /// Explains why OS initialization selected the memory fallback, if attempted.
    pub fn initialization_error(&self) -> Option<String> {
        self.0
            .lock()
            .expect("Clipboard lock poisoned")
            .initialization_error
            .clone()
    }
    /// Revision of successful text/file writes and clears through any shared handle.
    /// External OS clipboard writes do not advance it, so this alone is not proof
    /// of native ownership. Reads do not advance the revision.
    pub fn revision(&self) -> u64 {
        self.0
            .lock()
            .expect("Clipboard lock poisoned")
            .local_write_generation
    }
    pub fn read_text(&mut self) -> io::Result<String> {
        self.read_text_if_present()?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Clipboard contains no text"))
    }
    /// An absent text representation (including a file-only or cleared
    /// clipboard) is `None`. Native access/conversion failures remain errors.
    pub fn read_text_if_present(&mut self) -> io::Result<Option<String>> {
        match &mut self.0.lock().expect("Clipboard lock poisoned").backend {
            Backend::Memory(Contents::Text(value)) => Ok(Some(value.clone())),
            Backend::Memory(_) => Ok(None),
            #[cfg(all(
                feature = "system_clipboard",
                any(unix, windows),
                not(any(target_os = "android", target_os = "ios"))
            ))]
            Backend::System(clipboard) => native_if_present(clipboard.get_text()),
        }
    }
    pub fn write_text(&mut self, text: &str) -> io::Result<()> {
        let mut state = self.0.lock().expect("Clipboard lock poisoned");
        let next_generation = next_generation(&state)?;
        match &mut state.backend {
            Backend::Memory(value) => *value = Contents::Text(text.into()),
            #[cfg(all(
                feature = "system_clipboard",
                any(unix, windows),
                not(any(target_os = "android", target_os = "ios"))
            ))]
            Backend::System(clipboard) => clipboard.set_text(text).map_err(io::Error::other)?,
        }
        state.local_write_generation = next_generation;
        Ok(())
    }
    pub fn read_files(&mut self) -> io::Result<Vec<PathBuf>> {
        files_if_present(&mut self.0.lock().expect("Clipboard lock poisoned").backend)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Clipboard contains no files"))
    }
    pub fn write_files(&mut self, files: &[PathBuf]) -> io::Result<()> {
        let mut state = self.0.lock().expect("Clipboard lock poisoned");
        let next_generation = next_generation(&state)?;
        match &mut state.backend {
            Backend::Memory(value) => *value = Contents::Files(files.into()),
            #[cfg(all(
                feature = "system_clipboard",
                any(unix, windows),
                not(any(target_os = "android", target_os = "ios"))
            ))]
            Backend::System(clipboard) => {
                clipboard.set().file_list(files).map_err(io::Error::other)?;
            }
        }
        state.local_write_generation = next_generation;
        Ok(())
    }

    /// An absent image representation is `None`; native failures remain errors.
    /// Natively this decodes whatever image format the platform offers (PNG or
    /// TIFF on macOS, PNG on Linux, DIB on Windows) into RGBA.
    pub fn read_image_if_present(&mut self) -> io::Result<Option<ClipboardImage>> {
        match &mut self.0.lock().expect("Clipboard lock poisoned").backend {
            Backend::Memory(Contents::Image(image)) => Ok(Some(image.clone())),
            Backend::Memory(_) => Ok(None),
            #[cfg(all(
                feature = "system_clipboard",
                any(unix, windows),
                not(any(target_os = "android", target_os = "ios"))
            ))]
            Backend::System(clipboard) => Ok(native_if_present(clipboard.get().image())?.map(
                |image| ClipboardImage {
                    width: image.width,
                    height: image.height,
                    rgba: image.bytes.into_owned(),
                },
            )),
        }
    }
    pub fn write_image(&mut self, image: &ClipboardImage) -> io::Result<()> {
        if image
            .width
            .checked_mul(image.height)
            .and_then(|n| n.checked_mul(4))
            != Some(image.rgba.len())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Clipboard image size does not match its RGBA data",
            ));
        }
        let mut state = self.0.lock().expect("Clipboard lock poisoned");
        let next_generation = next_generation(&state)?;
        match &mut state.backend {
            Backend::Memory(value) => *value = Contents::Image(image.clone()),
            #[cfg(all(
                feature = "system_clipboard",
                any(unix, windows),
                not(any(target_os = "android", target_os = "ios"))
            ))]
            Backend::System(clipboard) => clipboard
                .set_image(arboard::ImageData {
                    width: image.width,
                    height: image.height,
                    bytes: std::borrow::Cow::Borrowed(&image.rgba),
                })
                .map_err(io::Error::other)?,
        }
        state.local_write_generation = next_generation;
        Ok(())
    }

    /// Remove all clipboard representations, not merely write an empty file list.
    /// A successful clear advances the shared revision, even if already empty.
    /// Afterwards `read_text_if_present()` returns `None`, and required text/file
    /// reads report absence. Native operation failures remain errors.
    pub fn clear(&mut self) -> io::Result<()> {
        let mut state = self.0.lock().expect("Clipboard lock poisoned");
        let next_generation = next_generation(&state)?;
        match &mut state.backend {
            Backend::Memory(value) => *value = Contents::Empty,
            #[cfg(all(
                feature = "system_clipboard",
                any(unix, windows),
                not(any(target_os = "android", target_os = "ios"))
            ))]
            Backend::System(clipboard) => clipboard.clear().map_err(io::Error::other)?,
        }
        state.local_write_generation = next_generation;
        Ok(())
    }

    /// Read files and local write identity under one lock, shared by cloned handles.
    /// The application owns any associated pending cut/move payload, never this crate.
    pub fn read_files_snapshot(&mut self) -> io::Result<ClipboardFilesSnapshot> {
        let mut state = self.0.lock().expect("Clipboard lock poisoned");
        let files = files_if_present(&mut state.backend)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "Clipboard contains no files")
        })?;
        Ok(ClipboardFilesSnapshot {
            source: Arc::downgrade(&self.0),
            local_write_generation: state.local_write_generation,
            files,
        })
    }

    /// Check the same service, local write generation and current file contents.
    /// Missing files or a different local write return `false`; read failures remain
    /// errors. Even a same-payload local rewrite invalidates the snapshot.
    ///
    /// In memory mode this proves no intervening write. In system mode an external
    /// same-payload replacement (or replacement and restoration between reads) is
    /// undetectable: arboard exposes no native change counter. A `true` result is NOT
    /// proof of native ownership and must not alone authorize a destructive move.
    pub fn matches_files_snapshot(
        &mut self,
        snapshot: &ClipboardFilesSnapshot,
    ) -> io::Result<bool> {
        if !Weak::ptr_eq(&Arc::downgrade(&self.0), &snapshot.source) {
            return Ok(false);
        }
        let mut state = self.0.lock().expect("Clipboard lock poisoned");
        if state.local_write_generation != snapshot.local_write_generation {
            return Ok(false);
        }
        Ok(files_if_present(&mut state.backend)?.as_deref() == Some(snapshot.files()))
    }
}

fn next_generation(state: &State) -> io::Result<u64> {
    state
        .local_write_generation
        .checked_add(1)
        .ok_or_else(|| io::Error::other("Clipboard local write generation exhausted"))
}

fn files_if_present(backend: &mut Backend) -> io::Result<Option<Vec<PathBuf>>> {
    match backend {
        Backend::Memory(Contents::Files(value)) => Ok(Some(value.clone())),
        Backend::Memory(_) => Ok(None),
        #[cfg(all(
            feature = "system_clipboard",
            any(unix, windows),
            not(any(target_os = "android", target_os = "ios"))
        ))]
        Backend::System(clipboard) => native_if_present(clipboard.get().file_list()),
    }
}

#[cfg(all(
    feature = "system_clipboard",
    any(unix, windows),
    not(any(target_os = "android", target_os = "ios"))
))]
fn native_if_present<T>(result: Result<T, arboard::Error>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(error) => Err(io::Error::other(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memory_clipboard_shares_one_content_across_handles_and_formats() {
        let mut clipboard = Clipboard::memory();
        let mut ui_handle = clipboard.clone();
        assert_eq!(clipboard.revision(), 0);
        assert_eq!(clipboard.read_text().unwrap(), "");
        assert_eq!(clipboard.revision(), 0);
        clipboard.write_text("hello").unwrap();
        assert_eq!(ui_handle.revision(), 1);
        assert_eq!(ui_handle.read_text().unwrap(), "hello");
        assert_eq!(
            ui_handle.read_text_if_present().unwrap().as_deref(),
            Some("hello")
        );
        let files = vec![
            PathBuf::from("/project/hello.txt"),
            PathBuf::from("/project/a b.rs"),
        ];
        ui_handle.write_files(&files).unwrap();
        assert_eq!(clipboard.revision(), 2);
        assert_eq!(clipboard.read_files().unwrap(), files);
        assert_eq!(clipboard.read_text_if_present().unwrap(), None);
        assert_eq!(
            clipboard.read_text().unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        clipboard.write_text("").unwrap();
        assert_eq!(ui_handle.revision(), 3);
        assert_eq!(
            ui_handle.read_text_if_present().unwrap(),
            Some(String::new())
        );
        assert_eq!(ui_handle.read_text().unwrap(), "");
        assert!(ui_handle.read_files().is_err());
    }

    #[test]
    fn file_snapshot_detects_shared_writes_even_when_contents_are_identical() {
        let mut clipboard = Clipboard::memory();
        let mut other = clipboard.clone();
        let files = vec![PathBuf::from("/project/file")];
        clipboard.write_files(&files).unwrap();
        let snapshot = clipboard.read_files_snapshot().unwrap();
        assert_eq!(snapshot.files(), files);
        assert_eq!(snapshot.local_write_generation(), 1);
        assert!(other.matches_files_snapshot(&snapshot).unwrap());
        other.write_files(&files).unwrap();
        assert!(!clipboard.matches_files_snapshot(&snapshot).unwrap());
        let snapshot = clipboard.read_files_snapshot().unwrap();
        other.write_text("replaced").unwrap();
        assert!(!clipboard.matches_files_snapshot(&snapshot).unwrap());
        assert!(clipboard.read_files_snapshot().is_err());
    }

    #[test]
    fn clear_is_shared_advances_revision_and_invalidates_pending_file_snapshot() {
        let mut clipboard = Clipboard::memory();
        let mut other = clipboard.clone();
        let files = vec![PathBuf::from("/project/cut-file")];
        clipboard.write_files(&files).unwrap();
        let pending_cut = clipboard.read_files_snapshot().unwrap();
        assert!(other.matches_files_snapshot(&pending_cut).unwrap());

        other.clear().unwrap();
        assert_eq!(clipboard.revision(), 2);
        assert!(!clipboard.matches_files_snapshot(&pending_cut).unwrap());
        assert_eq!(clipboard.read_text_if_present().unwrap(), None);
        assert_eq!(
            clipboard.read_text().unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            clipboard.read_files().unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(clipboard.read_files_snapshot().is_err());
        // Restoring the same file payload cannot resurrect the old cut snapshot.
        other.write_files(&files).unwrap();
        assert!(!clipboard.matches_files_snapshot(&pending_cut).unwrap());
        assert_eq!(clipboard.revision(), 3);
    }

    #[test]
    fn clear_removes_text_and_clearing_an_empty_clipboard_is_another_revision() {
        let mut clipboard = Clipboard::memory();
        clipboard.write_text("text to remove").unwrap();
        clipboard.clear().unwrap();
        assert_eq!(clipboard.revision(), 2);
        assert_eq!(clipboard.read_text_if_present().unwrap(), None);
        clipboard.clear().unwrap();
        assert_eq!(clipboard.revision(), 3);
        assert_eq!(clipboard.read_text_if_present().unwrap(), None);
        assert_eq!(
            clipboard.read_files().unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        // An explicitly written empty string still has a text representation.
        clipboard.write_text("").unwrap();
        assert_eq!(
            clipboard.read_text_if_present().unwrap(),
            Some(String::new())
        );
    }

    #[test]
    fn memory_image_is_one_more_exclusive_representation() {
        let mut clipboard = Clipboard::memory();
        assert_eq!(clipboard.read_image_if_present().unwrap(), None);
        let image = ClipboardImage {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 0, 255, 128],
        };
        clipboard.write_image(&image).unwrap();
        assert_eq!(clipboard.revision(), 1);
        assert_eq!(clipboard.read_image_if_present().unwrap(), Some(image));
        assert_eq!(clipboard.read_text_if_present().unwrap(), None);
        assert!(clipboard.read_files().is_err());
        clipboard.write_text("replaced").unwrap();
        assert_eq!(clipboard.read_image_if_present().unwrap(), None);
        let malformed = ClipboardImage {
            width: 2,
            height: 2,
            rgba: vec![0; 4],
        };
        assert_eq!(
            clipboard.write_image(&malformed).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(clipboard.revision(), 2);
    }

    #[test]
    fn file_snapshot_from_an_independent_clipboard_is_not_a_match() {
        let files = vec![PathBuf::from("/project/file")];
        let mut first = Clipboard::memory();
        let mut second = Clipboard::memory();
        first.write_files(&files).unwrap();
        second.write_files(&files).unwrap();
        let snapshot = first.read_files_snapshot().unwrap();
        assert!(!second.matches_files_snapshot(&snapshot).unwrap());
    }

    #[cfg(all(
        feature = "system_clipboard",
        any(unix, windows),
        not(any(target_os = "android", target_os = "ios"))
    ))]
    #[test]
    fn native_absent_format_is_not_an_error_but_operation_failures_are() {
        assert_eq!(
            native_if_present::<String>(Err(arboard::Error::ContentNotAvailable)).unwrap(),
            None
        );
        for error in [
            arboard::Error::ClipboardOccupied,
            arboard::Error::ConversionFailure,
            arboard::Error::ClipboardNotSupported,
            arboard::Error::Unknown {
                description: "backend failure".into(),
            },
        ] {
            assert!(native_if_present::<String>(Err(error)).is_err());
        }
        assert_eq!(native_if_present(Ok("text")).unwrap(), Some("text"));
    }

    #[test]
    fn plugin_preserves_application_clipboard_without_workbench_window_or_renderer() {
        let mut app = App::new();
        let mut clipboard = Clipboard::memory();
        clipboard.write_text("preserve me").unwrap();
        app.insert_resource(clipboard).add_plugins(ClipboardPlugin);
        app.update();
        assert_eq!(
            app.world_mut()
                .resource_mut::<Clipboard>()
                .read_text()
                .unwrap(),
            "preserve me"
        );
    }
}
