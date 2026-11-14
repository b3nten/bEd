//! Layered graph layout (Sugiyama-style) with cluster-aware ordering.
//!
//! Pipeline: [`visible`] turns a schedule snapshot plus the viewer's collapse/hide state into a
//! [`LayoutInput`]; [`compute_layout`] assigns ranks (longest path), orders each rank with
//! cluster-major barycenter sweeps and finally assigns coordinates so that every system set owns a
//! contiguous column. Everything here is pure and free of egui/Bevy app types so it can be tested
//! headlessly and cached by the viewer.

mod coords;
mod order;
mod rank;
pub mod visible;

use bevy::math::{Rect, Vec2};

use crate::snapshot::{NodeRef, OrderKind, SetIx};

/// Tunables in scene units (pixels at zoom 1).
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutParams {
    pub node_height: f32,
    pub rank_gap: f32,
    pub node_gap: f32,
    pub cluster_pad: f32,
    /// Extra padding per nesting level so outer boxes clear inner ones.
    pub cluster_pad_step: f32,
    pub cluster_header: f32,
    /// Runs of parallel nodes wider than this are wrapped into several sub-rows.
    pub max_row_width: f32,
    pub sweeps: usize,
}

impl Default for LayoutParams {
    fn default() -> Self {
        Self {
            node_height: 26.0,
            rank_gap: 46.0,
            node_gap: 14.0,
            cluster_pad: 10.0,
            cluster_pad_step: 6.0,
            cluster_header: 18.0,
            max_row_width: 1000.0,
            sweeps: 12,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LayoutNode {
    pub id: NodeRef,
    pub width: f32,
    /// Innermost cluster this node belongs to.
    pub cluster: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LayoutEdge {
    pub from: usize,
    pub to: usize,
    pub kind: OrderKind,
    /// Synthesised to keep connectivity through hidden nodes.
    pub via_hidden: bool,
}

#[derive(Clone, Debug)]
pub struct LayoutCluster {
    pub set: SetIx,
    pub parent: Option<usize>,
    pub name: String,
    pub label_width: f32,
}

#[derive(Clone, Debug, Default)]
pub struct LayoutInput {
    pub nodes: Vec<LayoutNode>,
    pub edges: Vec<LayoutEdge>,
    pub clusters: Vec<LayoutCluster>,
}

/// Cubic bezier control points of one edge.
#[derive(Clone, Debug)]
pub struct EdgePath {
    pub edge: usize,
    pub points: [Vec2; 4],
}

#[derive(Clone, Debug, Default)]
pub struct LayoutResult {
    pub node_rects: Vec<Rect>,
    /// `(cluster index, rect)`, outermost first.
    pub cluster_rects: Vec<(usize, Rect)>,
    pub edge_paths: Vec<EdgePath>,
    pub bounds: Rect,
    pub ranks: Vec<u32>,
    pub crossings: usize,
}

/// Topological level of every system of a schedule (0 = no predecessors).
pub fn ranks_of(schedule: &crate::snapshot::ScheduleSnapshot) -> Vec<u32> {
    let edges: Vec<(usize, usize)> = schedule
        .flat_edges
        .iter()
        .map(|e| (e.from as usize, e.to as usize))
        .collect();
    rank::longest_path_ranks(schedule.systems.len(), &edges)
}

pub fn compute_layout(input: &LayoutInput, params: &LayoutParams) -> LayoutResult {
    let n = input.nodes.len();
    let edge_pairs: Vec<(usize, usize)> = input.edges.iter().map(|e| (e.from, e.to)).collect();
    let mut ranks = rank::longest_path_ranks(n, &edge_pairs);
    rank::tighten_sources(&mut ranks, &edge_pairs);
    let cluster_of: Vec<Option<usize>> = input.nodes.iter().map(|node| node.cluster).collect();
    let cluster_parent: Vec<Option<usize>> = input.clusters.iter().map(|c| c.parent).collect();
    let ordering = order::order_ranks(&order::OrderInput {
        node_count: n,
        ranks: &ranks,
        edges: &edge_pairs,
        cluster_of: &cluster_of,
        cluster_parent: &cluster_parent,
        sweeps: params.sweeps,
    });
    let coords = coords::assign(input, &ordering, params);
    let edge_paths = input
        .edges
        .iter()
        .enumerate()
        .map(|(i, edge)| {
            let a = coords.node_rects[edge.from];
            let b = coords.node_rects[edge.to];
            let p0 = Vec2::new(a.center().x, a.max.y);
            let p3 = Vec2::new(b.center().x, b.min.y);
            let dy = (p3.y - p0.y).abs().max(20.0) * 0.5;
            EdgePath {
                edge: i,
                points: [p0, p0 + Vec2::new(0.0, dy), p3 - Vec2::new(0.0, dy), p3],
            }
        })
        .collect();
    LayoutResult {
        node_rects: coords.node_rects,
        cluster_rects: coords.cluster_rects,
        edge_paths,
        bounds: coords.bounds,
        ranks,
        crossings: ordering.crossings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(i: u32, cluster: Option<usize>) -> LayoutNode {
        LayoutNode {
            id: NodeRef::System(i),
            width: 60.0,
            cluster,
        }
    }

    fn edge(from: usize, to: usize) -> LayoutEdge {
        LayoutEdge {
            from,
            to,
            kind: OrderKind::Explicit,
            via_hidden: false,
        }
    }

    fn cluster(parent: Option<usize>, name: &str) -> LayoutCluster {
        LayoutCluster {
            set: 0,
            parent,
            name: name.to_string(),
            label_width: 40.0,
        }
    }

    #[test]
    fn edges_point_downwards_and_clusters_do_not_overlap() {
        // Cluster A = {0, 1, 2}, cluster B = {3, 4} nested in A? No: B is a sibling.
        let input = LayoutInput {
            nodes: vec![
                node(0, Some(0)),
                node(1, Some(0)),
                node(2, Some(0)),
                node(3, Some(1)),
                node(4, Some(1)),
                node(5, None),
            ],
            edges: vec![
                edge(0, 1),
                edge(1, 2),
                edge(3, 4),
                edge(5, 0),
                edge(5, 3),
                edge(0, 4),
            ],
            clusters: vec![cluster(None, "A"), cluster(None, "B")],
        };
        let params = LayoutParams::default();
        let result = compute_layout(&input, &params);
        for e in &input.edges {
            assert!(
                result.node_rects[e.from].max.y <= result.node_rects[e.to].min.y,
                "edge {:?} must go downwards",
                e
            );
        }
        let a = result
            .cluster_rects
            .iter()
            .find(|(c, _)| *c == 0)
            .unwrap()
            .1;
        let b = result
            .cluster_rects
            .iter()
            .find(|(c, _)| *c == 1)
            .unwrap()
            .1;
        assert!(
            a.max.x <= b.min.x || b.max.x <= a.min.x,
            "sibling columns overlap: {a:?} {b:?}"
        );
        for (i, rect) in result.node_rects.iter().enumerate() {
            match input.nodes[i].cluster {
                Some(0) => assert!(a.contains(rect.min) && a.contains(rect.max)),
                Some(1) => assert!(b.contains(rect.min) && b.contains(rect.max)),
                _ => assert!(!a.contains(rect.center()) && !b.contains(rect.center())),
            }
        }
        assert!(result.bounds.width() > 0.0 && result.bounds.height() > 0.0);
    }

    #[test]
    fn nested_cluster_sits_inside_parent() {
        let input = LayoutInput {
            nodes: vec![
                node(0, Some(0)),
                node(1, Some(1)),
                node(2, Some(1)),
                node(3, Some(0)),
            ],
            edges: vec![edge(0, 1), edge(1, 2), edge(2, 3)],
            clusters: vec![cluster(None, "outer"), cluster(Some(0), "inner")],
        };
        let result = compute_layout(&input, &LayoutParams::default());
        let outer = result.cluster_rects[0];
        let inner = result.cluster_rects[1];
        assert_eq!(outer.0, 0, "outermost cluster is emitted first");
        assert!(outer.1.contains(inner.1.min) && outer.1.contains(inner.1.max));
    }

    #[test]
    fn empty_input_is_fine() {
        let result = compute_layout(&LayoutInput::default(), &LayoutParams::default());
        assert!(result.node_rects.is_empty());
    }
}
