//! Layer assignment: giving each edge of a net's tree on the plane a layer, net by net, for
//! the fewest vias that keep within each layer's capacity.
//!
//! Each net is assigned exactly by dynamic programming over its tree, as in COLA (Lee and Wang,
//! 2008): the cost of a node's subtree is worked out for each layer of the edge into it, and
//! the vias at a cell span every layer that its edges and pins are on. An edge whose layer is
//! full costs far more than any number of vias, so nets go up a layer before they overflow one.
//! Nets go one at a time, each against the usage of those before, and the nets on edges that
//! still overflow are then assigned again a few times, with the price of those edges raised
//! each time, as negotiation does.

use super::format::{Capacities, Design, EdgeAt, Gcell, Segment};
use super::planar::PlanarSpace;

/// What one via costs, against an edge that overflows, which costs [`OVERFLOW`] for each unit
/// of capacity it is short.
const VIA: u64 = 1_000;
/// What a unit of overflow costs: more than any number of vias a net could take to avoid it.
const OVERFLOW: u64 = 1_000_000;

/// A net's tree on the plane, as nodes with parents, ready for layers.
#[derive(Clone, Debug, Default)]
pub(crate) struct PlanarTree {
    /// Cells, as nodes of the [`PlanarSpace`], the root first and every node after its parent.
    pub(crate) cells: Vec<u32>,
    /// By node, the index of its parent, or `u32::MAX` for the root.
    pub(crate) parent: Vec<u32>,
    /// By node, the lowest and highest layer of its pins, or `None`.
    pub(crate) pins: Vec<Option<(u8, u8)>>,
    /// By node, the layer of the edge from its parent, once assigned.
    pub(crate) layer: Vec<u8>,
}

/// The usage of every edge on every layer, in capacity units.
pub(crate) struct LayerGrid {
    /// Edges on the plane, numbered as the [`PlanarSpace`]'s resources.
    edges: usize,
    /// Of those, how many are horizontal: they come first.
    horizontal: usize,
    layers: usize,
    /// By layer then edge.
    capacity: Vec<u32>,
    used: Vec<u32>,
    /// By layer then edge, what using it costs on top, raised where it overflowed.
    history: Vec<u64>,
    /// The layers that have capacity for horizontal edges, then for vertical ones.
    candidates: [Vec<u8>; 2],
}

impl LayerGrid {
    pub(crate) fn new(design: &Design, capacities: &Capacities, plane: &PlanarSpace) -> Self {
        let (cols, rows, layers) = (plane.cols(), plane.rows(), design.layers());
        let horizontal = cols.saturating_sub(1) * rows;
        let edges = horizontal + cols * rows.saturating_sub(1);
        let mut capacity = vec![0u32; layers * edges];
        for layer in 0..layers {
            for y in 0..rows {
                for x in 0..cols {
                    if x + 1 < cols {
                        capacity[layer * edges + y * (cols - 1) + x] =
                            capacities.get(EdgeAt::Horizontal { layer, x, y });
                    }
                    if y + 1 < rows {
                        capacity[layer * edges + horizontal + y * cols + x] =
                            capacities.get(EdgeAt::Vertical { layer, x, y });
                    }
                }
            }
        }
        let pick = |caps: &[u32]| -> Vec<u8> {
            let some: Vec<u8> = (0..layers)
                .filter(|&l| caps[l] > 0)
                .map(|l| l as u8)
                .collect();
            if some.is_empty() {
                (0..layers as u8).collect()
            } else {
                some
            }
        };
        LayerGrid {
            edges,
            horizontal,
            layers,
            used: vec![0; capacity.len()],
            history: vec![0; capacity.len()],
            capacity,
            candidates: [
                pick(&design.horizontal_capacity),
                pick(&design.vertical_capacity),
            ],
        }
    }

    /// What adding `width` to edge `e` on `layer` costs.
    #[inline]
    fn cost(&self, layer: usize, e: usize, width: u32) -> u64 {
        let i = layer * self.edges + e;
        let after = self.used[i] + width;
        let over = after.saturating_sub(self.capacity[i].max(self.used[i]));
        u64::from(over) * OVERFLOW + self.history[i]
    }

    fn add(&mut self, layer: usize, e: usize, width: i64) {
        let i = layer * self.edges + e;
        self.used[i] = (i64::from(self.used[i]) + width) as u32;
    }

    /// Whether edge `e` overflows on `layer`.
    fn overflows(&self, layer: usize, e: usize) -> bool {
        let i = layer * self.edges + e;
        self.used[i] > self.capacity[i]
    }

    /// Raises the price of every edge that overflows, by `step` for each unit, and returns
    /// how many overflow.
    fn raise_history(&mut self, step: u64) -> usize {
        let mut count = 0;
        for i in 0..self.used.len() {
            let over = self.used[i].saturating_sub(self.capacity[i]);
            if over > 0 {
                self.history[i] += step * u64::from(over);
                count += 1;
            }
        }
        count
    }
}

/// A net to assign layers to: its tree, and what each wire of it takes on each layer.
pub(crate) struct LayerNet {
    pub(crate) tree: PlanarTree,
    /// By layer, the capacity one wire of the net takes.
    pub(crate) width: Vec<u32>,
}

/// Work space for assigning one net, kept between nets.
#[derive(Default)]
struct Scratch {
    /// By node, the edge on the plane from its parent.
    edge: Vec<usize>,
    /// By node, whether that edge is horizontal.
    horizontal: Vec<bool>,
    /// By node, its children, as a range of `child_list`.
    children: Vec<(u32, u32)>,
    child_list: Vec<u32>,
    /// By node, then candidate layer of its edge, the least cost of its subtree.
    cost: Vec<u64>,
    /// Where each node's row of `cost` starts.
    cost_at: Vec<usize>,
}

/// Assigns layers to every net, in the order given, then again to the nets on edges that
/// still overflow, up to `passes` more times.
pub(crate) fn assign(
    grid: &mut LayerGrid,
    plane: &PlanarSpace,
    nets: &mut [LayerNet],
    order: &[usize],
    passes: u32,
) {
    let mut scratch = Scratch::default();
    for &n in order {
        assign_net(grid, plane, &mut nets[n], &mut scratch);
        apply(grid, plane, &nets[n], 1);
    }
    for _ in 0..passes {
        if grid.raise_history(OVERFLOW / 4) == 0 {
            break;
        }
        // The nets on an overflowing edge, in the same order.
        let again: Vec<usize> = order
            .iter()
            .copied()
            .filter(|&n| {
                let t = &nets[n].tree;
                (1..t.cells.len()).any(|v| {
                    let p = t.parent[v] as usize;
                    let e = edge_of(plane, t.cells[p], t.cells[v]);
                    grid.overflows(usize::from(t.layer[v]), e)
                })
            })
            .collect();
        for &n in &again {
            apply(grid, plane, &nets[n], -1);
            assign_net(grid, plane, &mut nets[n], &mut scratch);
            apply(grid, plane, &nets[n], 1);
        }
    }
}

fn edge_of(plane: &PlanarSpace, a: u32, b: u32) -> usize {
    plane
        .edge(a as usize, b as usize)
        .unwrap_or_else(|| panic!("cells {a} and {b} of a tree are not neighbours"))
}

/// Adds a net's wires to the usage, or takes them away for a `sign` of -1.
fn apply(grid: &mut LayerGrid, plane: &PlanarSpace, net: &LayerNet, sign: i64) {
    let t = &net.tree;
    for v in 1..t.cells.len() {
        let p = t.parent[v] as usize;
        let e = edge_of(plane, t.cells[p], t.cells[v]);
        let l = usize::from(t.layer[v]);
        grid.add(l, e, sign * i64::from(net.width[l]));
    }
}

/// The best assignment of one node's child edges, given the layer of the edge into it and
/// its pins: the least cost of the vias there plus the children's subtrees, and the layer of
/// each child.
fn junction(
    grid: &LayerGrid,
    scratch: &Scratch,
    v: usize,
    incoming: Option<u8>,
    pins: Option<(u8, u8)>,
    choices: &mut Vec<u8>,
) -> u64 {
    let layers = grid.layers;
    let (from, to) = scratch.children[v];
    let kids = &scratch.child_list[from as usize..to as usize];
    choices.clear();
    // A single child and no pins: the vias are just the distance between the two layers.
    if let (Some(lin), None, [c]) = (incoming, pins, kids) {
        let c = *c as usize;
        let cands = &grid.candidates[usize::from(!scratch.horizontal[c])];
        let row = &scratch.cost[scratch.cost_at[c]..];
        let mut best = (u64::MAX, 0u8);
        for (k, &l) in cands.iter().enumerate() {
            let total = row[k].saturating_add(VIA * u64::from(l.abs_diff(lin)));
            if total < best.0 {
                best = (total, l);
            }
        }
        choices.push(best.1);
        return best.0;
    }
    // States are the lowest and highest layer so far, with `layers` for none yet.
    let none = layers;
    let side = layers + 1;
    let start = match (incoming, pins) {
        (Some(l), Some((a, b))) => (usize::from(l.min(a)), usize::from(l.max(b))),
        (Some(l), None) => (usize::from(l), usize::from(l)),
        (None, Some((a, b))) => (usize::from(a), usize::from(b)),
        (None, None) => (none, none),
    };
    let mut states = vec![u64::MAX; side * side];
    states[start.0 * side + start.1] = 0;
    // For backtracking: by child, by state, the state before and the layer chosen.
    let mut back: Vec<Vec<(u32, u8)>> = Vec::with_capacity(kids.len());
    for &c in kids {
        let c = c as usize;
        let cands = &grid.candidates[usize::from(!scratch.horizontal[c])];
        let row = &scratch.cost[scratch.cost_at[c]..];
        let mut next = vec![u64::MAX; side * side];
        let mut from_state = vec![(u32::MAX, 0u8); side * side];
        for (s, &base) in states.iter().enumerate() {
            if base == u64::MAX {
                continue;
            }
            let (lo, hi) = (s / side, s % side);
            for (k, &l) in cands.iter().enumerate() {
                let l = usize::from(l);
                let (nlo, nhi) = if lo == none {
                    (l, l)
                } else {
                    (lo.min(l), hi.max(l))
                };
                let t = nlo * side + nhi;
                let total = base.saturating_add(row[k]);
                if total < next[t] {
                    next[t] = total;
                    from_state[t] = (s as u32, l as u8);
                }
            }
        }
        states = next;
        back.push(from_state);
    }
    let mut best = (u64::MAX, usize::MAX);
    for (s, &c) in states.iter().enumerate() {
        if c == u64::MAX {
            continue;
        }
        let (lo, hi) = (s / side, s % side);
        let vias = if lo == none { 0 } else { (hi - lo) as u64 };
        let total = c.saturating_add(VIA * vias);
        if total < best.0 {
            best = (total, s);
        }
    }
    choices.resize(kids.len(), 0);
    let mut s = best.1;
    for k in (0..kids.len()).rev() {
        if s == usize::MAX {
            break;
        }
        let (prev, l) = back[k][s];
        choices[k] = l;
        s = prev as usize;
    }
    best.0
}

/// Assigns layers to one net's tree, against the usage so far.
fn assign_net(grid: &LayerGrid, plane: &PlanarSpace, net: &mut LayerNet, scratch: &mut Scratch) {
    let t = &mut net.tree;
    let n = t.cells.len();
    t.layer.resize(n, 0);
    if n < 2 {
        return;
    }
    scratch.edge.clear();
    scratch.horizontal.clear();
    scratch.edge.push(usize::MAX);
    scratch.horizontal.push(false);
    for v in 1..n {
        let (a, b) = (t.cells[t.parent[v] as usize], t.cells[v]);
        let e = edge_of(plane, a, b);
        scratch.edge.push(e);
        scratch.horizontal.push(e < grid.horizontal);
    }
    // Children, grouped by parent.
    let mut counts = vec![0u32; n + 1];
    for v in 1..n {
        counts[t.parent[v] as usize + 1] += 1;
    }
    for v in 0..n {
        counts[v + 1] += counts[v];
    }
    scratch.children.clear();
    scratch
        .children
        .extend((0..n).map(|v| (counts[v], counts[v])));
    scratch.child_list.clear();
    scratch.child_list.resize(n.saturating_sub(1), 0);
    for v in 1..n {
        let p = t.parent[v] as usize;
        let slot = &mut scratch.children[p].1;
        scratch.child_list[*slot as usize] = v as u32;
        *slot += 1;
    }
    // Rows of costs, one per node by the candidates for its edge.
    scratch.cost_at.clear();
    let mut size = 0;
    for v in 0..n {
        scratch.cost_at.push(size);
        if v > 0 {
            size += grid.candidates[usize::from(!scratch.horizontal[v])].len();
        }
    }
    scratch.cost.clear();
    scratch.cost.resize(size, u64::MAX);
    let mut choices = Vec::new();
    // Bottom up: children come after their parents.
    for v in (1..n).rev() {
        let horizontal = scratch.horizontal[v];
        let e = scratch.edge[v];
        let at = scratch.cost_at[v];
        let width = &net.width;
        for (k, &l) in grid.candidates[usize::from(!horizontal)].iter().enumerate() {
            let wire = grid.cost(usize::from(l), e, width[usize::from(l)]);
            let below = junction(grid, scratch, v, Some(l), t.pins[v], &mut choices);
            scratch.cost[at + k] = wire.saturating_add(below);
        }
    }
    // Top down, from the root's best.
    let mut stack = vec![(0usize, None::<u8>)];
    while let Some((v, incoming)) = stack.pop() {
        junction(grid, scratch, v, incoming, t.pins[v], &mut choices);
        let (from, to) = scratch.children[v];
        for (k, &c) in scratch.child_list[from as usize..to as usize]
            .iter()
            .enumerate()
        {
            t.layer[c as usize] = choices[k];
            stack.push((c as usize, Some(choices[k])));
        }
    }
}

/// The wires and vias of a net whose layers are assigned, with straight runs on one layer
/// through cells with nothing else joined as one wire.
pub(crate) fn segments(plane: &PlanarSpace, tree: &PlanarTree) -> Vec<Segment> {
    let n = tree.cells.len();
    let mut out = Vec::new();
    if n < 2 {
        return out;
    }
    let cell = |v: usize, layer: u8| {
        let (x, y) = plane.cell_xy(tree.cells[v] as usize);
        Gcell::new(x as u32, y as u32, layer)
    };
    // By node, the span of layers there, and how many edges meet it.
    let mut span: Vec<Option<(u8, u8)>> = tree.pins.clone();
    let mut degree = vec![0u32; n];
    let widen = |s: &mut Option<(u8, u8)>, l: u8| {
        *s = Some(match *s {
            Some((a, b)) => (a.min(l), b.max(l)),
            None => (l, l),
        });
    };
    for v in 1..n {
        let p = tree.parent[v] as usize;
        let l = tree.layer[v];
        widen(&mut span[v], l);
        widen(&mut span[p], l);
        degree[v] += 1;
        degree[p] += 1;
    }
    // By node, the wire its parent edge is part of.
    let mut wire = vec![usize::MAX; n];
    for v in 1..n {
        let p = tree.parent[v] as usize;
        let l = tree.layer[v];
        let joins = p != 0 && degree[p] == 2 && tree.pins[p].is_none() && tree.layer[p] == l && {
            let g = tree.parent[p] as usize;
            let straight = |a: usize, b: usize| {
                let (ax, ay) = plane.cell_xy(tree.cells[a] as usize);
                let (bx, by) = plane.cell_xy(tree.cells[b] as usize);
                (ax == bx, ay == by)
            };
            straight(g, p) == straight(p, v)
        };
        if joins {
            let w = wire[p];
            out[w] = Segment {
                from: out[w].from,
                to: cell(v, l),
            };
            wire[v] = w;
        } else {
            wire[v] = out.len();
            out.push(Segment {
                from: cell(p, l),
                to: cell(v, l),
            });
        }
    }
    for (v, s) in span.iter().enumerate() {
        if let Some((lo, hi)) = *s
            && lo < hi
        {
            out.push(Segment {
                from: cell(v, lo),
                to: cell(v, hi),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asic::format::read_design;

    #[test]
    fn a_bend_takes_one_via_and_pins_reach_their_layer() {
        let text = "grid 3 3 2\nvertical capacity 0 10\nhorizontal capacity 10 0\n\
                    minimum width 1 1\nminimum spacing 1 1\nvia spacing 1 1\n0 0 10 10\n\
                    num net 1\nA 0 2 1\n5 5 1\n25 25 1\n";
        let design = read_design(text.as_bytes()).unwrap();
        let caps = design.capacities();
        let plane = PlanarSpace::new(&caps, |_, c| c / 2);
        let mut grid = LayerGrid::new(&design, &caps, &plane);
        // (0,0) -> (1,0) -> (2,0) -> (2,1) -> (2,2)
        let cells = [0u32, 1, 2, 5, 8];
        let tree = PlanarTree {
            cells: cells.to_vec(),
            parent: vec![u32::MAX, 0, 1, 2, 3],
            pins: vec![Some((0, 0)), None, None, None, Some((0, 0))],
            layer: Vec::new(),
        };
        let mut nets = vec![LayerNet {
            tree,
            width: vec![2, 2],
        }];
        assign(&mut grid, &plane, &mut nets, &[0], 2);
        assert_eq!(nets[0].tree.layer[1..], [0, 0, 1, 1]);
        let segs = segments(&plane, &nets[0].tree);
        assert_eq!(
            segs,
            [
                Segment {
                    from: Gcell::new(0, 0, 0),
                    to: Gcell::new(2, 0, 0)
                },
                Segment {
                    from: Gcell::new(2, 0, 1),
                    to: Gcell::new(2, 2, 1)
                },
                Segment {
                    from: Gcell::new(2, 0, 0),
                    to: Gcell::new(2, 0, 1)
                },
                Segment {
                    from: Gcell::new(2, 2, 0),
                    to: Gcell::new(2, 2, 1)
                },
            ]
        );
    }
}
