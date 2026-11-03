//! Images in the preview: files beside the Markdown (or at absolute paths),
//! decoded on a worker like the image viewer's, and kept as textures. Web
//! images aren't fetched; their description shows instead.
use bedtime_image::{Decoded, decode};
use bedtime_ui::egui;
use bedtime_workbench::{ManagedTask, ManagedWork};
use bevy::prelude::Entity;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// Larger files aren't read.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

pub enum Image {
    Loading(ManagedTask<Result<Decoded, String>>),
    Ready {
        texture: egui::TextureHandle,
        /// Its own size, shown as points.
        size: egui::Vec2,
    },
    Failed(String),
}

#[derive(Default)]
pub struct Images {
    entries: HashMap<PathBuf, Image>,
    /// Asked for while drawing, not loaded yet.
    wanted: Vec<PathBuf>,
}

impl Images {
    /// The image at `path`; asks for it the first time.
    pub fn get(&mut self, path: &Path) -> Option<&Image> {
        if !self.entries.contains_key(path) && !self.wanted.iter().any(|p| p == path) {
            self.wanted.push(path.to_owned());
        }
        self.entries.get(path)
    }

    /// Turn finished decodes into textures; whether any are still loading.
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let mut loading = false;
        for (path, image) in &mut self.entries {
            let Image::Loading(task) = image else {
                continue;
            };
            *image = match task.try_recv() {
                Ok(Ok(decoded)) => match decoded.frames.first() {
                    Some(frame) => {
                        let size = [frame.size[0] as usize, frame.size[1] as usize];
                        let pixels = egui::ColorImage::from_rgba_unmultiplied(size, &frame.rgba);
                        Image::Ready {
                            texture: ctx.load_texture(
                                format!("markdown-image:{}", path.display()),
                                pixels,
                                egui::TextureOptions::LINEAR,
                            ),
                            size: egui::vec2(decoded.size[0] as f32, decoded.size[1] as f32),
                        }
                    }
                    None => Image::Failed("It has no frames".into()),
                },
                Ok(Err(error)) => Image::Failed(error),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Image::Failed("Loading stopped".into())
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    loading = true;
                    continue;
                }
            };
        }
        loading
    }

    /// Start loading what was asked for.
    pub fn load_wanted(&mut self, work: &mut ManagedWork, panel: Entity) {
        for path in std::mem::take(&mut self.wanted) {
            let file = path.clone();
            let image = match work.spawn_task(Some(panel), "Loading an image", move |_| {
                let length = std::fs::metadata(&file).map_err(|e| e.to_string())?.len();
                if length > MAX_FILE_BYTES {
                    return Err("It's too large to show here".into());
                }
                decode(&std::fs::read(&file).map_err(|e| e.to_string())?)
            }) {
                Ok(task) => Image::Loading(task),
                Err(error) => Image::Failed(error),
            };
            self.entries.insert(path, image);
        }
    }
}

/// The file an image's source names, relative to the Markdown's folder;
/// `None` for web and inline (`data:`) images.
pub fn source_path(base: Option<&Path>, source: &str) -> Option<PathBuf> {
    if source.contains("://") || source.starts_with("data:") || source.is_empty() {
        return None;
    }
    let file = source.split(['#', '?']).next().unwrap_or(source);
    let file = super::percent_decode(file);
    let file = Path::new(&file);
    if file.is_absolute() {
        return Some(file.to_owned());
    }
    Some(base?.join(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_are_files_beside_the_markdown() {
        let base = Some(Path::new("/docs"));
        assert_eq!(
            source_path(base, "img/My%20Logo.png?raw=true"),
            Some(PathBuf::from("/docs/img/My Logo.png"))
        );
        assert_eq!(source_path(base, "/abs.png"), Some(PathBuf::from("/abs.png")));
        assert_eq!(source_path(base, "https://x.org/a.png"), None);
        assert_eq!(source_path(None, "a.png"), None);
    }
}
