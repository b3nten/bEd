//! Coordinate assignment: ranks become rows; sets become columns that only reserve horizontal
//! space over the ranks they actually span; wide runs of parallel nodes wrap into sub-rows.

use bevy::math::{Rect, Vec2};

use super::{
    LayoutInput, LayoutParams,
    order::{Item, Ordering},
};

pub struct Coords {
    pub node_rects: Vec<Rect>,
    pub cluster_rects: Vec<(usize, Rect)>,
    pub bounds: Rect,
}

struct Ctx<'a> {
    input: &'a LayoutInput,
    params: &'a LayoutParams,
    ordering: &'a Ordering,
    /// Pseudo cluster index for top-level items.
    root: usize,
    rank_of: Vec<usize>,
    /// First/last rank of each cluster (`usize::MAX` when empty).
    start: Vec<usize>,
    end: Vec<usize>,
    pad_x: Vec<f32>,
    /// Total column width per cluster (root included, at index `root`).
    col_width: Vec<f32>,
    /// Width of the packed content per cluster (without padding).
    content_extent: Vec<f32>,
    /// x of each node / cluster relative to its parent's content origin.
    node_rel: Vec<f32>,
    cluster_rel: Vec<f32>,
    /// Sub-row of each node within its rank (0 unless its run was wrapped).
    sub_row: Vec<usize>,
    /// Nodes that belong to a wrapped run (kept fixed by the refinement pass).
    wrapped: Vec<bool>,
    /// Number of sub-rows per rank.
    rank_rows: Vec<usize>,
    /// Nodes with no edges and no cluster: laid out in a grid below the graph.
    isolated: Vec<bool>,
    /// Absolute x of each cluster column (root included).
    col_x: Vec<f32>,
}

fn items<'a>(ctx: &'a Ctx, c: usize) -> &'a [Item] {
    if c == ctx.root {
        &ctx.ordering.top_items
    } else {
        &ctx.ordering.items[c]
    }
}

/// Splits a run of nodes into sub-rows no wider than `max_row_width`; returns the sub-rows and
/// the widest one.
fn wrap_run(ctx: &Ctx, run: &[usize]) -> (Vec<Vec<usize>>, f32) {
    let gap = ctx.params.node_gap;
    let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
    let mut width = 0.0f32;
    let mut widest = 0.0f32;
    for &node in run {
        let w = ctx.input.nodes[node].width;
        let next = if rows.last().unwrap().is_empty() {
            w
        } else {
            width + gap + w
        };
        if !rows.last().unwrap().is_empty() && next > ctx.params.max_row_width {
            widest = widest.max(width);
            rows.push(Vec::new());
            width = w;
        } else {
            width = next;
        }
        rows.last_mut().unwrap().push(node);
    }
    widest = widest.max(width);
    (rows, widest)
}

/// Packs the items of `c` left to right; an item only has to clear earlier items whose rank span
/// overlaps its own. Returns the column width.
fn pack(ctx: &mut Ctx, c: usize) -> f32 {
    let list: Vec<Item> = items(ctx, c).to_vec();
    for item in &list {
        if let Item::Cluster(k) = item {
            pack(ctx, *k);
        }
    }
    let gap = ctx.params.node_gap;
    let mut placed: Vec<(f32, f32, usize, usize)> = Vec::new();
    let mut extent = 0.0f32;
    let mut place_block =
        |placed: &mut Vec<(f32, f32, usize, usize)>, r0: usize, r1: usize, width: f32| -> f32 {
            let x = placed
                .iter()
                .filter(|(_, _, p0, p1)| r0 <= *p1 && *p0 <= r1)
                .map(|(_, x1, _, _)| x1 + gap)
                .fold(0.0, f32::max);
            placed.push((x, x + width, r0, r1));
            extent = extent.max(x + width);
            x
        };

    let mut i = 0;
    while i < list.len() {
        match list[i] {
            Item::Cluster(k) => {
                if ctx.start[k] != usize::MAX {
                    let x = place_block(&mut placed, ctx.start[k], ctx.end[k], ctx.col_width[k]);
                    ctx.cluster_rel[k] = x;
                }
                i += 1;
            }
            Item::Node(first) => {
                // Maximal run of consecutive nodes on the same rank.
                let r = ctx.rank_of[first];
                let mut run = Vec::new();
                while i < list.len() {
                    match list[i] {
                        Item::Node(n) if ctx.rank_of[n] == r => {
                            if !ctx.isolated[n] {
                                run.push(n);
                            }
                            i += 1;
                        }
                        _ => break,
                    }
                }
                if run.is_empty() {
                    continue;
                }
                let (rows, widest) = wrap_run(ctx, &run);
                let x = place_block(&mut placed, r, r, widest);
                let wrapped = rows.len() > 1;
                ctx.rank_rows[r] = ctx.rank_rows[r].max(rows.len());
                for (k, row) in rows.iter().enumerate() {
                    let row_w: f32 = row.iter().map(|&n| ctx.input.nodes[n].width).sum::<f32>()
                        + gap * row.len().saturating_sub(1) as f32;
                    let mut cursor = x + (widest - row_w) / 2.0;
                    for &n in row {
                        ctx.node_rel[n] = cursor;
                        ctx.sub_row[n] = k;
                        ctx.wrapped[n] = wrapped;
                        cursor += ctx.input.nodes[n].width + gap;
                    }
                }
            }
        }
    }
    ctx.content_extent[c] = extent;
    let width = if c == ctx.root {
        extent
    } else {
        extent.max(ctx.input.clusters[c].label_width + 16.0) + 2.0 * ctx.pad_x[c]
    };
    ctx.col_width[c] = width;
    width
}

fn place(ctx: &mut Ctx, c: usize, x0: f32, node_x: &mut [f32]) {
    ctx.col_x[c] = x0;
    let pad = if c == ctx.root { 0.0 } else { ctx.pad_x[c] };
    let content = ctx.col_width[c] - 2.0 * pad;
    let origin = x0 + pad + ((content - ctx.content_extent[c]) / 2.0).max(0.0);
    let list: Vec<Item> = items(ctx, c).to_vec();
    for item in list {
        match item {
            Item::Node(n) => node_x[n] = origin + ctx.node_rel[n],
            Item::Cluster(k) => {
                let x = origin + ctx.cluster_rel[k];
                place(ctx, k, x, node_x);
            }
        }
    }
}

/// Number of stacked cluster headers/paddings starting (or ending) at the same rank as `c`.
fn stack_down(
    c: usize,
    same: &dyn Fn(usize, usize) -> bool,
    kids: &[Vec<usize>],
    memo: &mut [Option<u32>],
) -> u32 {
    if let Some(v) = memo[c] {
        return v;
    }
    let below = kids[c]
        .iter()
        .filter(|&&d| same(c, d))
        .map(|&d| stack_down(d, same, kids, memo))
        .max()
        .unwrap_or(0);
    memo[c] = Some(1 + below);
    1 + below
}

pub fn assign(input: &LayoutInput, ordering: &Ordering, params: &LayoutParams) -> Coords {
    let n = input.nodes.len();
    let cc = input.clusters.len();
    let root = cc;
    let rank_count = ordering.ranks.len();
    let mut rank_of = vec![0usize; n];
    for (r, nodes) in ordering.ranks.iter().enumerate() {
        for &node in nodes {
            rank_of[node] = r;
        }
    }
    let mut has_edge = vec![false; n];
    for e in &input.edges {
        has_edge[e.from] = true;
        has_edge[e.to] = true;
    }
    let isolated: Vec<bool> = input
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| !has_edge[i] && node.cluster.is_none())
        .collect();

    let depth: Vec<u32> = (0..cc)
        .map(|c| {
            let mut d = 0;
            let mut current = input.clusters[c].parent;
            while let Some(p) = current {
                d += 1;
                current = input.clusters[p].parent;
            }
            d
        })
        .collect();
    let max_depth = depth.iter().copied().max().unwrap_or(0);
    let pad_x: Vec<f32> = (0..cc)
        .map(|c| params.cluster_pad + (max_depth - depth[c]) as f32 * params.cluster_pad_step)
        .collect();

    let mut start = vec![usize::MAX; cc];
    let mut end = vec![0usize; cc];
    for (i, node) in input.nodes.iter().enumerate() {
        let r = rank_of[i];
        let mut current = node.cluster;
        while let Some(c) = current {
            start[c] = start[c].min(r);
            end[c] = end[c].max(r);
            current = input.clusters[c].parent;
        }
    }

    let mut ctx = Ctx {
        input,
        params,
        ordering,
        root,
        rank_of,
        start,
        end,
        pad_x,
        col_width: vec![0.0; cc + 1],
        content_extent: vec![0.0; cc + 1],
        node_rel: vec![0.0; n],
        cluster_rel: vec![0.0; cc],
        sub_row: vec![0; n],
        wrapped: vec![false; n],
        rank_rows: vec![1; rank_count],
        isolated,
        col_x: vec![0.0; cc + 1],
    };
    pack(&mut ctx, root);
    let mut node_x = vec![0.0f32; n];
    place(&mut ctx, root, 0.0, &mut node_x);

    // ---- Rows: leave room for stacked cluster headers / paddings and wrapped sub-rows ---------
    let kids: Vec<Vec<usize>> = (0..cc)
        .map(|c| {
            ordering.items[c]
                .iter()
                .filter_map(|item| match item {
                    Item::Cluster(k) => Some(*k),
                    Item::Node(_) => None,
                })
                .collect()
        })
        .collect();
    let (start, end) = (&ctx.start, &ctx.end);
    let same_start = |a: usize, b: usize| start[a] == start[b];
    let same_end = |a: usize, b: usize| end[a] == end[b];
    let mut memo_top = vec![None; cc];
    let mut memo_bottom = vec![None; cc];
    let mut headers_above = vec![0u32; rank_count];
    let mut pads_below = vec![0u32; rank_count];
    for c in 0..cc {
        if start[c] == usize::MAX {
            continue;
        }
        let parent = input.clusters[c].parent;
        if parent.is_none_or(|p| start[p] != start[c]) {
            let s = stack_down(c, &same_start, &kids, &mut memo_top);
            headers_above[start[c]] = headers_above[start[c]].max(s);
        }
        if parent.is_none_or(|p| end[p] != end[c]) {
            let s = stack_down(c, &same_end, &kids, &mut memo_bottom);
            pads_below[end[c]] = pads_below[end[c]].max(s);
        }
    }
    let header_step = params.cluster_header + 4.0;
    let sub_gap = params.node_gap * 0.75;
    let mut row_y = Vec::with_capacity(rank_count);
    let mut cursor = 0.0;
    for r in 0..rank_count {
        if headers_above[r] > 0 {
            cursor += headers_above[r] as f32 * header_step + params.cluster_pad;
        }
        row_y.push(cursor);
        let rows = ctx.rank_rows[r] as f32;
        cursor += rows * params.node_height + (rows - 1.0) * sub_gap + params.rank_gap;
        if pads_below[r] > 0 {
            cursor += pads_below[r] as f32 * params.cluster_pad;
        }
    }
    let graph_bottom = cursor - params.rank_gap;

    // ---- Node rects ----------------------------------------------------------------------------
    let mut node_rects = vec![Rect::default(); n];
    for (i, node) in input.nodes.iter().enumerate() {
        if ctx.isolated[i] {
            continue;
        }
        let y = row_y[ctx.rank_of[i]] + ctx.sub_row[i] as f32 * (params.node_height + sub_gap);
        node_rects[i] = Rect::from_corners(
            Vec2::new(node_x[i], y),
            Vec2::new(node_x[i] + node.width, y + params.node_height),
        );
    }
    refine_x(&mut node_rects, &ctx);

    // Isolated nodes: wrapped grid below the graph.
    let wrap_width = ctx.col_width[root].max(params.max_row_width);
    let mut x = 0.0f32;
    let mut y = if rank_count > 0 {
        graph_bottom + params.rank_gap * 1.5
    } else {
        0.0
    };
    for (i, node) in input.nodes.iter().enumerate() {
        if !ctx.isolated[i] {
            continue;
        }
        if x > 0.0 && x + node.width > wrap_width {
            x = 0.0;
            y += params.node_height + params.node_gap;
        }
        node_rects[i] = Rect::from_corners(
            Vec2::new(x, y),
            Vec2::new(x + node.width, y + params.node_height),
        );
        x += node.width + params.node_gap;
    }

    // ---- Cluster boxes: fixed column, vertical extent from members and children ---------------
    let mut top = vec![f32::INFINITY; cc];
    let mut bottom = vec![f32::NEG_INFINITY; cc];
    let mut by_depth: Vec<usize> = (0..cc).filter(|&c| start[c] != usize::MAX).collect();
    by_depth.sort_by_key(|&c| std::cmp::Reverse(depth[c]));
    for &c in &by_depth {
        let mut t = f32::INFINITY;
        let mut b = f32::NEG_INFINITY;
        for item in &ordering.items[c] {
            match *item {
                Item::Node(node) => {
                    t = t.min(node_rects[node].min.y - params.cluster_pad);
                    b = b.max(node_rects[node].max.y + params.cluster_pad);
                }
                Item::Cluster(kid) if top[kid].is_finite() => {
                    t = t.min(top[kid] - 4.0);
                    b = b.max(bottom[kid] + 4.0);
                }
                Item::Cluster(_) => {}
            }
        }
        top[c] = t - params.cluster_header;
        bottom[c] = b;
    }
    let mut cluster_rects: Vec<(usize, Rect)> = by_depth
        .iter()
        .map(|&c| {
            let x0 = ctx.col_x[c];
            let x1 = x0 + ctx.col_width[c];
            (
                c,
                Rect::from_corners(Vec2::new(x0, top[c]), Vec2::new(x1, bottom[c])),
            )
        })
        .collect();
    cluster_rects.sort_by_key(|(c, _)| depth[*c]);

    let mut bounds: Option<Rect> = None;
    for rect in node_rects
        .iter()
        .chain(cluster_rects.iter().map(|(_, r)| r))
    {
        bounds = Some(match bounds {
            Some(b) => b.union(*rect),
            None => *rect,
        });
    }
    let bounds = bounds
        .map(|b| Rect::from_corners(b.min - Vec2::splat(20.0), b.max + Vec2::splat(20.0)))
        .unwrap_or_default();

    Coords {
        node_rects,
        cluster_rects,
        bounds,
    }
}

/// Pulls every unwrapped node towards the mean x of its neighbours while keeping the per-rank
/// order and staying clear of the set columns and wrapped blocks next to it.
fn refine_x(rects: &mut [Rect], ctx: &Ctx) {
    let input = ctx.input;
    let params = ctx.params;
    let n = input.nodes.len();
    let gap = params.node_gap;
    let mut neighbours: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in &input.edges {
        neighbours[e.from].push(e.to);
        neighbours[e.to].push(e.from);
    }
    for _ in 0..3 {
        for c in 0..=ctx.root {
            let list = items(ctx, c);
            let pad = if c == ctx.root { 0.0 } else { ctx.pad_x[c] };
            let origin = ctx.col_x[c] + pad;
            let right_edge = ctx.col_x[c] + ctx.col_width[c] - pad;
            let ranks_here: std::collections::BTreeSet<usize> = list
                .iter()
                .filter_map(|item| match *item {
                    Item::Node(node) if !ctx.isolated[node] && !ctx.wrapped[node] => {
                        Some(ctx.rank_of[node])
                    }
                    _ => None,
                })
                .collect();
            for r in ranks_here {
                // Items of `c` occupying rank r, in order: (movable node, x0, x1).
                let seq: Vec<(Option<usize>, f32, f32)> = list
                    .iter()
                    .filter_map(|item| match *item {
                        Item::Node(node) if !ctx.isolated[node] && ctx.rank_of[node] == r => {
                            let movable = (!ctx.wrapped[node]).then_some(node);
                            Some((movable, rects[node].min.x, rects[node].max.x))
                        }
                        Item::Cluster(k)
                            if ctx.start[k] != usize::MAX
                                && ctx.start[k] <= r
                                && r <= ctx.end[k] =>
                        {
                            Some((None, ctx.col_x[k], ctx.col_x[k] + ctx.col_width[k]))
                        }
                        _ => None,
                    })
                    .collect();
                let movable: Vec<(usize, usize)> = seq
                    .iter()
                    .enumerate()
                    .filter_map(|(k, (node, _, _))| node.map(|node| (k, node)))
                    .collect();
                if movable.is_empty() {
                    continue;
                }
                let mut desired: Vec<f32> = Vec::with_capacity(movable.len());
                for &(k, node) in &movable {
                    let w = rects[node].width();
                    let lo = seq[..k]
                        .iter()
                        .filter(|(m, _, _)| m.is_none())
                        .map(|(_, _, x1)| x1 + gap)
                        .fold(origin, f32::max);
                    let hi = seq[k + 1..]
                        .iter()
                        .filter(|(m, _, _)| m.is_none())
                        .map(|(_, x0, _)| x0 - gap)
                        .fold(right_edge, f32::min);
                    let nb = &neighbours[node];
                    let target = if nb.is_empty() {
                        rects[node].min.x
                    } else {
                        nb.iter().map(|&m| rects[m].center().x).sum::<f32>() / nb.len() as f32
                            - w / 2.0
                    };
                    desired.push(target.clamp(lo, (hi - w).max(lo)));
                }
                let mut prev_right = f32::NEG_INFINITY;
                for (i, &(_, node)) in movable.iter().enumerate() {
                    desired[i] = desired[i].max(prev_right + gap);
                    prev_right = desired[i] + rects[node].width();
                }
                let mut next_left = f32::INFINITY;
                for (i, &(_, node)) in movable.iter().enumerate().rev() {
                    let w = rects[node].width();
                    desired[i] = desired[i].min(next_left - gap - w);
                    next_left = desired[i];
                }
                for (i, &(_, node)) in movable.iter().enumerate() {
                    let w = rects[node].width();
                    let y0 = rects[node].min.y;
                    rects[node] = Rect::from_corners(
                        Vec2::new(desired[i], y0),
                        Vec2::new(desired[i] + w, y0 + params.node_height),
                    );
                }
            }
        }
    }
}
