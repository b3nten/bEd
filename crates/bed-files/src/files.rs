//! Translated from ned files/files.{h,cpp}; see LICENSE and NOTICE.
use std::io;
use std::{fs, io::Read, path::Path};
pub const MAX_FILE_SIZE: usize = 1024 * 1024;
pub const BINARY_ERROR: &str = "Error: File appears to be binary and cannot be displayed.";
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadFile {
    pub raw: Vec<u8>,
    pub truncated: bool,
}
pub fn read_file_raw(path: &Path) -> io::Result<ReadFile> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Error: Unable to open file.",
        ));
    }
    let file_size = metadata.len();
    let truncated = file_size > MAX_FILE_SIZE as u64;
    let read_size = if truncated {
        MAX_FILE_SIZE
    } else {
        file_size as usize
    };
    let file = fs::File::open(path)?;
    let mut raw = read_text_bytes(file, read_size)?;
    if truncated {
        let mut notice = format!(
            "\n\n[File truncated - No Edits - showing first {}MB of {}MB]\n",
            MAX_FILE_SIZE / (1024 * 1024),
            file_size / (1024 * 1024)
        )
        .into_bytes();
        notice.extend(raw);
        raw = notice;
    }
    Ok(ReadFile { raw, truncated })
}

fn read_text_bytes(mut file: impl Read, read_size: usize) -> io::Result<Vec<u8>> {
    // Reject binary files after the same 1KiB probe used upstream, before
    // allocating/reading the remainder of a potentially large build artifact.
    let mut raw = Vec::with_capacity(read_size.min(1024));
    file.by_ref()
        .take(read_size.min(1024) as u64)
        .read_to_end(&mut raw)?;
    let prefix = &raw[..raw.len().min(1024)];
    let junk = prefix
        .iter()
        .filter(|&&c| c == 0 || (c < 32 && c != b'\n' && c != b'\r' && c != b'\t'))
        .count();
    if !prefix.is_empty() && junk > prefix.len() / 10 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, BINARY_ERROR));
    }
    let remaining = read_size.saturating_sub(raw.len());
    raw.reserve(remaining);
    file.take(remaining as u64).read_to_end(&mut raw)?;
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
    fn caps_at_one_mib_and_prepends_exact_truncation_notice() {
        let temp = TempDir::new();
        let path = temp.write("large", &vec![b'a'; MAX_FILE_SIZE + 1]);
        let read = read_file_raw(&path).unwrap();
        assert!(read.truncated);
        let notice = b"\n\n[File truncated - No Edits - showing first 1MB of 1MB]\n";
        assert!(read.raw.starts_with(notice));
        assert_eq!(read.raw.len(), notice.len() + MAX_FILE_SIZE);
        assert!(read.raw[notice.len()..].iter().all(|&b| b == b'a'));
        fs::write(&path, vec![b'a'; MAX_FILE_SIZE]).unwrap();
        let read = read_file_raw(&path).unwrap();
        assert!(!read.truncated);
        assert_eq!(read.raw.len(), MAX_FILE_SIZE);
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
