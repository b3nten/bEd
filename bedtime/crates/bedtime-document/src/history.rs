//! Transaction history. It owns representation changes, never source I/O.
use crate::DocumentModel;
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct DocumentCommit<C> {
    pub revision: u64,
    pub changes: Option<Vec<C>>,
}
enum Record<M: DocumentModel> {
    Changes(Vec<M::Change>),
    Replace(M),
}
struct Entry<M: DocumentModel, G> {
    record: Record<M>,
    label: String,
    group: Option<G>,
}
pub struct History<M: DocumentModel, G: Copy + Eq> {
    content: M,
    revision: u64,
    commits: VecDeque<DocumentCommit<M::Change>>,
    undo: VecDeque<Entry<M, G>>,
    redo: Vec<Entry<M, G>>,
    limit: usize,
}
impl<M: DocumentModel, G: Copy + Eq> History<M, G> {
    pub fn new(content: M, limit: usize) -> Self {
        Self {
            content,
            revision: 0,
            commits: VecDeque::new(),
            undo: VecDeque::new(),
            redo: Vec::new(),
            limit,
        }
    }
    pub fn content(&self) -> &M {
        &self.content
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn undo_label(&self) -> Option<String> {
        self.undo.back().map(|e| e.label.clone())
    }
    pub fn redo_label(&self) -> Option<String> {
        self.redo.last().map(|e| e.label.clone())
    }
    pub fn changes_since(&self, revision: u64) -> Option<Vec<DocumentCommit<M::Change>>> {
        if revision == self.revision {
            return Some(Vec::new());
        }
        if revision > self.revision
            || self
                .commits
                .front()
                .is_none_or(|c| c.revision > revision + 1)
        {
            return None;
        }
        Some(
            self.commits
                .iter()
                .filter(|c| c.revision > revision)
                .cloned()
                .collect(),
        )
    }
    pub fn edit(
        &mut self,
        changes: impl IntoIterator<Item = M::Change>,
        label: String,
        group: Option<G>,
    ) -> Result<u64, String> {
        let mut candidate = self.content.clone();
        let mut inverses = Vec::new();
        let mut committed = Vec::new();
        for change in changes {
            inverses.push(candidate.apply(&change)?);
            committed.push(change);
        }
        if candidate == self.content {
            return Ok(self.revision);
        }
        inverses.reverse();
        self.redo.clear();
        if group.is_some() && self.undo.back().is_some_and(|e| e.group == group) {
            if let Record::Changes(previous) = &mut self.undo.back_mut().unwrap().record {
                inverses.append(previous);
                *previous = inverses;
            }
        } else {
            self.push(Entry {
                record: Record::Changes(inverses),
                label,
                group,
            });
        }
        self.content = candidate;
        self.commit(Some(committed));
        Ok(self.revision)
    }
    pub fn replace(&mut self, model: M, label: &str) {
        if model == self.content {
            return;
        }
        let previous = std::mem::replace(&mut self.content, model);
        self.redo.clear();
        self.push(Entry {
            record: Record::Replace(previous),
            label: label.into(),
            group: None,
        });
        self.commit(None);
    }
    pub fn undo(&mut self, redo: bool) -> Result<Option<u64>, String> {
        let entry = if redo {
            self.redo.last()
        } else {
            self.undo.back()
        };
        let Some(entry) = entry else { return Ok(None) };
        let mut candidate = self.content.clone();
        let (inverse, changes) = match &entry.record {
            Record::Replace(model) => {
                candidate = model.clone();
                (Record::Replace(self.content.clone()), None)
            }
            Record::Changes(changes) => {
                let mut inverses = Vec::new();
                for change in changes {
                    inverses.push(candidate.apply(change)?);
                }
                inverses.reverse();
                (Record::Changes(inverses), Some(changes.clone()))
            }
        };
        let inverse = Entry {
            record: inverse,
            label: entry.label.clone(),
            group: None,
        };
        if redo {
            self.redo.pop();
            self.push(inverse);
        } else {
            self.undo.pop_back();
            self.redo.push(inverse);
        }
        self.content = candidate;
        self.commit(changes);
        Ok(Some(self.revision))
    }
    fn push(&mut self, entry: Entry<M, G>) {
        if self.limit == 0 {
            return;
        }
        self.undo.push_back(entry);
        while self.undo.len() > self.limit {
            self.undo.pop_front();
        }
    }
    fn commit(&mut self, changes: Option<Vec<M::Change>>) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("Document revision exhausted");
        self.commits.push_back(DocumentCommit {
            revision: self.revision,
            changes,
        });
        while self.commits.len() > 128 {
            self.commits.pop_front();
        }
    }
}
