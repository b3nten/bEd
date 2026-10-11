//! Workspace diagnostic pulls are owned by the client's poll path. Transport
//! callbacks enqueue reports; each provider keeps its own result IDs and items.
use crate::{
    diagnostics::{DiagnosticItem, LspDiagnostics},
    document_diagnostics::DocumentDiagnosticPull,
    jsonrpc::{ResponseError, RpcId},
    lsp_client::decode_diagnostics,
    message_handler::RpcSession,
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, VecDeque},
    rc::Rc,
    time::{Duration, Instant},
};

type Identity = Option<String>;
const PULL_INTERVAL: Duration = Duration::from_secs(30);

pub(crate) struct Provider {
    identifier: Identity,
    enabled: bool,
}
impl Provider {
    pub(crate) fn parse(value: &Value) -> Result<Self, ResponseError> {
        if !value.is_object() {
            return Err(ResponseError::invalid_params(
                "Expected diagnostic provider options",
            ));
        }
        let identifier = match value.get("identifier").filter(|value| !value.is_null()) {
            None => None,
            Some(value) => Some(
                value
                    .as_str()
                    .ok_or_else(|| {
                        ResponseError::invalid_params("Expected diagnostic provider identifier")
                    })?
                    .to_owned(),
            ),
        };
        let enabled = match value
            .get("workspaceDiagnostics")
            .filter(|value| !value.is_null())
        {
            None => false,
            Some(value) => value.as_bool().ok_or_else(|| {
                ResponseError::invalid_params("Expected workspaceDiagnostics boolean")
            })?,
        };
        Ok(Self {
            identifier,
            enabled,
        })
    }
}
pub(crate) enum Input {
    Register(String, Provider),
    Unregister(String),
    Refresh,
    Progress(String, Value),
}
pub(crate) type Inputs = Rc<RefCell<VecDeque<Input>>>;

struct Cached {
    id: Option<String>,
    items: Vec<DiagnosticItem>,
    version: i32,
    sequence: u64,
}
struct Pending {
    id: RpcId,
    sequence: u64,
    token: String,
    previous_ids: BTreeMap<String, String>,
}
#[derive(Default)]
struct State {
    cache: BTreeMap<String, Cached>,
    pending: Option<Pending>,
    last: Option<Instant>,
    refresh: bool,
    completed: bool,
    failed: bool,
    retry_at: Option<Instant>,
    retry_delay: u64,
}
struct Reply {
    provider: Identity,
    sequence: u64,
    result: Result<Value, ResponseError>,
}

#[derive(Default)]
pub(crate) struct WorkspaceDiagnosticPull {
    // None represents a static registration without an ID. Explicit server IDs
    // remain distinct, including strings that resemble an internal sentinel.
    registrations: BTreeMap<Option<String>, Provider>,
    providers: BTreeMap<Identity, State>,
    next_request: u64,
    cancel: Vec<RpcId>,
    pub(crate) inputs: Inputs,
    replies: Rc<RefCell<VecDeque<Reply>>>,
}
impl WorkspaceDiagnosticPull {
    pub(crate) fn set_static(&mut self, provider: Option<Provider>, id: Option<String>) {
        if let Some(provider) = provider {
            self.registrations.insert(id, provider);
        }
    }
    pub(crate) fn complete(&self) -> bool {
        !self.providers.is_empty() && self.providers.values().all(|state| state.completed)
    }
    pub(crate) fn refresh(&mut self) {
        for state in self.providers.values_mut() {
            if let Some(pending) = state.pending.take() {
                self.cancel.push(pending.id);
            }
            state.refresh = true;
            state.completed = false;
            state.failed = false;
            state.retry_at = None;
            state.retry_delay = 0;
        }
    }
    /// The document owner relinquished a URI. Forget its workspace IDs so the
    /// next pull returns disk contents instead of an unchanged unsaved report.
    pub(crate) fn close(&mut self, uri: &str, store: &LspDiagnostics) {
        if let Ok((uri, _)) = canonical_uri(uri, store) {
            for state in self.providers.values_mut() {
                state.cache.remove(&uri);
            }
        }
        self.refresh();
    }
    fn reconcile(&mut self, store: &LspDiagnostics, documents: &DocumentDiagnosticPull) {
        let desired: BTreeSet<_> = self
            .registrations
            .values()
            .filter(|provider| provider.enabled)
            .map(|provider| provider.identifier.clone())
            .collect();
        let removed: Vec<_> = self
            .providers
            .keys()
            .filter(|identity| !desired.contains(*identity))
            .cloned()
            .collect();
        let mut affected = BTreeSet::new();
        for identity in removed {
            if let Some(state) = self.providers.remove(&identity) {
                affected.extend(state.cache.into_keys());
                if let Some(pending) = state.pending {
                    self.cancel.push(pending.id);
                }
            }
        }
        for identity in desired {
            self.providers.entry(identity).or_insert_with(|| State {
                refresh: true,
                ..State::default()
            });
        }
        for uri in affected {
            self.publish(&uri, store, documents, true);
        }
    }
    fn publish(
        &self,
        uri: &str,
        store: &LspDiagnostics,
        documents: &DocumentDiagnosticPull,
        reset_version: bool,
    ) {
        // Never replace a document-owned buffer, even with an empty aggregate.
        if !documents.accepts_workspace(uri, -1, store) {
            return;
        }
        let Ok((_, path)) = canonical_uri(uri, store) else {
            return;
        };
        let cached_reports: Vec<_> = self
            .providers
            .values()
            .filter_map(|state| state.cache.get(uri))
            .collect();
        let reports: Vec<_> = cached_reports
            .iter()
            .copied()
            .filter(|cached| documents.accepts_workspace(uri, cached.version, store))
            .collect();
        if !cached_reports.is_empty() && reports.is_empty() {
            // A rejected old report cannot clear fresher diagnostics merely
            // because no eligible workspace contribution remains to aggregate.
            return;
        }
        let version = reports
            .iter()
            .map(|cached| cached.version)
            .max()
            .unwrap_or(-1);
        let items = reports
            .into_iter()
            .filter(|cached| cached.version == version)
            .flat_map(|cached| cached.items.iter().cloned())
            .collect();
        // Removing a provider can lower the aggregate's version. This deliberate
        // recomputation must clear that provider's former contribution as well.
        if reset_version {
            store.forget_version(&path);
        }
        store.replace(&path, items, version);
    }
    fn apply(
        &mut self,
        identity: &Identity,
        pending: &Pending,
        report: &Value,
        store: &LspDiagnostics,
        documents: &DocumentDiagnosticPull,
    ) -> Result<(), ResponseError> {
        let values = report["items"]
            .as_array()
            .ok_or_else(|| ResponseError::invalid_params("Expected workspace diagnostic items"))?;
        // Validate the complete chunk before changing cache or result IDs.
        let mut decoded = Vec::new();
        for value in values {
            let uri = value["uri"].as_str().ok_or_else(|| {
                ResponseError::invalid_params("Expected workspace diagnostic URI")
            })?;
            let (uri, _) = canonical_uri(uri, store)?;
            let id = match value.get("resultId").filter(|value| !value.is_null()) {
                None => None,
                Some(value) => Some(
                    value
                        .as_str()
                        .ok_or_else(|| {
                            ResponseError::invalid_params("Expected workspace diagnostic resultId")
                        })?
                        .to_owned(),
                ),
            };
            // Existing servers sometimes omit a closed-file version; accept it
            // with the same unknown-version meaning as protocol null.
            let version = match value.get("version").filter(|value| !value.is_null()) {
                None => -1,
                Some(value) => value
                    .as_i64()
                    .filter(|version| *version >= 0)
                    .and_then(|version| i32::try_from(version).ok())
                    .ok_or_else(|| {
                        ResponseError::invalid_params("Expected workspace diagnostic version")
                    })?,
            };
            let items = match value["kind"].as_str() {
                Some("full") => Some(
                    decode_diagnostics(
                        store,
                        Some(&json!({"uri":uri,"diagnostics":value["items"]})),
                    )?
                    .1,
                ),
                Some("unchanged") if id.is_some() => None,
                _ => {
                    return Err(ResponseError::invalid_params(
                        "Expected full or unchanged workspace diagnostic report",
                    ));
                }
            };
            decoded.push((uri, id, version, items));
        }
        // A chunk can repeat a URI; each report observes preceding valid reports
        // in that chunk, without exposing a partially validated external value.
        let mut staged = BTreeMap::new();
        for (uri, id, version, items) in decoded {
            let newest = self
                .providers
                .values()
                .filter_map(|state| state.cache.get(&uri))
                .map(|cached| cached.version)
                .max()
                .unwrap_or(-1);
            let newest = staged
                .get(&uri)
                .map_or(newest, |cached: &Cached| newest.max(cached.version));
            if version >= 0 && newest >= 0 && version < newest {
                continue;
            }
            let items = match items {
                Some(items) => items,
                None => {
                    let cached = staged
                        .get(&uri)
                        .or_else(|| {
                            self.providers
                                .get(identity)
                                .and_then(|state| state.cache.get(&uri))
                        })
                        .filter(|cached| {
                            cached.id.is_some()
                                && (cached.sequence == pending.sequence
                                    || cached.id.as_ref() == pending.previous_ids.get(&uri))
                        })
                        .ok_or_else(|| {
                            ResponseError::invalid_params(
                                "Unchanged workspace report has no matching cached content",
                            )
                        })?;
                    cached.items.clone()
                }
            };
            staged.insert(
                uri,
                Cached {
                    id,
                    items,
                    version,
                    sequence: pending.sequence,
                },
            );
        }
        let affected: Vec<_> = staged.keys().cloned().collect();
        if let Some(state) = self.providers.get_mut(identity) {
            state.cache.extend(staged);
        }
        for uri in affected {
            self.publish(&uri, store, documents, false);
        }
        Ok(())
    }
    pub(crate) fn poll(
        &mut self,
        session: &mut RpcSession,
        ready: bool,
        store: &LspDiagnostics,
        documents: &DocumentDiagnosticPull,
        last_error: &mut Option<String>,
    ) {
        let inputs: Vec<_> = self.inputs.borrow_mut().drain(..).collect();
        let mut partials = Vec::new();
        for input in inputs {
            match input {
                Input::Register(id, provider) => {
                    self.refresh();
                    self.registrations.insert(Some(id), provider);
                }
                Input::Unregister(id) => {
                    self.refresh();
                    self.registrations.remove(&Some(id));
                }
                Input::Refresh => self.refresh(),
                Input::Progress(token, value) => partials.push((token, value)),
            }
        }
        self.reconcile(store, documents);
        for id in self.cancel.drain(..) {
            let _ = session.cancel_request(&id);
        }
        for (token, report) in partials {
            let identity = self
                .providers
                .iter()
                .find(|(_, state)| {
                    state
                        .pending
                        .as_ref()
                        .is_some_and(|pending| pending.token == token)
                })
                .map(|(identity, _)| identity.clone());
            let Some(identity) = identity else {
                continue;
            };
            let pending = self
                .providers
                .get_mut(&identity)
                .unwrap()
                .pending
                .take()
                .unwrap();
            if let Err(error) = self.apply(&identity, &pending, &report, store, documents) {
                self.providers.get_mut(&identity).unwrap().failed = true;
                *last_error = Some(format!("Workspace diagnostics: {error}"));
            }
            self.providers.get_mut(&identity).unwrap().pending = Some(pending);
        }
        let replies: Vec<_> = self.replies.borrow_mut().drain(..).collect();
        for reply in replies {
            let Some(state) = self.providers.get_mut(&reply.provider) else {
                continue;
            };
            if !state
                .pending
                .as_ref()
                .is_some_and(|pending| pending.sequence == reply.sequence)
            {
                continue;
            }
            let pending = state.pending.take().unwrap();
            match reply.result {
                Ok(report) => {
                    let result = self.apply(&reply.provider, &pending, &report, store, documents);
                    let state = self.providers.get_mut(&reply.provider).unwrap();
                    match result {
                        Ok(()) => {
                            state.completed = !state.failed;
                            state.retry_delay = 0;
                        }
                        Err(error) => {
                            state.failed = true;
                            *last_error = Some(format!("Workspace diagnostics: {error}"));
                        }
                    }
                }
                Err(error) => {
                    let state = self.providers.get_mut(&reply.provider).unwrap();
                    state.completed = false;
                    let retry = error.code == -32801
                        || (error.code == -32802
                            && error
                                .data
                                .as_ref()
                                .and_then(|data| data["retriggerRequest"].as_bool())
                                != Some(false));
                    if retry {
                        state.retry_delay = if state.retry_delay == 0 {
                            200
                        } else {
                            (state.retry_delay * 2).min(2000)
                        };
                        state.retry_at =
                            Some(Instant::now() + Duration::from_millis(state.retry_delay));
                    } else {
                        *last_error = Some(format!("Workspace diagnostics: {error}"));
                    }
                }
            }
        }
        if !ready {
            return;
        }
        let now = Instant::now();
        let due: Vec<_> = self
            .providers
            .iter()
            .filter(|(_, state)| {
                state.pending.is_none()
                    && if let Some(retry_at) = state.retry_at {
                        retry_at <= now
                    } else {
                        state.refresh
                            || state
                                .last
                                .is_none_or(|last| now.duration_since(last) >= PULL_INTERVAL)
                    }
            })
            .map(|(identity, _)| identity.clone())
            .collect();
        for identity in due {
            self.next_request += 1;
            let sequence = self.next_request;
            let token = format!("bed.workspace-diagnostics-{sequence}");
            let state = self.providers.get_mut(&identity).unwrap();
            let previous_ids: BTreeMap<_, _> = state
                .cache
                .iter()
                .filter_map(|(uri, cached)| cached.id.as_ref().map(|id| (uri.clone(), id.clone())))
                .collect();
            let ids: Vec<_> = previous_ids
                .iter()
                .map(|(uri, value)| json!({"uri":uri,"value":value}))
                .collect();
            let mut params = json!({"previousResultIds":ids,"partialResultToken":token});
            if let Some(identifier) = &identity {
                params["identifier"] = json!(identifier);
            }
            state.completed = false;
            state.failed = false;
            state.refresh = false;
            state.retry_at = None;
            state.last = Some(now);
            let replies = self.replies.clone();
            let reply_provider = identity.clone();
            match session.send_request("workspace/diagnostic", Some(params), move |result| {
                replies.borrow_mut().push_back(Reply {
                    provider: reply_provider,
                    sequence,
                    result,
                });
            }) {
                Ok(id) => {
                    state.pending = Some(Pending {
                        id,
                        sequence,
                        token,
                        previous_ids,
                    })
                }
                Err(error) => {
                    state.failed = true;
                    *last_error = Some(format!("Workspace diagnostics: {error}"));
                }
            }
        }
    }
}

fn canonical_uri(uri: &str, store: &LspDiagnostics) -> Result<(String, String), ResponseError> {
    DocumentDiagnosticPull::canonical_uri(uri, store)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(identifier: Identity) -> Provider {
        Provider {
            identifier,
            enabled: true,
        }
    }
    fn pending(sequence: u64, previous_ids: BTreeMap<String, String>) -> Pending {
        Pending {
            id: RpcId::Number(sequence as i32),
            sequence,
            token: format!("test-{sequence}"),
            previous_ids,
        }
    }
    fn report(uri: &str, id: Option<&str>, version: Option<i32>, message: &str) -> Value {
        let items = if message.is_empty() {
            json!([])
        } else {
            json!([{"range":{"start":{"line":0,"character":0},
                "end":{"line":0,"character":1}},"message":message}])
        };
        let mut value = json!({"uri":uri,"kind":"full","version":version,"items":items});
        if let Some(id) = id {
            value["resultId"] = json!(id);
        }
        json!({"items":[value]})
    }
    fn activate(
        pull: &mut WorkspaceDiagnosticPull,
        id: &str,
        identifier: Identity,
        store: &LspDiagnostics,
        documents: &DocumentDiagnosticPull,
    ) {
        pull.set_static(Some(provider(identifier)), Some(id.into()));
        pull.reconcile(store, documents);
    }

    #[test]
    fn full_unchanged_and_empty_reports_preserve_and_clear_result_ids() {
        let store = LspDiagnostics::new();
        let documents = DocumentDiagnosticPull::default();
        let mut pull = WorkspaceDiagnosticPull::default();
        activate(&mut pull, "static", None, &store, &documents);
        let uri = canonical_uri("file:///tmp/bed-workspace-cache.py", &store)
            .unwrap()
            .0;
        pull.apply(
            &None,
            &pending(1, BTreeMap::new()),
            &report(&uri, Some("first"), None, "closed error"),
            &store,
            &documents,
        )
        .unwrap();
        assert_eq!(
            store.for_document("/tmp/bed-workspace-cache.py")[0].message,
            "closed error"
        );
        pull.apply(
            &None,
            &pending(2, BTreeMap::from([(uri.clone(), "first".into())])),
            &json!({"items":[{"uri":uri,"kind":"unchanged","resultId":"second","version":null}]}),
            &store,
            &documents,
        )
        .unwrap();
        assert_eq!(
            pull.providers[&None].cache[&uri].id.as_deref(),
            Some("second")
        );
        assert_eq!(store.for_document("/tmp/bed-workspace-cache.py").len(), 1);
        pull.apply(
            &None,
            &pending(3, BTreeMap::new()),
            &report(&uri, None, None, ""),
            &store,
            &documents,
        )
        .unwrap();
        assert!(store.for_document("/tmp/bed-workspace-cache.py").is_empty());
        assert_eq!(pull.providers[&None].cache[&uri].id, None);
        assert!(
            pull.apply(
                &None,
                &pending(4, BTreeMap::new()),
                &json!({"items":[{"uri":uri,"kind":"unchanged","resultId":"invalid"}]}),
                &store,
                &documents
            )
            .is_err()
        );
        assert_eq!(pull.providers[&None].cache[&uri].id, None);
    }

    #[test]
    fn malformed_chunk_commits_neither_items_nor_ids() {
        let store = LspDiagnostics::new();
        let documents = DocumentDiagnosticPull::default();
        let mut pull = WorkspaceDiagnosticPull::default();
        activate(&mut pull, "static", None, &store, &documents);
        let uri = "file:///tmp/bed-workspace-atomic.rs";
        pull.apply(
            &None,
            &pending(1, BTreeMap::new()),
            &report(uri, Some("valid"), None, "keep"),
            &store,
            &documents,
        )
        .unwrap();
        assert!(
            pull.apply(
                &None,
                &pending(2, BTreeMap::new()),
                &json!({"items":[
                    {"uri":uri,"kind":"full","resultId":"rejected","items":[]},
                    {"uri":"file:///tmp/bed-workspace-bad.rs","kind":"full","items":[false]}
                ]}),
                &store,
                &documents
            )
            .is_err()
        );
        assert_eq!(
            store.for_document("/tmp/bed-workspace-atomic.rs")[0].message,
            "keep"
        );
        let uri = canonical_uri(uri, &store).unwrap().0;
        assert_eq!(
            pull.providers[&None].cache[&uri].id.as_deref(),
            Some("valid")
        );
        assert!(pull.providers[&None].cache.len() == 1);
    }

    #[test]
    fn providers_aggregate_independently_and_unregister_only_their_own_items() {
        let store = LspDiagnostics::new();
        let documents = DocumentDiagnosticPull::default();
        let mut pull = WorkspaceDiagnosticPull::default();
        let first = Some("first".into());
        let second = Some("second".into());
        activate(&mut pull, "one", first.clone(), &store, &documents);
        activate(&mut pull, "two", second.clone(), &store, &documents);
        let uri = "file:///tmp/bed-workspace-providers.rs";
        pull.apply(
            &first,
            &pending(1, BTreeMap::new()),
            &report(uri, Some("a"), None, "one"),
            &store,
            &documents,
        )
        .unwrap();
        pull.apply(
            &second,
            &pending(2, BTreeMap::new()),
            &report(uri, Some("b"), None, "two"),
            &store,
            &documents,
        )
        .unwrap();
        assert_eq!(
            store.for_document("/tmp/bed-workspace-providers.rs").len(),
            2
        );
        pull.apply(
            &first,
            &pending(3, BTreeMap::new()),
            &report(uri, None, None, ""),
            &store,
            &documents,
        )
        .unwrap();
        assert_eq!(
            store.for_document("/tmp/bed-workspace-providers.rs")[0].message,
            "two"
        );
        pull.registrations.remove(&Some("one".into()));
        pull.reconcile(&store, &documents);
        assert_eq!(
            store.for_document("/tmp/bed-workspace-providers.rs")[0].message,
            "two"
        );
        pull.registrations.remove(&Some("two".into()));
        pull.reconcile(&store, &documents);
        assert!(
            store
                .for_document("/tmp/bed-workspace-providers.rs")
                .is_empty()
        );
    }

    #[test]
    fn lower_versions_never_mix_with_newer_provider_reports() {
        let store = LspDiagnostics::new();
        let documents = DocumentDiagnosticPull::default();
        let mut pull = WorkspaceDiagnosticPull::default();
        let first = Some("first".into());
        let second = Some("second".into());
        activate(&mut pull, "one", first.clone(), &store, &documents);
        activate(&mut pull, "two", second.clone(), &store, &documents);
        let uri = "file:///tmp/bed-workspace-versions.rs";
        pull.apply(
            &first,
            &pending(1, BTreeMap::new()),
            &report(uri, Some("a2"), Some(2), "first v2"),
            &store,
            &documents,
        )
        .unwrap();
        pull.apply(
            &second,
            &pending(2, BTreeMap::new()),
            &report(uri, Some("b1"), Some(1), "second stale"),
            &store,
            &documents,
        )
        .unwrap();
        assert!(pull.providers[&second].cache.is_empty());
        pull.apply(
            &second,
            &pending(3, BTreeMap::new()),
            &report(uri, Some("b2"), Some(2), "second v2"),
            &store,
            &documents,
        )
        .unwrap();
        assert_eq!(
            store.for_document("/tmp/bed-workspace-versions.rs").len(),
            2
        );
        pull.apply(
            &first,
            &pending(4, BTreeMap::new()),
            &report(uri, Some("a3"), Some(3), "first v3"),
            &store,
            &documents,
        )
        .unwrap();
        let items = store.for_document("/tmp/bed-workspace-versions.rs");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].message, "first v3");
        pull.registrations.remove(&Some("one".into()));
        pull.reconcile(&store, &documents);
        assert_eq!(
            store.for_document("/tmp/bed-workspace-versions.rs")[0].message,
            "second v2"
        );
    }

    #[test]
    fn rejected_stale_workspace_reports_preserve_current_unselected_open_buffer() {
        let store = LspDiagnostics::new();
        let mut documents = DocumentDiagnosticPull::default();
        let path = "/tmp/bed-workspace-unselected.txt";
        let uri = canonical_uri("file:///tmp/bed-workspace-unselected.txt", &store)
            .unwrap()
            .0;
        documents.open(uri.clone(), path.into(), "plaintext".into(), 1);
        store.replace(
            path,
            vec![DiagnosticItem {
                message: "fresh push".into(),
                ..Default::default()
            }],
            1,
        );
        let mut pull = WorkspaceDiagnosticPull::default();
        activate(&mut pull, "one", None, &store, &documents);
        pull.apply(
            &None,
            &pending(1, BTreeMap::new()),
            &report(&uri, Some("stale"), Some(0), "old"),
            &store,
            &documents,
        )
        .unwrap();
        assert_eq!(store.for_document(path)[0].message, "fresh push");
    }

    #[test]
    fn document_ownership_keeps_workspace_ids_internal_until_close_handoff() {
        let store = LspDiagnostics::new();
        let mut documents = DocumentDiagnosticPull::default();
        documents.set_static(Some(
            crate::document_diagnostics::Provider::parse(
                &json!({"interFileDependencies":false,"workspaceDiagnostics":true}),
            )
            .unwrap(),
        ));
        let path = "/tmp/bed-workspace-owned.py";
        let uri = canonical_uri("file:///tmp/bed-workspace-owned.py", &store)
            .unwrap()
            .0;
        documents.open(uri.clone(), path.into(), "python".into(), 1);
        store.replace(
            path,
            vec![DiagnosticItem {
                message: "unsaved buffer".into(),
                ..Default::default()
            }],
            1,
        );
        let mut pull = WorkspaceDiagnosticPull::default();
        activate(&mut pull, "one", None, &store, &documents);
        pull.apply(
            &None,
            &pending(1, BTreeMap::new()),
            &report(&uri, Some("disk"), Some(1), "disk"),
            &store,
            &documents,
        )
        .unwrap();
        assert_eq!(store.for_document(path)[0].message, "unsaved buffer");
        assert_eq!(
            pull.providers[&None].cache[&uri].id.as_deref(),
            Some("disk")
        );
        pull.close(&uri, &store);
        assert!(pull.providers[&None].cache.is_empty());
        assert!(pull.providers[&None].refresh);
    }

    #[test]
    fn completion_requires_every_provider_and_refresh_invalidates_it() {
        let store = LspDiagnostics::new();
        let documents = DocumentDiagnosticPull::default();
        let mut pull = WorkspaceDiagnosticPull::default();
        activate(&mut pull, "one", None, &store, &documents);
        activate(&mut pull, "two", Some(String::new()), &store, &documents);
        assert_eq!(pull.providers.len(), 2);
        assert!(!pull.complete());
        pull.providers.get_mut(&None).unwrap().completed = true;
        assert!(!pull.complete());
        pull.providers
            .get_mut(&Some(String::new()))
            .unwrap()
            .completed = true;
        assert!(pull.complete());
        pull.refresh();
        assert!(!pull.complete());
        pull.set_static(Some(provider(Some("anonymous".into()))), None);
        pull.set_static(
            Some(provider(Some("explicit".into()))),
            Some("$static".into()),
        );
        assert!(pull.registrations.contains_key(&None));
        assert!(pull.registrations.contains_key(&Some("$static".into())));
    }
}
