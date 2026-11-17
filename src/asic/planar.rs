//! The grid flattened onto one plane, which is what nets are routed over before their wires
//! are given layers.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::cost::Cost;
use crate::id::{NetId, NodeId};
use crate::search::Target;
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
    /// 2^64 over the columns, rounded up, for [`row_of`](Self::row_of).
    inverse: u64,
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
            inverse: (u64::MAX / cols.max(1) as u64).wrapping_add(1),
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
        let row = self.row_of(cell);
        (cell - row * self.cols, row)
    }

    /// The row of a cell: its number over the columns, by a multiplication rather than a
    /// division, which searches do for every step and every bound.
    #[inline]
    fn row_of(&self, cell: usize) -> usize {
        match u32::try_from(cell) {
            // Exact for every 32-bit numerator (Lemire, Kaser and Kurz, 2019).
            Ok(n) if self.cols > 1 => ((u128::from(self.inverse) * u128::from(n)) >> 64) as usize,
            _ => cell / self.cols.max(1),
        }
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
        let y = self.row_of(lo);
        // Not the last of its row, which has no neighbour on the right.
        if hi == lo + 1 && lo + 1 - y * self.cols < self.cols {
            Some(lo - y)
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

    /// Puts the box of a net, if it has one, back to `to`, as the lowest and highest column
    /// and row. Like [`grow_box`](Self::grow_box), this takes `&self`, and must not be called
    /// while an engine searches.
    pub(crate) fn reset_box(&self, net: NetId, to: [u32; 4]) {
        let at = net.index() * 4;
        if let Some(b) = self.boxes.get(at..at + 4) {
            for (slot, v) in b.iter().zip(to) {
                slot.store(v, Ordering::Relaxed);
            }
        }
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

    /// The least a way from a cell, arrived at along a column or not, to another cell can
    /// cost: a step for each edge between, and a turn if it takes one.
    #[inline]
    fn bound(
        &self,
        along_column: bool,
        (ax, ay): (usize, usize),
        (bx, by): (usize, usize),
    ) -> Cost {
        let (dx, dy) = (ax.abs_diff(bx), ay.abs_diff(by));
        let distance = Cost::from((dx + dy) as u32);
        if turns(along_column, dx, dy) {
            distance + self.bend
        } else {
            distance
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

/// Whether a way from a cell arrived at along a column, or not, to one `dx` columns and `dy`
/// rows away must turn. Going both ways takes a turn, and so does going the other way from
/// the one arrived along. The target can be arrived at either way.
#[inline]
fn turns(along_column: bool, dx: usize, dy: usize) -> bool {
    (dx > 0 && dy > 0) || (dx > 0 && along_column) || (dy > 0 && !along_column)
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
        self.bound(along_column, (ax, ay), self.cell_xy(self.cell_of(to)))
    }

    /// The bound of [`lower_bound`](RoutingSpace::lower_bound) for each target in turn, with
    /// the cell of `from` found once rather than for each, and the second node of a cell
    /// skipped when it follows the first at the same cost, as a terminal's two do: the bound
    /// does not depend on which way the target is arrived at.
    fn lower_bound_to_targets(
        &self,
        _net: NetId,
        from: NodeId,
        targets: &[Target<NodeId>],
    ) -> Cost {
        let along_column = from.index() % 2 == 1;
        let at = self.cell_xy(self.cell_of(from));
        let mut lowest = Cost::MAX;
        // Targets that cost nothing to finish at, as a terminal's access points do here, by
        // the least distance with no turn and with one, in whole steps: the bound is one of
        // the two, and the least of them is the least bound, as adding the bend keeps order.
        let (mut straight, mut turning) = (usize::MAX, usize::MAX);
        let mut previous = None;
        for target in targets {
            let cell = self.cell_of(target.node);
            if previous == Some((cell, target.cost)) {
                continue;
            }
            previous = Some((cell, target.cost));
            let to = self.cell_xy(cell);
            if target.cost == Cost::ZERO {
                let (dx, dy) = (at.0.abs_diff(to.0), at.1.abs_diff(to.1));
                if turns(along_column, dx, dy) {
                    turning = turning.min(dx + dy);
                } else {
                    straight = straight.min(dx + dy);
                    if straight == 0 {
                        return Cost::ZERO;
                    }
                }
            } else {
                lowest = lowest.min(self.bound(along_column, at, to) + target.cost);
            }
        }
        if straight != usize::MAX {
            lowest = lowest.min(Cost::from(straight as u32));
        }
        if turning != usize::MAX {
            lowest = lowest.min(Cost::from(turning as u32) + self.bend);
        }
        lowest
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asic::read_design;

    fn plane(cols: usize, rows: usize) -> PlanarSpace {
        let text = format!(
            "grid {cols} {rows} 2\nvertical capacity 0 2\nhorizontal capacity 2 0\n\
             minimum width 1 1\nminimum spacing 1 1\nvia spacing 1 1\n0 0 10 10\nnum net 0\n"
        );
        let design = read_design(text.as_bytes()).unwrap();
        PlanarSpace::new(&design.capacities(), |_, c| c / 2)
    }

    #[test]
    fn cells_and_edges_are_numbered_as_by_division() {
        for (cols, rows) in [(1, 5), (2, 3), (7, 4), (973, 3), (1024, 2)] {
            let plane = plane(cols, rows);
            let cells = cols * rows;
            let by_division = |a: usize, b: usize| {
                let (lo, hi) = (a.min(b), a.max(b));
                if hi == lo + 1 && hi % cols != 0 {
                    Some((lo / cols) * (cols - 1) + lo % cols)
                } else if hi == lo + cols {
                    Some(plane.horizontal_edges() + lo)
                } else {
                    None
                }
            };
            for cell in 0..cells {
                assert_eq!(plane.cell_xy(cell), (cell % cols, cell / cols));
                for other in [cell + 1, cell + cols, cell + 2, cell + cols + 1] {
                    assert_eq!(plane.edge(cell, other), by_division(cell, other));
                    assert_eq!(plane.edge(other, cell), by_division(cell, other));
                }
            }
        }
        // Numbers beyond the grid still divide exactly, past 32 bits too.
        let plane = plane(973, 3);
        for cell in [
            u32::MAX as usize,
            u32::MAX as usize - 1,
            1 << 31,
            (1 << 32) + 5,
        ] {
            assert_eq!(plane.cell_xy(cell), (cell % 973, cell / 973));
        }
    }
}
