//! The grid flattened onto one plane, which is what nets are routed over before their wires
//! are given layers.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::cost::Cost;
use crate::id::{NetId, NodeId};
use crate::space::{DenseSlots, RoutingSpace, Step};

use super::format::{Capacities, EdgeAt};

/// The cells of a design on one plane. An edge between two cells can take as many nets as all
/// layers together have tracks for, a track being the space one wire of the narrowest width
/// takes.
///
/// A cell is two nodes, one for arriving there along a row and one for arriving along a
/// column, so that a step can cost more for turning: node `2 * cell` is the first, and
/// `2 * cell + 1` the second, with cells numbered by row then column. A step costs 1, plus the
/// [bend cost](PlanarSpace::set_bend_cost) if it turns, since wires along rows and along
/// columns are on different layers and a turn takes a via. A net's terminal is both nodes of
/// its cell: see [`PlanarSpace::access`].
///
/// Resources are edges: the horizontal ones first, by row then column of the left cell, then
/// the vertical ones, by the row of the lower cell then column.
#[derive(Debug)]
pub struct PlanarSpace {
    cols: usize,
    rows: usize,
    /// By resource, how many nets the edge takes.
    capacity: Vec<u32>,
    /// What a turn costs on top of the step.
    bend: Cost,
    /// By net, the box searches for it stay within, as the lowest and highest column and row,
    /// four to a net. Empty for no limit. Atomic so that boxes can grow between iterations of
    /// an engine that holds the space; nothing changes them while it searches.
    boxes: Vec<AtomicU32>,
}

impl PlanarSpace {
    /// Flattens `capacities` onto one plane, with `tracks(layer, capacity)` the number of
    /// tracks an edge of that capacity has on a layer. Turns cost nothing more until
    /// [`set_bend_cost`](Self::set_bend_cost).
    pub fn new(capacities: &Capacities, tracks: impl Fn(usize, u32) -> u32) -> PlanarSpace {
        let (cols, rows, layers) = (capacities.cols(), capacities.rows(), capacities.layers());
        let hcount = cols.saturating_sub(1) * rows;
        let mut capacity = vec![0u32; hcount + cols * rows.saturating_sub(1)];
        for layer in 0..layers {
            for y in 0..rows {
                for x in 0..cols {
                    if x + 1 < cols {
                        let c = capacities.get(EdgeAt::Horizontal { layer, x, y });
                        capacity[y * (cols - 1) + x] += tracks(layer, c);
                    }
                    if y + 1 < rows {
                        let c = capacities.get(EdgeAt::Vertical { layer, x, y });
                        capacity[hcount + y * cols + x] += tracks(layer, c);
                    }
                }
            }
        }
        PlanarSpace {
            cols,
            rows,
            capacity,
            bend: Cost::ZERO,
            boxes: Vec::new(),
        }
    }

    /// Sets what a turn costs, on top of the step.
    pub fn set_bend_cost(&mut self, cost: Cost) {
        self.bend = cost;
    }

    /// Columns.
    #[inline]
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Rows.
    #[inline]
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// The number of a cell, by row then column.
    #[inline]
    pub fn cell_at(&self, x: usize, y: usize) -> usize {
        y * self.cols + x
    }

    /// The column and row of a cell.
    #[inline]
    pub fn cell_xy(&self, cell: usize) -> (usize, usize) {
        (cell % self.cols, cell / self.cols)
    }

    /// The cell of a node.
    #[inline]
    pub fn cell_of(&self, node: NodeId) -> usize {
        node.index() / 2
    }

    /// The two nodes of a cell, arriving along a row and along a column, which is what a
    /// terminal there can be reached at.
    #[inline]
    pub fn access(&self, cell: usize) -> [NodeId; 2] {
        [NodeId(2 * cell as u32), NodeId(2 * cell as u32 + 1)]
    }

    /// The resource of the edge between two neighbouring cells, if they are neighbours.
    pub fn edge(&self, a: usize, b: usize) -> Option<usize> {
        let (lo, hi) = (a.min(b), a.max(b));
        if hi == lo + 1 && hi % self.cols != 0 {
            let (x, y) = (lo % self.cols, lo / self.cols);
            Some(y * (self.cols - 1) + x)
        } else if hi == lo + self.cols {
            Some(self.horizontal_edges() + lo)
        } else {
            None
        }
    }

    /// How many edges run along rows. They are numbered before the others.
    #[inline]
    pub fn horizontal_edges(&self) -> usize {
        self.cols.saturating_sub(1) * self.rows
    }

    /// How many nets an edge takes.
    #[inline]
    pub fn edge_capacity(&self, edge: usize) -> u32 {
        self.capacity.get(edge).copied().unwrap_or(0)
    }

    /// Keeps each net's searches within a box, by net number, as the lowest and highest column
    /// and row.
    pub fn set_boxes(&mut self, boxes: &[[u32; 4]]) {
        self.boxes = boxes.iter().flatten().map(|&v| AtomicU32::new(v)).collect();
    }

    /// The box of a net, as the lowest and highest column and row, if it has one.
    pub fn net_box(&self, net: NetId) -> Option<[u32; 4]> {
        let at = net.index() * 4;
        let b = self.boxes.get(at..at + 4)?;
        Some([0, 1, 2, 3].map(|i| b[i].load(Ordering::Relaxed)))
    }

    /// Grows the box of a net by `by` cells on every side, within the grid.
    ///
    /// This takes `&self` so that it can be called between iterations of an engine that
    /// holds the space. Calling it while the engine searches would make results depend on
    /// timing.
    pub fn grow_box(&self, net: NetId, by: u32) {
        let Some([x0, x1, y0, y1]) = self.net_box(net) else {
            return;
        };
        let at = net.index() * 4;
        let (xmax, ymax) = ((self.cols - 1) as u32, (self.rows - 1) as u32);
        let new = [
            x0.saturating_sub(by),
            x1.saturating_add(by).min(xmax),
            y0.saturating_sub(by),
            y1.saturating_add(by).min(ymax),
        ];
        for (i, v) in new.into_iter().enumerate() {
            self.boxes[at + i].store(v, Ordering::Relaxed);
        }
    }

    /// Whether a cell is within a net's box.
    #[inline]
    fn allowed(&self, net: NetId, x: usize, y: usize) -> bool {
        match self.net_box(net) {
            Some([x0, x1, y0, y1]) => {
                (x0 as usize..=x1 as usize).contains(&x) && (y0 as usize..=y1 as usize).contains(&y)
            }
            None => true,
        }
    }
}

impl RoutingSpace for PlanarSpace {
    type Node = NodeId;
    type Resource = NodeId;
    type Slots = DenseSlots<NodeId>;
    type ResourceSlots = DenseSlots<NodeId>;

    fn new_slots(&self) -> Self::Slots {
        DenseSlots::new()
    }

    fn new_resource_slots(&self) -> Self::ResourceSlots {
        DenseSlots::new()
    }

    fn contains(&self, node: NodeId) -> bool {
        node.index() < 2 * self.cols * self.rows
    }

    fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
        let along_column = from.index() % 2 == 1;
        let (x, y) = self.cell_xy(self.cell_of(from));
        let straight = Cost::ONE;
        let turn = Cost::ONE + self.bend;
        let mut push = |x: usize, y: usize, column: bool| {
            if self.allowed(net, x, y) {
                let [row_node, column_node] = self.access(self.cell_at(x, y));
                out.push(Step {
                    to: if column { column_node } else { row_node },
                    cost: if column == along_column {
                        straight
                    } else {
                        turn
                    },
                });
            }
        };
        if x + 1 < self.cols {
            push(x + 1, y, false);
        }
        if x > 0 {
            push(x - 1, y, false);
        }
        if y + 1 < self.rows {
            push(x, y + 1, true);
        }
        if y > 0 {
            push(x, y - 1, true);
        }
    }

    fn lower_bound(&self, _net: NetId, from: NodeId, to: NodeId) -> Cost {
        let along_column = from.index() % 2 == 1;
        let (ax, ay) = self.cell_xy(self.cell_of(from));
        let (bx, by) = self.cell_xy(self.cell_of(to));
        let (dx, dy) = (ax.abs_diff(bx), ay.abs_diff(by));
        // Going both ways takes a turn, and so does going the other way from the one arrived
        // along. The target can be arrived at either way.
        let turns = (dx > 0 && dy > 0) || (dx > 0 && along_column) || (dy > 0 && !along_column);
        let distance = Cost::from((dx + dy) as u32);
        if turns {
            distance + self.bend
        } else {
            distance
        }
    }

    fn resources(&self, _net: NetId, from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
        if let Some(e) = self.edge(self.cell_of(from), self.cell_of(to)) {
            out.push(NodeId(e as u32));
        }
    }

    fn capacity(&self, resource: NodeId) -> u32 {
        self.edge_capacity(resource.index())
    }
}
