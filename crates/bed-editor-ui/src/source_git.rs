//! Git controls contributed to every text view of a working document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictChoice {
    Current,
    Incoming,
    Both,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceGitAction {
    ResolveConflict { id: u64, choice: ConflictChoice },
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceConflict {
    pub id: u64,
    /// Zero-based row of the opening conflict marker in the real document.
    pub row: i32,
    pub label: String,
    pub current_label: String,
    pub incoming_label: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceGitPresentation {
    pub conflicts: Vec<SourceConflict>,
}
