/// What a document is made of. Changes are pure and return their inverse, so
/// history can undo any model the same way.
pub trait DocumentModel: Clone + PartialEq + Send + Sync + 'static {
    type Change: Clone + Send + Sync + 'static;
    /// What can come with opening it in a viewer: where to put the cursor,
    /// what to select. `()` for models with nothing to point at.
    type OpenState: Default + Clone + Send + Sync + 'static;
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String>;
    /// The state for a line and column (from 0), for callers that only have
    /// those (a terminal's `+open main.rs:42`). Ignored by default.
    fn state_at_line(_line: u32, _column: u32) -> Self::OpenState {
        Self::OpenState::default()
    }
}
