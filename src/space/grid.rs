//! Grid routing spaces.
//!
//! [`Grid`] is a multi-layer rectangular grid: the simple space for tests and benchmarks,
//! and a stepping stone to PCB routing.

use core::f64::consts::SQRT_2;
use core::fmt;

use crate::cost::Cost;
use crate::id::{NetId, NodeId};
use crate::space::{DenseSlots, RoutingSpace, Step};

/// Which moves a [`Grid`] allows within a layer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Connectivity {
    /// Moves along the X and Y axes only.
    #[default]
    Orthogonal,
    /// Moves along the axes and the 45-degree diagonals.
    Octilinear,
}

/// One of the costs a [`Grid`] is configured with, as named by [`GridError::InvalidCost`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GridCostKind {
    /// The cost of a step along an axis.
    Orthogonal,
    /// The cost of a diagonal step.
    Diagonal,
    /// The cost of a layer change.
    Via,
}

impl fmt::Display for GridCostKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            GridCostKind::Orthogonal => "orthogonal",
            GridCostKind::Diagonal => "diagonal",
            GridCostKind::Via => "via",
        })
    }
}

/// Why a [`Grid`] could not be created or changed.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum GridError {
    /// A dimension is zero, so the grid would have no cells.
    EmptyDimension {
        /// The requested width.
        width: u32,
        /// The requested height.
        height: u32,
        /// The requested number of layers.
        layers: u32,
    },
    /// The number of cells does not fit in a `u32`, so cells could not have a [`NodeId`].
    TooManyCells {
        /// The requested width.
        width: u32,
        /// The requested height.
        height: u32,
        /// The requested number of layers.
        layers: u32,
    },
    /// A configured cost is negative, infinite or NaN.
    InvalidCost {
        /// Which cost is invalid.
        kind: GridCostKind,
        /// The rejected value.
        value: f64,
    },
    /// A position lies outside the grid.
    OutOfBounds {
        /// The X coordinate.
        x: u32,
        /// The Y coordinate.
        y: u32,
        /// The layer.
        layer: u32,
    },
}

impl fmt::Display for GridError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GridError::EmptyDimension {
                width,
                height,
                layers,
            } => write!(f, "grid of {width} x {height} x {layers} has no cells"),
            GridError::TooManyCells {
                width,
                height,
                layers,
            } => write!(
                f,
                "grid of {width} x {height} x {layers} has more cells than a u32 can number"
            ),
            GridError::InvalidCost { kind, value } => write!(
                f,
                "{kind} cost {value} is not a finite, non-negative number"
            ),
            GridError::OutOfBounds { x, y, layer } => {
                write!(f, "position ({x}, {y}, {layer}) is outside the grid")
            }
        }
    }
}

impl std::error::Error for GridError {}

/// Configures a [`Grid`]. Created by [`Grid::builder`].
///
/// Costs are taken as `f64` and validated by [`build`](GridBuilder::build), so that a
/// mistake is reported against the cost it concerns.
#[derive(Clone, Copy, Debug)]
pub struct GridBuilder {
    width: u32,
    height: u32,
    layers: u32,
    connectivity: Connectivity,
    orthogonal: f64,
    diagonal: f64,
    via: f64,
}

impl GridBuilder {
    /// Sets which moves are allowed within a layer. The default is
    /// [`Connectivity::Orthogonal`].
    #[must_use]
    pub fn connectivity(mut self, connectivity: Connectivity) -> Self {
        self.connectivity = connectivity;
        self
    }

    /// Sets the cost of a step along an axis. The default is 1.
    #[must_use]
    pub fn orthogonal_cost(mut self, cost: f64) -> Self {
        self.orthogonal = cost;
        self
    }

    /// Sets the cost of a diagonal step. The default is √2.
    ///
    /// It has no effect on routing unless the connectivity is
    /// [`Connectivity::Octilinear`], but it is validated regardless.
    #[must_use]
    pub fn diagonal_cost(mut self, cost: f64) -> Self {
        self.diagonal = cost;
        self
    }

    /// Sets the cost of moving to an adjacent layer. The default is 1.
    #[must_use]
    pub fn via_cost(mut self, cost: f64) -> Self {
        self.via = cost;
        self
    }

    /// Creates the grid, with no cell blocked and every capacity at 1.
    ///
    /// Zero costs are accepted, because free vias or free moves are meaningful and every
    /// bound in this module stays admissible with them.
    ///
    /// # Errors
    ///
    /// Returns an error if a dimension is zero, if the number of cells does not fit in a
    /// `u32`, or if a cost is negative, infinite or NaN.
    pub fn build(self) -> Result<Grid, GridError> {
        let GridBuilder {
            width,
            height,
            layers,
            connectivity,
            orthogonal,
            diagonal,
            via,
        } = self;

        if width == 0 || height == 0 || layers == 0 {
            return Err(GridError::EmptyDimension {
                width,
                height,
                layers,
            });
        }
        let cells = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|plane| plane.checked_mul(u64::from(layers)))
            .and_then(|cells| u32::try_from(cells).ok())
            .ok_or(GridError::TooManyCells {
                width,
                height,
                layers,
            })?;

        let cost = |kind, value| Cost::new(value).ok_or(GridError::InvalidCost { kind, value });
        let orthogonal = cost(GridCostKind::Orthogonal, orthogonal)?;
        let diagonal = cost(GridCostKind::Diagonal, diagonal)?;
        let via = cost(GridCostKind::Via, via)?;

        let mut used_costs = vec![orthogonal];
        if connectivity == Connectivity::Octilinear {
            used_costs.push(diagonal);
        }
        if layers > 1 {
            used_costs.push(via);
        }

        Ok(Grid {
            width,
            height,
            layers,
            // The cells of one plane are a divisor of `cells`, so this cannot overflow.
            plane_cells: width * height,
            cells,
            connectivity,
            orthogonal,
            diagonal,
            via,
            bound_scale: bound_scale(cells, &used_costs),
            blocked: vec![0; cells.div_ceil(BLOCKED_WORD_BITS) as usize],
            capacities: Vec::new(),
        })
    }
}

const BLOCKED_WORD_BITS: u32 = u64::BITS;

/// The capacity of every cell that has not been given another one.
const DEFAULT_CAPACITY: u32 = 1;

/// Returns the factor that keeps a computed distance from exceeding a summed one.
///
/// A distance computed as `n * cost` is rounded differently from the same cost added `n`
/// times: ten steps of 0.1 add up to less than `10.0 * 0.1`. A search sees the sum, so an
/// unscaled bound can exceed the true cost by a few units in the last place.
///
/// An optimal path visits each cell once, because dropping a cycle cannot raise a sum
/// when addition is monotonic. So it has fewer than `cells` steps, and adding them loses
/// at most `(cells - 1) * EPSILON / 2` of the total. Scaling by `1 - (cells + 4) * EPSILON`
/// covers that and the few rounding errors made computing the bound itself.
///
/// Whole-number costs with a small enough total never round at all, and they keep the
/// exact bound. That matters beyond tidiness: scaling makes `g + h` grow slightly along a
/// path, so a search can no longer see the exact ties between equally good paths that
/// let it run straight to the target.
fn bound_scale(cells: u32, used_costs: &[Cost]) -> f64 {
    /// Integers up to this size are all exactly representable.
    const EXACT_LIMIT: f64 = 9_007_199_254_740_992.0;

    let cells = f64::from(cells);
    let exact = used_costs
        .iter()
        .all(|cost| cost.get().fract() == 0.0 && cost.get() * cells <= EXACT_LIMIT);
    if exact {
        1.0
    } else {
        1.0 - (cells + 4.0) * f64::EPSILON
    }
}

/// A multi-layer rectangular grid of cells, where each cell is a node and a resource.
///
/// # Nodes
///
/// Cells are numbered plane by plane and row by row, as
/// `x + width * y + width * height * layer`, so that node IDs are dense and
/// [`DenseSlots`] applies. [`node_at`](Grid::node_at) and [`position`](Grid::position)
/// convert between the two forms.
///
/// # Moves
///
/// A cell can step to its neighbours along the axes, also along the diagonals under
/// [`Connectivity::Octilinear`], and to the same position on an adjacent layer. Steps are
/// produced in increasing [`NodeId`] order: the layer below, then the row below, the
/// cell's own row, the row above, and the layer above.
///
/// # Blocking
///
/// A blocked cell is removed from the graph: no step enters it, and
/// [`expand`](RoutingSpace::expand) from it produces nothing. Letting a start escape from
/// a blocked cell would make routes one-way and produce routes that occupy a cell they
/// are not allowed in, so a search from one finds nothing instead. Its
/// [`capacity`](RoutingSpace::capacity) reads 0, so that a checker treats any use of it
/// as overuse, including a route that starts and ends there without taking a step.
/// Clearing the block restores the capacity it had.
///
/// A diagonal step looks only at the cell it arrives at. It can pass between two blocked
/// cells that touch at a corner, and two diagonals that cross each other share no
/// resource. This grid is a logical model, and a space with real geometry has to treat
/// these cases itself.
///
/// # Costs
///
/// All costs must be finite and non-negative, and zero is allowed. Very large costs are
/// allowed too, with sums saturating at [`Cost::MAX`] like any other [`Cost`] arithmetic.
///
/// # Lower bound
///
/// The bound is admissible for every cost configuration, and accounts for rounding when
/// a search adds up step costs. It is the layer distance times the via cost, plus a planar
/// term. Under [`Connectivity::Orthogonal`] that term is the Manhattan distance times the
/// orthogonal cost, and under [`Connectivity::Octilinear`] it is the smallest of:
///
/// - diagonals only, which is cheapest when diagonals cost less than orthogonal steps,
///   since two diagonals can zigzag along an axis;
/// - the octile distance, as many diagonals as fit and orthogonal steps for the rest;
/// - orthogonal steps only, which is cheapest when a diagonal costs more than two of them.
///
/// Blocked cells are ignored, and the bound is exact only on an unobstructed grid with
/// room for the moves it counts.
///
/// # Nets
///
/// There are no per-net rules yet, so every method ignores its `net` argument.
///
/// # Foreign nodes
///
/// A [`NodeId`] beyond the last cell behaves as a blocked cell would: it has no steps,
/// consumes no resources and has a capacity of 0. Bounds involving one are
/// [`Cost::ZERO`].
///
/// # Examples
///
/// ```
/// use guide::id::NetId;
/// use guide::space::grid::{Connectivity, Grid};
/// use guide::space::RoutingSpace;
///
/// let mut grid = Grid::builder(3, 3, 2)
///     .connectivity(Connectivity::Octilinear)
///     .via_cost(5.0)
///     .build()?;
/// grid.set_blocked(1, 0, 0, true)?;
///
/// let net = NetId(0);
/// let corner = grid.node_at(0, 0, 0).unwrap();
/// let mut steps = Vec::new();
/// grid.expand(net, corner, &mut steps);
///
/// let reached: Vec<_> = steps.iter().map(|s| grid.position(s.to).unwrap()).collect();
/// assert_eq!(reached, [(0, 1, 0), (1, 1, 0), (0, 0, 1)]);
/// assert_eq!(steps[1].cost.get(), core::f64::consts::SQRT_2);
/// assert_eq!(steps[2].cost.get(), 5.0);
///
/// let far = grid.node_at(2, 2, 1).unwrap();
/// assert!(grid.lower_bound(net, corner, far).get() <= 2.0 * core::f64::consts::SQRT_2 + 5.0);
/// # Ok::<(), guide::space::grid::GridError>(())
/// ```
#[derive(Clone)]
pub struct Grid {
    width: u32,
    height: u32,
    layers: u32,
    plane_cells: u32,
    cells: u32,
    connectivity: Connectivity,
    orthogonal: Cost,
    diagonal: Cost,
    via: Cost,
    bound_scale: f64,
    /// One bit per cell, because a grid can have billions of them.
    blocked: Vec<u64>,
    /// Empty until a cell is given a capacity other than the default, since most grids
    /// never need the table.
    capacities: Vec<u32>,
}

impl Grid {
    /// Starts configuring a grid of the given dimensions.
    ///
    /// The dimensions are checked by [`GridBuilder::build`].
    pub fn builder(width: u32, height: u32, layers: u32) -> GridBuilder {
        GridBuilder {
            width,
            height,
            layers,
            connectivity: Connectivity::Orthogonal,
            orthogonal: 1.0,
            diagonal: SQRT_2,
            via: 1.0,
        }
    }

    /// Creates a 4-connected grid with unit costs.
    ///
    /// # Errors
    ///
    /// Returns an error if a dimension is zero, or if the number of cells does not fit in
    /// a `u32`.
    pub fn new(width: u32, height: u32, layers: u32) -> Result<Grid, GridError> {
        Grid::builder(width, height, layers).build()
    }

    /// Returns the number of cells along X.
    #[inline]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Returns the number of cells along Y.
    #[inline]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Returns the number of layers.
    #[inline]
    pub const fn layers(&self) -> u32 {
        self.layers
    }

    /// Returns the total number of cells. Valid node IDs are below this.
    #[inline]
    pub const fn cell_count(&self) -> u32 {
        self.cells
    }

    /// Returns which moves are allowed within a layer.
    #[inline]
    pub const fn connectivity(&self) -> Connectivity {
        self.connectivity
    }

    /// Returns the cost of a step along an axis.
    #[inline]
    pub const fn orthogonal_cost(&self) -> Cost {
        self.orthogonal
    }

    /// Returns the cost of a diagonal step.
    #[inline]
    pub const fn diagonal_cost(&self) -> Cost {
        self.diagonal
    }

    /// Returns the cost of moving to an adjacent layer.
    #[inline]
    pub const fn via_cost(&self) -> Cost {
        self.via
    }

    /// Returns the node at a position, or `None` if it lies outside the grid.
    #[inline]
    pub const fn node_at(&self, x: u32, y: u32, layer: u32) -> Option<NodeId> {
        if x < self.width && y < self.height && layer < self.layers {
            // Each term stays below the cell count, which fits in a u32.
            Some(NodeId(x + self.width * y + self.plane_cells * layer))
        } else {
            None
        }
    }

    /// Returns the `(x, y, layer)` position of a node, or `None` if it is not in the grid.
    #[inline]
    pub const fn position(&self, node: NodeId) -> Option<(u32, u32, u32)> {
        if node.0 < self.cells {
            Some(self.unpack(node.0))
        } else {
            None
        }
    }

    /// Returns whether no route can use `node`.
    ///
    /// Nodes outside the grid count as blocked.
    #[inline]
    pub fn is_blocked(&self, node: NodeId) -> bool {
        node.0 >= self.cells || self.cell_blocked(node.0)
    }

    /// Blocks or clears the cell at a position.
    ///
    /// # Errors
    ///
    /// Returns an error if the position lies outside the grid.
    pub fn set_blocked(
        &mut self,
        x: u32,
        y: u32,
        layer: u32,
        blocked: bool,
    ) -> Result<(), GridError> {
        let id = self.require_node(x, y, layer)?.0;
        let word = &mut self.blocked[(id / BLOCKED_WORD_BITS) as usize];
        let mask = 1 << (id % BLOCKED_WORD_BITS);
        if blocked {
            *word |= mask;
        } else {
            *word &= !mask;
        }
        Ok(())
    }

    /// Sets how many nets can share the cell at a position before it counts as overused.
    ///
    /// A capacity of 0 leaves the cell routable, with any use of it counting as overuse,
    /// which is what makes it different from blocking the cell.
    ///
    /// # Errors
    ///
    /// Returns an error if the position lies outside the grid.
    pub fn set_capacity(
        &mut self,
        x: u32,
        y: u32,
        layer: u32,
        capacity: u32,
    ) -> Result<(), GridError> {
        let id = self.require_node(x, y, layer)?;
        if self.capacities.is_empty() {
            if capacity == DEFAULT_CAPACITY {
                return Ok(());
            }
            self.capacities
                .resize(self.cells as usize, DEFAULT_CAPACITY);
        }
        self.capacities[id.index()] = capacity;
        Ok(())
    }

    fn require_node(&self, x: u32, y: u32, layer: u32) -> Result<NodeId, GridError> {
        self.node_at(x, y, layer)
            .ok_or(GridError::OutOfBounds { x, y, layer })
    }

    /// Splits a cell number that is known to be in the grid.
    #[inline]
    const fn unpack(&self, id: u32) -> (u32, u32, u32) {
        let in_plane = id % self.plane_cells;
        (
            in_plane % self.width,
            in_plane / self.width,
            id / self.plane_cells,
        )
    }

    /// Reads the block of a cell number that is known to be in the grid.
    #[inline]
    fn cell_blocked(&self, id: u32) -> bool {
        let word = self.blocked[(id / BLOCKED_WORD_BITS) as usize];
        (word >> (id % BLOCKED_WORD_BITS)) & 1 != 0
    }

    #[inline]
    fn push_if_free(&self, id: u32, cost: Cost, out: &mut Vec<Step<NodeId>>) {
        if !self.cell_blocked(id) {
            out.push(Step {
                to: NodeId(id),
                cost,
            });
        }
    }

    /// Pushes the cells of an adjacent row, given the one directly above or below `x`.
    #[inline]
    fn push_adjacent_row(&self, above_or_below: u32, x: u32, out: &mut Vec<Step<NodeId>>) {
        let diagonals = self.connectivity == Connectivity::Octilinear;
        if diagonals && x > 0 {
            self.push_if_free(above_or_below - 1, self.diagonal, out);
        }
        self.push_if_free(above_or_below, self.orthogonal, out);
        if diagonals && x + 1 < self.width {
            self.push_if_free(above_or_below + 1, self.diagonal, out);
        }
    }

    /// Returns the cost of the cheapest moves spanning a distance within a layer.
    #[inline]
    fn planar_distance(&self, dx: u32, dy: u32) -> f64 {
        let orthogonal = self.orthogonal.get();
        let far = f64::from(dx.max(dy));
        let near = f64::from(dx.min(dy));
        match self.connectivity {
            Connectivity::Orthogonal => (far + near) * orthogonal,
            Connectivity::Octilinear => {
                // A path of `a` diagonals and `b` orthogonal steps needs a + b >= far, as
                // each move advances at most 1 along the longer axis, and
                // 2a + b >= far + near, as a diagonal advances at most 2 in total. The
                // cheapest such (a, b) lies at a corner of that region, whatever the
                // costs are, so taking the smallest corner needs no case analysis.
                let diagonal = self.diagonal.get();
                let diagonals_only = far * diagonal;
                let octile = near * diagonal + (far - near) * orthogonal;
                let orthogonal_only = (far + near) * orthogonal;
                diagonals_only.min(octile).min(orthogonal_only)
            }
        }
    }
}

impl fmt::Debug for Grid {
    /// Leaves out the per-cell tables, which can hold billions of entries.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Grid")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("layers", &self.layers)
            .field("connectivity", &self.connectivity)
            .field("orthogonal", &self.orthogonal)
            .field("diagonal", &self.diagonal)
            .field("via", &self.via)
            .finish_non_exhaustive()
    }
}

impl RoutingSpace for Grid {
    type Node = NodeId;
    type Resource = NodeId;
    type Slots = DenseSlots<NodeId>;

    #[inline]
    fn new_slots(&self) -> DenseSlots<NodeId> {
        DenseSlots::new()
    }

    fn expand(&self, _net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
        let id = from.0;
        if self.is_blocked(from) {
            return;
        }
        let (x, y, layer) = self.unpack(id);

        // None of this can overflow: every neighbour tested for lies inside the grid, and
        // so has a number below the cell count.
        if layer > 0 {
            self.push_if_free(id - self.plane_cells, self.via, out);
        }
        if y > 0 {
            self.push_adjacent_row(id - self.width, x, out);
        }
        if x > 0 {
            self.push_if_free(id - 1, self.orthogonal, out);
        }
        if x + 1 < self.width {
            self.push_if_free(id + 1, self.orthogonal, out);
        }
        if y + 1 < self.height {
            self.push_adjacent_row(id + self.width, x, out);
        }
        if layer + 1 < self.layers {
            self.push_if_free(id + self.plane_cells, self.via, out);
        }
    }

    #[inline]
    fn lower_bound(&self, _net: NetId, from: NodeId, to: NodeId) -> Cost {
        let (Some(from), Some(to)) = (self.position(from), self.position(to)) else {
            return Cost::ZERO;
        };
        let planar = self.planar_distance(from.0.abs_diff(to.0), from.1.abs_diff(to.1));
        let layers = f64::from(from.2.abs_diff(to.2)) * self.via.get();
        // Layer changes leave the position alone, so the two terms add up. Clamping makes
        // an overflow saturate as path sums do, and nothing here can be negative or NaN.
        let bound = (planar + layers).min(f64::MAX) * self.bound_scale;
        Cost::new(bound).unwrap_or(Cost::ZERO)
    }

    #[inline]
    fn resources(&self, _net: NetId, _from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
        if to.0 < self.cells {
            out.push(to);
        }
    }

    #[inline]
    fn capacity(&self, resource: NodeId) -> u32 {
        if self.is_blocked(resource) {
            0
        } else {
            self.capacities
                .get(resource.index())
                .copied()
                .unwrap_or(DEFAULT_CAPACITY)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::NodeSlots;
    use proptest::prelude::*;

    const NET: NetId = NetId(0);

    fn steps(grid: &Grid, from: NodeId) -> Vec<(u32, u32, u32, f64)> {
        let mut out = Vec::new();
        grid.expand(NET, from, &mut out);
        out.iter()
            .map(|step| {
                let (x, y, layer) = grid.position(step.to).unwrap();
                (x, y, layer, step.cost.get())
            })
            .collect()
    }

    fn octilinear(width: u32, height: u32, layers: u32) -> GridBuilder {
        Grid::builder(width, height, layers).connectivity(Connectivity::Octilinear)
    }

    #[test]
    fn packs_plane_by_plane_and_row_by_row() {
        let grid = Grid::new(3, 4, 2).unwrap();
        assert_eq!(grid.cell_count(), 24);
        assert_eq!(grid.node_at(0, 0, 0), Some(NodeId(0)));
        assert_eq!(grid.node_at(1, 0, 0), Some(NodeId(1)));
        assert_eq!(grid.node_at(0, 1, 0), Some(NodeId(3)));
        assert_eq!(grid.node_at(0, 0, 1), Some(NodeId(12)));
        assert_eq!(grid.node_at(2, 3, 1), Some(NodeId(23)));
    }

    #[test]
    fn positions_round_trip() {
        let grid = Grid::new(3, 4, 5).unwrap();
        let mut next = 0;
        for layer in 0..5 {
            for y in 0..4 {
                for x in 0..3 {
                    let node = grid.node_at(x, y, layer).unwrap();
                    assert_eq!(node, NodeId(next));
                    assert_eq!(grid.position(node), Some((x, y, layer)));
                    next += 1;
                }
            }
        }
        assert_eq!(next, grid.cell_count());
    }

    #[test]
    fn rejects_positions_and_nodes_outside_the_grid() {
        let mut grid = Grid::new(3, 4, 2).unwrap();
        assert_eq!(grid.node_at(3, 0, 0), None);
        assert_eq!(grid.node_at(0, 4, 0), None);
        assert_eq!(grid.node_at(0, 0, 2), None);
        assert_eq!(grid.position(NodeId(24)), None);
        let outside = GridError::OutOfBounds {
            x: 3,
            y: 0,
            layer: 0,
        };
        assert_eq!(grid.set_blocked(3, 0, 0, true), Err(outside));
        assert_eq!(grid.set_capacity(3, 0, 0, 2), Err(outside));
    }

    #[test]
    fn rejects_bad_dimensions() {
        for (width, height, layers) in [(0, 1, 1), (1, 0, 1), (1, 1, 0), (0, u32::MAX, u32::MAX)] {
            assert_eq!(
                Grid::new(width, height, layers).unwrap_err(),
                GridError::EmptyDimension {
                    width,
                    height,
                    layers
                }
            );
        }
        // Each of these overflows at a different point: the cell count of a u32, the
        // same with layers doing the damage, and the u64 the count is computed in.
        for (width, height, layers) in [
            (65536, 65536, 1),
            (65536, 32768, 2),
            (u32::MAX, u32::MAX, u32::MAX),
        ] {
            assert_eq!(
                Grid::new(width, height, layers).unwrap_err(),
                GridError::TooManyCells {
                    width,
                    height,
                    layers
                }
            );
        }
    }

    #[test]
    fn largest_grid_addresses_every_corner_without_overflow() {
        // 65535 * 65537 is exactly u32::MAX. The block table is about 512 MiB of zero
        // pages that this test never touches.
        let mut grid = octilinear(65535, 65537, 1).build().unwrap();
        assert_eq!(grid.cell_count(), u32::MAX);
        let last = grid.node_at(65534, 65536, 0).unwrap();
        assert_eq!(last, NodeId(u32::MAX - 1));
        assert_eq!(grid.position(last), Some((65534, 65536, 0)));
        assert_eq!(
            steps(&grid, last),
            [
                (65533, 65535, 0, SQRT_2),
                (65534, 65535, 0, 1.0),
                (65533, 65536, 0, 1.0),
            ]
        );
        assert_eq!(grid.position(NodeId(u32::MAX)), None);

        grid.set_blocked(65534, 65536, 0, true).unwrap();
        assert!(grid.is_blocked(last));
    }

    #[test]
    fn rejects_invalid_costs_and_names_them() {
        for bad in [-1.0, f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            let kinds = [
                (
                    Grid::builder(1, 1, 1).orthogonal_cost(bad),
                    GridCostKind::Orthogonal,
                ),
                (
                    Grid::builder(1, 1, 1).diagonal_cost(bad),
                    GridCostKind::Diagonal,
                ),
                (Grid::builder(1, 1, 1).via_cost(bad), GridCostKind::Via),
            ];
            for (builder, expected) in kinds {
                match builder.build() {
                    Err(GridError::InvalidCost { kind, value }) => {
                        assert_eq!(kind, expected);
                        assert_eq!(value.to_bits(), bad.to_bits());
                    }
                    other => panic!("expected an invalid cost error, got {other:?}"),
                }
            }
        }
    }

    #[test]
    fn errors_have_messages() {
        let message = Grid::builder(2, 2, 1)
            .via_cost(-1.0)
            .build()
            .unwrap_err()
            .to_string();
        assert_eq!(message, "via cost -1 is not a finite, non-negative number");
        let message = Grid::new(65536, 65536, 1).unwrap_err().to_string();
        assert!(message.contains("65536 x 65536 x 1"));
    }

    #[test]
    fn accepts_zero_and_extreme_costs() {
        let grid = Grid::builder(2, 1, 2)
            .orthogonal_cost(0.0)
            .via_cost(f64::MAX)
            .build()
            .unwrap();
        assert_eq!(grid.orthogonal_cost(), Cost::ZERO);
        assert_eq!(grid.via_cost(), Cost::MAX);
        assert_eq!(
            steps(&grid, NodeId(0)),
            [(1, 0, 0, 0.0), (0, 0, 1, f64::MAX)]
        );
    }

    #[test]
    fn defaults_are_four_connected_with_unit_costs() {
        let grid = Grid::new(2, 2, 1).unwrap();
        assert_eq!(grid.connectivity(), Connectivity::Orthogonal);
        assert_eq!(grid.orthogonal_cost(), Cost::ONE);
        assert_eq!(grid.via_cost(), Cost::ONE);
        assert_eq!(grid.diagonal_cost().get(), SQRT_2);
    }

    #[test]
    fn orthogonal_centre_has_four_neighbours_in_id_order() {
        let grid = Grid::builder(3, 3, 1).orthogonal_cost(2.0).build().unwrap();
        let centre = grid.node_at(1, 1, 0).unwrap();
        assert_eq!(
            steps(&grid, centre),
            [
                (1, 0, 0, 2.0),
                (0, 1, 0, 2.0),
                (2, 1, 0, 2.0),
                (1, 2, 0, 2.0),
            ]
        );
    }

    #[test]
    fn octilinear_centre_has_eight_neighbours_in_id_order() {
        let grid = octilinear(3, 3, 1)
            .orthogonal_cost(2.0)
            .diagonal_cost(3.0)
            .build()
            .unwrap();
        let centre = grid.node_at(1, 1, 0).unwrap();
        assert_eq!(
            steps(&grid, centre),
            [
                (0, 0, 0, 3.0),
                (1, 0, 0, 2.0),
                (2, 0, 0, 3.0),
                (0, 1, 0, 2.0),
                (2, 1, 0, 2.0),
                (0, 2, 0, 3.0),
                (1, 2, 0, 2.0),
                (2, 2, 0, 3.0),
            ]
        );
    }

    #[test]
    fn corners_and_edges_stop_at_the_boundary() {
        let grid = octilinear(3, 3, 1).build().unwrap();
        assert_eq!(
            steps(&grid, grid.node_at(0, 0, 0).unwrap()),
            [(1, 0, 0, 1.0), (0, 1, 0, 1.0), (1, 1, 0, SQRT_2)]
        );
        assert_eq!(
            steps(&grid, grid.node_at(2, 2, 0).unwrap()),
            [(1, 1, 0, SQRT_2), (2, 1, 0, 1.0), (1, 2, 0, 1.0)]
        );
        // A row must not wrap around: from the end of one, +1 is the start of the next.
        assert_eq!(
            steps(&grid, grid.node_at(2, 1, 0).unwrap()),
            [
                (1, 0, 0, SQRT_2),
                (2, 0, 0, 1.0),
                (1, 1, 0, 1.0),
                (1, 2, 0, SQRT_2),
                (2, 2, 0, 1.0),
            ]
        );
    }

    #[test]
    fn single_cell_has_no_neighbours() {
        let grid = octilinear(1, 1, 1).build().unwrap();
        assert!(steps(&grid, NodeId(0)).is_empty());
    }

    #[test]
    fn layer_changes_reach_adjacent_layers_only() {
        let grid = Grid::builder(1, 1, 4).via_cost(7.0).build().unwrap();
        assert_eq!(steps(&grid, NodeId(0)), [(0, 0, 1, 7.0)]);
        assert_eq!(steps(&grid, NodeId(1)), [(0, 0, 0, 7.0), (0, 0, 2, 7.0)]);
        assert_eq!(steps(&grid, NodeId(3)), [(0, 0, 2, 7.0)]);
    }

    #[test]
    fn steps_never_enter_a_blocked_cell() {
        let mut grid = octilinear(3, 3, 2).build().unwrap();
        grid.set_blocked(1, 0, 0, true).unwrap();
        grid.set_blocked(2, 2, 0, true).unwrap();
        grid.set_blocked(1, 1, 1, true).unwrap();
        let reached = steps(&grid, grid.node_at(1, 1, 0).unwrap());
        assert_eq!(
            reached,
            [
                (0, 0, 0, SQRT_2),
                (2, 0, 0, SQRT_2),
                (0, 1, 0, 1.0),
                (2, 1, 0, 1.0),
                (0, 2, 0, SQRT_2),
                (1, 2, 0, 1.0),
            ]
        );
    }

    #[test]
    fn a_blocked_start_has_no_steps_until_cleared() {
        let mut grid = Grid::new(2, 1, 1).unwrap();
        grid.set_blocked(0, 0, 0, true).unwrap();
        assert!(grid.is_blocked(NodeId(0)));
        assert!(!grid.is_blocked(NodeId(1)));
        assert!(steps(&grid, NodeId(0)).is_empty());
        assert!(steps(&grid, NodeId(1)).is_empty());

        grid.set_blocked(0, 0, 0, false).unwrap();
        assert!(!grid.is_blocked(NodeId(0)));
        assert_eq!(steps(&grid, NodeId(0)), [(1, 0, 0, 1.0)]);
    }

    #[test]
    fn blocking_uses_the_right_bit_across_word_boundaries() {
        let mut grid = Grid::new(130, 1, 1).unwrap();
        for x in [63, 64, 128] {
            grid.set_blocked(x, 0, 0, true).unwrap();
        }
        for x in 0..130 {
            assert_eq!(grid.is_blocked(NodeId(x)), matches!(x, 63 | 64 | 128));
        }
    }

    #[test]
    fn expansion_appends_without_clearing_and_repeats_exactly() {
        let mut grid = octilinear(4, 4, 3).build().unwrap();
        grid.set_blocked(2, 2, 1, true).unwrap();
        let from = grid.node_at(1, 1, 1).unwrap();
        let sentinel = Step {
            to: NodeId(999),
            cost: Cost::MAX,
        };

        let mut first = vec![sentinel];
        grid.expand(NET, from, &mut first);
        assert_eq!(first[0], sentinel);
        assert_eq!(first.len(), 10);
        assert!(first[1..].windows(2).all(|pair| pair[0].to < pair[1].to));

        let mut second = vec![sentinel];
        grid.clone().expand(NetId(7), from, &mut second);
        assert_eq!(first, second);
    }

    #[test]
    fn resources_are_the_arrival_cell_appended_to_out() {
        let grid = Grid::new(2, 1, 1).unwrap();
        let mut out = vec![NodeId(9)];
        grid.resources(NET, NodeId(0), NodeId(1), &mut out);
        assert_eq!(out, [NodeId(9), NodeId(1)]);
        grid.resources(NET, NodeId(0), NodeId(2), &mut out);
        assert_eq!(out, [NodeId(9), NodeId(1)]);
    }

    #[test]
    fn capacity_defaults_to_one_and_reads_zero_when_blocked() {
        let mut grid = Grid::new(2, 2, 1).unwrap();
        assert_eq!(grid.capacity(NodeId(0)), 1);
        assert_eq!(grid.capacity(NodeId(4)), 0);

        grid.set_capacity(1, 0, 0, 1).unwrap();
        assert!(grid.capacities.is_empty());

        grid.set_capacity(1, 0, 0, 3).unwrap();
        assert_eq!(grid.capacity(NodeId(1)), 3);
        assert_eq!(grid.capacity(NodeId(0)), 1);
        assert_eq!(grid.capacity(NodeId(4)), 0);

        grid.set_blocked(1, 0, 0, true).unwrap();
        assert_eq!(grid.capacity(NodeId(1)), 0);
        grid.set_blocked(1, 0, 0, false).unwrap();
        assert_eq!(grid.capacity(NodeId(1)), 3);

        // Unlike a block, a zero capacity leaves the cell reachable.
        grid.set_capacity(1, 0, 0, 0).unwrap();
        assert_eq!(grid.capacity(NodeId(1)), 0);
        assert_eq!(steps(&grid, NodeId(0)), [(1, 0, 0, 1.0), (0, 1, 0, 1.0)]);
    }

    #[test]
    fn foreign_nodes_are_inert() {
        let grid = Grid::new(2, 2, 1).unwrap();
        assert!(grid.is_blocked(NodeId(4)));
        assert!(steps(&grid, NodeId(4)).is_empty());
        assert!(steps(&grid, NodeId(u32::MAX)).is_empty());
        assert_eq!(grid.lower_bound(NET, NodeId(4), NodeId(0)), Cost::ZERO);
        assert_eq!(grid.lower_bound(NET, NodeId(0), NodeId(4)), Cost::ZERO);
    }

    #[test]
    fn uses_dense_slots() {
        let grid = Grid::new(2, 2, 1).unwrap();
        assert_eq!(grid.new_slots().slot(NodeId(3)), 3);
    }

    #[test]
    fn debug_leaves_out_the_tables() {
        let grid = Grid::new(2, 2, 1).unwrap();
        let text = format!("{grid:?}");
        assert!(text.contains("width: 2"));
        assert!(!text.contains("blocked"));
    }

    #[test]
    fn manhattan_bound_is_exact_for_whole_number_costs() {
        let grid = Grid::builder(10, 10, 4)
            .orthogonal_cost(3.0)
            .via_cost(5.0)
            .build()
            .unwrap();
        let from = grid.node_at(1, 2, 0).unwrap();
        let to = grid.node_at(8, 0, 3).unwrap();
        assert_eq!(grid.lower_bound(NET, from, to).get(), 9.0 * 3.0 + 3.0 * 5.0);
        assert_eq!(grid.lower_bound(NET, to, from).get(), 9.0 * 3.0 + 3.0 * 5.0);
        assert_eq!(grid.lower_bound(NET, from, from), Cost::ZERO);
    }

    #[test]
    fn octile_bound_picks_the_cheapest_mix_of_moves() {
        let bound = |orthogonal, diagonal| {
            let grid = octilinear(10, 10, 1)
                .orthogonal_cost(orthogonal)
                .diagonal_cost(diagonal)
                .build()
                .unwrap();
            let from = grid.node_at(0, 0, 0).unwrap();
            let to = grid.node_at(6, 2, 0).unwrap();
            grid.lower_bound(NET, from, to).get()
        };
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-9;

        // Ordinary octile distance: 2 diagonals and 4 orthogonal steps.
        assert!(close(bound(1.0, SQRT_2), 2.0 * SQRT_2 + 4.0));
        // A diagonal dearer than two orthogonal steps is never worth it.
        assert!(close(bound(1.0, 5.0), 8.0));
        // Diagonals cheaper than orthogonal steps can zigzag along the axis.
        assert!(close(bound(1.0, 0.25), 6.0 * 0.25));
        // Free moves of either kind.
        assert_eq!(bound(0.0, 1.0), 0.0);
        assert_eq!(bound(1.0, 0.0), 0.0);
    }

    #[test]
    fn zigzag_really_is_cheaper_than_the_octile_formula() {
        // The case that makes the textbook formula overestimate: 2 along X with no Y
        // distance costs 0.5 by way of two diagonals, not 2.0.
        let grid = octilinear(3, 2, 1).diagonal_cost(0.25).build().unwrap();
        let bound = grid.lower_bound(NET, NodeId(0), NodeId(2)).get();
        assert!(bound <= 0.5);
        let distances = all_pairs(&grid);
        assert_eq!(distances[0][2], Some(Cost::new(0.5).unwrap()));
    }

    #[test]
    fn bound_survives_the_rounding_of_long_sums() {
        // Ten additions of 0.1 come to less than 10.0 * 0.1, and similar gaps open up
        // for other costs and lengths, so compare with the sum at every distance.
        for cost in [0.1, 0.2, 0.3, 0.7, 1.1, SQRT_2, 1e-9, 1e9 + 0.1] {
            let grid = Grid::builder(2000, 1, 1)
                .orthogonal_cost(cost)
                .build()
                .unwrap();
            let mut sum = Cost::ZERO;
            for x in 1..2000 {
                sum = sum + grid.orthogonal_cost();
                let bound = grid.lower_bound(NET, NodeId(0), NodeId(x));
                assert!(bound <= sum, "{bound} > {sum} after {x} steps of {cost}");
            }
        }
    }

    #[test]
    fn bound_is_close_despite_the_rounding_margin() {
        let grid = octilinear(1000, 1000, 1).build().unwrap();
        let bound = grid.lower_bound(NET, NodeId(0), grid.node_at(999, 999, 0).unwrap());
        let exact = 999.0 * SQRT_2;
        assert!(bound.get() <= exact && bound.get() >= exact * (1.0 - 1e-9));
    }

    #[test]
    fn bound_saturates_like_path_sums() {
        let grid = Grid::builder(10, 1, 1)
            .orthogonal_cost(f64::MAX)
            .build()
            .unwrap();
        let bound = grid.lower_bound(NET, NodeId(0), NodeId(9));
        assert!(bound <= Cost::MAX);
        assert!(bound.get() > f64::MAX / 2.0);
    }

    /// Shortest distances between all pairs, by Bellman-Ford over `expand`.
    ///
    /// Distances are accumulated from the source through `Cost` addition, which is how a
    /// search adds them up, so they include the same rounding and saturation.
    fn all_pairs(grid: &Grid) -> Vec<Vec<Option<Cost>>> {
        let cells = grid.cell_count();
        let mut edges = Vec::new();
        let mut out = Vec::new();
        for from in 0..cells {
            out.clear();
            grid.expand(NET, NodeId(from), &mut out);
            edges.extend(
                out.iter()
                    .map(|step| (from as usize, step.to.index(), step.cost)),
            );
        }
        (0..cells as usize)
            .map(|source| {
                let mut distance = vec![None; cells as usize];
                distance[source] = Some(Cost::ZERO);
                loop {
                    let mut changed = false;
                    for &(from, to, cost) in &edges {
                        let Some(reached) = distance[from] else {
                            continue;
                        };
                        let candidate = reached + cost;
                        if distance[to].is_none_or(|known| candidate < known) {
                            distance[to] = Some(candidate);
                            changed = true;
                        }
                    }
                    if !changed {
                        return distance;
                    }
                }
            })
            .collect()
    }

    fn any_cost() -> impl Strategy<Value = f64> {
        prop_oneof![
            Just(0.0),
            Just(f64::from_bits(1)),
            Just(f64::MIN_POSITIVE),
            Just(1e-300),
            Just(1e-9),
            Just(0.1),
            Just(0.25),
            Just(1.0),
            Just(SQRT_2),
            Just(2.0),
            Just(3.0),
            Just(1e9),
            Just(1e300),
            Just(f64::MAX),
            0.0..10.0,
            1e-3..1e3,
        ]
    }

    fn any_connectivity() -> impl Strategy<Value = Connectivity> {
        prop_oneof![
            Just(Connectivity::Orthogonal),
            Just(Connectivity::Octilinear)
        ]
    }

    /// A small random grid, usually with some cells blocked.
    fn any_grid() -> impl Strategy<Value = Grid> {
        (
            (1u32..=4, 1u32..=4, 1u32..=3),
            any_connectivity(),
            (any_cost(), any_cost(), any_cost()),
            proptest::collection::vec(proptest::bool::weighted(0.2), 48),
            any::<bool>(),
        )
            .prop_map(
                |((width, height, layers), connectivity, costs, blocks, use_blocks)| {
                    let mut grid = Grid::builder(width, height, layers)
                        .connectivity(connectivity)
                        .orthogonal_cost(costs.0)
                        .diagonal_cost(costs.1)
                        .via_cost(costs.2)
                        .build()
                        .unwrap();
                    if use_blocks {
                        for node in 0..grid.cell_count() {
                            if blocks[node as usize] {
                                let (x, y, layer) = grid.position(NodeId(node)).unwrap();
                                grid.set_blocked(x, y, layer, true).unwrap();
                            }
                        }
                    }
                    grid
                },
            )
    }

    proptest! {
        #[test]
        fn lower_bound_never_exceeds_the_shortest_distance(grid in any_grid()) {
            let distances = all_pairs(&grid);
            for from in 0..grid.cell_count() {
                for to in 0..grid.cell_count() {
                    if let Some(distance) = distances[from as usize][to as usize] {
                        let bound = grid.lower_bound(NET, NodeId(from), NodeId(to));
                        prop_assert!(
                            bound <= distance,
                            "{from} to {to}: bound {bound} > distance {distance} on {grid:?}"
                        );
                    }
                }
            }
        }

        /// The bound is not promised to be consistent to the last bit, because rounding
        /// can open tiny gaps, but anything larger would be a flaw in the formula.
        #[test]
        fn lower_bound_is_consistent_up_to_rounding(grid in any_grid()) {
            let mut out = Vec::new();
            for from in 0..grid.cell_count() {
                out.clear();
                grid.expand(NET, NodeId(from), &mut out);
                for to in 0..grid.cell_count() {
                    let here = grid.lower_bound(NET, NodeId(from), NodeId(to)).get();
                    for step in &out {
                        let there = grid.lower_bound(NET, step.to, NodeId(to));
                        let limit = (step.cost + there).get() * (1.0 + 1e-12);
                        prop_assert!(here <= limit, "{here} > {limit} on {grid:?}");
                    }
                }
            }
        }

        /// Checks expansion against a slow, independent statement of the rules.
        #[test]
        fn expansion_matches_a_brute_force_scan(grid in any_grid()) {
            let mut out = Vec::new();
            for from in 0..grid.cell_count() {
                let (fx, fy, fl) = grid.position(NodeId(from)).unwrap();
                let mut expected = Vec::new();
                if !grid.is_blocked(NodeId(from)) {
                    for to in 0..grid.cell_count() {
                        let (tx, ty, tl) = grid.position(NodeId(to)).unwrap();
                        let moved = (fx.abs_diff(tx), fy.abs_diff(ty), fl.abs_diff(tl));
                        let cost = match moved {
                            (1, 0, 0) | (0, 1, 0) => grid.orthogonal_cost(),
                            (1, 1, 0) if grid.connectivity() == Connectivity::Octilinear => {
                                grid.diagonal_cost()
                            }
                            (0, 0, 1) => grid.via_cost(),
                            _ => continue,
                        };
                        if !grid.is_blocked(NodeId(to)) {
                            expected.push(Step { to: NodeId(to), cost });
                        }
                    }
                }
                out.clear();
                grid.expand(NET, NodeId(from), &mut out);
                prop_assert_eq!(&out, &expected);
            }
        }

        #[test]
        fn positions_round_trip_on_any_grid(
            width in 1u32..=300,
            height in 1u32..=300,
            layers in 1u32..=30,
            node in any::<u32>(),
        ) {
            let grid = Grid::new(width, height, layers).unwrap();
            let node = NodeId(node % grid.cell_count());
            let (x, y, layer) = grid.position(node).unwrap();
            prop_assert!(x < width && y < height && layer < layers);
            prop_assert_eq!(grid.node_at(x, y, layer), Some(node));
        }
    }
}
