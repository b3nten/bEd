//! Image references are resolved and decoded on one latest-only worker per panel.
//! The worker retains at most 64 MiB of decoded source pixels and produces a
//! separately bounded 64 MiB atlas. Packing briefly owns both; source pixels are
//! released before the result reaches the UI thread.
use crate::gpu::AtlasGpu;
use bed_plugin_image::{DecodedImage, decode_preview};
use bed_remote::{RemoteClient, Request, Response};
use bed_workbench_api::{
    Revision, TextureHandle,
    gpu::{GpuContext, RenderOutput, RenderTarget},
};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

const MAX_SOURCE_BYTES: usize = 128 * 1024 * 1024;
const MAX_PIXELS_BYTES: usize = 64 * 1024 * 1024;
const MAX_ATLAS_SIDE: u32 = 4096;

/// Decode a URL path/fragment without treating '+' as a space.
pub fn percent_decode(value: &str) -> Result<String, String> {
    let mut decoded = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'%' {
            let digit = |v: u8| (v as char).to_digit(16).map(|v| v as u8);
            let high = bytes.get(cursor + 1).copied().and_then(digit);
            let low = bytes.get(cursor + 2).copied().and_then(digit);
            let (Some(high), Some(low)) = (high, low) else {
                return Err("Invalid percent escape in link".into());
            };
            decoded.push(high * 16 + low);
            cursor += 3;
        } else {
            decoded.push(bytes[cursor]);
            cursor += 1;
        }
    }
    if decoded.contains(&0) {
        return Err("Links cannot contain NUL characters".into());
    }
    String::from_utf8(decoded).map_err(|_| "Link path is not valid UTF-8".into())
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.file_name().is_some_and(|name| name != "..") {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push("..");
                }
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

/// Resolve a filesystem link against the document directory. URL schemes and
/// protocol-relative URLs deliberately remain outside the filesystem loader.
pub fn resolve_link(document_path: &str, target: &str) -> Result<String, String> {
    let path = target.split(['?', '#']).next().unwrap_or_default();
    let decoded = percent_decode(path)?;
    if decoded.starts_with("//")
        || decoded
            .split('/')
            .next()
            .is_some_and(|part| part.contains(':'))
    {
        return Err("This link is not a supported filesystem path".into());
    }
    if decoded.is_empty() {
        return Ok(normalize(Path::new(document_path))
            .to_string_lossy()
            .into_owned());
    }
    let path = Path::new(&decoded);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(document_path)
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(path)
    };
    Ok(normalize(&joined).to_string_lossy().into_owned())
}

#[derive(Clone)]
struct Context {
    revision: Revision,
    document_path: Option<String>,
    root: String,
    remote: Option<RemoteClient>,
    sources: Vec<String>,
}
impl Context {
    fn matches(
        &self,
        revision: Revision,
        document_path: Option<&str>,
        root: &str,
        remote: &Option<RemoteClient>,
        sources: &[String],
    ) -> bool {
        self.revision == revision
            && self.document_path.as_deref() == document_path
            && self.root == root
            && self.sources == sources
            && match (&self.remote, remote) {
                (None, None) => true,
                (Some(a), Some(b)) => a.same_connection(b),
                _ => false,
            }
    }
}

#[derive(Clone, Copy, Debug)]
struct ImageRect {
    size: [u32; 2],
    uv0: [f32; 2],
    uv1: [f32; 2],
}
enum Entry {
    Ready(ImageRect),
    Error(String),
}
pub enum AssetState<'a> {
    Loading,
    Error(&'a str),
    Ready {
        size: [u32; 2],
        uv0: [f32; 2],
        uv1: [f32; 2],
    },
}
struct AtlasPixels {
    size: [u32; 2],
    rgba: Arc<[u8]>,
}
struct LoadResult {
    generation: u64,
    entries: HashMap<String, Entry>,
    atlas: Option<AtlasPixels>,
}
struct Job {
    generation: u64,
    context: Context,
}
#[derive(Default)]
struct WorkerState {
    pending: Option<Job>,
    result: Option<LoadResult>,
    stopped: bool,
}
struct SharedWorker {
    state: Mutex<WorkerState>,
    ready: Condvar,
    generation: AtomicU64,
}
struct Worker {
    shared: Arc<SharedWorker>,
}
impl Worker {
    fn new() -> Result<Self, String> {
        let shared = Arc::new(SharedWorker {
            state: Mutex::new(WorkerState::default()),
            ready: Condvar::new(),
            generation: AtomicU64::new(0),
        });
        let thread_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("bed-markdown-images".into())
            .spawn(move || {
                loop {
                    let job = {
                        let mut state = thread_shared.state.lock().unwrap();
                        while state.pending.is_none() && !state.stopped {
                            state = thread_shared.ready.wait(state).unwrap();
                        }
                        if state.stopped {
                            break;
                        }
                        state.pending.take().unwrap()
                    };
                    let current =
                        || thread_shared.generation.load(Ordering::Acquire) == job.generation;
                    if let Some(result) = load_assets(&job, &current) {
                        let mut state = thread_shared.state.lock().unwrap();
                        if current() && !state.stopped {
                            state.result = Some(result);
                        }
                    }
                }
            })
            .map_err(|error| format!("Could not start Markdown image worker: {error}"))?;
        Ok(Self { shared })
    }
    fn submit(&self, job: Job) {
        self.shared
            .generation
            .store(job.generation, Ordering::Release);
        let mut state = self.shared.state.lock().unwrap();
        state.pending = Some(job);
        state.result = None;
        self.shared.ready.notify_one();
    }
    fn take_result(&self) -> Option<LoadResult> {
        self.shared.state.lock().unwrap().result.take()
    }
    fn close(&self) {
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        let mut state = self.shared.state.lock().unwrap();
        state.stopped = true;
        state.pending = None;
        state.result = None;
        self.shared.ready.notify_one();
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.close();
    }
}

pub struct AssetManager {
    worker: Worker,
    context: Option<Context>,
    generation: u64,
    version: u64,
    entries: HashMap<String, Entry>,
    atlas: Option<AtlasPixels>,
    gpu: Option<AtlasGpu>,
    texture: TextureHandle,
}
impl AssetManager {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            worker: Worker::new()?,
            context: None,
            generation: 0,
            version: 0,
            entries: HashMap::new(),
            atlas: None,
            gpu: None,
            texture: TextureHandle::next(),
        })
    }
    /// Cheap for an unchanged source context. A Markdown revision also reloads
    /// referenced files, so path assets cannot stick across document contexts.
    pub fn sync(
        &mut self,
        revision: Revision,
        document_path: Option<&str>,
        project_root: &str,
        remote: Option<RemoteClient>,
        sources: &[String],
    ) {
        let document_path = document_path.filter(|path| !path.is_empty());
        if self.context.as_ref().is_some_and(|previous| {
            previous.matches(revision, document_path, project_root, &remote, sources)
        }) {
            return;
        }
        let context = Context {
            revision,
            document_path: document_path.map(str::to_owned),
            root: project_root.into(),
            remote,
            sources: sources.to_vec(),
        };
        self.submit(context);
    }
    fn submit(&mut self, context: Context) {
        self.generation = self.generation.wrapping_add(1);
        self.version = self.version.wrapping_add(1);
        self.entries.clear();
        self.atlas = None;
        self.gpu = None;
        self.worker.submit(Job {
            generation: self.generation,
            context: context.clone(),
        });
        self.context = Some(context);
    }
    /// Explicit reload for images changed independently of the Markdown source.
    pub fn refresh(&mut self) {
        if let Some(context) = self.context.clone() {
            self.submit(context);
        }
    }
    pub fn close(&mut self) {
        self.worker.close();
        self.context = None;
        self.entries.clear();
        self.atlas = None;
        self.gpu = None;
        self.version = self.version.wrapping_add(1);
    }
    pub fn poll(&mut self) {
        if let Some(result) = self.worker.take_result()
            && result.generation == self.generation
        {
            self.entries = result.entries;
            self.atlas = result.atlas;
            self.gpu = None;
            self.version = self.version.wrapping_add(1);
        }
    }
    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn texture_handle(&self) -> TextureHandle {
        self.texture
    }
    pub fn get(&self, source: &str) -> AssetState<'_> {
        match self.entries.get(source) {
            Some(Entry::Ready(rect)) => AssetState::Ready {
                size: rect.size,
                uv0: rect.uv0,
                uv1: rect.uv1,
            },
            Some(Entry::Error(error)) => AssetState::Error(error),
            None => AssetState::Loading,
        }
    }
    pub fn render_output(&self) -> Option<RenderOutput> {
        self.atlas.as_ref().map(|atlas| RenderOutput {
            handle: self.texture,
            size: atlas.size,
            depth: false,
            revision: self.version,
        })
    }
    pub fn render(
        &mut self,
        gpu: &mut GpuContext<'_>,
        target: &RenderTarget,
    ) -> Result<(), String> {
        let atlas = self
            .atlas
            .as_ref()
            .ok_or("Markdown image atlas is unavailable")?;
        if target.size != atlas.size {
            return Err("Markdown image atlas target changed size".into());
        }
        if self
            .gpu
            .as_ref()
            .is_none_or(|renderer| renderer.generation != gpu.generation)
        {
            self.gpu = Some(AtlasGpu::new(gpu, atlas.size, &atlas.rgba)?);
        }
        self.gpu.as_ref().unwrap().render(gpu, target);
        Ok(())
    }
}

fn read_local(path: &str, current: &impl Fn() -> bool) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("Image reference is not a regular file".into());
    }
    if metadata.len() > MAX_SOURCE_BYTES as u64 {
        return Err("Image source exceeds the 128 MiB file limit".into());
    }
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("Image reference is not a regular file".into());
    }
    let mut bytes = Vec::new();
    let mut chunk = vec![0; bed_remote::TRANSFER_CHUNK_BYTES];
    loop {
        if !current() {
            return Err("Image loading superseded".into());
        }
        let count = file.read(&mut chunk).map_err(|error| error.to_string())?;
        if count == 0 {
            return Ok(bytes);
        }
        if bytes.len() + count > MAX_SOURCE_BYTES {
            return Err("Image source exceeds the 128 MiB file limit".into());
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
}

fn read_remote(
    call: &impl Fn(Request) -> Result<Response, String>,
    root: &str,
    path: &str,
    current: &impl Fn() -> bool,
) -> Result<Vec<u8>, String> {
    let root_path = normalize(Path::new(root));
    let path = normalize(Path::new(path));
    let relative = path
        .strip_prefix(&root_path)
        .map_err(|_| "Image path is outside the SSH workspace")?;
    // Resolve in-workspace symlinks using the remote capability check, then use
    // bounded regular-file reads rather than receiving a potentially huge frame.
    let Response::Path { path } = call(Request::Canonicalize {
        root: root.into(),
        path: relative.to_string_lossy().into_owned(),
        allow_missing: false,
    })?
    else {
        return Err("Unexpected SSH image path response".into());
    };
    let relative = Path::new(&path)
        .strip_prefix(&root_path)
        .map_err(|_| "Image path is outside the SSH workspace")?
        .to_string_lossy()
        .into_owned();
    let mut bytes = Vec::new();
    loop {
        if !current() {
            return Err("Image loading superseded".into());
        }
        let Response::FileChunk { bytes: chunk } = call(Request::ReadFileChunk {
            root: root.into(),
            path: relative.clone(),
            offset: bytes.len() as u64,
            max_bytes: bed_remote::TRANSFER_CHUNK_BYTES.min(MAX_SOURCE_BYTES + 1 - bytes.len()),
        })?
        else {
            return Err("Unexpected SSH image file response".into());
        };
        if chunk.is_empty() {
            return Ok(bytes);
        }
        if bytes.len().saturating_add(chunk.len()) > MAX_SOURCE_BYTES {
            return Err("Image source exceeds the 128 MiB file limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
}

struct LoadedImage {
    sources: Vec<String>,
    decoded: DecodedImage,
}
fn load_assets(job: &Job, current: &impl Fn() -> bool) -> Option<LoadResult> {
    let mut result = LoadResult {
        generation: job.generation,
        entries: HashMap::new(),
        atlas: None,
    };
    let context = &job.context;
    let document = match context.document_path.as_deref() {
        Some(document) if Path::new(document).is_absolute() => document.to_owned(),
        Some(document) => Path::new(&context.root)
            .join(document)
            .to_string_lossy()
            .into_owned(),
        None => Path::new(&context.root)
            .join("untitled.md")
            .to_string_lossy()
            .into_owned(),
    };
    let mut seen = HashSet::new();
    let mut by_path = HashMap::<String, usize>::new();
    let mut images = Vec::<LoadedImage>::new();
    let mut decoded_bytes = 0usize;
    for source in &context.sources {
        if !current() {
            return None;
        }
        if !seen.insert(source) {
            continue;
        }
        let path = match resolve_link(&document, source) {
            Ok(path) => path,
            Err(error) => {
                result.entries.insert(source.clone(), Entry::Error(error));
                continue;
            }
        };
        if let Some(&index) = by_path.get(&path) {
            images[index].sources.push(source.clone());
            continue;
        }
        let decoded = (|| {
            let bytes = if let Some(remote) = &context.remote {
                read_remote(
                    &|request| remote.call(request).map_err(|error| error.to_string()),
                    &context.root,
                    &path,
                    current,
                )?
            } else {
                read_local(&path, current)?
            };
            if !current() {
                return Err("Image loading superseded".into());
            }
            let decoded = decode_preview(&bytes, &path)?;
            if decoded.size.contains(&0) || decoded.size.iter().any(|&side| side > MAX_ATLAS_SIDE) {
                return Err(
                    "Image is too large for Markdown preview (maximum 4096 pixels per side)".into(),
                );
            }
            if decoded_bytes.saturating_add(decoded.rgba.len()) > MAX_PIXELS_BYTES {
                return Err("Markdown preview images exceed the combined 64 MiB limit".into());
            }
            Ok(decoded)
        })();
        if !current() {
            return None;
        }
        match decoded {
            Ok(decoded) => {
                decoded_bytes += decoded.rgba.len();
                by_path.insert(path, images.len());
                images.push(LoadedImage {
                    sources: vec![source.clone()],
                    decoded,
                });
            }
            Err(error) => {
                result.entries.insert(source.clone(), Entry::Error(error));
            }
        }
    }
    if !images.is_empty() {
        let (atlas, entries) = build_atlas(images);
        result.atlas = atlas;
        result.entries.extend(entries);
    }
    current().then_some(result)
}

/// Rectangles are shelf-packed by descending height. Try several widths to
/// avoid allocating a 4096-pixel-wide texture for a single small illustration.
fn pack_rectangles(sizes: &[[u32; 2]]) -> ([u32; 2], Vec<Option<[u32; 2]>>) {
    let mut order: Vec<_> = (0..sizes.len()).collect();
    order.sort_by_key(|&index| std::cmp::Reverse(sizes[index][1]));
    let max_width = sizes.iter().map(|size| size[0]).max().unwrap_or(1).max(1);
    let mut width = max_width.next_power_of_two().min(MAX_ATLAS_SIDE);
    let pack = |width| {
        let (mut x, mut y, mut row_height) = (0u32, 0u32, 0u32);
        let mut positions = vec![None; sizes.len()];
        for &index in &order {
            let [w, h] = sizes[index];
            if w == 0 || h == 0 || w > width || h > MAX_ATLAS_SIDE {
                continue;
            }
            if x + w > width {
                y += row_height;
                x = 0;
                row_height = 0;
            }
            if y + h > MAX_ATLAS_SIDE {
                continue;
            }
            positions[index] = Some([x, y]);
            x += w;
            row_height = row_height.max(h);
        }
        ([width, (y + row_height).max(1)], positions)
    };
    let mut best = None;
    loop {
        let candidate = pack(width);
        if candidate.1.iter().all(Option::is_some)
            && best
                .as_ref()
                .is_none_or(|(size, _): &([u32; 2], Vec<Option<[u32; 2]>>)| {
                    u64::from(candidate.0[0]) * u64::from(candidate.0[1])
                        < u64::from(size[0]) * u64::from(size[1])
                })
        {
            best = Some(candidate);
        }
        if width == MAX_ATLAS_SIDE {
            break;
        }
        width = (width * 2).min(MAX_ATLAS_SIDE);
    }
    best.unwrap_or_else(|| pack(MAX_ATLAS_SIDE))
}

fn build_atlas(images: Vec<LoadedImage>) -> (Option<AtlasPixels>, HashMap<String, Entry>) {
    let sizes: Vec<_> = images.iter().map(|image| image.decoded.size).collect();
    let (size, positions) = pack_rectangles(&sizes);
    let mut entries = HashMap::new();
    let mut rgba = vec![0u8; size[0] as usize * size[1] as usize * 4];
    let mut any = false;
    for (image, position) in images.into_iter().zip(positions) {
        let Some([x, y]) = position else {
            for source in image.sources {
                entries.insert(
                    source,
                    Entry::Error(
                        "Images do not fit within the Markdown preview image limit".into(),
                    ),
                );
            }
            continue;
        };
        any = true;
        let [width, height] = image.decoded.size;
        for row in 0..height as usize {
            let from = row * width as usize * 4;
            let to = ((y as usize + row) * size[0] as usize + x as usize) * 4;
            rgba[to..to + width as usize * 4]
                .copy_from_slice(&image.decoded.rgba[from..from + width as usize * 4]);
        }
        // Half-texel insets keep linear ImGui sampling inside this image and
        // prevent colors from neighboring atlas entries bleeding at the edges.
        let rect = ImageRect {
            size: [width, height],
            uv0: [
                (x as f32 + 0.5) / size[0] as f32,
                (y as f32 + 0.5) / size[1] as f32,
            ],
            uv1: [
                (x as f32 + width as f32 - 0.5) / size[0] as f32,
                (y as f32 + height as f32 - 0.5) / size[1] as f32,
            ],
        };
        for source in image.sources {
            entries.insert(source, Entry::Ready(rect));
        }
    }
    (
        any.then(|| AtlasPixels {
            size,
            rgba: rgba.into(),
        }),
        entries,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_resolve_relative_absolute_encoded_and_parent_paths() {
        assert_eq!(
            resolve_link(
                "/workspace/docs/guide.md",
                "../images/hello%20world.png?raw=1#figure"
            )
            .unwrap(),
            "/workspace/images/hello world.png"
        );
        assert_eq!(
            resolve_link("/workspace/guide.md", "/images/a.png").unwrap(),
            "/images/a.png"
        );
        assert_eq!(
            resolve_link("docs/guide.md", "../../a.png").unwrap(),
            "../a.png"
        );
        assert_eq!(
            resolve_link("/guide.md", "../../../a.png").unwrap(),
            "/a.png"
        );
        assert_eq!(
            resolve_link("/guide.md", "%23literal.png").unwrap(),
            "/#literal.png"
        );
        assert_eq!(
            percent_decode("hello+world%20%C3%A9").unwrap(),
            "hello+world é"
        );
    }

    #[test]
    fn url_and_invalid_path_inputs_do_not_reach_the_filesystem() {
        for target in [
            "https://host/p.png",
            "https%3A//host/p.png",
            "data:image/png;base64,AA",
            "//host/p.png",
            "%2F%2Fhost/p.png",
            "%00.png",
            "%XX.png",
            "%FF.png",
        ] {
            assert!(resolve_link("/guide.md", target).is_err(), "{target}");
        }
    }

    #[test]
    fn atlas_packing_fits_limits_and_rejects_excess() {
        let (size, positions) = pack_rectangles(&[[4096, 4096], [1, 1]]);
        assert_eq!(size, [4096, 4096]);
        assert_eq!(positions, [Some([0, 0]), None]);
        let (size, positions) = pack_rectangles(&[[10, 10], [10, 10]]);
        assert!(positions.iter().all(Option::is_some));
        assert!(u64::from(size[0]) * u64::from(size[1]) * 4 <= MAX_PIXELS_BYTES as u64);
        assert!(size[0] < MAX_ATLAS_SIDE);
        let (_, positions) = pack_rectangles(&[[4097, 1], [0, 1]]);
        assert!(positions.iter().all(Option::is_none));
    }

    #[test]
    fn atlas_copies_exact_pixels_and_shares_rectangles_for_aliases() {
        let (atlas, entries) = build_atlas(vec![LoadedImage {
            sources: vec!["a.png".into(), "a.png#alias".into()],
            decoded: DecodedImage {
                size: [2, 1],
                rgba: Arc::from([255, 0, 0, 255, 0, 255, 0, 128]),
            },
        }]);
        let atlas = atlas.unwrap();
        assert_eq!(atlas.size, [2, 1]);
        assert_eq!(&*atlas.rgba, &[255, 0, 0, 255, 0, 255, 0, 128]);
        let Entry::Ready(a) = entries["a.png"] else {
            panic!()
        };
        let Entry::Ready(b) = entries["a.png#alias"] else {
            panic!()
        };
        assert_eq!(a.uv0, b.uv0);
        assert_eq!(a.uv1, b.uv1);
    }

    #[test]
    fn ssh_reads_resolve_in_workspace_and_use_bounded_chunks() {
        use std::cell::RefCell;
        let calls = RefCell::new(Vec::new());
        let bytes = read_remote(
            &|request| {
                calls.borrow_mut().push(request.clone());
                match request {
                    Request::Canonicalize { root, path, .. } => {
                        assert_eq!(root, "/workspace");
                        assert_eq!(path, "images/a.png");
                        Ok(Response::Path {
                            path: "/workspace/images/a.png".into(),
                        })
                    }
                    Request::ReadFileChunk {
                        root,
                        path,
                        offset,
                        max_bytes,
                    } => {
                        assert_eq!(root, "/workspace");
                        assert_eq!(path, "images/a.png");
                        assert!(max_bytes <= bed_remote::TRANSFER_CHUNK_BYTES);
                        Ok(Response::FileChunk {
                            bytes: if offset == 0 {
                                vec![1, 2, 3]
                            } else {
                                Vec::new()
                            },
                        })
                    }
                    _ => panic!("Unexpected request"),
                }
            },
            "/workspace",
            "/workspace/images/a.png",
            &|| true,
        )
        .unwrap();
        assert_eq!(bytes, [1, 2, 3]);
        assert_eq!(calls.borrow().len(), 3);
        assert!(
            read_remote(
                &|_| panic!("outside paths cannot issue IO"),
                "/workspace",
                "/outside/a.png",
                &|| true
            )
            .is_err()
        );
    }

    #[test]
    fn superseded_asset_jobs_stop_before_io() {
        let job = Job {
            generation: 1,
            context: Context {
                revision: (1, 1),
                document_path: Some("/does/not/exist.md".into()),
                root: "/".into(),
                remote: None,
                sources: vec!["picture.png".into()],
            },
        };
        assert!(load_assets(&job, &|| false).is_none());
    }

    #[test]
    fn local_images_reload_without_reusing_old_pixels_and_close_releases_output() {
        use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "bed-markdown-assets-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("images")).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        let image = root.join("images/picture.svg");
        let write_image = |width| {
            fs::write(&image, format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='{width}' height='2'><rect width='{width}' height='2' fill='red'/></svg>"
        )).unwrap()
        };
        write_image(3);
        let mut assets = AssetManager::new().unwrap();
        assets.sync(
            (1, 1),
            Some(root.join("guide.md").to_str().unwrap()),
            root.to_str().unwrap(),
            None,
            &["images/picture.svg".into(), "missing.svg".into()],
        );
        let wait = |assets: &mut AssetManager| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                assets.poll();
                if !matches!(assets.get("images/picture.svg"), AssetState::Loading) {
                    break;
                }
                assert!(Instant::now() < deadline, "Image worker timed out");
                thread::sleep(Duration::from_millis(1));
            }
        };
        wait(&mut assets);
        assert!(matches!(
            assets.get("images/picture.svg"),
            AssetState::Ready { size: [3, 2], .. }
        ));
        assert!(matches!(assets.get("missing.svg"), AssetState::Error(_)));
        let version = assets.version();
        write_image(4);
        assets.refresh();
        assert!(assets.render_output().is_none());
        assert!(matches!(
            assets.get("images/picture.svg"),
            AssetState::Loading
        ));
        wait(&mut assets);
        assert!(matches!(
            assets.get("images/picture.svg"),
            AssetState::Ready { size: [4, 2], .. }
        ));
        assert!(assets.version() > version);
        assets.close();
        assert!(assets.render_output().is_none());
        assert!(assets.gpu.is_none());
    }

    #[test]
    #[ignore = "requires a native GPU adapter; run with --ignored on desktop CI"]
    fn native_atlas_preserves_rgba_and_recreates_after_device_generation_change() {
        use bed_workbench_api::gpu::{renderer_device_descriptor, wgpu};
        use std::{future::Future, task::Wake};
        fn block_on<T>(future: impl Future<Output = T>) -> T {
            struct ThreadWake(thread::Thread);
            impl Wake for ThreadWake {
                fn wake(self: Arc<Self>) {
                    self.0.unpark();
                }
            }
            let waker = std::task::Waker::from(Arc::new(ThreadWake(thread::current())));
            let mut context = std::task::Context::from_waker(&waker);
            let mut future = std::pin::pin!(future);
            loop {
                match future.as_mut().poll(&mut context) {
                    std::task::Poll::Ready(value) => return value,
                    std::task::Poll::Pending => thread::park(),
                }
            }
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
        let (device, queue) =
            block_on(adapter.request_device(&renderer_device_descriptor(&adapter))).unwrap();
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut assets = AssetManager::new().unwrap();
        let pixels: Arc<[u8]> = Arc::from([255, 128, 0, 128, 16, 32, 64, 255]);
        assets.atlas = Some(AtlasPixels {
            size: [2, 1],
            rgba: Arc::clone(&pixels),
        });
        let target = RenderTarget::new(&device, assets.render_output().unwrap()).unwrap();
        for generation in [1, 2] {
            let mut encoder = device.create_command_encoder(&Default::default());
            assets
                .render(
                    &mut GpuContext {
                        instance: &instance,
                        adapter: &adapter,
                        device: &device,
                        queue: &queue,
                        encoder: &mut encoder,
                        error_handlers: None,
                        generation,
                    },
                    &target,
                )
                .unwrap();
            assert_eq!(assets.gpu.as_ref().unwrap().generation, generation);
            queue.submit([encoder.finish()]);
        }
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Markdown atlas readback"),
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(1),
                },
            },
            target.texture.size(),
        );
        queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                sender.send(result).unwrap();
            });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(10)),
            })
            .unwrap();
        receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .unwrap();
        let readback = buffer.slice(..).get_mapped_range();
        assert_eq!(&readback[..8], &*pixels);
        drop(readback);
        buffer.unmap();
        assert!(block_on(validation.pop()).is_none());
    }
}
