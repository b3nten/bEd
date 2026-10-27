//! Disk-backed byte views. World sources own bounded shared caches; panels
//! own navigation and user jobs. All filesystem access runs on managed workers.
use crate::{CopyFormat, HexView};
use bedtime_clipboard::Clipboard;
use bedtime_ui::{UiContext, egui};
use bedtime_workbench::*;
use bedtime_workbench::{
    DocumentByteError as ReadError, DocumentByteMetadata as Stamp, DocumentByteSource as ByteSource,
};
use bevy::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Seek, SeekFrom},
    ops::Range,
    sync::Arc,
};

pub(crate) const PAGE_BYTES: u64 = 64 * 1024;

const COPY_LIMIT: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct PreparedHex {
    source: ByteSource,
    stamp: Stamp,
    bytes: Arc<[u8]>,
}

fn read_range(source: &ByteSource, stamp: &Stamp, range: Range<u64>) -> Result<Vec<u8>, ReadError> {
    source.read_range(&stamp.version, range)
}

pub fn prepare(source: &ByteSource) -> Result<Arc<PreparedHex>, String> {
    let stamp = source.metadata().map_err(|e| e.to_string())?;

    let bytes = read_range(source, &stamp, 0..stamp.len.min(PAGE_BYTES)).map_err(error_text)?;
    Ok(Arc::new(PreparedHex {
        source: source.clone(),
        stamp,
        bytes: bytes.into(),
    }))
}

/// Initial presentation readiness, after a version-checked first page is accepted.
pub fn open_result(world: &World, panel: Entity) -> Option<Result<(), String>> {
    let state = world.get::<HexPanel>(panel)?;
    if let Some(error) = &state.load_error {
        return Some(Err(error.clone()));
    }
    let cache = document_byte_cache(world, &state.source)?;
    Some(match &cache.error {
        Some(error) => Err(error.clone()),
        None => Ok(()),
    })
}

fn error_text(error: ReadError) -> String {
    error.to_string()
}

pub struct HexPlugin;
impl Plugin for HexPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, update);
    }
}

struct UserJob<T: Send + 'static> {
    stamp: Stamp,
    task: ManagedTask<Result<T, ReadError>>,
}

#[derive(Component)]
#[require(PanelInstance = PanelInstance::of::<Self>())]
pub struct HexPanel {
    source: ByteSource,
    load: Option<ManagedTask<Result<Arc<PreparedHex>, String>>>,
    load_error: Option<String>,
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

impl HexPanel {
    pub fn new(source: ByteSource) -> Self {
        Self {
            source,
            load: None,
            load_error: None,
            view: HexView::default(),
            needed: 0..0,
            goto: String::new(),
            show_goto: false,
            focus_goto: false,
            query: String::new(),
            text_search: false,
            show_find: false,
            focus_find: false,
            find: None,
            copy: None,
            message: None,
            inspector: false,
            big_endian: false,
        }
    }
    pub fn source(&self) -> &ByteSource {
        &self.source
    }
}

impl Panel for HexPanel {
    const KEY: &'static str = "bedtime.hex";
    const LABEL: &'static str = "Hex";
    fn draw(ui: &mut egui::Ui, world: &mut World, _: &UiContext<'_>, panel: Entity) {
        if draw(ui, world, panel, |_, _| {}) {
            reload(world, panel);
        }
    }
}
impl Drop for HexPanel {
    fn drop(&mut self) {
        if let Some(job) = &self.load {
            job.cancel();
        }
        if let Some(job) = &self.find {
            job.task.cancel();
        }
        if let Some(job) = &self.copy {
            job.task.cancel();
        }
    }
}

pub fn accept(world: &mut World, panel: Entity, initial: &PreparedHex) {
    let path = &initial.source;
    accept_document_byte_cache(
        world,
        path.clone(),
        initial.stamp.clone(),
        initial.bytes.clone(),
    );
    if world.get::<HexPanel>(panel).is_none() {
        world.entity_mut(panel).insert(HexPanel::new(path.clone()));
    }
    let mut state = world.get_mut::<HexPanel>(panel).unwrap();
    if state.source != *path {
        if let Some(job) = state.find.take() {
            job.task.cancel();
        }
        if let Some(job) = state.copy.take() {
            job.task.cancel();
        }
    }
    if let Some(job) = state.load.take() {
        job.cancel();
    }
    state.source = path.clone();
    state.load_error = None;
    world
        .get_mut::<HexPanel>(panel)
        .unwrap()
        .view
        .clamp(initial.stamp.len);
}

/// Draw the complete panel, with an optional host contribution to its context menu.
/// Returns a reload intention; hosts with a broader file lifecycle may route it there.
pub fn draw(
    ui: &mut egui::Ui,
    world: &mut World,
    panel: Entity,
    menu: impl FnOnce(&mut egui::Ui, &mut World),
) -> bool {
    let Some(path) = world.get::<HexPanel>(panel).map(|s| s.source.clone()) else {
        return false;
    };
    let Some(mut state) = world.entity_mut(panel).take::<HexPanel>() else {
        return false;
    };
    let Some(source) = document_byte_cache(world, &path) else {
        if let Some(error) = &state.load_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        } else {
            ui.spinner();
        }
        let retry = state.load_error.is_some() && ui.button("Reload").clicked();
        world.entity_mut(panel).insert(state);
        return retry;
    };
    let stamp = source.stamp.clone();
    let len = stamp.len;
    let mut reload = false;
    if let Some(error) = state.load_error.as_ref().or(source.error.as_ref()) {
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
                match crate::parse_offset(&state.goto) {
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
        menu(ui, world);
    });
    world.entity_mut(panel).insert(state);
    if let Some(forward) = search {
        start_search(world, panel, path.clone(), stamp.clone(), forward);
    }
    if let Some(format) = copy {
        if format == CopyFormat::Offset {
            ui.ctx().copy_text(format!(
                "0x{:X}",
                world.get::<HexPanel>(panel).unwrap().view.cursor()
            ));
        } else {
            start_copy(world, panel, path, stamp, format);
        }
    }
    reload
}

fn start_search(world: &mut World, panel: Entity, path: ByteSource, stamp: Stamp, forward: bool) {
    let mut state = world.get_mut::<HexPanel>(panel).unwrap();
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
        crate::parse_hex(&state.query)
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
    let mut state = world.get_mut::<HexPanel>(panel).unwrap();
    match task {
        Ok(task) => state.find = Some(UserJob { stamp, task }),
        Err(error) => state.message = Some(error),
    }
}

fn search_file(
    path: &ByteSource,
    stamp: &Stamp,
    pattern: &[u8],
    start: u64,
    forward: bool,
    cancelled: &impl Fn() -> bool,
) -> Result<Option<Range<u64>>, ReadError> {
    let mut file = path.open()?;
    if file.metadata().version != stamp.version {
        return Err(ReadError::Changed);
    }
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
                return Err(ReadError::Io(std::io::Error::other("Search cancelled")));
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
                        file.verify()?;
                        return Ok(last);
                    }
                }
            }
            let keep = (pattern.len() - 1).min(buffer.len());
            carry = buffer[buffer.len() - keep..].to_vec();
            offset += count as u64;
        }
        if last.is_some() {
            file.verify()?;
            return Ok(last);
        }
    }
    file.verify()?;
    Ok(None)
}

fn start_copy(
    world: &mut World,
    panel: Entity,
    path: ByteSource,
    stamp: Stamp,
    format: CopyFormat,
) {
    let mut state = world.get_mut::<HexPanel>(panel).unwrap();
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
                return Err(ReadError::Io(std::io::Error::other("Copy cancelled")));
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
    let mut state = world.get_mut::<HexPanel>(panel).unwrap();
    match task {
        Ok(task) => state.copy = Some(UserJob { stamp, task }),
        Err(error) => state.message = Some(error),
    }
}

/// Reload only this source; the shared cache is replaced after a checked read.
pub fn reload(world: &mut World, panel: Entity) {
    let Some(source) = world.get::<HexPanel>(panel).map(|s| s.source.clone()) else {
        return;
    };
    let result = world
        .get_resource_mut::<ManagedWork>()
        .ok_or_else(|| "Managed work is unavailable".to_owned())
        .and_then(|mut work| work.spawn_task(Some(panel), "Load hex", move |_| prepare(&source)));
    let mut state = world.get_mut::<HexPanel>(panel).unwrap();
    if let Some(old) = state.load.take() {
        old.cancel();
    }
    match result {
        Ok(task) => {
            state.load = Some(task);
            state.load_error = None;
        }
        Err(error) => state.load_error = Some(error),
    }
}
fn poll_loads(world: &mut World) {
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<HexPanel>>()
        .iter(world)
        .collect();
    for panel in panels {
        let state = world.get::<HexPanel>(panel).unwrap();
        let result = state.load.as_ref().and_then(|task| match task.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err("Hex loader stopped".into()))
            }
        });
        if let Some(result) = result {
            world.get_mut::<HexPanel>(panel).unwrap().load = None;
            match result {
                Ok(initial) => accept(world, panel, &initial),
                Err(error) => world.get_mut::<HexPanel>(panel).unwrap().load_error = Some(error),
            }
        }
        let state = world.get::<HexPanel>(panel).unwrap();
        if state.load.is_none()
            && state.load_error.is_none()
            && document_byte_cache(world, &state.source).is_none()
        {
            reload(world, panel);
        }
    }
}

pub fn update(world: &mut World) {
    let viewed: Vec<_> = world
        .query_filtered::<(Entity, &DocumentView), With<HexPanel>>()
        .iter(world)
        .map(|(p, v)| (p, v.document()))
        .collect();
    for (panel, id) in viewed {
        if let Ok(source) = document_bytes(world, id) {
            if world
                .get::<HexPanel>(panel)
                .is_some_and(|p| p.source != source)
            {
                let mut state = world.get_mut::<HexPanel>(panel).unwrap();
                if let Some(job) = state.load.take() {
                    job.cancel();
                }
                if let Some(job) = state.find.take() {
                    job.task.cancel();
                }
                if let Some(job) = state.copy.take() {
                    job.task.cancel();
                }
                state.source = source;
                state.load_error = None;
            }
        }
    }
    poll_loads(world);
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<HexPanel>>()
        .iter(world)
        .map(|p| (p, world.get::<HexPanel>(p).unwrap().source.clone()))
        .collect();
    let mut demand = BTreeMap::<ByteSource, BTreeSet<u64>>::new();
    for (panel, path) in &panels {
        let Some(stamp) = document_byte_cache(world, path).map(|s| s.stamp.clone()) else {
            continue;
        };
        let mut state = world.entity_mut(*panel).take::<HexPanel>().unwrap();
        if state
            .find
            .as_ref()
            .is_some_and(|j| j.stamp.version != stamp.version)
        {
            state.find.take().unwrap().task.cancel();
        }
        if state
            .copy
            .as_ref()
            .is_some_and(|j| j.stamp.version != stamp.version)
        {
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
    update_document_byte_cache(world, demand);
}

fn poll<T: Send + 'static>(
    task: &ManagedTask<Result<T, ReadError>>,
) -> Option<Result<T, ReadError>> {
    match task.try_recv() {
        Ok(result) => Some(result),
        Err(std::sync::mpsc::TryRecvError::Empty) => None,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(ReadError::Io(
            std::io::Error::other("Hex worker stopped"),
        ))),
    }
}

#[cfg(feature = "automation")]
pub fn inspect(world: &World, panel: Entity) -> Option<serde_json::Value> {
    let state = world.get::<HexPanel>(panel)?;
    let path = state.source.clone();
    let source = document_byte_cache(world, &path)?;
    Some(
        serde_json::json!({"len":source.stamp.len,"cursor":state.view.cursor(),"selection":[state.view.selection(source.stamp.len).start,state.view.selection(source.stamp.len).end],"top_row":state.view.top_row,"rendered_rows":state.view.rendered_rows,"cached_pages":source.pages.len(),"cached_bytes":source.pages.values().map(|p|p.len()).sum::<usize>(),"rect":state.view.rect.map(|r|serde_json::json!({"min":[r.min.x,r.min.y],"max":[r.max.x,r.max.y]})),"message":state.message,"error":source.error,"searching":state.find.is_some(),"query":state.query}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file_bytes(path: impl AsRef<std::path::Path>) -> ByteSource {
        let source = bedtime_filesystem::Filesystem::local()
            .document_source(path)
            .unwrap();
        ByteSource::new(source.id(), source.snapshot().unwrap())
    }

    use std::{
        fs::{self, File},
        io::Write,
        path::PathBuf,
    };

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "bed-hex-{}-{}",
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
        let initial = prepare(&file_bytes(&path)).unwrap();
        assert_eq!(initial.stamp.len, offset + 3);
        assert_eq!(initial.bytes.len(), PAGE_BYTES as usize);
        assert_eq!(
            read_range(&file_bytes(&path), &initial.stamp, offset..offset + 3).unwrap(),
            [0x12, 0x34, 0x56]
        );
    }

    #[test]
    fn source_replacement_and_truncation_reject_old_page_reads() {
        let fixture = Fixture::new();
        let path = fixture.path();
        fs::write(&path, b"abcdef").unwrap();
        let first = prepare(&file_bytes(&path)).unwrap();
        let replacement = fixture.0.join("new");
        fs::write(&replacement, b"ghijkl").unwrap();
        fs::rename(&replacement, &path).unwrap();
        assert!(matches!(
            read_range(&file_bytes(&path), &first.stamp, 0..3),
            Err(ReadError::Changed)
        ));
        let second = prepare(&file_bytes(&path)).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(2)
            .unwrap();
        assert!(matches!(
            read_range(&file_bytes(&path), &second.stamp, 0..6),
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
        let initial = prepare(&file_bytes(&path)).unwrap();
        let cancel = || false;
        let find = |start, forward| {
            search_file(
                &file_bytes(&path),
                &initial.stamp,
                b"abcd",
                start,
                forward,
                &cancel,
            )
            .unwrap()
        };
        assert_eq!(find(7, true), Some(crossing as u64..crossing as u64 + 4));
        assert_eq!(find(crossing as u64 + 4, true), Some(3..7));
        assert_eq!(find(crossing as u64, false), Some(3..7));
        assert_eq!(find(0, false), Some(crossing as u64..crossing as u64 + 4));
        // Wrap at a cursor inside the match must still find the crossing match.
        assert_eq!(find(5, true), Some(crossing as u64..crossing as u64 + 4));
        assert_eq!(find(5, false), Some(3..7));
        assert_eq!(
            search_file(
                &file_bytes(&path),
                &initial.stamp,
                b"missing",
                0,
                true,
                &cancel
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn empty_file_and_nonregular_sources_do_not_read_unbounded_data() {
        let fixture = Fixture::new();
        let path = fixture.path();
        fs::write(&path, []).unwrap();
        let initial = prepare(&file_bytes(&path)).unwrap();
        assert_eq!(initial.stamp.len, 0);
        assert!(initial.bytes.is_empty());
        assert!(
            bedtime_filesystem::Filesystem::local()
                .document_source(&fixture.0)
                .unwrap()
                .snapshot()
                .is_err()
        );
    }
    fn host() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugin, HexPlugin));
        let layout = app
            .world_mut()
            .spawn((Layout::default(), ActiveLayout))
            .id();
        let area = app
            .world_mut()
            .spawn((Area::default(), InLayout(layout)))
            .id();
        (app, area)
    }

    fn wait_for(app: &mut App, predicate: impl Fn(&World) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.update();
            if predicate(app.world()) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Hex operation did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn concrete_panel_loads_without_application_helpers_and_shares_source_cache() {
        let fixture = Fixture::new();
        let path = fixture.path();
        fs::write(&path, [0x41; 100]).unwrap();
        let source = file_bytes(&path);
        let (mut app, area) = host();
        let a = app
            .world_mut()
            .spawn((HexPanel::new(source.clone()), InArea(area)))
            .id();
        let b = app
            .world_mut()
            .spawn((HexPanel::new(source.clone()), InArea(area)))
            .id();
        wait_for(&mut app, |world| {
            document_byte_cache(world, &source).is_some()
        });
        assert_eq!(
            app.world().get::<PanelInstance>(a).unwrap().key,
            "bedtime.hex"
        );
        assert!(document_byte_cache(app.world(), &source).is_some());
        app.world_mut().despawn(a);
        app.update();
        assert!(document_byte_cache(app.world(), &source).is_some());
        // Finish the sibling's independent initial load before retiring it.
        wait_for(&mut app, |world| {
            world.get::<HexPanel>(b).unwrap().load.is_none()
        });
        app.world_mut().despawn(b);
        app.update();
        assert!(document_byte_cache(app.world(), &source).is_none());
    }

    #[test]
    fn explicit_source_move_retains_navigation_and_rebinds_future_reads() {
        let fixture = Fixture::new();
        let path = fixture.path();
        fs::write(&path, [0x41; 100]).unwrap();
        let (mut app, area) = host();
        let source = file_bytes(&path);
        let panel = app
            .world_mut()
            .spawn((HexPanel::new(source.clone()), InArea(area)))
            .id();
        wait_for(&mut app, |world| {
            document_byte_cache(world, &source).is_some()
        });
        app.world_mut()
            .get_mut::<HexPanel>(panel)
            .unwrap()
            .view
            .go_to(42, 100);
        let destination = fixture.0.join("moved.bin");
        fs::rename(&path, &destination).unwrap();
        let moved = file_bytes(&destination);
        accept(app.world_mut(), panel, &prepare(&moved).unwrap());
        app.update();
        assert_eq!(
            app.world().get::<HexPanel>(panel).unwrap().view.cursor(),
            42
        );
        assert_eq!(app.world().get::<HexPanel>(panel).unwrap().source(), &moved);
        assert!(document_byte_cache(app.world(), &source).is_none());
        assert_eq!(
            document_byte_cache(app.world(), &moved).unwrap().byte(42),
            Some(0x41)
        );
    }
}
