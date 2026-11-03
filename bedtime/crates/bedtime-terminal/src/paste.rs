//! What a paste (or a drop) of something other than text types into a shell.
//!
//! Files become their shell-escaped paths. An image with no file behind it
//! (a screenshot, "Copy Image" in a browser) is saved as a PNG in a temporary
//! folder and its path is typed instead, which is how command-line tools that
//! take images (coding agents, `open`, converters) expect to receive them.
use bedtime_clipboard::{Clipboard, ClipboardImage};
use std::{
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

/// What Primary+V types. `bridged` is the text UI's clipboard bridge already
/// read for this chord, if any. A file list wins over text because a file
/// copied in Finder also offers its bare name as text; text wins over an image
/// because rich copies (a spreadsheet range, a web page) also offer a picture.
pub(crate) fn clipboard_paste(
    clipboard: Option<&mut Clipboard>,
    bridged: Option<String>,
) -> io::Result<Option<String>> {
    let Some(clipboard) = clipboard else {
        return Ok(bridged.filter(|text| !text.is_empty()));
    };
    match clipboard.read_files() {
        Ok(files) if !files.is_empty() => return Ok(Some(paths_text(&files))),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let text = match bridged {
        Some(text) => Some(text),
        None => clipboard.read_text_if_present()?,
    };
    if let Some(text) = text.filter(|text| !text.is_empty()) {
        return Ok(Some(text));
    }
    match clipboard.read_image_if_present()? {
        Some(image) => {
            let path = save_image(&image, &paste_directory())?;
            Ok(Some(paths_text(&[path])))
        }
        None => Ok(None),
    }
}

/// Paths separated by spaces, each escaped for POSIX shells.
pub(crate) fn paths_text(paths: &[impl AsRef<Path>]) -> String {
    paths
        .iter()
        .map(|path| shell_escape(&path.as_ref().to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Backslash-escape shell metacharacters, as Terminal.app does for a dropped
/// file. Characters a backslash can't make literal (newlines) are quoted.
pub(crate) fn shell_escape(text: &str) -> String {
    if text.contains(['\n', '\r']) {
        return format!("'{}'", text.replace('\'', r"'\''"));
    }
    let mut escaped = String::with_capacity(text.len());
    for (index, c) in text.chars().enumerate() {
        let special = matches!(
            c,
            ' ' | '\t'
                | '\\'
                | '\''
                | '"'
                | '`'
                | '$'
                | '!'
                | '&'
                | '|'
                | ';'
                | '<'
                | '>'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '*'
                | '?'
                | '#'
        ) || c == '~' && index == 0;
        if special {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

fn paste_directory() -> PathBuf {
    std::env::temp_dir().join("bedtime-paste")
}

/// Encode as PNG under `directory` with a fresh name; returns the file's path.
pub(crate) fn save_image(image: &ClipboardImage, directory: &Path) -> io::Result<PathBuf> {
    use image::{
        ExtendedColorType, ImageEncoder,
        codecs::png::{CompressionType, FilterType, PngEncoder},
    };
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let (Ok(width), Ok(height)) = (u32::try_from(image.width), u32::try_from(image.height)) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Pasted image is too large",
        ));
    };
    std::fs::create_dir_all(directory)?;
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = directory.join(format!("pasted-image-{millis}-{count}.png"));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    // Pasting is synchronous so the path is typed before anything typed after
    // it; fast compression keeps a full-screen screenshot quick to write.
    PngEncoder::new_with_quality(
        io::BufWriter::new(file),
        CompressionType::Fast,
        FilterType::Adaptive,
    )
    .write_image(&image.rgba, width, height, ExtendedColorType::Rgba8)
    .map_err(|error| {
        let _ = std::fs::remove_file(&path);
        io::Error::other(error)
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_escaped_for_the_shell() {
        assert_eq!(shell_escape("/tmp/plain.png"), "/tmp/plain.png");
        assert_eq!(
            shell_escape("/Users/me/Screen Shot (1).png"),
            r"/Users/me/Screen\ Shot\ \(1\).png"
        );
        assert_eq!(shell_escape("it's $HOME&"), r"it\'s\ \$HOME\&");
        assert_eq!(shell_escape("~/a~b"), r"\~/a~b");
        assert_eq!(shell_escape("日本語.txt"), "日本語.txt");
        assert_eq!(
            shell_escape("a\nb'c"),
            r"'a
b'\''c'"
        );
        assert_eq!(
            paths_text(&[PathBuf::from("/a b"), PathBuf::from("/c")]),
            r"/a\ b /c"
        );
    }

    #[test]
    fn files_win_over_their_names_as_text_and_text_wins_over_images() {
        let mut clipboard = Clipboard::memory();
        clipboard
            .write_files(&[PathBuf::from("/project/a b.rs"), PathBuf::from("/x")])
            .unwrap();
        assert_eq!(
            clipboard_paste(Some(&mut clipboard), Some("a b.rs".into())).unwrap(),
            Some(r"/project/a\ b.rs /x".into())
        );
        clipboard.write_text("echo hi").unwrap();
        assert_eq!(
            clipboard_paste(Some(&mut clipboard), None).unwrap(),
            Some("echo hi".into())
        );
        assert_eq!(
            clipboard_paste(Some(&mut clipboard), Some("bridged".into())).unwrap(),
            Some("bridged".into())
        );
        clipboard.clear().unwrap();
        assert_eq!(clipboard_paste(Some(&mut clipboard), None).unwrap(), None);
        assert_eq!(
            clipboard_paste(None, Some("headless".into())).unwrap(),
            Some("headless".into())
        );
    }

    #[test]
    fn an_image_is_saved_as_a_png_and_its_path_is_pasted() {
        let mut clipboard = Clipboard::memory();
        let image = ClipboardImage {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 128],
        };
        clipboard.write_image(&image).unwrap();
        let pasted = clipboard_paste(Some(&mut clipboard), None)
            .unwrap()
            .unwrap();
        let path = PathBuf::from(&pasted);
        assert!(path.starts_with(paste_directory()), "{pasted}");
        let decoded = image::open(&path).unwrap().into_rgba8();
        assert_eq!(decoded.dimensions(), (2, 1));
        assert_eq!(decoded.into_raw(), image.rgba);
        std::fs::remove_file(path).unwrap();
    }
}
