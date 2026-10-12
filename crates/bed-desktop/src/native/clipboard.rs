use std::{
    fs::OpenOptions,
    io,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_IMAGE: AtomicU64 = AtomicU64::new(0);

/// Keep the file after paste: the receiving process may read it much later.
/// The OS owns cleanup of its temporary directory.
pub(super) fn save_paste_image(image: arboard::ImageData<'_>) -> io::Result<PathBuf> {
    let width = u32::try_from(image.width).map_err(io::Error::other)?;
    let height = u32::try_from(image.height).map_err(io::Error::other)?;
    let pixels = image::RgbaImage::from_raw(width, height, image.bytes.into_owned())
        .filter(|image| image.width() > 0 && image.height() > 0)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Invalid clipboard image pixels")
        })?;
    let (path, mut file) = loop {
        let path = std::env::temp_dir().join(format!(
            "bed-clipboard-{}-{}.png",
            std::process::id(),
            NEXT_IMAGE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => break (path, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    if let Err(error) = pixels.write_to(&mut file, image::ImageFormat::Png) {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(io::Error::other(error));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_images_are_unique_readable_pngs_with_alpha() {
        let pixels = [255, 0, 20, 128, 10, 30, 40, 0];
        let image = || arboard::ImageData {
            width: 2,
            height: 1,
            bytes: pixels.as_slice().into(),
        };
        let first = save_paste_image(image()).unwrap();
        let second = save_paste_image(image()).unwrap();
        assert_ne!(first, second);
        for path in [first, second] {
            let decoded = image::open(&path).unwrap().into_rgba8();
            assert_eq!(decoded.dimensions(), (2, 1));
            assert_eq!(decoded.as_raw(), &pixels);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
            std::fs::remove_file(path).unwrap();
        }
    }
}
