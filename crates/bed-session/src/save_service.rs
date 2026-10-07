//! Translated from ned editor/services/save_service.{h,cpp}; see LICENSE and NOTICE.
use std::io;

use std::{
    cell::Cell,
    fs::File,
    io::Write,
    rc::Rc,
    time::{Duration, Instant},
};

use bed_core::{
    editor_events::{DidSave, EditorEvents},
    editor_state::EditorState,
};

const WRITE_CHUNK: usize = 64 * 1024;

fn check_save_size(state: &EditorState) -> io::Result<()> {
    let total = state
        .byte_size()
        .saturating_add(if state.utf8_bom { 3 } else { 0 });
    if total > bed_files::files::MAX_FILE_SIZE {
        return Err(bed_files::files::file_too_large(
            std::path::Path::new(&state.path),
            total as u64,
        ));
    }
    Ok(())
}

pub struct EditorSave {
    last_edit: Rc<Cell<Option<Instant>>>,
    idle: Duration,
}

impl Default for EditorSave {
    fn default() -> Self {
        Self {
            last_edit: Rc::new(Cell::new(None)),
            idle: Duration::from_millis(1000),
        }
    }
}

impl EditorSave {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self, events: &mut EditorEvents) {
        let last_edit = Rc::clone(&self.last_edit);
        events.subscribe_did_edit(move |_| last_edit.set(Some(Instant::now())));
    }

    pub fn set_autosave_idle_ms(&mut self, ms: i32) {
        self.idle = Duration::from_millis(ms.max(0) as u64);
    }

    pub fn on_did_edit(&self, state: &EditorState) {
        if !state.path.is_empty() && state.dirty {
            self.last_edit.set(Some(Instant::now()));
        }
    }

    pub fn cancel_pending(&self) {
        self.last_edit.set(None);
    }

    pub fn is_due(&self) -> bool {
        self.last_edit
            .get()
            .is_some_and(|last| last.elapsed() >= self.idle)
    }

    /// Generate the same bytes as a local save, without touching a filesystem.
    pub fn bytes_for_save(state: &EditorState) -> io::Result<Vec<u8>> {
        check_save_size(state)?;
        let mut bytes = Vec::with_capacity(state.byte_size() + 3);
        if state.utf8_bom {
            bytes.extend_from_slice(&[0xef, 0xbb, 0xbf]);
        }
        bytes.extend(state.join());
        Ok(bytes)
    }

    pub fn save(&mut self, state: &mut EditorState, events: &mut EditorEvents) -> io::Result<bool> {
        self.cancel_pending();
        if state.path.is_empty() || !state.dirty {
            return Ok(false);
        }
        check_save_size(state)?;
        let mut file = File::create(&state.path)?;
        if state.utf8_bom {
            file.write_all(&[0xef, 0xbb, 0xbf])?;
        }
        let total = state.byte_size();
        let mut buffer = vec![0; total.clamp(1, WRITE_CHUNK)];
        let mut offset = 0;
        while offset < total {
            let len = WRITE_CHUNK.min(total - offset);
            state.copy_bytes(offset, len, &mut buffer);
            file.write_all(&buffer[..len])?;
            offset += len;
        }
        // Unlike unchecked ostream::close, keep dirty on any write/flush error.
        file.flush()?;
        drop(file);
        state.mark_saved();
        events.emit_did_save_document(
            &DidSave {
                path: state.path.clone(),
                version: state.version,
            },
            state,
        );
        Ok(true)
    }

    pub fn poll(&mut self, state: &mut EditorState, events: &mut EditorEvents) -> io::Result<bool> {
        let Some(last_edit) = self.last_edit.get() else {
            return Ok(false);
        };
        if state.path.is_empty() || !state.dirty {
            self.cancel_pending();
            return Ok(false);
        }
        if last_edit.elapsed() < self.idle {
            return Ok(false);
        }
        self.save(state, events)
    }
}
