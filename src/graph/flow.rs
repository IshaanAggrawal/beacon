//! Max-flow over real-valued capacities (Edmonds–Karp), plus decomposition of
//! the result into source-to-sink paths so we can say where support came from.
//!
//! The graphs here are a few dozen project nodes, so a simple BFS
//! augmenting-path algorithm is plenty fast and easy to audit.

use std::collections::VecDeque;

const EPS: f64 = 1e-12;
/// Stand-in for "no limit" that keeps arithmetic well-behaved.
pub const UNBOUNDED: f64 = 1e9;

#[derive(Debug, Clone)]
struct Edge {
    to: usize,
    cap: f64,
    flow: f64,
}

#[derive(Debug, Clone, Default)]
pub struct FlowGraph {
    // Edges are stored in pairs: 2k is forward, 2k+1 its residual twin.
    edges: Vec<Edge>,
    adj: Vec<Vec<usize>>,
}

impl FlowGraph {
    pub fn new(nodes: usize) -> Self {
        Self {
            edges: Vec::new(),
            adj: vec![Vec::new(); nodes],
        }
    }

    pub fn add_edge(&mut self, from: usize, to: usize, cap: f64) {
        if cap <= EPS {
            return;
        }
        self.adj[from].push(self.edges.len());
        self.edges.push(Edge { to, cap, flow: 0.0 });
        self.adj[to].push(self.edges.len());
        self.edges.push(Edge {
            to: from,
            cap: 0.0,
            flow: 0.0,
        });
    }

    fn residual(&self, e: usize) -> f64 {
        self.edges[e].cap - self.edges[e].flow
    }

    pub fn max_flow(&mut self, source: usize, sink: usize) -> f64 {
        let mut total = 0.0;
        loop {
            // BFS for the shortest augmenting path, remembering the edge used to reach each node.
            let mut via = vec![None; self.adj.len()];
            let mut queue = VecDeque::from([source]);
            while let Some(u) = queue.pop_front() {
                if u == sink {
                    break;
                }
                for &e in &self.adj[u] {
                    let v = self.edges[e].to;
                    if v != source && via[v].is_none() && self.residual(e) > EPS {
                        via[v] = Some(e);
                        queue.push_back(v);
                    }
                }
            }
            if via[sink].is_none() {
                return total;
            }

            let mut push = f64::INFINITY;
            let mut v = sink;
            while let Some(e) = via[v] {
                push = push.min(self.residual(e));
                v = self.edges[e ^ 1].to;
            }
            let mut v = sink;
            while let Some(e) = via[v] {
                self.edges[e].flow += push;
                self.edges[e ^ 1].flow -= push;
                v = self.edges[e ^ 1].to;
            }
            total += push;
        }
    }

    /// Splits the current flow into simple source-to-sink paths. Call after
    /// `max_flow`. Consumes the flow, so call it once.
    pub fn decompose(&mut self, source: usize, sink: usize) -> Vec<(Vec<usize>, f64)> {
        let mut paths = Vec::new();
        loop {
            let mut via = vec![None; self.adj.len()];
            let mut seen = vec![false; self.adj.len()];
            let mut stack = vec![source];
            seen[source] = true;
            while let Some(u) = stack.pop() {
                if u == sink {
                    break;
                }
                for &e in &self.adj[u] {
                    let v = self.edges[e].to;
                    // Only forward edges carry real flow.
                    if e % 2 == 0 && !seen[v] && self.edges[e].flow > EPS {
                        seen[v] = true;
                        via[v] = Some(e);
                        stack.push(v);
                    }
                }
            }
            if via[sink].is_none() {
                return paths;
            }

            let mut amount = f64::INFINITY;
            let mut nodes = vec![sink];
            let mut v = sink;
            while let Some(e) = via[v] {
                amount = amount.min(self.edges[e].flow);
                v = self.edges[e ^ 1].to;
                nodes.push(v);
            }
            let mut v = sink;
            while let Some(e) = via[v] {
                self.edges[e].flow -= amount;
                v = self.edges[e ^ 1].to;
            }
            nodes.reverse();
            paths.push((nodes, amount));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn textbook_example() {
        // 0 -> 1 -> 3 and 0 -> 2 -> 3 with a cross edge 1 -> 2.
        let mut g = FlowGraph::new(4);
        g.add_edge(0, 1, 0.7);
        g.add_edge(0, 2, 0.4);
        g.add_edge(1, 2, 0.5);
        g.add_edge(1, 3, 0.3);
        g.add_edge(2, 3, 0.6);
        assert!(close(g.max_flow(0, 3), 0.9));
    }

    #[test]
    fn decomposition_adds_up_to_the_flow() {
        let mut g = FlowGraph::new(4);
        g.add_edge(0, 1, 0.7);
        g.add_edge(0, 2, 0.4);
        g.add_edge(1, 2, 0.5);
        g.add_edge(1, 3, 0.3);
        g.add_edge(2, 3, 0.6);
        let total = g.max_flow(0, 3);
        let paths = g.decompose(0, 3);
        let sum: f64 = paths.iter().map(|(_, a)| a).sum();
        assert!(close(sum, total));
        assert!(
            paths
                .iter()
                .all(|(p, _)| p.first() == Some(&0) && p.last() == Some(&3))
        );
    }

    #[test]
    fn a_bottleneck_caps_everything_behind_it() {
        // Source -> gate (0.2) -> ten nodes that each reach the sink.
        let mut g = FlowGraph::new(13);
        g.add_edge(0, 1, 0.2);
        for n in 2..12 {
            g.add_edge(1, n, UNBOUNDED);
            g.add_edge(n, 12, 1.0);
        }
        assert!(close(g.max_flow(0, 12), 0.2));
    }
}
