//! On-demand inspection shared by local and SSH tree tooltips.
use crate::{BinaryInfo, FileInfo};
use object::{
    Object, ReadRef,
    read::{ReadCache, macho::FatArch},
};
use std::{
    cell::Cell,
    fs::{self, File},
    io::{self, Read},
    ops::Range,
    path::Path,
    process::{Command, Stdio},
    time::UNIX_EPOCH,
};

pub(super) fn inspect(root: &Path, entry: &Path, target: Option<&Path>) -> io::Result<FileInfo> {
    let link = fs::symlink_metadata(entry)?;
    let metadata = target
        .and_then(|p| fs::metadata(p).ok())
        .unwrap_or_else(|| link.clone());
    let is_directory = metadata.is_dir();
    let mut info = FileInfo {
        file_type: if is_directory {
            "Folder".into()
        } else {
            extension_type(entry)
        },
        mime_type: None,
        size: metadata.len(),
        modified_unix_seconds: metadata.modified().ok().and_then(|t| {
            match t.duration_since(UNIX_EPOCH) {
                Ok(d) => i64::try_from(d.as_secs()).ok(),
                Err(e) => i64::try_from(e.duration().as_secs())
                    .ok()
                    .map(|s| -s - i64::from(e.duration().subsec_nanos() != 0)),
            }
        }),
        is_directory,
        symlink_target: if link.is_symlink() {
            fs::read_link(entry)
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        } else {
            None
        },
        readonly: metadata.permissions().readonly(),
        git: git_info(root, entry, is_directory),
        binary: None,
    };
    if metadata.is_file()
        && let Some(target) = target
    {
        if let Ok(mut file) = File::open(target) {
            let mut prefix = vec![0; 8192];
            if let Ok(count) = file.read(&mut prefix)
                && let Some(kind) = infer::get(&prefix[..count])
            {
                let category = match kind.matcher_type() {
                    infer::MatcherType::Image => "image",
                    infer::MatcherType::Audio => "audio",
                    infer::MatcherType::Video => "video",
                    infer::MatcherType::Font => "font",
                    infer::MatcherType::Archive => "archive",
                    infer::MatcherType::Book
                    | infer::MatcherType::Doc
                    | infer::MatcherType::Text => "document",
                    _ => "file",
                };
                info.file_type = type_label(kind.extension(), category);
                info.mime_type = Some(kind.mime_type().into());
            }
        }
        info.binary = binary_info(target);
        if let Some(binary) = &info.binary {
            info.file_type = format!("{} binary", binary.format);
        }
    } else if link.is_symlink() && target.is_none() {
        info.file_type = "Symbolic link (target unavailable)".into();
    }
    Ok(info)
}

fn extension_type(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ext.is_empty() {
        return if name.is_empty() {
            "File".into()
        } else {
            "File (no extension)".into()
        };
    }
    type_label(&ext, "file")
}

fn type_label(ext: &str, category: &str) -> String {
    let kind = match ext {
        "rs" => "Rust source",
        "c" | "h" => "C source",
        "cpp" | "cc" | "hpp" => "C++ source",
        "py" => "Python source",
        "js" | "jsx" => "JavaScript source",
        "ts" | "tsx" => "TypeScript source",
        "go" => "Go source",
        "json" => "JSON document",
        "toml" => "TOML document",
        "yaml" | "yml" => "YAML document",
        "md" => "Markdown document",
        "txt" => "Text document",
        "html" | "htm" => "HTML document",
        "css" => "CSS stylesheet",
        "sh" => "Shell script",
        "svg" => "SVG image",
        "png" => "PNG image",
        "jpg" | "jpeg" => "JPEG image",
        "gif" => "GIF image",
        "webp" => "WebP image",
        "tif" | "tiff" => "TIFF image",
        "wav" => "WAVE audio",
        "aiff" => "AIFF audio",
        "flac" => "FLAC audio",
        "ogg" => "Ogg media",
        "mp3" => "MP3 audio",
        "aac" => "AAC audio",
        "m4a" => "MPEG-4 audio",
        "ttf" => "TrueType font",
        "otf" => "OpenType font",
        "woff" => "WOFF font",
        "woff2" => "WOFF2 font",
        "pdf" => "PDF document",
        "zip" => "ZIP archive",
        "gz" => "Gzip archive",
        "sqlite" => "SQLite database",
        "glb" => "Binary glTF model",
        "gltf" => "glTF model",
        "pdb" => "Program debug database",
        "dsym" => "Debug symbol bundle",
        _ => return format!("{} {category}", ext.to_uppercase()),
    };
    kind.into()
}

/// Cap all header/section-name requests before ReadCache can allocate. Debug
/// inspection never reads sample data, executable segments or DWARF contents.
#[derive(Clone, Copy)]
struct BoundedRead<'a> {
    cache: &'a ReadCache<File>,
    budget: &'a Cell<u64>,
}
impl BoundedRead<'_> {
    fn charge(self, size: u64) -> Result<(), ()> {
        let remaining = self.budget.get().checked_sub(size).ok_or(())?;
        self.budget.set(remaining);
        Ok(())
    }
}
impl<'a> ReadRef<'a> for BoundedRead<'a> {
    fn len(self) -> Result<u64, ()> {
        self.cache.len()
    }
    fn read_bytes_at(self, offset: u64, size: u64) -> Result<&'a [u8], ()> {
        self.charge(size)?;
        self.cache.read_bytes_at(offset, size)
    }
    fn read_bytes_at_until(self, range: Range<u64>, delimiter: u8) -> Result<&'a [u8], ()> {
        self.charge((range.end.saturating_sub(range.start)).min(4096))?;
        self.cache.read_bytes_at_until(range, delimiter)
    }
}
#[derive(Clone, Copy)]
struct SliceRead<'a> {
    data: BoundedRead<'a>,
    offset: u64,
    size: u64,
}
impl<'a> ReadRef<'a> for SliceRead<'a> {
    fn len(self) -> Result<u64, ()> {
        Ok(self.size)
    }
    fn read_bytes_at(self, offset: u64, size: u64) -> Result<&'a [u8], ()> {
        if offset.checked_add(size).ok_or(())? > self.size {
            return Err(());
        }
        self.data
            .read_bytes_at(self.offset.checked_add(offset).ok_or(())?, size)
    }
    fn read_bytes_at_until(self, range: Range<u64>, delimiter: u8) -> Result<&'a [u8], ()> {
        if range.start > range.end || range.end > self.size {
            return Err(());
        }
        self.data.read_bytes_at_until(
            self.offset.checked_add(range.start).ok_or(())?
                ..self.offset.checked_add(range.end).ok_or(())?,
            delimiter,
        )
    }
}
fn object_info<'a>(data: impl ReadRef<'a>) -> Option<BinaryInfo> {
    let object = object::File::parse(data).ok()?;
    let embedded = object.has_debug_symbols();
    let debug_symbols = if embedded {
        "Embedded debug information".into()
    } else if let Ok(Some(pdb)) = object.pdb_info() {
        format!(
            "External PDB reference: {}",
            String::from_utf8_lossy(pdb.path())
        )
    } else if let Ok(Some((name, _))) = object.gnu_debuglink() {
        format!(
            "External debug file reference: {}",
            String::from_utf8_lossy(name)
        )
    } else {
        "No embedded debug information".into()
    };
    Some(BinaryInfo {
        format: format!("{:?}", object.format()),
        architecture: format!("{:?}", object.architecture()),
        debug_symbols,
    })
}
fn binary_info(path: &Path) -> Option<BinaryInfo> {
    let cache = ReadCache::new(File::open(path).ok()?);
    let budget = Cell::new(16 * 1024 * 1024);
    let data = BoundedRead {
        cache: &cache,
        budget: &budget,
    };
    let kind = object::FileKind::parse(data).ok()?;
    let ranges: Vec<_> = match kind {
        object::FileKind::MachOFat32 => object::read::macho::MachOFatFile32::parse(data)
            .ok()?
            .arches()
            .iter()
            .map(FatArch::file_range)
            .collect(),
        object::FileKind::MachOFat64 => object::read::macho::MachOFatFile64::parse(data)
            .ok()?
            .arches()
            .iter()
            .map(FatArch::file_range)
            .collect(),
        _ => return object_info(data),
    };
    let infos: Vec<_> = ranges
        .into_iter()
        .filter_map(|(offset, size)| {
            // The outer reader has already bounded the fat header. Each slice uses
            // the same budget, and the range shares its immutable backing cache.
            if offset.checked_add(size)? > data.len().ok()? {
                return None;
            }
            object_info(SliceRead { data, offset, size })
        })
        .collect();
    if infos.is_empty() {
        return Some(BinaryInfo {
            format: "Universal Mach-O".into(),
            architecture: "Multiple architectures".into(),
            debug_symbols: "Debug information unavailable".into(),
        });
    }
    Some(BinaryInfo {
        format: "Universal Mach-O".into(),
        architecture: infos
            .iter()
            .map(|i| i.architecture.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        debug_symbols: infos
            .iter()
            .map(|i| format!("{}: {}", i.architecture, i.debug_symbols))
            .collect::<Vec<_>>()
            .join("; "),
    })
}
fn git_info(root: &Path, path: &Path, directory: bool) -> Option<String> {
    if directory {
        return None;
    }
    let relative = path.strip_prefix(root).ok()?;
    let output = Command::new("git")
        .arg("--literal-pathspecs")
        .arg("-C")
        .arg(root)
        .args([
            "status",
            "--porcelain=v1",
            "-z",
            "--ignored",
            "--untracked-files=all",
            "--",
        ])
        .arg(relative)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let status = output.stdout.get(..2);
    match status {
        Some(b"??") => Some("Untracked".into()),
        Some(b"!!") => Some("Ignored".into()),
        Some(pair) => {
            let label = |code| match code {
                b'M' => "modified",
                b'A' => "added",
                b'D' => "deleted",
                b'R' => "renamed",
                b'C' => "copied",
                b'U' => "conflicted",
                b'T' => "type changed",
                _ => "changed",
            };
            if pair.contains(&b'U') || matches!(pair, b"AA" | b"DD") {
                return Some("Merge conflict".into());
            }
            let mut parts = Vec::new();
            if pair[0] != b' ' {
                parts.push(format!("Staged: {}", label(pair[0])));
            }
            if pair[1] != b' ' {
                parts.push(format!("Working tree: {}", label(pair[1])));
            }
            Some(parts.join(" · "))
        }
        None => {
            let tracked = Command::new("git")
                .arg("--literal-pathspecs")
                .arg("-C")
                .arg(root)
                .args(["ls-files", "--error-unmatch", "--"])
                .arg(relative)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .ok()?;
            tracked.success().then(|| "Tracked · clean".into())
        }
    }
}
