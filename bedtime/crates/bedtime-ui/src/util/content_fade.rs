//! Mark window content for the desktop renderer's fade when its value changes.
use dear_imgui_rs::{Ui, sys};
use std::hash::{DefaultHasher, Hash, Hasher};

// Reserved keys in each window's native storage. The frame stamp prevents a
// window that no longer registers content from retaining an old marker.
pub const FRAME_KEY: u32 = 0xbedf_ad01;
pub const FIRST_VERTEX_KEY: u32 = 0xbedf_ad02;
pub const IDENTITY_KEY: u32 = 0xbedf_ad03;

/// Call after Begin has drawn decorations and before drawing the content.
/// This only describes the content; the desktop animation owner applies motion
/// and honors the animation setting. Embedded hosts keep their own rendering.
pub fn register(ui: &Ui, content: &impl Hash) {
    let mut hash = DefaultHasher::new();
    content.hash(&mut hash);
    ui.with_bound_context(|| unsafe {
        let window = sys::igGetCurrentWindow();
        let storage = &mut (*window).StateStorage;
        sys::ImGuiStorage_SetInt(storage, FRAME_KEY, ui.frame_count() as i32);
        sys::ImGuiStorage_SetInt(
            storage,
            FIRST_VERTEX_KEY,
            (*(*window).DrawList).VtxBuffer.Size,
        );
        sys::ImGuiStorage_SetInt(storage, IDENTITY_KEY, hash.finish() as i32);
    });
}
