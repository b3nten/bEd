//! A clipboard usable by games, UI and workbench independently.
//!
//! Adapted from Bevy 0.20's `bevy_clipboard` resource/plugin design, distributed
//! under MIT OR Apache-2.0 (see the accompanying licenses). File lists, shared
//! ownership and fallback after native initialization failure are Bedtime additions.
//! `system_clipboard` opts into OS access on desktop targets. Otherwise the same
//! API uses an in-process clipboard. An operation on a working native backend
//! reports its error; it never silently substitutes stale in-process contents.
use bevy_app::{App, Plugin};
use bevy_ecs::resource::Resource;
use std::{
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
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
    Text(String),
    Files(Vec<PathBuf>),
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
    pub fn read_text(&mut self) -> io::Result<String> {
        match &mut self.0.lock().expect("Clipboard lock poisoned").backend {
            Backend::Memory(Contents::Text(value)) => Ok(value.clone()),
            Backend::Memory(_) => Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Clipboard contains no text",
            )),
            #[cfg(all(
                feature = "system_clipboard",
                any(unix, windows),
                not(any(target_os = "android", target_os = "ios"))
            ))]
            Backend::System(clipboard) => clipboard.get_text().map_err(io::Error::other),
        }
    }
    pub fn write_text(&mut self, text: &str) -> io::Result<()> {
        match &mut self.0.lock().expect("Clipboard lock poisoned").backend {
            Backend::Memory(value) => {
                *value = Contents::Text(text.into());
                Ok(())
            }
            #[cfg(all(
                feature = "system_clipboard",
                any(unix, windows),
                not(any(target_os = "android", target_os = "ios"))
            ))]
            Backend::System(clipboard) => clipboard.set_text(text).map_err(io::Error::other),
        }
    }
    pub fn read_files(&mut self) -> io::Result<Vec<PathBuf>> {
        match &mut self.0.lock().expect("Clipboard lock poisoned").backend {
            Backend::Memory(Contents::Files(value)) => Ok(value.clone()),
            Backend::Memory(_) => Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Clipboard contains no files",
            )),
            #[cfg(all(
                feature = "system_clipboard",
                any(unix, windows),
                not(any(target_os = "android", target_os = "ios"))
            ))]
            Backend::System(clipboard) => clipboard.get().file_list().map_err(io::Error::other),
        }
    }
    pub fn write_files(&mut self, files: &[PathBuf]) -> io::Result<()> {
        match &mut self.0.lock().expect("Clipboard lock poisoned").backend {
            Backend::Memory(value) => {
                *value = Contents::Files(files.into());
                Ok(())
            }
            #[cfg(all(
                feature = "system_clipboard",
                any(unix, windows),
                not(any(target_os = "android", target_os = "ios"))
            ))]
            Backend::System(clipboard) => {
                clipboard.set().file_list(files).map_err(io::Error::other)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memory_clipboard_shares_one_content_across_handles_and_formats() {
        let mut clipboard = Clipboard::memory();
        let mut ui_handle = clipboard.clone();
        assert_eq!(clipboard.read_text().unwrap(), "");
        clipboard.write_text("hello").unwrap();
        assert_eq!(ui_handle.read_text().unwrap(), "hello");
        let files = vec![
            PathBuf::from("/project/hello.txt"),
            PathBuf::from("/project/a b.rs"),
        ];
        ui_handle.write_files(&files).unwrap();
        assert_eq!(clipboard.read_files().unwrap(), files);
        assert!(clipboard.read_text().is_err());
        clipboard.write_text("").unwrap();
        assert_eq!(ui_handle.read_text().unwrap(), "");
        assert!(ui_handle.read_files().is_err());
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
