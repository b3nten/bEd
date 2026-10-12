//! Stable logical texture identity, independent of a GPU or UI backend.
use std::sync::atomic::{AtomicU64, Ordering};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureHandle(pub u64);
impl TextureHandle {
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}
