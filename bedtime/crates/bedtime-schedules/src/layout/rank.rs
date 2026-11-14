//! Rank (layer) assignment.

use std::collections::VecDeque;

/// Longest-path layering: every node sits one layer below its deepest predecessor.
///
/// The input is expected to be a DAG; if a cycle sneaks in, the node with the fewest remaining
/// predecessors is forced to keep the algorithm going.
pub fn longest_path_ranks(n: usize, edges: &[(usize, usize)]) -> Vec<u32> {
    let mut indeg = vec![0usize; n];
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(a, b) in edges {
        if a == b || a >= n || b >= n {
            continue;
        }
        out[a].push(b);
        indeg[b] += 1;
    }
    let mut rank = vec![0u32; n];
    let mut done = vec![false; n];
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut processed = 0;
    while processed < n {
        let u = match queue.pop_front() {
            Some(u) => u,
            None => {
                // Cycle: break it at the node with the fewest pending predecessors.
                let pick = (0..n)
                    .filter(|&i| !done[i])
                    .min_by_key(|&i| indeg[i])
                    .expect("unprocessed node exists");
                indeg[pick] = 0;
                pick
            }
        };
        if done[u] {
            continue;
        }
        done[u] = true;
        processed += 1;
        for &v in &out[u] {
            if done[v] {
                continue;
            }
            rank[v] = rank[v].max(rank[u] + 1);
            indeg[v] = indeg[v].saturating_sub(1);
            if indeg[v] == 0 {
                queue.push_back(v);
            }
        }
    }
    rank
}

/// Moves every node that has successors but no predecessors down to just above its earliest
/// successor, so sources spread over the ranks where they are needed instead of piling up in
/// rank 0. Nodes with no edges at all stay where they are.
pub fn tighten_sources(ranks: &mut [u32], edges: &[(usize, usize)]) {
    let n = ranks.len();
    let mut has_pred = vec![false; n];
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(a, b) in edges {
        if a == b || a >= n || b >= n {
            continue;
        }
        has_pred[b] = true;
        succ[a].push(b);
    }
    // Process in decreasing rank order so a chain of sources tightens in one pass.
    let mut order: Vec<usize> = (0..n)
        .filter(|&i| !has_pred[i] && !succ[i].is_empty())
        .collect();
    order.sort_by_key(|&i| std::cmp::Reverse(ranks[i]));
    for i in order {
        let earliest = succ[i].iter().map(|&s| ranks[s]).min().unwrap_or(0);
        ranks[i] = earliest.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_respect_edges() {
        let edges = [(0, 1), (1, 2), (0, 2), (3, 2)];
        let ranks = longest_path_ranks(4, &edges);
        assert_eq!(ranks, vec![0, 1, 2, 0]);
        for (a, b) in edges {
            assert!(ranks[a] < ranks[b]);
        }
    }

    #[test]
    fn sources_move_next_to_their_successors() {
        // 0 -> 1 -> 2, and 3 -> 2: 3 should sit at rank 1, not 0.
        let edges = [(0, 1), (1, 2), (3, 2)];
        let mut ranks = longest_path_ranks(4, &edges);
        tighten_sources(&mut ranks, &edges);
        assert_eq!(ranks, vec![0, 1, 2, 1]);
    }

    #[test]
    fn cycles_do_not_hang() {
        let ranks = longest_path_ranks(3, &[(0, 1), (1, 2), (2, 0)]);
        assert_eq!(ranks.len(), 3);
    }
}
