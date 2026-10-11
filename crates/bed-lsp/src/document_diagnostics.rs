//! Document diagnostic pulls have one writer: the client's poll/lifecycle path.
//! Transport callbacks only enqueue replies and registration requests.
use crate::{
    diagnostics::{DiagnosticItem, LspDiagnostics},
    jsonrpc::{ResponseError, RpcId},
    lsp_client::decode_diagnostics,
    lsp_uri::LspUri,
    message_handler::RpcSession,
};
use bed_editing::util::doc_path;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, VecDeque},
    rc::Rc,
    time::{Duration, Instant},
};

type Key = (String, String); // provider identifier, canonical URI
const EDIT_DELAY: Duration = Duration::from_millis(75);
const MAX_PENDING: usize = 4;

struct Filter {
    language: Option<String>,
    scheme: Option<String>,
    pattern: Option<globset::GlobMatcher>,
}
pub(crate) struct Provider {
    registration_id: Option<String>,
    identifier: Option<String>,
    selector: Option<Vec<Filter>>,
    inter_file: bool,
}
impl Provider {
    pub(crate) fn parse(value: &Value) -> Result<Self, ResponseError> {
        if !value.is_object() {
            return Err(ResponseError::invalid_params(
                "Expected diagnostic provider options",
            ));
        }
        let string = |value: &Value| -> Result<Option<String>, ResponseError> {
            if value.is_null() {
                Ok(None)
            } else {
                value.as_str().map(|s| Some(s.to_owned())).ok_or_else(|| {
                    ResponseError::invalid_params("Expected diagnostic provider string")
                })
            }
        };
        let selector = if value["documentSelector"].is_null() {
            None
        } else {
            let values = value["documentSelector"].as_array().ok_or_else(|| {
                ResponseError::invalid_params("Expected diagnostic document selector")
            })?;
            Some(
                values
                    .iter()
                    .map(|value| {
                        if let Some(language) = value.as_str() {
                            return Ok(Filter {
                                language: Some(language.into()),
                                scheme: None,
                                pattern: None,
                            });
                        }
                        if !value.is_object() {
                            return Err(ResponseError::invalid_params(
                                "Expected diagnostic document filter",
                            ));
                        }
                        let pattern = string(&value["pattern"])?
                            .map(|pattern| {
                                globset::GlobBuilder::new(&pattern)
                                    .literal_separator(true)
                                    .build()
                                    .map(|glob| glob.compile_matcher())
                                    .map_err(|error| {
                                        ResponseError::invalid_params(error.to_string())
                                    })
                            })
                            .transpose()?;
                        let language = string(&value["language"])?;
                        let scheme = string(&value["scheme"])?;
                        if language.is_none() && scheme.is_none() && pattern.is_none() {
                            return Err(ResponseError::invalid_params(
                                "Expected a text document selector field",
                            ));
                        }
                        Ok(Filter {
                            language,
                            scheme,
                            pattern,
                        })
                    })
                    .collect::<Result<Vec<_>, ResponseError>>()?,
            )
        };
        for name in ["interFileDependencies", "workspaceDiagnostics"] {
            if !value[name].is_null() && !value[name].is_boolean() {
                return Err(ResponseError::invalid_params(
                    "Expected diagnostic provider boolean",
                ));
            }
        }
        Ok(Self {
            registration_id: string(&value["id"])?,
            identifier: string(&value["identifier"])?,
            selector,
            inter_file: value["interFileDependencies"].as_bool().unwrap_or(false),
        })
    }
    fn identity(&self) -> String {
        self.identifier
            .as_ref()
            .map(|id| format!("id:{id}"))
            .unwrap_or_else(|| "$default".into())
    }
    fn matches(&self, document: &Document) -> bool {
        let Some(filters) = &self.selector else {
            return true;
        };
        let Ok(uri) = LspUri::parse(&document.uri) else {
            return false;
        };
        filters.iter().any(|filter| {
            filter
                .language
                .as_ref()
                .is_none_or(|id| id == "*" || id == &document.language)
                && filter
                    .scheme
                    .as_ref()
                    .is_none_or(|scheme| scheme == "*" || scheme == uri.scheme())
                && filter
                    .pattern
                    .as_ref()
                    .is_none_or(|glob| glob.is_match(document.path.replace('\\', "/")))
        })
    }
}
pub(crate) enum Input {
    Register(String, Provider),
    Unregister(String),
    Refresh,
    FilesChanged(Vec<String>),
}
pub(crate) type Inputs = Rc<RefCell<VecDeque<Input>>>;
struct Document {
    uri: String,
    path: String,
    language: String,
    version: i32,
    revision: u64,
    synchronized: bool,
}
#[derive(Default)]
struct Job {
    due: Option<Instant>,
    pending: Option<Pending>,
    retry_delay: u64,
}
struct Pending {
    id: RpcId,
    sequence: u64,
    revision_watermark: u64,
    primary_revision: u64,
    cache_ids: BTreeMap<String, String>,
}
struct Reply {
    key: Key,
    sequence: u64,
    result: Result<Value, ResponseError>,
}
struct Cached {
    id: Option<String>,
    items: Vec<DiagnosticItem>,
    revision: Option<u64>,
    sequence: u64,
    origin: String,
}

#[derive(Default)]
pub(crate) struct DocumentDiagnosticPull {
    providers: BTreeMap<Option<String>, Provider>, // None is the unregistered static provider
    documents: BTreeMap<String, Document>,
    revisions: BTreeMap<String, u64>, // retain closed lifetimes to reject old related reports
    next_revision: u64,
    next_request: u64,
    jobs: BTreeMap<Key, Job>,
    cache: BTreeMap<Key, Cached>,
    cancel: Vec<RpcId>,
    released: BTreeSet<String>,
    pub(crate) inputs: Inputs,
    replies: Rc<RefCell<VecDeque<Reply>>>,
}
impl DocumentDiagnosticPull {
    fn bump(&mut self, uri: &str) -> u64 {
        self.next_revision += 1;
        self.revisions.insert(uri.into(), self.next_revision);
        self.next_revision
    }
    pub(crate) fn contains(&self, uri: &str) -> bool {
        self.documents.contains_key(uri)
    }
    pub(crate) fn take_released_uris(&mut self) -> Vec<String> {
        std::mem::take(&mut self.released).into_iter().collect()
    }
    pub(crate) fn set_static(&mut self, provider: Option<Provider>) {
        if let Some(provider) = provider {
            let id = provider.registration_id.clone();
            self.providers.insert(id, provider);
        }
    }
    pub(crate) fn open(&mut self, uri: String, path: String, language: String, version: i32) {
        if self.documents.contains_key(&uri) {
            self.change(&uri, version, true);
            return;
        }
        let revision = self.bump(&uri);
        self.documents.insert(
            uri.clone(),
            Document {
                uri: uri.clone(),
                path,
                language,
                version,
                revision,
                synchronized: true,
            },
        );
        self.dirty(&uri, Duration::ZERO, true);
    }
    pub(crate) fn change(&mut self, uri: &str, version: i32, synchronized: bool) {
        let revision = self.bump(uri);
        if let Some(document) = self.documents.get_mut(uri) {
            document.version = version;
            document.revision = revision;
            document.synchronized = synchronized;
        }
        self.dirty(uri, EDIT_DELAY, true);
    }
    pub(crate) fn save(&mut self, uri: &str) {
        if let Some(document) = self.documents.get_mut(uri) {
            document.synchronized = true;
        }
        self.dirty(uri, Duration::ZERO, true);
    }
    pub(crate) fn close(&mut self, uri: &str, store: &LspDiagnostics) -> bool {
        let document = self.documents.remove(uri);
        self.bump(uri);
        self.dirty(uri, EDIT_DELAY, true);
        let had_report = self.cache.keys().any(|(_, cached_uri)| cached_uri == uri);
        self.cache.retain(|(_, cached_uri), _| cached_uri != uri);
        let keys: Vec<_> = self
            .jobs
            .keys()
            .filter(|(_, job_uri)| job_uri == uri)
            .cloned()
            .collect();
        for key in keys {
            self.remove_job(&key);
        }
        if had_report && let Some(document) = document {
            store.clear(&document.path);
            self.released.insert(uri.into());
        }
        had_report
    }
    pub(crate) fn refresh(&mut self) {
        let now = Instant::now();
        for job in self.jobs.values_mut() {
            if let Some(pending) = job.pending.take() {
                self.cancel.push(pending.id);
            }
            job.due = Some(now);
            job.retry_delay = 0;
        }
    }
    fn dirty(&mut self, uri: &str, delay: Duration, dependencies: bool) {
        let inter_file: BTreeSet<_> = self
            .providers
            .values()
            .filter(|provider| provider.inter_file)
            .map(Provider::identity)
            .collect();
        for ((provider, job_uri), job) in &mut self.jobs {
            if job_uri == uri || (dependencies && inter_file.contains(provider)) {
                if let Some(pending) = job.pending.take() {
                    self.cancel.push(pending.id);
                }
                job.due = Some(Instant::now() + delay);
                job.retry_delay = 0;
            }
        }
    }
    fn remove_job(&mut self, key: &Key) {
        if let Some(job) = self.jobs.remove(key)
            && let Some(pending) = job.pending
        {
            self.cancel.push(pending.id);
        }
    }
    fn reconcile(&mut self, store: &LspDiagnostics) {
        let desired: BTreeSet<Key> = self
            .documents
            .values()
            .flat_map(|document| {
                self.providers
                    .values()
                    .filter(|provider| provider.matches(document))
                    .map(|provider| (provider.identity(), document.uri.clone()))
            })
            .collect();
        let removed: Vec<_> = self
            .jobs
            .keys()
            .filter(|key| !desired.contains(*key))
            .cloned()
            .collect();
        for key in removed {
            self.remove_job(&key);
        }
        for key in &desired {
            self.jobs.entry(key.clone()).or_insert_with(|| Job {
                due: Some(Instant::now()),
                ..Job::default()
            });
        }
        let active: BTreeSet<_> = self.providers.values().map(Provider::identity).collect();
        let removed_uris: BTreeSet<_> = self
            .cache
            .iter()
            .filter(|((provider, _), cached)| {
                !active.contains(provider)
                    || !desired.contains(&(provider.clone(), cached.origin.clone()))
            })
            .map(|((_, uri), _)| uri.clone())
            .collect();
        self.cache.retain(|(provider, _), cached| {
            active.contains(provider)
                && desired.contains(&(provider.clone(), cached.origin.clone()))
        });
        for uri in removed_uris {
            self.publish(&uri, store);
            if !self.cache.keys().any(|(_, cached_uri)| cached_uri == &uri) {
                self.released.insert(uri);
            }
        }
    }
    pub(crate) fn canonical_uri(
        uri: &str,
        store: &LspDiagnostics,
    ) -> Result<(String, String), ResponseError> {
        let parsed =
            LspUri::parse(uri).map_err(|error| ResponseError::invalid_params(error.to_string()))?;
        if parsed.scheme() != "file"
            || !parsed.path().starts_with('/')
            || !matches!(parsed.authority(), "" | "localhost")
            || parsed.has_query()
            || parsed.has_fragment()
        {
            return Err(ResponseError::invalid_params(
                "Expected local absolute file diagnostic URI",
            ));
        }
        let path = if store.uses_remote_paths() {
            parsed.path().to_owned()
        } else {
            doc_path::normalize(&parsed.fs_path())
        };
        let canonical = if store.uses_remote_paths() {
            LspUri::file_uri_from_remote_path(&path)
        } else {
            LspUri::file_uri_from_path(&path)
        }
        .map_err(|error| ResponseError::invalid_params(error.to_string()))?
        .to_string();
        Ok((canonical, path))
    }
    fn publish(&self, uri: &str, store: &LspDiagnostics) {
        let Ok((_, path)) = Self::canonical_uri(uri, store) else {
            return;
        };
        let revision = self.revisions.get(uri).copied();
        let items = self
            .cache
            .iter()
            .filter(|((_, cached_uri), report)| cached_uri == uri && report.revision == revision)
            .flat_map(|(_, report)| report.items.iter().cloned())
            .collect();
        let version = self
            .documents
            .get(uri)
            .map(|document| document.version)
            .unwrap_or(-1);
        store.replace(&path, items, version);
    }
    /// While a document pull owns an open buffer, workspace reports cannot
    /// describe its unsaved contents reliably (including null-version reports).
    pub(crate) fn accepts_workspace(
        &self,
        uri: &str,
        version: i32,
        store: &LspDiagnostics,
    ) -> bool {
        let Ok((uri, _)) = Self::canonical_uri(uri, store) else {
            return true;
        };
        let Some(document) = self.documents.get(&uri) else {
            return true;
        };
        if version >= 0 && version < document.version {
            return false;
        }
        !self
            .providers
            .values()
            .any(|provider| provider.matches(document))
    }
    fn apply(
        &mut self,
        key: &Key,
        pending: &Pending,
        report: &Value,
        store: &LspDiagnostics,
    ) -> Result<(), ResponseError> {
        if self.revisions.get(&key.1).copied() != Some(pending.primary_revision) {
            return Ok(());
        }
        // Decode the entire external report before committing any part of it.
        let mut reports = vec![(key.1.clone(), report)];
        if let Some(related) = report
            .get("relatedDocuments")
            .filter(|value| !value.is_null())
        {
            let related = related.as_object().ok_or_else(|| {
                ResponseError::invalid_params("Expected related diagnostic reports")
            })?;
            reports.extend(related.iter().map(|(uri, report)| (uri.clone(), report)));
        }
        let mut decoded = Vec::new();
        let mut seen = BTreeSet::new();
        for (uri, report) in reports {
            let (uri, _) = Self::canonical_uri(&uri, store)?;
            if !seen.insert(uri.clone()) {
                return Err(ResponseError::invalid_params(
                    "Duplicate canonical diagnostic URI",
                ));
            }
            let id = match report.get("resultId").filter(|value| !value.is_null()) {
                None => None,
                Some(value) => Some(
                    value
                        .as_str()
                        .ok_or_else(|| {
                            ResponseError::invalid_params("Expected diagnostic resultId")
                        })?
                        .to_owned(),
                ),
            };
            let items = match report["kind"].as_str() {
                Some("full") => Some(
                    decode_diagnostics(
                        store,
                        Some(&json!({"uri":uri,"diagnostics":report["items"]})),
                    )?
                    .1,
                ),
                Some("unchanged") if id.is_some() => None,
                _ => {
                    return Err(ResponseError::invalid_params(
                        "Expected full or unchanged diagnostic report",
                    ));
                }
            };
            let current = self.revisions.get(&uri).copied();
            if current.is_some_and(|revision| revision > pending.revision_watermark) {
                continue;
            }
            if self
                .documents
                .get(&uri)
                .is_some_and(|document| !document.synchronized)
            {
                // Related reports cannot describe an unsaved buffer whose
                // server negotiated no change synchronization.
                continue;
            }
            let cache_key = (key.0.clone(), uri.clone());
            if self
                .cache
                .get(&cache_key)
                .is_some_and(|cached| cached.sequence > pending.sequence)
            {
                // A newer pull already committed this related URI at the same
                // buffer version. The older primary cannot replace its report.
                continue;
            }
            let items = match items {
                Some(items) => items,
                None => {
                    let cached = self.cache.get(&cache_key).filter(|cached| {
                        cached.id.is_some() && cached.id.as_ref() == pending.cache_ids.get(&uri)
                    });
                    let Some(cached) = cached else {
                        return Err(ResponseError::invalid_params(
                            "Unchanged diagnostic report has no matching cached content",
                        ));
                    };
                    cached.items.clone()
                }
            };
            let origin = if self.jobs.contains_key(&cache_key) {
                // An open selected document owns its report independently of
                // which request supplied it through relatedDocuments.
                uri.clone()
            } else {
                key.1.clone()
            };
            decoded.push((
                cache_key,
                Cached {
                    id,
                    items,
                    revision: current,
                    sequence: pending.sequence,
                    origin,
                },
            ));
        }
        let affected: BTreeSet<_> = decoded.iter().map(|((_, uri), _)| uri.clone()).collect();
        self.cache.extend(decoded);
        for uri in affected {
            self.publish(&uri, store);
        }
        Ok(())
    }
    pub(crate) fn poll(
        &mut self,
        session: &mut RpcSession,
        ready: bool,
        store: &LspDiagnostics,
        last_error: &mut Option<String>,
    ) {
        let inputs: Vec<_> = self.inputs.borrow_mut().drain(..).collect();
        for input in inputs {
            match input {
                Input::Register(id, provider) => {
                    // A registration changes provider scope; pending work must
                    // be canceled before the new selector can own any result.
                    self.refresh();
                    self.providers.insert(Some(id), provider);
                }
                Input::Unregister(id) => {
                    self.refresh();
                    self.providers.remove(&Some(id));
                }
                Input::Refresh => self.refresh(),
                Input::FilesChanged(uris) => {
                    for uri in uris {
                        // A watched disk event does not change an open editor
                        // buffer, but invalidates captured closed-file reports.
                        if !self.documents.contains_key(&uri) {
                            self.bump(&uri);
                        }
                        self.dirty(&uri, EDIT_DELAY, true);
                    }
                }
            }
        }
        self.reconcile(store);
        for id in std::mem::take(&mut self.cancel) {
            let _ = session.cancel_request(&id);
        }
        let replies: Vec<_> = self.replies.borrow_mut().drain(..).collect();
        for reply in replies {
            let Some(job) = self.jobs.get_mut(&reply.key) else {
                continue;
            };
            if job
                .pending
                .as_ref()
                .is_none_or(|pending| pending.sequence != reply.sequence)
            {
                continue;
            }
            let pending = job.pending.take().unwrap();
            match reply.result {
                Ok(report) => {
                    if let Err(error) = self.apply(&reply.key, &pending, &report, store) {
                        *last_error = Some(format!("Document diagnostics: {error}"));
                        // Malformed unchanged results cannot safely carry a
                        // previous ID into the next request.
                        if error.message.starts_with("Unchanged diagnostic") {
                            if let Some(cache) = self.cache.get_mut(&reply.key) {
                                cache.id = None;
                            }
                            let job = self.jobs.get_mut(&reply.key).unwrap();
                            job.retry_delay = if job.retry_delay == 0 {
                                200
                            } else {
                                (job.retry_delay * 2).min(2000)
                            };
                            job.due = Some(Instant::now() + Duration::from_millis(job.retry_delay));
                        }
                    } else {
                        self.jobs.get_mut(&reply.key).unwrap().retry_delay = 0;
                    }
                }
                Err(error) => {
                    let retry = error.code == -32801
                        || (error.code == -32802
                            && error
                                .data
                                .as_ref()
                                .and_then(|data| data["retriggerRequest"].as_bool())
                                != Some(false));
                    if retry {
                        let job = self.jobs.get_mut(&reply.key).unwrap();
                        job.retry_delay = if job.retry_delay == 0 {
                            200
                        } else {
                            (job.retry_delay * 2).min(2000)
                        };
                        job.due = Some(Instant::now() + Duration::from_millis(job.retry_delay));
                    } else {
                        *last_error = Some(format!("Document diagnostics: {error}"));
                    }
                }
            }
        }
        if !ready {
            return;
        }
        let mut capacity = MAX_PENDING.saturating_sub(
            self.jobs
                .values()
                .filter(|job| job.pending.is_some())
                .count(),
        );
        let due: Vec<_> = self
            .jobs
            .iter()
            .filter(|(key, job)| {
                job.pending.is_none()
                    && job.due.is_some_and(|due| due <= Instant::now())
                    && self
                        .documents
                        .get(&key.1)
                        .is_some_and(|document| document.synchronized)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in due {
            if capacity == 0 {
                break;
            }
            self.next_request += 1;
            let sequence = self.next_request;
            let mut params = json!({"textDocument":{"uri":key.1}});
            if let Some(identifier) = self
                .providers
                .values()
                .find(|provider| provider.identity() == key.0)
                .and_then(|provider| provider.identifier.as_ref())
            {
                params["identifier"] = json!(identifier);
            }
            if let Some(id) = self.cache.get(&key).and_then(|cache| cache.id.as_ref()) {
                params["previousResultId"] = json!(id);
            }
            let queue = self.replies.clone();
            let reply_key = key.clone();
            match session.send_request("textDocument/diagnostic", Some(params), move |result| {
                queue.borrow_mut().push_back(Reply {
                    key: reply_key,
                    sequence,
                    result,
                });
            }) {
                Ok(id) => {
                    let cache_ids = self
                        .cache
                        .iter()
                        .filter(|((provider, _), _)| provider == &key.0)
                        .filter_map(|((_, uri), cache)| {
                            cache.id.as_ref().map(|id| (uri.clone(), id.clone()))
                        })
                        .collect();
                    let job = self.jobs.get_mut(&key).unwrap();
                    job.due = None;
                    job.pending = Some(Pending {
                        id,
                        sequence,
                        revision_watermark: self.next_revision,
                        primary_revision: self.documents[&key.1].revision,
                        cache_ids,
                    });
                    capacity -= 1;
                }
                Err(error) => {
                    self.jobs.get_mut(&key).unwrap().due = None;
                    *last_error = Some(error.to_string());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(path: &str, language: &str) -> Document {
        Document {
            uri: LspUri::file_uri_from_remote_path(path).unwrap().to_string(),
            path: path.into(),
            language: language.into(),
            version: 1,
            revision: 1,
            synchronized: true,
        }
    }
    #[test]
    fn selectors_combine_filter_fields_and_respect_protocol_glob_segments() {
        let selected = document("/project/deep/file.py", "python");
        let provider = Provider::parse(&json!({"documentSelector":[
            {"language":"python","scheme":"file","pattern":"**/*.py"},
            {"language":"rust"}
        ]}))
        .unwrap();
        assert!(provider.matches(&selected));
        assert!(!provider.matches(&document("/project/file.txt", "python")));
        assert!(!provider.matches(&document("/project/file.py", "plaintext")));
        assert!(provider.matches(&document("/project/file.txt", "rust")));
        let single_segment =
            Provider::parse(&json!({"documentSelector":[{"pattern":"/project/*.py"}]})).unwrap();
        assert!(!single_segment.matches(&selected));
        assert!(single_segment.matches(&document("/project/file.py", "python")));
        for selector in [
            json!(null),
            json!(["*"]),
            json!([{"language":"*","scheme":"*"}]),
        ] {
            assert!(
                Provider::parse(&json!({"documentSelector":selector}))
                    .unwrap()
                    .matches(&selected)
            );
        }
        assert!(
            !Provider::parse(&json!({"documentSelector":[]}))
                .unwrap()
                .matches(&selected)
        );
    }
    #[test]
    fn invalid_filters_and_foreign_file_uris_cannot_acquire_document_scope() {
        for selector in [
            json!([{}]),
            json!([{"notebook":"jupyter"}]),
            json!([{"language":null,"scheme":null,"pattern":null}]),
            json!([{"pattern":{"baseUri":"file:///project","pattern":"*.py"}}]),
        ] {
            assert!(Provider::parse(&json!({"documentSelector":selector})).is_err());
        }
        let store = LspDiagnostics::new();
        for uri in [
            "file://foreign-host/project/a.py",
            "file:///project/a.py?query",
            "file:///project/a.py#fragment",
            "untitled:///project/a.py",
        ] {
            assert!(DocumentDiagnosticPull::canonical_uri(uri, &store).is_err());
        }
        store.set_remote_paths(true);
        let (uri, path) = DocumentDiagnosticPull::canonical_uri(
            "file:///target/alias/../space%20file.py",
            &store,
        )
        .unwrap();
        assert_eq!(path, "/target/alias/../space file.py");
        assert_eq!(uri, "file:///target/alias/../space%20file.py");
    }
}
