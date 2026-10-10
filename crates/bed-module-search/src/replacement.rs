//! One workspace-owned replacement job; all writes go through the host/session.
use crate::{MODULE_ID, content_search::ContentSearch};
use bed_document_session::{ByteEdit, DocumentId, EditorSession};
use bed_editing::{editor_operations::EditorOperations, text_search::CompiledSearch};
use bed_files::content_search::ContentMatch;
use bed_workbench_api::{EditToken, HostRequest, Revision, SaveToken, SavedDocument};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    path::Path,
    rc::Rc,
};

struct FileReplacement {
    path: String,
    matches: Vec<ContentMatch>,
}
struct PendingFile {
    path: String,
    document: DocumentId,
    count: usize,
    advance_offset: Option<usize>,
}
enum Stage {
    Ready,
    Opening(String),
    Applying(EditToken, PendingFile),
    Save(PendingFile),
    Saving(SaveToken, PendingFile),
    Saved(PendingFile),
}
pub(crate) struct ReplacementJob {
    owner: Rc<RefCell<ContentSearch>>,
    files: VecDeque<FileReplacement>,
    matcher: CompiledSearch,
    replacement: String,
    stage: Stage,
    seen: HashSet<DocumentId>,
    replaced: usize,
    saved: usize,
    failures: Vec<String>,
    next_only: bool,
    revisions: HashMap<DocumentId, Revision>,
}
impl ReplacementJob {
    pub fn new(owner: Rc<RefCell<ContentSearch>>, all: bool) -> Result<Self, String> {
        let search = owner.borrow();
        if !search.can_replace() {
            return Err("Wait for complete, current search results before replacing".into());
        }
        let matcher = CompiledSearch::new(search.query.as_bytes(), &search.options())?;
        let matches = if all {
            search.results.clone()
        } else {
            vec![search.results[search.selected_index].clone()]
        };
        let mut grouped: BTreeMap<String, Vec<ContentMatch>> = BTreeMap::new();
        for found in matches {
            grouped
                .entry(found.file.full_path.clone())
                .or_default()
                .push(found);
        }
        let files = grouped
            .into_iter()
            .map(|(path, matches)| FileReplacement { path, matches })
            .collect();
        let replacement = search.replacement.clone();
        let revisions = search
            .buffer_revisions
            .values()
            .map(|(document, revision)| (*document, *revision))
            .collect();
        drop(search);
        Ok(Self {
            owner,
            files,
            matcher,
            replacement,
            stage: Stage::Ready,
            seen: HashSet::new(),
            replaced: 0,
            saved: 0,
            failures: Vec::new(),
            next_only: !all,
            revisions,
        })
    }
    /// Advance at most one file per tick so broad replacements leave the UI responsive.
    pub fn tick(&mut self, session: &mut EditorSession, requests: &mut Vec<HostRequest>) -> bool {
        if !self.owner.borrow().active {
            self.files.clear();
        }
        match &self.stage {
            Stage::Applying(..) | Stage::Saving(..) => return false,
            Stage::Opening(path) => {
                if session.document_for_path(Path::new(path)).is_some() {
                    self.stage = Stage::Ready;
                } else if session.open_pending(Path::new(path)) && session.remote_connected() {
                    return false;
                } else {
                    self.failures
                        .push(format!("{path}: unable to open document"));
                    self.files.pop_front();
                    self.stage = Stage::Ready;
                }
            }
            Stage::Save(_) => {
                let Stage::Save(file) = std::mem::replace(&mut self.stage, Stage::Ready) else {
                    unreachable!()
                };
                let token = SaveToken::next();
                requests.push(HostRequest::SaveDocumentsWithResult {
                    recipient: MODULE_ID.into(),
                    token,
                    documents: vec![file.document],
                });
                self.stage = Stage::Saving(token, file);
                return false;
            }
            Stage::Saved(_) => {
                let Stage::Saved(file) = std::mem::replace(&mut self.stage, Stage::Ready) else {
                    unreachable!()
                };
                if let Some(offset) = file.advance_offset
                    && let Ok((row, column)) = session.with_document(file.document, |state| {
                        state.row_col_from_offset(offset.min(state.byte_size()))
                    })
                {
                    let relative = self
                        .owner
                        .borrow()
                        .results
                        .iter()
                        .find(|found| found.file.full_path == file.path)
                        .map(|found| found.file.relative_path.clone());
                    if let Some(relative) = relative {
                        self.owner.borrow_mut().advance_to = Some((relative, row, column as usize));
                    }
                }
            }
            Stage::Ready => {}
        }
        let Some(file) = self.files.front() else {
            let mut search = self.owner.borrow_mut();
            search.message = Some(format!(
                "Replaced {} occurrences · saved {} files{}",
                self.replaced,
                self.saved,
                if self.failures.is_empty() {
                    String::new()
                } else {
                    format!("\n{}", self.failures.join("\n"))
                }
            ));
            search.invalidate();
            return true;
        };
        self.owner.borrow_mut().message = Some(format!(
            "Replacing… {} files saved · {} files remaining",
            self.saved,
            self.files.len()
        ));
        let document = match session.request_open_file(Path::new(&file.path)) {
            Ok(Some(document)) => document,
            Ok(None) => {
                self.stage = Stage::Opening(file.path.clone());
                return false;
            }
            Err(error) => {
                self.failures.push(format!("{}: {error}", file.path));
                self.files.pop_front();
                return false;
            }
        };
        let file = self.files.pop_front().unwrap();
        // A symlink alias can discover the same document twice. Mutate it only once.
        if self.seen.contains(&document) {
            return false;
        }
        let result = (|| -> Result<(Revision, Vec<ByteEdit>, Option<usize>), String> {
            let revision = session
                .document_revision(document)
                .map_err(|error| error.to_string())?;
            if self
                .revisions
                .get(&document)
                .is_some_and(|expected| *expected != revision)
            {
                return Err("Document changed; refresh before replacing".into());
            }
            if session
                .snapshot(document)
                .map_err(|error| error.to_string())?
                .disk_conflict
                .is_some()
            {
                return Err("Resolve the disk conflict before replacing".into());
            }
            session
                .with_document(document, |state| {
                    let mut edits = Vec::new();
                    let mut advance = None;
                    for found in &file.matches {
                        if found.row < 0
                            || found.row >= state.line_count()
                            || state.line(found.row).as_slice() != &*found.line
                        {
                            return Err("Match changed; refresh before replacing".to_owned());
                        }
                        let bytes = self.matcher.replacement(
                            &found.line,
                            found.range.clone(),
                            &self.replacement,
                        )?;
                        let start = state.offset_from_row_col(found.row, found.range.start as i32);
                        let end = state.offset_from_row_col(found.row, found.range.end as i32);
                        if self.next_only {
                            let normalized =
                                EditorOperations::normalize_line_endings(state, &bytes);
                            advance = Some(
                                start + normalized.len() + usize::from(found.range.is_empty()),
                            );
                        }
                        edits.push(ByteEdit {
                            range: start..end,
                            bytes,
                        });
                    }
                    Ok((revision, edits, advance))
                })
                .map_err(|error| error.to_string())?
        })();
        match result {
            Ok((revision, edits, advance_offset)) => {
                self.seen.insert(document);
                let token = EditToken::next();
                let pending = PendingFile {
                    path: file.path,
                    document,
                    count: edits.len(),
                    advance_offset,
                };
                requests.push(HostRequest::ApplyModuleEditsWithResult {
                    recipient: MODULE_ID.into(),
                    token,
                    document,
                    revision,
                    edits,
                });
                self.stage = Stage::Applying(token, pending);
            }
            Err(error) => self.failures.push(format!("{}: {error}", file.path)),
        }
        false
    }
    pub fn owns(&self, search: &Rc<RefCell<ContentSearch>>) -> bool {
        Rc::ptr_eq(&self.owner, search)
    }
    pub fn edit_result(&mut self, token: EditToken, result: Result<Revision, String>) {
        if !matches!(&self.stage, Stage::Applying(expected, _) if *expected == token) {
            return;
        }
        let Stage::Applying(_, file) = std::mem::replace(&mut self.stage, Stage::Ready) else {
            unreachable!()
        };
        match result {
            Ok(_) => {
                self.replaced += file.count;
                self.stage = Stage::Save(file);
            }
            Err(error) => self.failures.push(format!("{}: {error}", file.path)),
        }
    }
    pub fn save_result(&mut self, token: SaveToken, result: Result<Vec<SavedDocument>, String>) {
        if !matches!(&self.stage, Stage::Saving(expected, _) if *expected == token) {
            return;
        }
        let Stage::Saving(_, file) = std::mem::replace(&mut self.stage, Stage::Ready) else {
            unreachable!()
        };
        match result {
            Ok(_) => {
                self.saved += 1;
                self.stage = Stage::Saved(file);
            }
            Err(error) => self.failures.push(format!(
                "{}: {error} (replacement remains unsaved)",
                file.path
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::{
        fs, thread,
        time::{Duration, Instant},
    };

    fn search(
        session: &EditorSession,
        root: &Path,
        query: &str,
        replacement: &str,
        regex: bool,
    ) -> Rc<RefCell<ContentSearch>> {
        let mut search = ContentSearch::new_lazy();
        search.open();
        search.query = query.into();
        search.replacement = replacement.into();
        search.regex = regex;
        search.tick(root.to_str().unwrap(), session);
        thread::sleep(Duration::from_millis(210));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            search.tick(root.to_str().unwrap(), session);
            if !search.needs_tick() {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(search.error.is_none(), "{:?}", search.error);
        Rc::new(RefCell::new(search))
    }
    fn run(job: &mut ReplacementJob, session: &mut EditorSession, fail_save: bool) {
        for _ in 0..100 {
            let mut requests = Vec::new();
            if job.tick(session, &mut requests) {
                return;
            }
            for request in requests {
                match request {
                    HostRequest::ApplyModuleEditsWithResult {
                        token,
                        document,
                        revision,
                        edits,
                        ..
                    } => {
                        let result = session
                            .apply_edits(document, revision, &edits)
                            .and_then(|_| session.document_revision(document))
                            .map_err(|error| error.to_string());
                        job.edit_result(token, result);
                    }
                    HostRequest::SaveDocumentsWithResult {
                        token, documents, ..
                    } => {
                        let result = if fail_save {
                            Err("simulated write failure".into())
                        } else {
                            documents
                                .into_iter()
                                .map(|document| {
                                    session.save(document).map_err(|error| error.to_string())?;
                                    Ok(SavedDocument {
                                        document,
                                        revision: session.document_revision(document).unwrap(),
                                    })
                                })
                                .collect()
                        };
                        job.save_result(token, result);
                    }
                    _ => panic!("Unexpected replacement request"),
                }
            }
        }
        panic!("Replacement did not finish");
    }
    #[test]
    fn replace_all_saves_dirty_buffers_and_each_file_has_one_undo_unit() {
        let temp = TempDir::new();
        let first = temp.write("a", b"value=1\nvalue=2\n");
        let second = temp.write("b", b"value=3\n");
        let mut session = EditorSession::new();
        let document = session.open_file(&first).unwrap();
        let revision = session.document_revision(document).unwrap();
        session
            .apply_edits(
                document,
                revision,
                &[ByteEdit {
                    range: 0..0,
                    bytes: b"unsaved\n".to_vec(),
                }],
            )
            .unwrap();
        let search = search(&session, temp.root(), r"value=(\d+)", "number:$1", true);
        assert_eq!(search.borrow().results.len(), 3);
        let mut job = ReplacementJob::new(search.clone(), true).unwrap();
        run(&mut job, &mut session, false);
        assert_eq!(fs::read(&first).unwrap(), b"unsaved\nnumber:1\nnumber:2\n");
        assert_eq!(fs::read(&second).unwrap(), b"number:3\n");
        assert!(!session.snapshot(document).unwrap().dirty);
        session.undo_document(document).unwrap();
        assert_eq!(
            session.snapshot(document).unwrap().bytes,
            b"unsaved\nvalue=1\nvalue=2\n"
        );
        assert!(
            search
                .borrow()
                .message
                .as_ref()
                .unwrap()
                .contains("Replaced 3 occurrences · saved 2 files")
        );
    }
    #[test]
    fn next_deletion_advances_and_write_failure_preserves_dirty_replacement() {
        let temp = TempDir::new();
        let path = temp.write("a", b"cat cat cat");
        let mut session = EditorSession::new();
        let search = search(&session, temp.root(), "cat", "", false);
        search.borrow_mut().selected_index = 1;
        let mut job = ReplacementJob::new(search.clone(), false).unwrap();
        run(&mut job, &mut session, false);
        assert_eq!(fs::read(&path).unwrap(), b"cat  cat");
        assert_eq!(search.borrow().advance_to, Some(("a".into(), 0, 4)));
        let search = self::search(&session, temp.root(), "cat", "dog", false);
        let mut job = ReplacementJob::new(search.clone(), true).unwrap();
        run(&mut job, &mut session, true);
        let document = session.document_for_path(&path).unwrap();
        assert_eq!(session.snapshot(document).unwrap().bytes, b"dog  dog");
        assert!(session.snapshot(document).unwrap().dirty);
        assert_eq!(fs::read(&path).unwrap(), b"cat  cat");
        assert!(
            search
                .borrow()
                .message
                .as_ref()
                .unwrap()
                .contains("replacement remains unsaved")
        );
    }
    #[test]
    fn changed_buffers_and_truncated_results_cannot_be_replaced() {
        let temp = TempDir::new();
        let path = temp.write("a", b"cat");
        let mut session = EditorSession::new();
        let document = session.open_file(&path).unwrap();
        let search = search(&session, temp.root(), "cat", "dog", false);
        search.borrow_mut().limit_reached = true;
        assert!(ReplacementJob::new(search.clone(), true).is_err());
        search.borrow_mut().limit_reached = false;
        let mut job = ReplacementJob::new(search.clone(), true).unwrap();
        let revision = session.document_revision(document).unwrap();
        session
            .apply_edits(
                document,
                revision,
                &[ByteEdit {
                    range: 0..3,
                    bytes: b"new".to_vec(),
                }],
            )
            .unwrap();
        run(&mut job, &mut session, false);
        assert_eq!(session.snapshot(document).unwrap().bytes, b"new");
        assert_eq!(fs::read(&path).unwrap(), b"cat");
        assert!(
            search
                .borrow()
                .message
                .as_ref()
                .unwrap()
                .contains("Document changed")
        );
    }
}
