//! Line-array LCS translated from ned editor/services/git/line_diff.{h,cpp}.
//! Source attribution and revision: LICENSE, NOTICE, UPSTREAM_REVISION.
use std::collections::HashSet;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineDiff {
    /// One-based indices in the new document, as expected by the gutter.
    pub added_lines: HashSet<i32>,
    pub additions: i32,
    pub deletions: i32,
}

pub fn diff_lines(old: &[Vec<u8>], new: &[Vec<u8>]) -> LineDiff {
    let mut prefix = 0;
    while prefix < old.len().min(new.len()) && old[prefix] == new[prefix] {
        prefix += 1;
    }
    let (mut old_end, mut new_end) = (old.len(), new.len());
    while old_end > prefix && new_end > prefix && old[old_end - 1] == new[new_end - 1] {
        old_end -= 1;
        new_end -= 1;
    }
    diff_slice(&old[prefix..old_end], &new[prefix..new_end], prefix)
}

fn diff_slice(old: &[Vec<u8>], new: &[Vec<u8>], new_offset: usize) -> LineDiff {
    let (n, m) = (old.len(), new.len());
    let mut out = LineDiff::default();
    if n == 0 || m == 0 || n.saturating_mul(m) > 4_000_000 {
        out.additions = m as i32;
        out.deletions = n as i32;
        out.added_lines
            .extend((0..m).map(|j| (new_offset + j + 1) as i32));
        return out;
    }
    let columns = m + 1;
    let mut dp = vec![0; (n + 1) * columns];
    for i in 0..n {
        for j in 0..m {
            dp[(i + 1) * columns + j + 1] = if old[i] == new[j] {
                dp[i * columns + j] + 1
            } else {
                dp[(i + 1) * columns + j].max(dp[i * columns + j + 1])
            };
        }
    }
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        if old[i - 1] == new[j - 1] {
            i -= 1;
            j -= 1;
        } else if dp[i * columns + j - 1] >= dp[(i - 1) * columns + j] {
            out.added_lines.insert((new_offset + j) as i32);
            out.additions += 1;
            j -= 1;
        } else {
            out.deletions += 1;
            i -= 1;
        }
    }
    while j > 0 {
        out.added_lines.insert((new_offset + j) as i32);
        out.additions += 1;
        j -= 1;
    }
    out.deletions += i as i32;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(values: &[&str]) -> Vec<Vec<u8>> {
        values.iter().map(|line| line.as_bytes().to_vec()).collect()
    }

    #[test]
    fn lcs_ties_prefer_insertion_during_backtrack() {
        let diff = diff_lines(&lines(&["a", "b"]), &lines(&["b", "a"]));
        assert_eq!(diff.added_lines, HashSet::from([2]));
        assert_eq!((diff.additions, diff.deletions), (1, 1));
    }

    #[test]
    fn empty_input_and_equal_prefix_suffix_keep_one_based_indices() {
        assert_eq!(diff_lines(&[], &[]), LineDiff::default());
        assert_eq!(
            diff_lines(&[], &lines(&["a", "b"])).added_lines,
            HashSet::from([1, 2])
        );
        assert_eq!(diff_lines(&lines(&["a", "b"]), &[]).deletions, 2);
        let diff = diff_lines(
            &lines(&["head", "old", "tail"]),
            &lines(&["head", "one", "two", "tail"]),
        );
        assert_eq!(diff.added_lines, HashSet::from([2, 3]));
        assert_eq!((diff.additions, diff.deletions), (2, 1));
    }

    #[test]
    fn massive_middle_fallback_does_not_mark_equal_ends() {
        let mut old = lines(&["prefix"]);
        old.extend((0..2001).map(|i| format!("old{i}").into_bytes()));
        old.push(b"suffix".to_vec());
        let mut new = lines(&["prefix"]);
        new.extend((0..2001).map(|i| format!("new{i}").into_bytes()));
        new.push(b"suffix".to_vec());
        let diff = diff_lines(&old, &new);
        assert_eq!((diff.additions, diff.deletions), (2001, 2001));
        assert_eq!(diff.added_lines.len(), 2001);
        assert!(!diff.added_lines.contains(&1));
        assert!(!diff.added_lines.contains(&2003));
    }
}
