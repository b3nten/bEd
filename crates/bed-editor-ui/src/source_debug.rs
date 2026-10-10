//! Host-supplied source debugging decorations. The host owns debugger state.

/// Adapter status for an enabled source breakpoint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BreakpointStatus {
    #[default]
    Pending,
    Verified,
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceBreakpoint {
    /// Zero-based document row.
    pub row: i32,
    pub enabled: bool,
    pub status: BreakpointStatus,
}

/// Supplying this presentation enables the breakpoint column for this view,
/// including when there are no breakpoints yet. Omit it in ordinary embeds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceDebugPresentation {
    pub breakpoints: Vec<SourceBreakpoint>,
    /// Zero-based execution location, independent of the editor's caret.
    /// Hosts should omit it when the source differs from the running binary.
    pub execution_row: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceDebugAction {
    ToggleBreakpoint { row: i32 },
}
