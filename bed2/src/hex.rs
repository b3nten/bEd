//! Disk-backed byte views. World sources own bounded shared caches; panels
//! own navigation and user jobs. All filesystem access runs on managed workers.
use bedtime_hex2::{CopyFormat, HexView};
use bedtime_ui2::{UiContext, egui};
use bedtime_workbench2::*;
use bevy::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, Metadata},
    io::{Read, Seek, SeekFrom},
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) const PAGE_BYTES: u64 = 64 * 1024;
const CACHE_PAGES: usize = 32;
const COPY_LIMIT: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}
impl Stamp {
    fn new(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            identity: (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Initial {
    stamp: Stamp,
    bytes: Arc<[u8]>,
}

#[derive(Debug)]
enum ReadError {
    Changed,
    Failed(String),
}
impl From<std::io::Error> for ReadError {
    fn from(error: std::io::Error) -> Self {
        Self::Failed(error.to_string())
    }
}
fn checked_file(path: &Path, stamp: &Stamp) -> Result<File, ReadError> {
    // Avoid blocking opens of FIFOs/devices if a path is replaced after probing.
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(ReadError::Failed("Not a regular file".into()));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() || Stamp::new(&file.metadata()?) != *stamp {
        return Err(ReadError::Changed);
    }
    Ok(file)
}
fn check_after(file: &File, path: &Path, stamp: &Stamp) -> Result<(), ReadError> {
    if Stamp::new(&file.metadata()?) != *stamp || Stamp::new(&fs::metadata(path)?) != *stamp {
        return Err(ReadError::Changed);
    }
    Ok(())
}
fn read_range(path: &Path, stamp: &Stamp, range: Range<u64>) -> Result<Vec<u8>, ReadError> {
    let mut file = checked_file(path, stamp)?;
    file.seek(SeekFrom::Start(range.start))?;
    let mut bytes = vec![0; (range.end - range.start) as usize];
    file.read_exact(&mut bytes).map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            ReadError::Changed
        } else {
            e.into()
        }
    })?;
    check_after(&file, path, stamp)?;
    Ok(bytes)
}
pub(crate) fn prepare(path: &Path) -> Result<Arc<Initial>, String> {
    let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Hex requires a regular file".into());
    }
    let stamp = Stamp::new(&metadata);
    let bytes = read_range(path, &stamp, 0..stamp.len.min(PAGE_BYTES)).map_err(error_text)?;
    Ok(Arc::new(Initial {
        stamp,
        bytes: bytes.into(),
    }))
}
fn error_text(error: ReadError) -> String {
    match error {
        ReadError::Changed => "File changed during reading. Reload to refresh.".into(),
        ReadError::Failed(message) => message,
    }
}
struct PageJob {
    page: u64,
    task: ManagedTask<Result<Vec<u8>, ReadError>>,
}
struct Source {
    stamp: Stamp,
    pages: BTreeMap<u64, Arc<[u8]>>,
    job: Option<PageJob>,
    error: Option<String>,
}
impl Drop for Source {
    fn drop(&mut self) {
        if let Some(job) = &self.job {
            job.task.cancel();
        }
    }
}
impl Source {
    fn byte(&self, offset: u64) -> Option<u8> {
        self.pages
            .get(&(offset / PAGE_BYTES))?
            .get((offset % PAGE_BYTES) as usize)
            .copied()
    }
}
#[derive(Resource, Default)]
struct Sources(BTreeMap<PathBuf, Source>);
struct UserJob<T: Send + 'static> {
    stamp: Stamp,
    task: ManagedTask<Result<T, ReadError>>,
}
#[derive(Component, Default)]
pub(crate) struct HexPreview {
    pub view: HexView,
    needed: Range<u64>,
    goto: String,
    show_goto: bool,
    focus_goto: bool,
    query: String,
    text_search: bool,
    show_find: bool,
    focus_find: bool,
    find: Option<UserJob<Option<Range<u64>>>>,
    copy: Option<UserJob<String>>,
    message: Option<String>,
    inspector: bool,
    big_endian: bool,
}
impl Drop for HexPreview {
    fn drop(&mut self) {
        if let Some(job) = &self.find {
            job.task.cancel();
        }
        if let Some(job) = &self.copy {
            job.task.cancel();
        }
    }
}
pub(crate) fn accept(world: &mut World, panel: Entity, path: &Path, initial: &Initial) {
    if world.get_resource::<Sources>().is_none() {
        world.init_resource::<Sources>();
    }
    let mut sources = world.resource_mut::<Sources>();
    if sources
        .0
        .get(path)
        .is_none_or(|s| s.stamp != initial.stamp || s.error.is_some())
    {
        sources.0.insert(
            path.to_owned(),
            Source {
                stamp: initial.stamp.clone(),
                pages: BTreeMap::from([(0, initial.bytes.clone())]),
                job: None,
                error: None,
            },
        );
    }
    if world.get::<HexPreview>(panel).is_none() {
        world.entity_mut(panel).insert(HexPreview::default());
    }
    world
        .get_mut::<HexPreview>(panel)
        .unwrap()
        .view
        .clamp(initial.stamp.len);
}

pub(crate) fn draw(ui: &mut egui::Ui, world: &mut World, _: &UiContext<'_>, panel: Entity) {
    let Some(path) = crate::files::panel_path(world, panel) else {
        return;
    };
    let dirty = crate::file_editor::disk_view_has_unsaved(world, &path);
    let Some(mut state) = world.entity_mut(panel).take::<HexPreview>() else {
        return;
    };
    let Some(source) = world.get_resource::<Sources>().and_then(|s| s.0.get(&path)) else {
        world.entity_mut(panel).insert(state);
        return;
    };
    let stamp = source.stamp.clone();
    let len = stamp.len;
    let mut reload = false;
    if dirty {
        ui.weak("Showing file on disk. The text editor has unsaved changes.");
    }
    if let Some(error) = &source.error {
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(ui.visuals().error_fg_color, error);
            reload = ui.button("Reload").clicked();
        });
    }
    let mut search = None;
    let mut copy = None;
    if state.show_goto {
        ui.horizontal(|ui| {
            ui.label("Offset");
            let edit = ui.add(
                egui::TextEdit::singleline(&mut state.goto)
                    .desired_width(170.0)
                    .hint_text("Decimal or 0x…"),
            );
            if state.focus_goto {
                edit.request_focus();
                state.focus_goto = false;
            }
            if ui.button("Go").clicked()
                || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            {
                match bedtime_hex2::parse_offset(&state.goto) {
                    Ok(offset) if offset < len || (len == 0 && offset == 0) => {
                        state.view.go_to(offset, len);
                        state.message = None;
                    }
                    Ok(_) => state.message = Some("Offset is beyond the end of the file".into()),
                    Err(error) => state.message = Some(error),
                }
            }
            if ui.small_button("Close").clicked() {
                state.show_goto = false;
            }
        });
    }
    if state.show_find {
        ui.horizontal_wrapped(|ui| {
            ui.label("Find");
            let edit = ui.add(egui::TextEdit::singleline(&mut state.query).desired_width(190.0));
            if state.focus_find {
                edit.request_focus();
                state.focus_find = false;
            }
            let mode = ui.checkbox(&mut state.text_search, "UTF-8 text");
            if edit.changed() || mode.changed() {
                if let Some(job) = state.find.take() {
                    job.task.cancel();
                }
                state.message = None;
            }
            if ui.button("Previous").clicked() {
                search = Some(false);
            }
            if ui.button("Next").clicked()
                || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            {
                search = Some(true);
            }
            if state.find.is_some() && ui.button("Cancel").clicked() {
                if let Some(job) = state.find.take() {
                    job.task.cancel();
                }
                state.message = Some("Search cancelled".into());
            }
            if ui.small_button("Close").clicked() {
                state.show_find = false;
                if let Some(job) = state.find.take() {
                    job.task.cancel();
                }
            }
        });
    }
    if let Some(message) = &state.message {
        ui.weak(message);
    }
    if state.inspector {
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut state.big_endian, "Big endian");
            let bytes: Option<Vec<u8>> = (0..8)
                .map(|i| {
                    state
                        .view
                        .cursor()
                        .checked_add(i)
                        .and_then(|o| source.byte(o))
                })
                .collect();
            if let Some(mut bytes) = bytes {
                if state.big_endian {
                    bytes[..2].reverse();
                }
                let short: [u8; 2] = bytes[..2].try_into().unwrap();
                if state.big_endian {
                    bytes[..2].reverse();
                    bytes[..4].reverse();
                }
                let word: [u8; 4] = bytes[..4].try_into().unwrap();
                if state.big_endian {
                    bytes[..4].reverse();
                    bytes.reverse();
                }
                let array: [u8; 8] = bytes.try_into().unwrap();
                ui.monospace(format!(
                    "u16 {}  i16 {}  u32 {}  i32 {}  u64 {}  i64 {}  f32 {:?}  f64 {:?}",
                    u16::from_le_bytes(short),
                    i16::from_le_bytes(short),
                    u32::from_le_bytes(word),
                    i32::from_le_bytes(word),
                    u64::from_le_bytes(array),
                    i64::from_le_bytes(array),
                    f32::from_le_bytes(word),
                    f64::from_le_bytes(array)
                ));
            } else {
                ui.weak("Eight loaded bytes required at cursor");
            }
        });
    }
    let selection = state.view.selection(len);
    ui.horizontal_wrapped(|ui| {
        ui.weak(format!(
            "{len} bytes  ·  Offset 0x{:X}  ·  {} selected  ·  Read-only",
            state.view.cursor(),
            selection.end - selection.start
        ));
        if ui.small_button("Go to…").clicked() {
            state.show_goto = !state.show_goto;
            state.focus_goto = state.show_goto;
        }
        if ui.small_button("Find…").clicked() {
            state.show_find = !state.show_find;
            state.focus_find = state.show_find;
        }
        ui.checkbox(&mut state.inspector, "Inspector");
        if state.find.is_some() || state.copy.is_some() {
            ui.spinner();
        }
    });
    let output = state
        .view
        .show(ui, panel, len, |offset| source.byte(offset));
    state.needed = output.needed;
    state.show_find |= output.find;
    state.focus_find |= output.find;
    state.show_goto |= output.goto;
    state.focus_goto |= output.goto;
    copy = copy.or(output.copy);
    output.response.context_menu(|ui| {
        for (label, format) in [
            ("Copy hex", CopyFormat::Hex),
            ("Copy text (UTF-8, replace invalid bytes)", CopyFormat::Text),
            ("Copy formatted rows", CopyFormat::Rows),
            ("Copy offset", CopyFormat::Offset),
        ] {
            if ui.button(label).clicked() {
                copy = Some(format);
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Go to offset…").clicked() {
            state.show_goto = true;
            state.focus_goto = true;
            ui.close();
        }
        if ui.button("Find…").clicked() {
            state.show_find = true;
            state.focus_find = true;
            ui.close();
        }
        crate::opening::viewer_menu(ui, world, panel);
    });
    world.entity_mut(panel).insert(state);
    if reload {
        let _ = crate::files::load_preview(world, panel, path.clone(), true);
    }
    if let Some(forward) = search {
        start_search(world, panel, path.clone(), stamp.clone(), forward);
    }
    if let Some(format) = copy {
        if format == CopyFormat::Offset {
            ui.ctx().copy_text(format!(
                "0x{:X}",
                world.get::<HexPreview>(panel).unwrap().view.cursor()
            ));
        } else {
            start_copy(world, panel, path, stamp, format);
        }
    }
}

fn start_search(world: &mut World, panel: Entity, path: PathBuf, stamp: Stamp, forward: bool) {
    let mut state = world.get_mut::<HexPreview>(panel).unwrap();
    if let Some(job) = state.find.take() {
        job.task.cancel();
    }
    let pattern = if state.text_search {
        if state.query.is_empty() {
            Err("Enter text to find".into())
        } else {
            Ok(state.query.as_bytes().to_vec())
        }
    } else {
        bedtime_hex2::parse_hex(&state.query)
    };
    let pattern = match pattern {
        Ok(p) if p.len() <= PAGE_BYTES as usize => p,
        Ok(_) => {
            state.message = Some("Search patterns are limited to 64 KiB".into());
            return;
        }
        Err(error) => {
            state.message = Some(error);
            return;
        }
    };
    let start = if forward {
        state.view.selection(stamp.len).end
    } else {
        state.view.selection(stamp.len).start
    };
    state.message = None;
    let capture = stamp.clone();
    let task = world.get_resource_mut::<ManagedWork>().unwrap().spawn_task(
        Some(panel),
        "Find bytes",
        move |cancel| {
            search_file(&path, &capture, &pattern, start, forward, &|| {
                cancel.is_cancelled()
            })
        },
    );
    let mut state = world.get_mut::<HexPreview>(panel).unwrap();
    match task {
        Ok(task) => state.find = Some(UserJob { stamp, task }),
        Err(error) => state.message = Some(error),
    }
}
fn search_file(
    path: &Path,
    stamp: &Stamp,
    pattern: &[u8],
    start: u64,
    forward: bool,
    cancelled: &impl Fn() -> bool,
) -> Result<Option<Range<u64>>, ReadError> {
    let mut file = checked_file(path, stamp)?;
    let start = start.min(stamp.len);
    let intervals = if forward {
        [start..stamp.len, 0..start]
    } else {
        [0..start, start..stamp.len]
    };
    for interval in intervals {
        // Split by match start, not by read boundary: matches may straddle the
        // cursor/wrap boundary as well as a page boundary.
        file.seek(SeekFrom::Start(interval.start))?;
        let read_end = interval
            .end
            .saturating_add(pattern.len() as u64 - 1)
            .min(stamp.len);
        let mut offset = interval.start;
        let mut carry = Vec::new();
        let mut last = None;
        while offset < read_end {
            if cancelled() {
                return Err(ReadError::Failed("Search cancelled".into()));
            }
            let count = (read_end - offset).min(PAGE_BYTES) as usize;
            let mut buffer = std::mem::take(&mut carry);
            let base = offset - buffer.len() as u64;
            let old = buffer.len();
            buffer.resize(old + count, 0);
            file.read_exact(&mut buffer[old..]).map_err(|e| {
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    ReadError::Changed
                } else {
                    e.into()
                }
            })?;
            for (index, window) in buffer.windows(pattern.len()).enumerate() {
                let at = base + index as u64;
                if at >= interval.end {
                    break;
                }
                if window == pattern {
                    last = Some(at..at + pattern.len() as u64);
                    if forward {
                        check_after(&file, path, stamp)?;
                        return Ok(last);
                    }
                }
            }
            let keep = (pattern.len() - 1).min(buffer.len());
            carry = buffer[buffer.len() - keep..].to_vec();
            offset += count as u64;
        }
        if last.is_some() {
            check_after(&file, path, stamp)?;
            return Ok(last);
        }
    }
    check_after(&file, path, stamp)?;
    Ok(None)
}
fn start_copy(world: &mut World, panel: Entity, path: PathBuf, stamp: Stamp, format: CopyFormat) {
    let mut state = world.get_mut::<HexPreview>(panel).unwrap();
    let range = state.view.selection(stamp.len);
    if range.end - range.start > COPY_LIMIT {
        state.message = Some("Copy is limited to 16 MiB of selected bytes".into());
        return;
    }
    if let Some(job) = state.copy.take() {
        job.task.cancel();
    }
    let capture = stamp.clone();
    let task = world.get_resource_mut::<ManagedWork>().unwrap().spawn_task(
        Some(panel),
        "Copy bytes",
        move |cancel| {
            if cancel.is_cancelled() {
                return Err(ReadError::Failed("Copy cancelled".into()));
            }
            let bytes = read_range(&path, &capture, range.clone())?;
            let mut text = String::new();
            use std::fmt::Write;
            match format {
                CopyFormat::Text => text = String::from_utf8_lossy(&bytes).into_owned(),
                CopyFormat::Hex => {
                    for (i, b) in bytes.iter().enumerate() {
                        if i > 0 {
                            text.push(' ');
                        }
                        write!(text, "{b:02X}").unwrap();
                    }
                }
                CopyFormat::Rows => {
                    for (i, row) in bytes.chunks(16).enumerate() {
                        write!(text, "{:08X}  ", range.start + i as u64 * 16).unwrap();
                        for j in 0..16 {
                            if j == 8 {
                                text.push(' ');
                            }
                            if let Some(b) = row.get(j) {
                                write!(text, "{b:02X} ").unwrap();
                            } else {
                                text.push_str("   ");
                            }
                        }
                        text.push(' ');
                        for b in row {
                            text.push(if b.is_ascii_graphic() || *b == b' ' {
                                *b as char
                            } else {
                                '.'
                            });
                        }
                        text.push('\n');
                    }
                }
                CopyFormat::Offset => unreachable!(),
            }
            Ok(text)
        },
    );
    let mut state = world.get_mut::<HexPreview>(panel).unwrap();
    match task {
        Ok(task) => state.copy = Some(UserJob { stamp, task }),
        Err(error) => state.message = Some(error),
    }
}

pub(crate) fn update(world: &mut World) {
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<HexPreview>>()
        .iter(world)
        .filter_map(|p| crate::files::panel_path(world, p).map(|path| (p, path)))
        .collect();
    let mut demand = BTreeMap::<PathBuf, BTreeSet<u64>>::new();
    for (panel, path) in &panels {
        let Some(stamp) = world
            .get_resource::<Sources>()
            .and_then(|s| s.0.get(path))
            .map(|s| s.stamp.clone())
        else {
            continue;
        };
        let mut state = world.entity_mut(*panel).take::<HexPreview>().unwrap();
        if state.find.as_ref().is_some_and(|j| j.stamp != stamp) {
            state.find.take().unwrap().task.cancel();
        }
        if state.copy.as_ref().is_some_and(|j| j.stamp != stamp) {
            state.copy.take().unwrap().task.cancel();
        }
        if let Some(result) = state.find.as_ref().and_then(|j| poll(&j.task)) {
            state.find = None;
            match result {
                Ok(Some(range)) => {
                    state.view.select(range, stamp.len);
                    state.message = None;
                }
                Ok(None) => state.message = Some("No match".into()),
                Err(error) => state.message = Some(error_text(error)),
            }
        }
        if let Some(result) = state.copy.as_ref().and_then(|j| poll(&j.task)) {
            state.copy = None;
            match result {
                Ok(text) => {
                    if let Some(mut clipboard) = world.get_resource_mut::<Clipboard>() {
                        if let Err(error) = clipboard.write_text(&text) {
                            state.message = Some(error.to_string());
                        }
                    }
                }
                Err(error) => state.message = Some(error_text(error)),
            }
        }
        let visible = world
            .get::<PanelPresentation>(*panel)
            .is_some_and(|p| p.visible);
        let wanted = demand.entry(path.clone()).or_default();
        if visible || state.view.rect.is_none() {
            let first = state.needed.start / PAGE_BYTES;
            let last = state.needed.end.saturating_sub(1) / PAGE_BYTES;
            // A viewport request is bounded independently of file length.
            for page in first
                ..=last
                    .min(first.saturating_add(3))
                    .saturating_add(1)
                    .min(stamp.len.saturating_sub(1) / PAGE_BYTES)
            {
                if stamp.len > 0 {
                    wanted.insert(page);
                }
            }
            if state.inspector && state.view.cursor() < stamp.len {
                wanted.insert(state.view.cursor() / PAGE_BYTES);
                if state.view.cursor().saturating_add(7) < stamp.len {
                    wanted.insert((state.view.cursor() + 7) / PAGE_BYTES);
                }
            }
        }
        world.entity_mut(*panel).insert(state);
    }
    if let Some(mut sources) = world.remove_resource::<Sources>() {
        let wanted = demand;
        sources.0.retain(|path, _| wanted.contains_key(path));
        for (path, source) in &mut sources.0 {
            let pages = &wanted[path];
            if let Some(result) = source.job.as_ref().and_then(|j| poll(&j.task)) {
                let page = source.job.take().unwrap().page;
                match result {
                    Ok(bytes) => {
                        source.pages.insert(page, bytes.into());
                    }
                    Err(error) => {
                        source.pages.clear();
                        source.error = Some(error_text(error));
                    }
                }
            }
            while source.pages.len() > CACHE_PAGES {
                let remove = source.pages.keys().copied().find(|p| !pages.contains(p));
                if let Some(page) = remove.or_else(|| source.pages.keys().next().copied()) {
                    source.pages.remove(&page);
                } else {
                    break;
                }
            }
            if source
                .job
                .as_ref()
                .is_some_and(|job| !pages.contains(&job.page))
            {
                source.job.take().unwrap().task.cancel();
            }
            if source.job.is_none()
                && source.error.is_none()
                && let Some(page) = pages
                    .iter()
                    .find(|p| !source.pages.contains_key(p))
                    .copied()
            {
                let capture = source.stamp.clone();
                let path = path.clone();
                if let Some(mut work) = world.get_resource_mut::<ManagedWork>()
                    && !work.is_closing()
                {
                    match work.spawn_task(None, "Read hex page", move |_| {
                        let start = page * PAGE_BYTES;
                        read_range(
                            &path,
                            &capture,
                            start..start.saturating_add(PAGE_BYTES).min(capture.len),
                        )
                    }) {
                        Ok(task) => source.job = Some(PageJob { page, task }),
                        Err(error) => source.error = Some(error),
                    }
                }
            }
        }
        world.insert_resource(sources);
    }
}
fn poll<T: Send + 'static>(
    task: &ManagedTask<Result<T, ReadError>>,
) -> Option<Result<T, ReadError>> {
    match task.try_recv() {
        Ok(result) => Some(result),
        Err(std::sync::mpsc::TryRecvError::Empty) => None,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            Some(Err(ReadError::Failed("Hex worker stopped".into())))
        }
    }
}

#[cfg(feature = "automation")]
pub(crate) fn inspect(world: &World, panel: Entity) -> Option<serde_json::Value> {
    let state = world.get::<HexPreview>(panel)?;
    let path = crate::files::panel_path(world, panel)?;
    let source = world.get_resource::<Sources>()?.0.get(&path)?;
    Some(
        serde_json::json!({"len":source.stamp.len,"cursor":state.view.cursor(),"selection":[state.view.selection(source.stamp.len).start,state.view.selection(source.stamp.len).end],"top_row":state.view.top_row,"rendered_rows":state.view.rendered_rows,"cached_pages":source.pages.len(),"cached_bytes":source.pages.values().map(|p|p.len()).sum::<usize>(),"rect":state.view.rect.map(|r|serde_json::json!({"min":[r.min.x,r.min.y],"max":[r.max.x,r.max.y]})),"message":state.message,"error":source.error,"searching":state.find.is_some(),"query":state.query}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "bed2-hex-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn path(&self) -> PathBuf {
            self.0.join("bytes.bin")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn sparse_file_loads_one_page_and_reads_exactly_beyond_four_gib() {
        let fixture = Fixture::new();
        let path = fixture.path();
        let mut file = File::create(&path).unwrap();
        let offset = (1_u64 << 32) + 37;
        file.set_len(offset + 3).unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();
        file.write_all(&[0x12, 0x34, 0x56]).unwrap();
        drop(file);
        let initial = prepare(&path).unwrap();
        assert_eq!(initial.stamp.len, offset + 3);
        assert_eq!(initial.bytes.len(), PAGE_BYTES as usize);
        assert_eq!(
            read_range(&path, &initial.stamp, offset..offset + 3).unwrap(),
            [0x12, 0x34, 0x56]
        );
    }
    #[test]
    fn source_replacement_and_truncation_reject_old_page_reads() {
        let fixture = Fixture::new();
        let path = fixture.path();
        fs::write(&path, b"abcdef").unwrap();
        let first = prepare(&path).unwrap();
        let replacement = fixture.0.join("new");
        fs::write(&replacement, b"ghijkl").unwrap();
        fs::rename(&replacement, &path).unwrap();
        assert!(matches!(
            read_range(&path, &first.stamp, 0..3),
            Err(ReadError::Changed)
        ));
        let second = prepare(&path).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(2)
            .unwrap();
        assert!(matches!(
            read_range(&path, &second.stamp, 0..6),
            Err(ReadError::Changed)
        ));
    }
    #[test]
    fn search_crosses_pages_wraps_and_finds_previous() {
        let fixture = Fixture::new();
        let path = fixture.path();
        let mut bytes = vec![0; PAGE_BYTES as usize + 50];
        bytes[3..7].copy_from_slice(b"abcd");
        let crossing = PAGE_BYTES as usize - 2;
        bytes[crossing..crossing + 4].copy_from_slice(b"abcd");
        fs::write(&path, &bytes).unwrap();
        let initial = prepare(&path).unwrap();
        let cancel = || false;
        let find = |start, forward| {
            search_file(&path, &initial.stamp, b"abcd", start, forward, &cancel).unwrap()
        };
        assert_eq!(find(7, true), Some(crossing as u64..crossing as u64 + 4));
        assert_eq!(find(crossing as u64 + 4, true), Some(3..7));
        assert_eq!(find(crossing as u64, false), Some(3..7));
        assert_eq!(find(0, false), Some(crossing as u64..crossing as u64 + 4));
        // Wrap at a cursor inside the match must still find the crossing match.
        assert_eq!(find(5, true), Some(crossing as u64..crossing as u64 + 4));
        assert_eq!(find(5, false), Some(3..7));
        assert_eq!(
            search_file(&path, &initial.stamp, b"missing", 0, true, &cancel).unwrap(),
            None
        );
    }
    #[test]
    fn empty_file_and_nonregular_sources_do_not_read_unbounded_data() {
        let fixture = Fixture::new();
        let path = fixture.path();
        fs::write(&path, []).unwrap();
        let initial = prepare(&path).unwrap();
        assert_eq!(initial.stamp.len, 0);
        assert!(initial.bytes.is_empty());
        assert!(prepare(&fixture.0).is_err());
    }
}
