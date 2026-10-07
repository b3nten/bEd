//! Translated from ned files/files.{h,cpp}; see LICENSE and NOTICE.
use std::io;
use std::{fs, io::Read, path::Path};
pub const MAX_FILE_SIZE: usize = bed_remote::MAX_FILE_BYTES;
pub const BINARY_ERROR: &str = "Error: File appears to be binary and cannot be displayed.";
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadFile {
    pub raw: Vec<u8>,
}
pub fn read_file_raw(path: &Path) -> io::Result<ReadFile> {
    let file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Error: Unable to open file.",
        ));
    }
    let file_size = metadata.len();
    if file_size > MAX_FILE_SIZE as u64 {
        return Err(file_too_large(path, file_size));
    }
    let raw = read_text_bytes(&file, file_size as usize)?;
    if raw.len() > MAX_FILE_SIZE {
        return Err(file_too_large(
            path,
            file.metadata()?.len().max(raw.len() as u64),
        ));
    }
    Ok(ReadFile { raw })
}

pub fn file_too_large(path: &Path, actual: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "Cannot edit '{}': file is {actual} bytes; Bed's limit is {} MiB ({MAX_FILE_SIZE} bytes). Open it in another editor or split it into smaller files.",
            path.display(),
            MAX_FILE_SIZE / (1024 * 1024),
        ),
    )
}

fn read_text_bytes(mut file: impl Read, expected_size: usize) -> io::Result<Vec<u8>> {
    // Reject binary files after the same 1KiB probe used upstream, before
    // allocating/reading the remainder of a potentially large build artifact.
    let mut raw = Vec::with_capacity(expected_size.min(1024));
    file.by_ref().take(1024).read_to_end(&mut raw)?;
    let prefix = &raw[..raw.len().min(1024)];
    let junk = prefix
        .iter()
        .filter(|&&c| c == 0 || (c < 32 && c != b'\n' && c != b'\r' && c != b'\t'))
        .count();
    if !prefix.is_empty() && junk > prefix.len() / 10 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, BINARY_ERROR));
    }
    raw.reserve(expected_size.saturating_sub(raw.len()));
    file.take((MAX_FILE_SIZE + 1 - raw.len()) as u64)
        .read_to_end(&mut raw)?;
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    #[test]
    fn binary_probe_rejects_before_reading_or_allocating_the_file_body() {
        struct BinaryProbe {
            read: usize,
        }
        impl Read for BinaryProbe {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                assert!(self.read < 1024, "binary body must not be read");
                let count = out.len().min(1024 - self.read);
                out[..count].fill(0);
                self.read += count;
                Ok(count)
            }
        }
        let error = read_text_bytes(BinaryProbe { read: 0 }, MAX_FILE_SIZE).unwrap_err();
        assert_eq!(error.to_string(), BINARY_ERROR);
    }
    #[test]
    fn binary_detection_checks_first_1024_with_strict_threshold() {
        let temp = TempDir::new();
        let mut bytes = vec![b'a'; 2048];
        bytes[..102].fill(0);
        bytes[1024..].fill(0);
        let path = temp.write("boundary", &bytes);
        assert_eq!(read_file_raw(&path).unwrap().raw, bytes);
        bytes[102] = 0;
        fs::write(&path, &bytes).unwrap();
        let error = read_file_raw(&path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), BINARY_ERROR);
        let path = temp.write("utf8", b"\xef\xbb\xbfabc\n\r\t\xff");
        assert_eq!(
            read_file_raw(&path).unwrap().raw,
            b"\xef\xbb\xbfabc\n\r\t\xff"
        );
    }
    #[test]
    fn reads_beyond_the_old_limit_without_inserting_text_and_accepts_the_boundary() {
        let temp = TempDir::new();
        let bytes = vec![b'a'; 2 * 1024 * 1024];
        let path = temp.write("large", &bytes);
        let read = read_file_raw(&path).unwrap();
        assert_eq!(read.raw, bytes);
        fs::write(&path, vec![b'a'; MAX_FILE_SIZE]).unwrap();
        let read = read_file_raw(&path).unwrap();
        assert_eq!(read.raw.len(), MAX_FILE_SIZE);
    }
    #[test]
    fn oversized_file_fails_with_path_actual_size_and_actionable_limit() {
        let temp = TempDir::new();
        let path = temp.write("too-large.txt", b"");
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_FILE_SIZE as u64 + 1)
            .unwrap();
        let error = read_file_raw(&path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let message = error.to_string();
        assert!(message.contains("too-large.txt"));
        assert!(message.contains(&(MAX_FILE_SIZE + 1).to_string()));
        assert!(message.contains("16 MiB"));
        assert!(message.contains("another editor"));
    }
    #[test]
    fn growing_reader_is_bounded_even_when_initial_metadata_is_small() {
        let bytes = read_text_bytes(io::repeat(b'a'), 1).unwrap();
        assert_eq!(bytes.len(), MAX_FILE_SIZE + 1);
    }
    #[test]
    fn empty_files_read_and_directories_fail() {
        let temp = TempDir::new();
        let path = temp.write("empty", b"");
        assert!(read_file_raw(&path).unwrap().raw.is_empty());
        assert!(read_file_raw(temp.root()).is_err());
        assert!(read_file_raw(&temp.path("missing")).is_err());
    }
}
