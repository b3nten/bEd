//! Translated from ned lsp/lsp_request.h; see LICENSE and NOTICE.
use crate::workspace_lsp::LspRequestOrigin;
use std::sync::Mutex;
pub type Ticket = u64;
struct State<T> {
    ticket: Ticket,
    pending: bool,
    result: Option<T>,
    origin: Option<LspRequestOrigin>,
}
pub struct LspRequestState<T> {
    inner: Mutex<State<T>>,
}
impl<T> Default for LspRequestState<T> {
    fn default() -> Self {
        Self {
            inner: Mutex::new(State {
                ticket: 0,
                pending: false,
                result: None,
                origin: None,
            }),
        }
    }
}
impl<T> LspRequestState<T> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn begin(&self) -> Ticket {
        let mut state = self.inner.lock().unwrap();
        state.ticket = state.ticket.wrapping_add(1);
        state.pending = true;
        state.result = None;
        state.origin = None;
        state.ticket
    }
    pub fn begin_for(&self, mut origin: LspRequestOrigin) -> LspRequestOrigin {
        let mut state = self.inner.lock().unwrap();
        state.ticket = state.ticket.wrapping_add(1);
        origin.ticket = state.ticket;
        state.pending = true;
        state.result = None;
        state.origin = Some(origin);
        origin
    }
    pub fn origin(&self) -> Option<LspRequestOrigin> {
        self.inner.lock().unwrap().origin
    }
    pub fn deliver_for(&self, origin: LspRequestOrigin, result: Option<T>) {
        let mut state = self.inner.lock().unwrap();
        if state.origin != Some(origin) || state.ticket != origin.ticket {
            return;
        }
        state.pending = false;
        state.result = result;
    }
    pub fn snapshot_for(&self, origin: &LspRequestOrigin) -> Option<T>
    where
        T: Clone,
    {
        let state = self.inner.lock().unwrap();
        (state.origin.as_ref() == Some(origin))
            .then(|| state.result.clone())
            .flatten()
    }
    pub fn cancel(&self) {
        let mut state = self.inner.lock().unwrap();
        state.ticket = state.ticket.wrapping_add(1);
        state.pending = false;
        state.result = None;
        state.origin = None;
    }
    pub fn deliver(&self, ticket: Ticket, result: Option<T>) {
        let mut state = self.inner.lock().unwrap();
        if state.ticket != ticket {
            return;
        }
        state.pending = false;
        state.result = result;
    }
    pub fn is_pending(&self) -> bool {
        self.inner.lock().unwrap().pending
    }
    pub fn snapshot(&self) -> Option<T>
    where
        T: Clone,
    {
        self.inner.lock().unwrap().result.clone()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use bed_editing::identity::{DocumentId, ViewId, WorkspaceId};
    #[test]
    fn routed_replies_require_exact_workspace_document_version_view_and_ticket() {
        let state = LspRequestState::new();
        let origin = state.begin_for(LspRequestOrigin {
            workspace_id: WorkspaceId(1),
            document_id: DocumentId(2),
            view_id: ViewId(3),
            server_generation: 4,
            document_generation: 5,
            version: 6,
            ticket: 0,
        });
        for stale in [
            LspRequestOrigin {
                workspace_id: WorkspaceId(7),
                ..origin
            },
            LspRequestOrigin {
                document_id: DocumentId(7),
                ..origin
            },
            LspRequestOrigin {
                view_id: ViewId(7),
                ..origin
            },
            LspRequestOrigin {
                server_generation: 7,
                ..origin
            },
            LspRequestOrigin {
                document_generation: 7,
                ..origin
            },
            LspRequestOrigin {
                version: 7,
                ..origin
            },
            LspRequestOrigin {
                ticket: 7,
                ..origin
            },
        ] {
            state.deliver_for(stale, Some("stale"));
            assert!(state.is_pending());
        }
        state.deliver_for(origin, Some("current"));
        assert_eq!(state.snapshot_for(&origin), Some("current"));
        assert!(
            state
                .snapshot_for(&LspRequestOrigin {
                    view_id: ViewId(7),
                    ..origin
                })
                .is_none()
        );
        state.cancel();
        state.deliver_for(origin, Some("late"));
        assert!(state.snapshot().is_none());
    }
    #[test]
    fn upstream_empty_result_clears_pending() {
        let state = LspRequestState::<Vec<i32>>::new();
        let ticket = state.begin();
        assert!(state.is_pending());
        assert_eq!(state.snapshot(), None);
        state.deliver(ticket, Some(vec![]));
        assert!(!state.is_pending());
        assert_eq!(state.snapshot(), Some(vec![]));
    }
    #[test]
    fn upstream_stale_deliveries_drop() {
        let state = LspRequestState::new();
        let first = state.begin();
        let second = state.begin();
        state.deliver(first, Some("stale"));
        assert!(state.is_pending());
        state.deliver(second, Some("fresh"));
        assert_eq!(state.snapshot(), Some("fresh"));
        assert!(!state.is_pending());
    }
    #[test]
    fn upstream_cancel_invalidates_replies() {
        let state = LspRequestState::new();
        let ticket = state.begin();
        state.cancel();
        state.deliver(ticket, Some("late"));
        assert!(!state.is_pending());
        assert_eq!(state.snapshot(), None);
    }
    #[test]
    fn cross_thread_errors_clear_pending_without_resurrecting_cancel() {
        let state = std::sync::Arc::new(LspRequestState::<String>::new());
        let ticket = state.begin();
        let worker = state.clone();
        std::thread::spawn(move || worker.deliver(ticket, None))
            .join()
            .unwrap();
        assert!(!state.is_pending());
        assert_eq!(state.snapshot(), None);
    }
}
