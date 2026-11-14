//! Images (feature `image`): `decode` turns a file's bytes into frames of
//! pixels, `ImageView` shows them zoomable, and the loaders (installed in
//! every bedtime-ui context) let `egui::Image` show `file://` paths and
//! anything `decode` reads.
mod decode;
mod loader;
mod transform;
mod view;
pub use decode::{
    Decoded, Frame, MAX_DECODED_BYTES, MAX_SIDE, SUPPORTED_EXTENSIONS, decode, supported,
};
pub use loader::{FileLoader, ImageLoader, MAX_FILE_BYTES, install};
pub use transform::ImageTransform;
pub use view::{Backdrop, ImageView};
