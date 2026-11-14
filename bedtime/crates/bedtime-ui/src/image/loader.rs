//! egui loaders, so `egui::Image` shows anything `decode` can: `file://`
//! paths are read (`FileLoader`), and bytes from any loader are decoded on a
//! thread of their own (`ImageLoader`), with `decode`'s limits. Animated GIF
//! and WebP play the way egui plays them (it asks for `uri#frame`); other
//! animations show their first frame (`ImageView` plays them all).
use super::{Decoded, decode};
use egui::{
    ColorImage, Context, FrameDurations, Id,
    load::{
        BytesLoadResult, BytesLoader, BytesPoll, ImageLoadResult, ImagePoll, LoadError, SizeHint,
    },
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    thread,
};

/// Files larger than this aren't read.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Add the loaders to `ctx`. bedtime-ui does this for its contexts.
pub fn install(ctx: &Context) {
    let loaders = ctx.loaders();
    let has = |id: &str| loaders.image.lock().iter().any(|l| l.id() == id);
    if has(IMAGE_LOADER) {
        return;
    }
    ctx.add_bytes_loader(Arc::new(FileLoader::default()));
    ctx.add_image_loader(Arc::new(ImageLoader::default()));
}

const IMAGE_LOADER: &str = egui::generate_loader_id!(ImageLoader);

enum Entry<T> {
    Pending,
    Ready(T),
    Failed(String),
}

/// A loader's results by URI, shared with its threads.
type Cache<T> = Arc<Mutex<HashMap<String, Entry<T>>>>;

/// Reads `file://` URIs on a thread.
#[derive(Default)]
pub struct FileLoader {
    files: Cache<Arc<[u8]>>,
}

impl BytesLoader for FileLoader {
    fn id(&self) -> &str {
        egui::generate_loader_id!(FileLoader)
    }

    fn load(&self, ctx: &Context, uri: &str) -> BytesLoadResult {
        let Some(path) = uri.strip_prefix("file://") else {
            return Err(LoadError::NotSupported);
        };
        let mut files = self.files.lock().unwrap();
        match files.get(uri) {
            Some(Entry::Ready(bytes)) => {
                return Ok(BytesPoll::Ready {
                    size: None,
                    bytes: egui::load::Bytes::Shared(bytes.clone()),
                    mime: None,
                });
            }
            Some(Entry::Failed(why)) => return Err(LoadError::Loading(why.clone())),
            Some(Entry::Pending) => return Ok(BytesPoll::Pending { size: None }),
            None => {}
        }
        files.insert(uri.to_owned(), Entry::Pending);
        let (shared, key, path, ctx) = (
            self.files.clone(),
            uri.to_owned(),
            path.to_owned(),
            ctx.clone(),
        );
        let spawned = thread::Builder::new()
            .name("image-file".into())
            .spawn(move || {
                let read = std::fs::metadata(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|meta| {
                        if meta.len() > MAX_FILE_BYTES {
                            Err("It's too large to show here".into())
                        } else {
                            std::fs::read(&path).map_err(|e| e.to_string())
                        }
                    });
                let entry = match read {
                    Ok(bytes) => Entry::Ready(bytes.into()),
                    Err(why) => Entry::Failed(why),
                };
                // Forgotten meanwhile: don't bring it back.
                let mut files = shared.lock().unwrap();
                if files.contains_key(&key) {
                    files.insert(key, entry);
                }
                ctx.request_repaint();
            });
        if let Err(why) = spawned {
            files.insert(uri.to_owned(), Entry::Failed(why.to_string()));
        }
        Ok(BytesPoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        self.files.lock().unwrap().remove(uri);
    }

    fn forget_all(&self) {
        self.files.lock().unwrap().clear();
    }

    fn byte_size(&self) -> usize {
        self.files
            .lock()
            .unwrap()
            .values()
            .map(|entry| match entry {
                Entry::Ready(bytes) => bytes.len(),
                _ => 0,
            })
            .sum()
    }

    fn has_pending(&self) -> bool {
        self.files
            .lock()
            .unwrap()
            .values()
            .any(|entry| matches!(entry, Entry::Pending))
    }
}

/// A decoded image, and its frames as egui images (made when first shown).
struct Pictured {
    decoded: Arc<Decoded>,
    frames: Vec<OnceLock<Arc<ColorImage>>>,
}

/// Decodes bytes (from any bytes loader) on a thread.
#[derive(Default)]
pub struct ImageLoader {
    images: Cache<Arc<Pictured>>,
}

impl egui::load::ImageLoader for ImageLoader {
    fn id(&self) -> &str {
        IMAGE_LOADER
    }

    fn load(&self, ctx: &Context, uri: &str, _: SizeHint) -> ImageLoadResult {
        // egui asks for an animated GIF or WebP's frames as `uri#frame`.
        let (base, frame) = match egui::decode_animated_image_uri(uri) {
            Ok((base, frame)) => (base, frame),
            Err(_) => (uri, 0),
        };
        let mut images = self.images.lock().unwrap();
        match images.get(base) {
            Some(Entry::Ready(pictured)) => {
                let pictured = pictured.clone();
                drop(images);
                if pictured.frames.len() > 1 {
                    let delays = pictured.decoded.frames.iter().map(|f| f.delay).collect();
                    ctx.data_mut(|data| {
                        data.insert_temp(Id::new(base), FrameDurations::new(delays))
                    });
                }
                return Ok(ImagePoll::Ready {
                    image: color_image(&pictured, frame),
                });
            }
            Some(Entry::Failed(why)) => return Err(LoadError::Loading(why.clone())),
            Some(Entry::Pending) => return Ok(ImagePoll::Pending { size: None }),
            None => {}
        }
        let bytes = match ctx.try_load_bytes(base)? {
            BytesPoll::Pending { size } => return Ok(ImagePoll::Pending { size }),
            BytesPoll::Ready { bytes, .. } => bytes,
        };
        images.insert(base.to_owned(), Entry::Pending);
        let (images, base, ctx) = (self.images.clone(), base.to_owned(), ctx.clone());
        let bytes: Arc<[u8]> = match bytes {
            egui::load::Bytes::Static(bytes) => bytes.into(),
            egui::load::Bytes::Shared(bytes) => bytes,
        };
        thread::Builder::new()
            .name("image-decode".into())
            .spawn(move || {
                let entry = match decode(&bytes) {
                    Ok(decoded) if decoded.frames.is_empty() => {
                        Entry::Failed("It has no frames".into())
                    }
                    Ok(decoded) => Entry::Ready(Arc::new(Pictured {
                        frames: (0..decoded.frames.len()).map(|_| OnceLock::new()).collect(),
                        decoded: Arc::new(decoded),
                    })),
                    Err(why) => Entry::Failed(why),
                };
                let mut images = images.lock().unwrap();
                if images.contains_key(&base) {
                    images.insert(base, entry);
                }
                ctx.request_repaint();
            })
            .map_err(|why| LoadError::Loading(why.to_string()))?;
        Ok(ImagePoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        let base = egui::decode_animated_image_uri(uri).map_or(uri, |(base, _)| base);
        self.images.lock().unwrap().remove(base);
    }

    fn forget_all(&self) {
        self.images.lock().unwrap().clear();
    }

    fn byte_size(&self) -> usize {
        self.images
            .lock()
            .unwrap()
            .values()
            .map(|entry| match entry {
                Entry::Ready(pictured) => pictured
                    .decoded
                    .frames
                    .iter()
                    .map(|frame| frame.rgba.len())
                    .sum(),
                _ => 0,
            })
            .sum()
    }

    fn has_pending(&self) -> bool {
        self.images
            .lock()
            .unwrap()
            .values()
            .any(|entry| matches!(entry, Entry::Pending))
    }
}

/// Frame `index` (the last if there are fewer) as an egui image, at the
/// decoded image's own size in points (an SVG renders larger, to stay sharp).
fn color_image(pictured: &Pictured, index: usize) -> Arc<ColorImage> {
    let index = index.min(pictured.frames.len() - 1);
    pictured.frames[index]
        .get_or_init(|| {
            let frame = &pictured.decoded.frames[index];
            let mut image = ColorImage::from_rgba_unmultiplied(
                [frame.size[0] as usize, frame.size[1] as usize],
                &frame.rgba,
            );
            let [width, height] = pictured.decoded.size;
            image.source_size = egui::vec2(width as f32, height as f32);
            Arc::new(image)
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbaImage::from_pixel(width, height, image::Rgba([255, 0, 0, 255]))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    /// Ask until it's no longer pending.
    fn load(ctx: &Context, uri: &str) -> ImageLoadResult {
        let loader = ctx
            .loaders()
            .image
            .lock()
            .iter()
            .find(|l| l.id() == IMAGE_LOADER)
            .cloned()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match loader.load(ctx, uri, SizeHint::default()) {
                Ok(ImagePoll::Pending { .. }) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5))
                }
                other => return other,
            }
        }
    }

    #[test]
    fn files_are_read_and_decoded() {
        let ctx = Context::default();
        install(&ctx);
        install(&ctx); // Once is enough.
        assert_eq!(
            ctx.loaders()
                .image
                .lock()
                .iter()
                .filter(|l| l.id() == IMAGE_LOADER)
                .count(),
            1
        );
        let path =
            std::env::temp_dir().join(format!("bedtime-ui-loader-{}.png", std::process::id()));
        std::fs::write(&path, png(3, 2)).unwrap();
        let uri = format!("file://{}", path.display());
        let Ok(ImagePoll::Ready { image }) = load(&ctx, &uri) else {
            panic!("not loaded");
        };
        assert_eq!(image.size, [3, 2]);
        assert_eq!(image.source_size, egui::vec2(3.0, 2.0));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn bytes_that_are_not_an_image_fail() {
        let ctx = Context::default();
        install(&ctx);
        ctx.include_bytes("bytes://nonsense.png", b"not an image".to_vec());
        assert!(matches!(
            load(&ctx, "bytes://nonsense.png"),
            Err(LoadError::Loading(_))
        ));
        assert!(matches!(
            load(&ctx, "file:///nowhere/at/all.png"),
            Err(LoadError::Loading(_))
        ));
    }
}
