//! Crossing reduction with cluster-major ordering.
//!
//! Every node carries a *cluster path* (outermost set first). Within a rank, nodes are sorted by a
//! key made of one *slot* per level of that path: clusters occupy even integer slots (twice their
//! position among their siblings) and a node occupies a fractional slot derived from its
//! barycenter, placed between the sibling clusters whose mean position it falls between. Members
//! of a set are therefore always contiguous, sibling sets keep one global order across ranks, and
//! free nodes can still move by barycenter and interleave with sets.

/// A child of a cluster (or of the root): a node placed directly in it, or a nested cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Node(usize),
    Cluster(usize),
}

/// Result of the ordering pass.
#[derive(Debug, Clone)]
pub struct Ordering {
    /// Node indices per rank, left to right.
    pub ranks: Vec<Vec<usize>>,
    /// Ordered items per cluster.
    pub items: Vec<Vec<Item>>,
    /// Ordered top-level items.
    pub top_items: Vec<Item>,
    pub crossings: usize,
}

pub struct OrderInput<'a> {
    pub node_count: usize,
    pub ranks: &'a [u32],
    pub edges: &'a [(usize, usize)],
    /// Innermost cluster of each node.
    pub cluster_of: &'a [Option<usize>],
    /// Parent of each cluster.
    pub cluster_parent: &'a [Option<usize>],
    pub sweeps: usize,
}

struct State<'a> {
    input: &'a OrderInput<'a>,
    /// Cluster path (root first) of each node.
    path: Vec<Vec<usize>>,
    /// Position of each cluster among its sibling clusters.
    sib_pos: Vec<usize>,
    /// Mean normalised position of each cluster's members.
    cluster_mean: Vec<f32>,
    /// Sibling clusters per parent (`None` = root).
    sibling_clusters: Vec<Vec<usize>>,
    top_clusters: Vec<usize>,
    /// Fractional slot of each node among its parent's items.
    node_slot: Vec<f32>,
    /// Per-rank order.
    ranks: Vec<Vec<usize>>,
    /// Normalised position (0..1) of each node within its rank.
    pos: Vec<f32>,
}

impl State<'_> {
    fn siblings_of(&self, parent: Option<usize>) -> &[usize] {
        match parent {
            Some(p) => &self.sibling_clusters[p],
            None => &self.top_clusters,
        }
    }

    fn key(&self, node: usize) -> Vec<f32> {
        let mut key: Vec<f32> = self.path[node]
            .iter()
            .map(|c| 2.0 * self.sib_pos[*c] as f32)
            .collect();
        key.push(self.node_slot[node]);
        key
    }

    /// Sorts one rank; `bary` holds each node's target position in `0..=1`.
    fn sort_rank(&mut self, rank: usize, bary: &[f32]) {
        let nodes = std::mem::take(&mut self.ranks[rank]);
        for &n in &nodes {
            let parent = self.input.cluster_of[n];
            let before = self
                .siblings_of(parent)
                .iter()
                .filter(|c| self.cluster_mean[**c] < bary[n])
                .count();
            self.node_slot[n] = 2.0 * before as f32 - 1.0 + bary[n].clamp(0.0, 1.0);
        }
        let mut keyed: Vec<(Vec<f32>, usize)> = nodes.iter().map(|&n| (self.key(n), n)).collect();
        keyed.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        });
        self.ranks[rank] = keyed.into_iter().map(|k| k.1).collect();
        self.update_pos(rank);
    }

    fn update_pos(&mut self, rank: usize) {
        let len = self.ranks[rank].len().max(1) as f32;
        for (i, &n) in self.ranks[rank].iter().enumerate() {
            self.pos[n] = (i as f32 + 0.5) / len;
        }
    }

    /// Recomputes cluster means and reorders sibling clusters by them.
    fn reorder_clusters(&mut self) {
        let cluster_count = self.sib_pos.len();
        let mut sum = vec![0.0f32; cluster_count];
        let mut count = vec![0usize; cluster_count];
        for (n, path) in self.path.iter().enumerate() {
            for &c in path {
                sum[c] += self.pos[n];
                count[c] += 1;
            }
        }
        for c in 0..cluster_count {
            self.cluster_mean[c] = if count[c] == 0 {
                0.5
            } else {
                sum[c] / count[c] as f32
            };
        }
        let means = &self.cluster_mean;
        let sort = |list: &mut Vec<usize>| {
            list.sort_by(|a, b| {
                means[*a]
                    .partial_cmp(&means[*b])
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        };
        sort(&mut self.top_clusters);
        for list in &mut self.sibling_clusters {
            sort(list);
        }
        for list in self
            .sibling_clusters
            .iter()
            .chain(std::iter::once(&self.top_clusters))
        {
            for (i, &c) in list.iter().enumerate() {
                self.sib_pos[c] = i;
            }
        }
    }

    /// Item lists per parent, ordered by slot.
    fn items(&self) -> (Vec<Vec<Item>>, Vec<Item>) {
        let cluster_count = self.sib_pos.len();
        let mut items: Vec<Vec<(f32, Item)>> = vec![Vec::new(); cluster_count];
        let mut top: Vec<(f32, Item)> = Vec::new();
        for (n, parent) in self.input.cluster_of.iter().enumerate() {
            let entry = (self.node_slot[n], Item::Node(n));
            match parent {
                Some(p) => items[*p].push(entry),
                None => top.push(entry),
            }
        }
        for (c, parent) in self.input.cluster_parent.iter().enumerate() {
            let entry = (2.0 * self.sib_pos[c] as f32, Item::Cluster(c));
            match parent {
                Some(p) => items[*p].push(entry),
                None => top.push(entry),
            }
        }
        let finish = |mut list: Vec<(f32, Item)>| -> Vec<Item> {
            list.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            list.into_iter().map(|(_, item)| item).collect()
        };
        (items.into_iter().map(finish).collect(), finish(top))
    }
}

pub fn order_ranks(input: &OrderInput) -> Ordering {
    let n = input.node_count;
    let cluster_count = input.cluster_parent.len();
    let rank_count = input
        .ranks
        .iter()
        .copied()
        .max()
        .map_or(0, |r| r as usize + 1);

    let mut sibling_clusters: Vec<Vec<usize>> = vec![Vec::new(); cluster_count];
    let mut top_clusters = Vec::new();
    for (c, parent) in input.cluster_parent.iter().enumerate() {
        match parent {
            Some(p) => sibling_clusters[*p].push(c),
            None => top_clusters.push(c),
        }
    }
    let mut sib_pos = vec![0usize; cluster_count];
    for list in sibling_clusters
        .iter()
        .chain(std::iter::once(&top_clusters))
    {
        for (i, &c) in list.iter().enumerate() {
            sib_pos[c] = i;
        }
    }
    let path: Vec<Vec<usize>> = (0..n)
        .map(|node| {
            let mut chain = Vec::new();
            let mut current = input.cluster_of[node];
            while let Some(c) = current {
                chain.push(c);
                current = input.cluster_parent[c];
            }
            chain.reverse();
            chain
        })
        .collect();

    let mut ranks: Vec<Vec<usize>> = vec![Vec::new(); rank_count];
    for node in 0..n {
        ranks[input.ranks[node] as usize].push(node);
    }
    let mut state = State {
        input,
        path,
        sib_pos,
        cluster_mean: vec![0.5; cluster_count],
        sibling_clusters,
        top_clusters,
        node_slot: vec![0.0; n],
        ranks,
        pos: vec![0.5; n],
    };
    // Initial order: by node index, clusters interleaved at their mean.
    let initial: Vec<f32> = (0..n).map(|node| node as f32 / n.max(1) as f32).collect();
    for r in 0..rank_count {
        state.sort_rank(r, &initial);
    }
    state.reorder_clusters();

    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut succs: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(a, b) in input.edges {
        if a != b && a < n && b < n {
            succs[a].push(b);
            preds[b].push(a);
        }
    }

    let mut best = snapshot(&state);
    let mut bary = vec![0.0f32; n];
    for _ in 0..input.sweeps {
        for r in 1..rank_count {
            for &node in &state.ranks[r] {
                bary[node] = mean_pos(&state.pos, &preds[node], state.pos[node]);
            }
            state.sort_rank(r, &bary);
        }
        state.reorder_clusters();
        for r in (0..rank_count.saturating_sub(1)).rev() {
            for &node in &state.ranks[r] {
                bary[node] = mean_pos(&state.pos, &succs[node], state.pos[node]);
            }
            state.sort_rank(r, &bary);
        }
        state.reorder_clusters();
        // Re-sort every rank against the final cluster order so ranks and items agree.
        for r in 0..rank_count {
            let current = state.pos.clone();
            state.sort_rank(r, &current);
        }
        let candidate = snapshot(&state);
        if candidate.crossings < best.crossings {
            best = candidate;
        }
        if best.crossings == 0 {
            break;
        }
    }
    best
}

fn mean_pos(pos: &[f32], neighbours: &[usize], fallback: f32) -> f32 {
    if neighbours.is_empty() {
        fallback
    } else {
        neighbours.iter().map(|&m| pos[m]).sum::<f32>() / neighbours.len() as f32
    }
}

fn snapshot(state: &State) -> Ordering {
    let (items, top_items) = state.items();
    Ordering {
        ranks: state.ranks.clone(),
        items,
        top_items,
        crossings: count_crossings(&state.ranks, state.input.ranks, state.input.edges),
    }
}

/// Counts crossings between edges of adjacent ranks.
pub fn count_crossings(order: &[Vec<usize>], ranks: &[u32], edges: &[(usize, usize)]) -> usize {
    let n = ranks.len();
    let mut pos = vec![0usize; n];
    for rank in order {
        for (i, &node) in rank.iter().enumerate() {
            pos[node] = i;
        }
    }
    let rank_count = order.len();
    let mut per_rank: Vec<Vec<(usize, usize)>> = vec![Vec::new(); rank_count];
    for &(a, b) in edges {
        if a >= n || b >= n {
            continue;
        }
        let (ra, rb) = (ranks[a], ranks[b]);
        if rb == ra + 1 {
            per_rank[ra as usize].push((pos[a], pos[b]));
        } else if ra == rb + 1 {
            per_rank[rb as usize].push((pos[b], pos[a]));
        }
    }
    let mut total = 0;
    for list in per_rank.iter_mut() {
        list.sort_unstable();
        let seq: Vec<usize> = list.iter().map(|e| e.1).collect();
        total += inversions(&seq);
    }
    total
}

fn inversions(seq: &[usize]) -> usize {
    let max = seq.iter().copied().max().map_or(0, |m| m + 1);
    let mut tree = vec![0usize; max + 1];
    let mut count = 0;
    for (seen, &v) in seq.iter().enumerate() {
        // Number of earlier elements <= v.
        let mut i = v + 1;
        let mut le = 0;
        while i > 0 {
            le += tree[i];
            i &= i - 1;
        }
        count += seen - le;
        let mut i = v + 1;
        while i <= max {
            tree[i] += 1;
            i += i.isolate_lowest_one();
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inversion_count() {
        assert_eq!(inversions(&[0, 1, 2]), 0);
        assert_eq!(inversions(&[2, 1, 0]), 3);
        assert_eq!(inversions(&[1, 0, 2]), 1);
    }

    #[test]
    fn sweeps_reduce_crossings_and_keep_clusters_contiguous() {
        // Two ranks, four nodes each; edges form a crossing pattern; nodes 4,5 in cluster 0.
        let ranks = [0, 0, 0, 0, 1, 1, 1, 1];
        let edges = [(0, 7), (1, 6), (2, 5), (3, 4)];
        let cluster_of = [None, None, None, None, Some(0), Some(0), None, None];
        let parent = [None];
        let input = OrderInput {
            node_count: 8,
            ranks: &ranks,
            edges: &edges,
            cluster_of: &cluster_of,
            cluster_parent: &parent,
            sweeps: 8,
        };
        let initial_order = vec![vec![0, 1, 2, 3], vec![4, 5, 6, 7]];
        let initial = count_crossings(&initial_order, &ranks, &edges);
        let result = order_ranks(&input);
        assert!(
            result.crossings < initial,
            "{initial} -> {}",
            result.crossings
        );
        let bottom = &result.ranks[1];
        let p4 = bottom.iter().position(|&n| n == 4).unwrap();
        let p5 = bottom.iter().position(|&n| n == 5).unwrap();
        assert_eq!(
            p4.abs_diff(p5),
            1,
            "cluster members must be adjacent: {bottom:?}"
        );
        // Free nodes and the cluster share one ordered item list.
        assert_eq!(result.top_items.len(), 7);
        // The item order agrees with the bottom rank: cluster 0 vs nodes 6 and 7.
        let item_pos = |item: Item| result.top_items.iter().position(|i| *i == item).unwrap();
        for free in [6usize, 7] {
            let free_left = bottom.iter().position(|&n| n == free).unwrap() < p4;
            assert_eq!(
                free_left,
                item_pos(Item::Node(free)) < item_pos(Item::Cluster(0))
            );
        }
    }

    #[test]
    fn sweeps_improve_a_random_dag() {
        // Deterministic LCG-generated layered DAG without clusters: 8 ranks of 10 nodes, each
        // node linked to two random nodes of the next rank.
        let n = 80;
        let mut seed: u64 = 42;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as usize
        };
        let ranks: Vec<u32> = (0..n).map(|i| (i / 10) as u32).collect();
        let mut edges = Vec::new();
        for a in 0..70 {
            for _ in 0..2 {
                let b = (a / 10 + 1) * 10 + next() % 10;
                edges.push((a, b));
            }
        }
        let cluster_of = vec![None; n];
        let input = OrderInput {
            node_count: n,
            ranks: &ranks,
            edges: &edges,
            cluster_of: &cluster_of,
            cluster_parent: &[],
            sweeps: 12,
        };
        let initial_order: Vec<Vec<usize>> =
            (0..8).map(|r| (r * 10..r * 10 + 10).collect()).collect();
        let initial = count_crossings(&initial_order, &ranks, &edges);
        let result = order_ranks(&input);
        println!("random dag crossings: {initial} -> {}", result.crossings);
        assert!(
            result.crossings * 2 < initial,
            "{initial} -> {}",
            result.crossings
        );
    }

    #[test]
    fn free_nodes_can_sit_between_clusters() {
        // Rank 0: a (cluster 0), b (free), c (cluster 1). Rank 1: a2, b2, c2 straight below.
        let ranks = [0, 0, 0, 1, 1, 1];
        let edges = [(0, 3), (1, 4), (2, 5)];
        let cluster_of = [Some(0), None, Some(1), Some(0), None, Some(1)];
        let parent = [None, None];
        let input = OrderInput {
            node_count: 6,
            ranks: &ranks,
            edges: &edges,
            cluster_of: &cluster_of,
            cluster_parent: &parent,
            sweeps: 8,
        };
        let result = order_ranks(&input);
        assert_eq!(result.crossings, 0);
        let top = &result.ranks[0];
        let bottom = &result.ranks[1];
        let pos = |rank: &Vec<usize>, n| rank.iter().position(|&x| x == n).unwrap();
        assert_eq!(pos(top, 0) < pos(top, 1), pos(bottom, 3) < pos(bottom, 4));
        assert_eq!(pos(top, 1) < pos(top, 2), pos(bottom, 4) < pos(bottom, 5));
        // The item list agrees with the rank order for node/cluster pairs.
        let item_pos = |item: Item| result.top_items.iter().position(|i| *i == item).unwrap();
        for (free, cluster, member) in [(1usize, 0usize, 0usize), (1, 1, 2)] {
            let left_in_rank = pos(top, free) < pos(top, member);
            let left_in_items = item_pos(Item::Node(free)) < item_pos(Item::Cluster(cluster));
            assert_eq!(left_in_rank, left_in_items, "{:?}", result.top_items);
        }
    }
}
