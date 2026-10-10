//! Visual rows over a real document. Historical text never enters its buffer.
use crate::source_git::{SourceConflict, SourceGitPresentation};
use bed_editing::editor_state::EditorState;
use std::{ops::Range, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffActionKind {
    Stage,
    Unstage,
    Discard,
}
impl DiffActionKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Stage => "Stage",
            Self::Unstage => "Unstage",
            Self::Discard => "Discard",
        }
    }
}
#[derive(Clone, Debug)]
pub struct DiffPresentation {
    pub baseline: Arc<[u8]>,
    /// Change this identity whenever the baseline or saved snapshot changes. The view
    /// caches its projection independently of the allocation backing the bytes.
    pub comparison: u64,
    pub actions: Vec<DiffActionKind>,
    /// Keep hunk controls visible while a pending operation prevents activation.
    /// This flag does not change the projection or its cached identity.
    pub actions_enabled: bool,
    /// Authoritative zero-context hunks for the saved comparison. Unsaved text
    /// is projected locally until the next saved snapshot becomes available.
    pub saved: Option<DiffSnapshot>,
}
#[derive(Clone, Debug)]
pub struct DiffSnapshot {
    pub current: Arc<[u8]>,
    pub hunks: Arc<[DiffHunkRange]>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHunkRange {
    pub old_range: Range<usize>,
    pub new_range: Range<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffAction {
    pub kind: DiffActionKind,
    pub comparison: u64,
    /// Zero-based line ranges, excluding the trailing empty editor row.
    pub old_range: Range<usize>,
    pub new_range: Range<usize>,
    pub revision: (u64, u64),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
}
#[derive(Clone, Debug)]
pub(crate) enum ProjectedRow {
    Document {
        row: i32,
        old_row: Option<usize>,
        kind: DiffLineKind,
    },
    Historical {
        old_row: usize,
        bytes: Vec<u8>,
    },
    Hunk {
        old_range: Range<usize>,
        new_range: Range<usize>,
    },
    Conflict(SourceConflict),
}
#[derive(Clone, Debug, Default)]
pub(crate) struct RowProjection {
    pub rows: Vec<ProjectedRow>,
    document_rows: Vec<usize>,
    identity_lines: usize,
}

#[derive(Clone, Copy)]
enum Edit {
    Equal(usize, usize),
    Delete(usize),
    Insert(usize),
}
fn lines(bytes: &[u8]) -> Vec<Vec<u8>> {
    if bytes.is_empty() {
        return Vec::new();
    }
    // Compare the same line representation as the text editor, including CR-only
    // files. Retain a newline sentinel while omitting the synthetic EOF row.
    let (parts, _) = EditorState::split_lines(bytes);
    let last = parts.len() - 1;
    parts
        .into_iter()
        .enumerate()
        .filter_map(|(index, mut line)| {
            if index == last && line.is_empty() {
                return None;
            }
            if index < last {
                line.push(b'\n');
            }
            Some(line)
        })
        .collect()
}
fn display_line(line: &[u8]) -> Vec<u8> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line).to_vec()
}
fn authoritative_edits(
    old: &[Vec<u8>],
    new: &[Vec<u8>],
    hunks: &[DiffHunkRange],
) -> Option<Vec<Edit>> {
    let (mut old_at, mut new_at) = (0, 0);
    let mut result = Vec::new();
    for hunk in hunks {
        let (o, n) = (&hunk.old_range, &hunk.new_range);
        if o.start < old_at
            || n.start < new_at
            || o.start > o.end
            || n.start > n.end
            || o.end > old.len()
            || n.end > new.len()
            || o.start - old_at != n.start - new_at
        {
            return None;
        }
        for offset in 0..o.start - old_at {
            if old[old_at + offset] != new[new_at + offset] {
                return None;
            }
            result.push(Edit::Equal(old_at + offset, new_at + offset));
        }
        result.extend(o.clone().map(Edit::Delete));
        result.extend(n.clone().map(Edit::Insert));
        old_at = o.end;
        new_at = n.end;
    }
    if old.len() - old_at != new.len() - new_at {
        return None;
    }
    for offset in 0..old.len() - old_at {
        if old[old_at + offset] != new[new_at + offset] {
            return None;
        }
        result.push(Edit::Equal(old_at + offset, new_at + offset));
    }
    Some(result)
}
fn text_lines(bytes: &[u8]) -> Vec<Vec<u8>> {
    lines(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes))
}
fn edits(old: &[Vec<u8>], new: &[Vec<u8>]) -> Vec<Edit> {
    let mut prefix = 0;
    while prefix < old.len().min(new.len()) && old[prefix] == new[prefix] {
        prefix += 1;
    }
    let (mut oe, mut ne) = (old.len(), new.len());
    while oe > prefix && ne > prefix && old[oe - 1] == new[ne - 1] {
        oe -= 1;
        ne -= 1;
    }
    let (n, m) = (oe - prefix, ne - prefix);
    let mut out: Vec<_> = (0..prefix).map(|i| Edit::Equal(i, i)).collect();
    if n.saturating_mul(m) > 1_000_000 || n == 0 || m == 0 {
        out.extend((prefix..oe).map(Edit::Delete));
        out.extend((prefix..ne).map(Edit::Insert));
    } else {
        let cols = m + 1;
        let mut dp = vec![0u32; (n + 1) * cols];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                dp[i * cols + j] = if old[prefix + i] == new[prefix + j] {
                    1 + dp[(i + 1) * cols + j + 1]
                } else {
                    dp[(i + 1) * cols + j].max(dp[i * cols + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if old[prefix + i] == new[prefix + j] {
                out.push(Edit::Equal(prefix + i, prefix + j));
                i += 1;
                j += 1;
            } else if dp[(i + 1) * cols + j] >= dp[i * cols + j + 1] {
                out.push(Edit::Delete(prefix + i));
                i += 1;
            } else {
                out.push(Edit::Insert(prefix + j));
                j += 1;
            }
        }
        out.extend((i..n).map(|i| Edit::Delete(prefix + i)));
        out.extend((j..m).map(|j| Edit::Insert(prefix + j)));
    }
    out.extend((0..old.len() - oe).map(|i| Edit::Equal(oe + i, ne + i)));
    out
}
impl RowProjection {
    pub fn build(
        state: &EditorState,
        diff: Option<&DiffPresentation>,
        git: Option<&SourceGitPresentation>,
    ) -> Self {
        if diff.is_none() && git.is_none_or(|git| git.conflicts.is_empty()) {
            return Self {
                identity_lines: state.line_count() as usize,
                ..Default::default()
            };
        }
        let mut rows = Vec::new();
        if let Some(diff) = diff {
            let old = text_lines(&diff.baseline);
            let new = lines(&state.join());
            let changes = diff
                .saved
                .as_ref()
                .filter(|snapshot| text_lines(&snapshot.current) == new)
                .and_then(|snapshot| authoritative_edits(&old, &new, &snapshot.hunks))
                .unwrap_or_else(|| edits(&old, &new));
            let (mut at, mut old_at, mut new_at) = (0, 0, 0);
            while at < changes.len() {
                if let Edit::Equal(o, n) = changes[at] {
                    rows.push(ProjectedRow::Document {
                        row: n as i32,
                        old_row: Some(o),
                        kind: DiffLineKind::Context,
                    });
                    old_at = o + 1;
                    new_at = n + 1;
                    at += 1;
                } else {
                    let start = at;
                    let (old_start, new_start) = (old_at, new_at);
                    while at < changes.len() && !matches!(changes[at], Edit::Equal(..)) {
                        match changes[at] {
                            Edit::Delete(o) => old_at = o + 1,
                            Edit::Insert(n) => new_at = n + 1,
                            _ => {}
                        }
                        at += 1;
                    }
                    rows.push(ProjectedRow::Hunk {
                        old_range: old_start..old_at,
                        new_range: new_start..new_at,
                    });
                    for change in &changes[start..at] {
                        match *change {
                            Edit::Delete(o) => rows.push(ProjectedRow::Historical {
                                old_row: o,
                                bytes: display_line(&old[o]),
                            }),
                            Edit::Insert(n) => rows.push(ProjectedRow::Document {
                                row: n as i32,
                                old_row: None,
                                kind: DiffLineKind::Added,
                            }),
                            _ => {}
                        }
                    }
                }
            }
            // Editors always have an editable empty row, including after a final
            // newline. It is not a Git line and has no old-side line number.
            for row in new.len()..state.line_count() as usize {
                rows.push(ProjectedRow::Document {
                    row: row as i32,
                    old_row: None,
                    kind: DiffLineKind::Context,
                });
            }
        } else {
            rows.extend((0..state.line_count()).map(|row| ProjectedRow::Document {
                row,
                old_row: None,
                kind: DiffLineKind::Context,
            }));
        }
        if let Some(git) = git {
            for conflict in git.conflicts.iter().rev() {
                if let Some(index) = rows.iter().position(|entry| matches!(entry, ProjectedRow::Document { row, .. } if *row == conflict.row)) {
                    rows.insert(index, ProjectedRow::Conflict(conflict.clone()));
                }
            }
        }
        let mut document_rows = vec![0; state.line_count() as usize];
        for (visual, entry) in rows.iter().enumerate() {
            if let ProjectedRow::Document { row, .. } = entry {
                document_rows[*row as usize] = visual;
            }
        }
        Self {
            rows,
            document_rows,
            identity_lines: 0,
        }
    }
    pub fn len(&self) -> usize {
        if self.rows.is_empty() {
            self.identity_lines
        } else {
            self.rows.len()
        }
    }
    pub fn visual_row(&self, row: i32) -> usize {
        if self.rows.is_empty() {
            return row.max(0) as usize;
        }
        self.document_rows
            .get(row.max(0) as usize)
            .copied()
            .unwrap_or(0)
    }
    pub fn document_row(&self, visual: usize) -> Option<i32> {
        if self.rows.is_empty() {
            return (visual < self.identity_lines).then_some(visual as i32);
        }
        match self.rows.get(visual) {
            Some(ProjectedRow::Document { row, .. }) => Some(*row),
            _ => None,
        }
    }
    pub fn nearest_document_row(&self, visual: usize) -> i32 {
        if self.rows.is_empty() {
            return visual.min(self.identity_lines.saturating_sub(1)) as i32;
        }
        self.rows
            .iter()
            .skip(visual)
            .find_map(|row| match row {
                ProjectedRow::Document { row, .. } => Some(*row),
                _ => None,
            })
            .or_else(|| {
                self.rows
                    .iter()
                    .take(visual)
                    .rev()
                    .find_map(|row| match row {
                        ProjectedRow::Document { row, .. } => Some(*row),
                        _ => None,
                    })
            })
            .unwrap_or(0)
    }
    pub fn is_projected(&self) -> bool {
        !self.rows.is_empty() && self.rows.len() != self.document_rows.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn projection(old: &[u8], new: &[u8]) -> RowProjection {
        let mut state = EditorState::new();
        state.set_from_bytes(new);
        RowProjection::build(
            &state,
            Some(&DiffPresentation {
                baseline: old.into(),
                comparison: 1,
                actions: vec![],
                actions_enabled: true,
                saved: None,
            }),
            None,
        )
    }
    #[test]
    fn replacement_rows_keep_real_document_coordinates() {
        let p = projection(b"head\nold\ntail\n", b"head\nnew\ntail\n");
        assert!(
            matches!(&p.rows[1], ProjectedRow::Hunk { old_range, new_range } if *old_range == (1..2) && *new_range == (1..2))
        );
        assert!(matches!(&p.rows[2], ProjectedRow::Historical { bytes, .. } if bytes == b"old"));
        assert_eq!(p.visual_row(1), 3);
        assert_eq!(p.document_row(3), Some(1));
        assert_eq!(p.document_row(2), None);
        assert_eq!(p.nearest_document_row(2), 1);
        assert_eq!(p.visual_row(3), 5);
    }
    #[test]
    fn empty_files_deletions_and_missing_final_newlines_are_distinct() {
        assert_eq!(projection(b"", b"").rows.len(), 1);
        let p = projection(b"old\n", b"");
        assert!(matches!(&p.rows[0], ProjectedRow::Hunk { new_range, .. } if new_range.is_empty()));
        assert_eq!(p.document_row(p.visual_row(0)), Some(0));
        assert!(
            projection(b"line\n", b"line")
                .rows
                .iter()
                .any(|row| matches!(row, ProjectedRow::Hunk { .. }))
        );
        let p = projection(b"", b"new\n");
        assert!(matches!(&p.rows[0], ProjectedRow::Hunk { old_range, .. } if old_range.is_empty()));
    }
    #[test]
    fn every_live_row_appears_once_even_with_conflict_controls() {
        let mut state = EditorState::new();
        state.set_from_bytes(b"one\ntwo\nthree");
        let p = RowProjection::build(
            &state,
            None,
            Some(&SourceGitPresentation {
                conflicts: vec![SourceConflict {
                    id: 1,
                    row: 1,
                    ..Default::default()
                }],
            }),
        );
        assert!(matches!(p.rows[1], ProjectedRow::Conflict(_)));
        for row in 0..state.line_count() {
            assert_eq!(p.document_row(p.visual_row(row)), Some(row));
        }
    }
    #[test]
    fn display_normalizes_bom_and_all_editor_line_endings() {
        for bytes in [
            &b"\xef\xbb\xbfhead\r\ntail\r\n"[..],
            &b"head\rtail\r"[..],
            &b"head\ntail\n"[..],
        ] {
            let p = projection(bytes, b"head\ntail\n");
            assert!(
                !p.rows
                    .iter()
                    .any(|row| matches!(row, ProjectedRow::Hunk { .. }))
            );
            for row in 0..3 {
                assert_eq!(p.document_row(p.visual_row(row)), Some(row));
            }
        }
    }
    #[test]
    fn saved_git_hunks_remain_separate_above_the_local_diff_work_limit() {
        let old: Vec<u8> = (0..1100)
            .flat_map(|row| format!("line {row}\n").into_bytes())
            .collect();
        let mut new = old.clone();
        new.splice(0..6, b"changed".iter().copied());
        let last = new.len() - "line 1099\n".len();
        new.splice(last..new.len(), b"last changed\n".iter().copied());
        let mut state = EditorState::new();
        state.set_from_bytes(&new);
        let presentation = DiffPresentation {
            baseline: old.into(),
            comparison: 4,
            actions: vec![DiffActionKind::Stage],
            actions_enabled: true,
            saved: Some(DiffSnapshot {
                current: new.into(),
                hunks: vec![
                    DiffHunkRange {
                        old_range: 0..1,
                        new_range: 0..1,
                    },
                    DiffHunkRange {
                        old_range: 1099..1100,
                        new_range: 1099..1100,
                    },
                ]
                .into(),
            }),
        };
        let p = RowProjection::build(&state, Some(&presentation), None);
        let hunks: Vec<_> = p
            .rows
            .iter()
            .filter_map(|row| match row {
                ProjectedRow::Hunk {
                    old_range,
                    new_range,
                } => Some((old_range.clone(), new_range.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(hunks, vec![(0..1, 0..1), (1099..1100, 1099..1100)]);
        // Additional unsaved edits cannot accidentally reuse the saved ranges.
        state.set_from_bytes(b"entirely unsaved\n");
        let p = RowProjection::build(&state, Some(&presentation), None);
        assert!(
            matches!(&p.rows[0], ProjectedRow::Hunk { old_range, new_range } if *old_range == (0..1100) && *new_range == (0..1))
        );
    }
}
