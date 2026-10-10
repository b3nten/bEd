use crate::controller::GitController;
use bed_document_session::{DocumentId, EditorSession, editor_session::ByteEdit};
use bed_editor_ui::{
    ConflictChoice, SourceConflict, SourceGitAction, SourceGitPresentation,
    extensions::SourceGitExtension,
};
use bed_git::RepositoryOperation;
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    io,
    ops::Range,
};

#[derive(Clone, Debug)]
struct Conflict {
    id: u64,
    row: i32,
    range: Range<usize>,
    current: Range<usize>,
    incoming: Range<usize>,
    current_label: String,
    incoming_label: String,
}

impl SourceGitExtension for GitController {
    fn presentation(
        &self,
        session: &EditorSession,
        document: DocumentId,
    ) -> io::Result<Option<SourceGitPresentation>> {
        let Some(status) = self.status.as_ref() else {
            return Ok(None);
        };
        if !status.entries.iter().any(|entry| entry.conflicted) {
            return Ok(None);
        }
        let path = session.with_document(document, |state| state.path.clone())?;
        if !status
            .entries
            .iter()
            .any(|entry| entry.conflicted && self.absolute(&entry.path).to_string_lossy() == path)
        {
            return Ok(None);
        }
        let revision = session.document_revision(document)?;
        if let Some((cached_revision, operation, presentation)) =
            self.conflict_cache.borrow().get(&document)
            && *cached_revision == revision
            && *operation == status.operation
        {
            return Ok(Some(presentation.clone()));
        }
        let snapshot = session.snapshot(document)?;
        let rebase = status.operation == RepositoryOperation::Rebase;
        let conflicts = parse(&snapshot.bytes)
            .into_iter()
            .map(|conflict| SourceConflict {
                id: conflict.id,
                row: conflict.row,
                label: "Merge conflict".into(),
                current_label: if rebase {
                    format!("Base branch ({})", conflict.current_label)
                } else {
                    conflict.current_label
                },
                incoming_label: if rebase {
                    format!("Replayed commit ({})", conflict.incoming_label)
                } else {
                    conflict.incoming_label
                },
            })
            .collect();
        let presentation = SourceGitPresentation { conflicts };
        self.conflict_cache
            .borrow_mut()
            .insert(document, (revision, status.operation, presentation.clone()));
        Ok(Some(presentation))
    }
    fn action(
        &mut self,
        session: &mut EditorSession,
        document: DocumentId,
        action: SourceGitAction,
    ) -> io::Result<()> {
        let SourceGitAction::ResolveConflict { id, choice } = action;
        let revision = session.document_revision(document)?;
        let snapshot = session.snapshot(document)?;
        let conflict = parse(&snapshot.bytes)
            .into_iter()
            .find(|conflict| conflict.id == id)
            .ok_or_else(|| {
                io::Error::other("The conflict changed; refresh before choosing a resolution")
            })?;
        let mut bytes = Vec::new();
        if matches!(choice, ConflictChoice::Current | ConflictChoice::Both) {
            bytes.extend_from_slice(&snapshot.bytes[conflict.current]);
        }
        if matches!(choice, ConflictChoice::Incoming | ConflictChoice::Both) {
            bytes.extend_from_slice(&snapshot.bytes[conflict.incoming]);
        }
        session.apply_edits(
            document,
            revision,
            &[ByteEdit {
                range: conflict.range,
                bytes,
            }],
        )?;
        Ok(())
    }
}

/// Recognize complete ordinary/diff3 blocks. Nested or malformed blocks never
/// expose a destructive choice; users can still edit their text manually.
fn parse(bytes: &[u8]) -> Vec<Conflict> {
    let mut result = Vec::new();
    let mut lines = Vec::new();
    let mut offset = 0;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        lines.push((offset, offset + line.len(), line));
        offset += line.len();
    }
    let mut index = 0;
    while index < lines.len() {
        let (start, current_start, line) = lines[index];
        if !marker(line, b"<<<<<<<") {
            index += 1;
            continue;
        }
        let row = index as i32;
        let current_label = marker_label(line, "Current");
        let mut scan = index + 1;
        let mut base = None;
        let mut divider = None;
        let mut end = None;
        let mut malformed = false;
        while scan < lines.len() {
            let (_, _, line) = lines[scan];
            if marker(line, b"<<<<<<<") {
                malformed = true;
                break;
            }
            if marker(line, b"|||||||") {
                if base.is_some() || divider.is_some() {
                    malformed = true;
                    break;
                }
                base = Some(scan);
            }
            if marker(line, b"=======") {
                if divider.is_some() {
                    malformed = true;
                    break;
                }
                divider = Some(scan);
            }
            if marker(line, b">>>>>>>") {
                end = Some(scan);
                break;
            }
            scan += 1;
        }
        if malformed {
            // Skip the entire outer block, including nested marker starts.
            while scan < lines.len() && !marker(lines[scan].2, b">>>>>>>") {
                scan += 1;
            }
            index = scan.saturating_add(1);
            continue;
        }
        if let (Some(divider), Some(end)) = (divider, end) {
            let current_end = lines[base.unwrap_or(divider)].0;
            let incoming_start = lines[divider].1;
            let incoming_end = lines[end].0;
            let block_end = lines[end].1;
            let mut hasher = DefaultHasher::new();
            start.hash(&mut hasher);
            bytes[start..block_end].hash(&mut hasher);
            result.push(Conflict {
                id: hasher.finish(),
                row,
                range: start..block_end,
                current: current_start..current_end,
                incoming: incoming_start..incoming_end,
                current_label,
                incoming_label: marker_label(lines[end].2, "Incoming"),
            });
            index = end + 1;
        } else {
            index = scan.saturating_add(1);
        }
    }
    result
}
fn marker(line: &[u8], prefix: &[u8]) -> bool {
    line.starts_with(prefix)
        && line
            .get(prefix.len())
            .is_none_or(|byte| matches!(byte, b' ' | b'\r' | b'\n'))
}
fn marker_label(line: &[u8], default: &str) -> String {
    let label = String::from_utf8_lossy(&line[7..]).trim().to_owned();
    if label.is_empty() {
        default.into()
    } else {
        label
    }
}

pub(crate) fn has_conflict_markers(bytes: &[u8]) -> bool {
    bytes.split(|b| *b == b'\n').any(|line| {
        marker(line, b"<<<<<<<")
            || marker(line, b"=======")
            || marker(line, b">>>>>>>")
            || marker(line, b"|||||||")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_and_diff3_conflicts_keep_precise_byte_ranges() {
        for text in [
            "before\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> topic\nafter\n",
            "before\n<<<<<<< HEAD\nours\n||||||| base\nold\n=======\ntheirs\n>>>>>>> topic\nafter\n",
        ] {
            let conflict = parse(text.as_bytes()).pop().unwrap();
            assert_eq!(conflict.row, 1);
            assert_eq!(&text.as_bytes()[conflict.current], b"ours\n");
            assert_eq!(&text.as_bytes()[conflict.incoming], b"theirs\n");
            assert_eq!(&text[conflict.range], &text[7..text.len() - 6]);
        }
    }
    #[test]
    fn malformed_and_nested_blocks_offer_no_automatic_resolution() {
        assert!(parse(b"<<<<<<< ours\ntext\n=======\ntext\n").is_empty());
        assert!(
            parse(
                b"<<<<<<< ours\n<<<<<<< nested\na\n=======\nb\n>>>>>>> n\n=======\nc\n>>>>>>> o\n"
            )
            .is_empty()
        );
    }
    #[test]
    fn choosing_both_is_one_shared_undo_transaction() {
        let bytes = b"<<<<<<< ours\na\n=======\nb\n>>>>>>> theirs\n";
        let mut session = EditorSession::default();
        let document = session.create_document(bytes).unwrap();
        let conflict = parse(bytes).pop().unwrap();
        let mut controller = GitController::default();
        controller
            .action(
                &mut session,
                document,
                SourceGitAction::ResolveConflict {
                    id: conflict.id,
                    choice: ConflictChoice::Both,
                },
            )
            .unwrap();
        assert_eq!(session.snapshot(document).unwrap().bytes, b"a\nb\n");
        let view = session.create_view(document).unwrap();
        session
            .with_commands(view, |commands| commands.undo())
            .unwrap();
        assert_eq!(session.snapshot(document).unwrap().bytes, bytes);
    }
    #[test]
    fn changed_conflict_identity_cannot_replace_a_different_block() {
        let bytes = b"<<<<<<< ours\na\n=======\nb\n>>>>>>> theirs\n";
        let mut session = EditorSession::default();
        let document = session.create_document(bytes).unwrap();
        let original = parse(bytes).pop().unwrap().id;
        session
            .apply_edits(
                document,
                session.document_revision(document).unwrap(),
                &[ByteEdit {
                    range: 13..14,
                    bytes: b"changed".to_vec(),
                }],
            )
            .unwrap();
        let before = session.snapshot(document).unwrap().bytes;
        assert!(
            GitController::default()
                .action(
                    &mut session,
                    document,
                    SourceGitAction::ResolveConflict {
                        id: original,
                        choice: ConflictChoice::Current
                    }
                )
                .is_err()
        );
        assert_eq!(session.snapshot(document).unwrap().bytes, before);
    }
}
