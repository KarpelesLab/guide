//! Explicit graph routing spaces.
//!
//! [`Graph`] is an immutable directed graph in compressed sparse row form. This is the
//! shape an FPGA routing resource graph takes, which can reach hundreds of millions of
//! nodes, so the storage is laid out to keep the bytes per node and edge low.

use core::fmt;
use core::mem::size_of;
use std::collections::BTreeSet;

use crate::cost::Cost;
use crate::id::{NetId, NodeId};
use crate::space::{DenseSlots, RoutingSpace, Step};

/// Removes self-loops and merges parallel edges in sorted edge lists, in place.
fn merge_edges(offsets: &mut [u32], targets: &mut Vec<NodeId>, step_costs: &mut Vec<Cost>) {
    // Where each target was last written. Positions from before the current node's start
    // belong to other nodes, so this array never needs resetting.
    const UNSEEN: u32 = u32::MAX;
    let mut written_at = vec![UNSEEN; offsets.len() - 1];

    let mut write = 0u32;
    let mut read = 0usize;
    for node in 0..offsets.len() - 1 {
        let end = offsets[node + 1] as usize;
        let start = write;
        offsets[node] = start;
        while read < end {
            let (target, step_cost) = (targets[read], step_costs[read]);
            read += 1;
            if target.index() == node {
                continue;
            }
            let seen = written_at[target.index()];
            if seen != UNSEEN && seen >= start {
                let kept = &mut step_costs[seen as usize];
                *kept = (*kept).min(step_cost);
            } else {
                written_at[target.index()] = write;
                targets[write as usize] = target;
                step_costs[write as usize] = step_cost;
                write += 1;
            }
        }
    }
    *offsets
        .last_mut()
        .expect("offsets holds at least one entry") = write;
    targets.truncate(write as usize);
    step_costs.truncate(write as usize);
}

/// An estimate of the cost of getting between two nodes of a [`Graph`].
///
/// The estimate must never exceed the true cost, or searches lose their optimality
/// guarantee. That includes rounding: a search adds step costs one at a time, so a
/// computed distance with fractional costs needs some margin against the sum.
///
/// Closures implement this, and [`Graph`] is generic over it, so that an estimator can
/// be inlined into the search loop. Estimators are only called with nodes of the graph.
pub trait LowerBound {
    /// Returns a lower bound on the cost of getting from `from` to `to`.
    fn estimate(&self, from: NodeId, to: NodeId) -> Cost;
}

impl<F: Fn(NodeId, NodeId) -> Cost> LowerBound for F {
    #[inline]
    fn estimate(&self, from: NodeId, to: NodeId) -> Cost {
        self(from, to)
    }
}

/// The estimator that knows nothing: always [`Cost::ZERO`], which is always correct and
/// turns A\* into Dijkstra's algorithm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZeroBound;

impl LowerBound for ZeroBound {
    #[inline]
    fn estimate(&self, _from: NodeId, _to: NodeId) -> Cost {
        Cost::ZERO
    }
}

/// Why a [`GraphBuilder`] rejected a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GraphError {
    /// A node is not below the number of nodes the builder was created with.
    NodeOutOfRange {
        /// The rejected node.
        node: NodeId,
        /// The number of nodes in the graph.
        num_nodes: u32,
    },
    /// The graph would have more edges than a `u32` can number.
    TooManyEdges,
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::NodeOutOfRange { node, num_nodes } => {
                write!(f, "node {} is out of range for {num_nodes} nodes", node.0)
            }
            GraphError::TooManyEdges => f.write_str("graph has more edges than a u32 can number"),
        }
    }
}

impl std::error::Error for GraphError {}

/// The capacity of every node that has not been given another one.
const DEFAULT_CAPACITY: u32 = 1;

/// The most distinct values a [`Column`] can index with a `u16`.
const MAX_TABLE_LEN: usize = 1 << 16;

/// One value per node or edge, stored as compactly as the values allow.
///
/// Real routing graphs draw these from a short list: VPR's routing resource graphs, for
/// instance, have a handful of switch types and wire types shared by hundreds of millions
/// of elements. So a table lookup replaces an 8-byte cost with 2 bytes in the common case
/// and the full column stays available for graphs where every value differs.
#[derive(Clone)]
enum Column<T> {
    /// Every element has this value. Takes no space per element.
    Uniform(T),
    /// Each element holds an index into a table of distinct values, in ascending order.
    Table { table: Box<[T]>, index: Box<[u16]> },
    /// Each element holds its own value.
    Direct(Box<[T]>),
}

impl<T: Copy + Ord> Column<T> {
    /// Picks the smallest form for `values`, where no values at all means every element
    /// has the `default`.
    fn compact(values: Vec<T>, default: T) -> Column<T> {
        // A tree set keeps the table order independent of hashing, and bounds the memory
        // used here no matter how many values there are.
        let mut distinct = BTreeSet::new();
        for &value in &values {
            distinct.insert(value);
            if distinct.len() > MAX_TABLE_LEN {
                return Column::Direct(values.into_boxed_slice());
            }
        }
        let table: Box<[T]> = distinct.into_iter().collect();
        match *table {
            [] => Column::Uniform(default),
            [only] => Column::Uniform(only),
            _ => {
                let index = values
                    .iter()
                    // Below MAX_TABLE_LEN, so it fits, and the value is always present.
                    .map(|value| table.partition_point(|entry| entry < value) as u16)
                    .collect();
                Column::Table { table, index }
            }
        }
    }

    /// Reads the value of an element, whose index the caller has checked.
    #[inline]
    fn get(&self, element: usize) -> T {
        match self {
            Column::Uniform(value) => *value,
            Column::Table { table, index } => table[usize::from(index[element])],
            Column::Direct(values) => values[element],
        }
    }

    fn heap_bytes(&self) -> usize {
        match self {
            Column::Uniform(_) => 0,
            Column::Table { table, index } => {
                size_of::<T>() * table.len() + size_of::<u16>() * index.len()
            }
            Column::Direct(values) => size_of::<T>() * values.len(),
        }
    }
}

/// An edge waiting for [`GraphBuilder::build`] to sort it into place.
#[derive(Clone, Copy)]
struct PendingEdge {
    from: NodeId,
    to: NodeId,
    cost: Cost,
}

/// Collects the nodes and edges of a [`Graph`].
///
/// Each change is validated as it is made, so an error points at the call that caused it
/// and [`build`](GraphBuilder::build) itself cannot fail.
///
/// While building, each edge takes 16 bytes, and each node takes 8 for a base cost and 4
/// for a capacity once any node has been given one. [`build`](GraphBuilder::build) briefly
/// needs 8 more per node and 12 more per edge on top of those.
///
/// # Examples
///
/// ```
/// use guide::cost::Cost;
/// use guide::id::NodeId;
/// use guide::space::graph::{GraphBuilder, GraphError};
///
/// let mut builder = GraphBuilder::new(2);
/// builder.add_edge(NodeId(0), NodeId(1), Cost::ONE)?;
/// assert_eq!(
///     builder.add_edge(NodeId(0), NodeId(2), Cost::ONE),
///     Err(GraphError::NodeOutOfRange { node: NodeId(2), num_nodes: 2 }),
/// );
/// assert_eq!(builder.build().num_edges(), 1);
/// # Ok::<(), GraphError>(())
/// ```
#[derive(Clone)]
pub struct GraphBuilder {
    num_nodes: u32,
    edges: Vec<PendingEdge>,
    /// Empty until a node gets a cost other than zero, as with `capacities`, so a graph
    /// that doesn't use a per-node value doesn't allocate for it.
    base_costs: Vec<Cost>,
    capacities: Vec<u32>,
    /// `u32::MAX` outside of tests, which can't afford to reach that many edges.
    max_edges: usize,
}

impl GraphBuilder {
    /// Starts a graph with nodes `NodeId(0)` up to but excluding `NodeId(num_nodes)`, with
    /// no edges, base costs of zero and capacities of 1.
    pub fn new(num_nodes: u32) -> GraphBuilder {
        GraphBuilder {
            num_nodes,
            edges: Vec::new(),
            base_costs: Vec::new(),
            capacities: Vec::new(),
            max_edges: u32::MAX as usize,
        }
    }

    /// Like [`new`](GraphBuilder::new), reserving room for `edges` edges up front.
    ///
    /// On a large graph this avoids the extra peak memory of growing the edge list.
    pub fn with_edge_capacity(num_nodes: u32, edges: usize) -> GraphBuilder {
        let mut builder = GraphBuilder::new(num_nodes);
        builder.edges.reserve_exact(edges.min(builder.max_edges));
        builder
    }

    /// Adds a directed edge.
    ///
    /// Edges out of the same node are kept in the order they were added, which is the
    /// order [`expand`](RoutingSpace::expand) produces them in.
    ///
    /// [`build`](GraphBuilder::build) drops self-loops, and merges parallel edges into the
    /// cheapest one at the position of the first, as [`RoutingSpace::expand`] requires.
    /// Only a domain can tell parallel edges apart, so one that needs to know which was
    /// chosen has to keep that record itself.
    ///
    /// # Errors
    ///
    /// Returns an error if either node is out of range, or if the graph already has as
    /// many edges as a `u32` can number. The builder is unchanged in that case.
    pub fn add_edge(&mut self, from: NodeId, to: NodeId, cost: Cost) -> Result<(), GraphError> {
        self.check_node(from)?;
        self.check_node(to)?;
        if self.edges.len() >= self.max_edges {
            return Err(GraphError::TooManyEdges);
        }
        self.edges.push(PendingEdge { from, to, cost });
        Ok(())
    }

    /// Sets the cost of entering `node`, which defaults to zero.
    ///
    /// # Errors
    ///
    /// Returns an error if the node is out of range.
    pub fn set_base_cost(&mut self, node: NodeId, cost: Cost) -> Result<(), GraphError> {
        self.check_node(node)?;
        set_in_lazy_column(
            &mut self.base_costs,
            self.num_nodes,
            Cost::ZERO,
            node.index(),
            cost,
        );
        Ok(())
    }

    /// Sets how many nets can share `node` before it counts as overused, which defaults
    /// to 1.
    ///
    /// A capacity of 0 leaves the node routable, with any use of it counting as overuse.
    ///
    /// # Errors
    ///
    /// Returns an error if the node is out of range.
    pub fn set_capacity(&mut self, node: NodeId, capacity: u32) -> Result<(), GraphError> {
        self.check_node(node)?;
        set_in_lazy_column(
            &mut self.capacities,
            self.num_nodes,
            DEFAULT_CAPACITY,
            node.index(),
            capacity,
        );
        Ok(())
    }

    /// Creates the graph, with [`ZeroBound`] as its estimator.
    ///
    /// Use [`Graph::with_lower_bound`] on the result to supply a better one.
    pub fn build(self) -> Graph {
        let GraphBuilder {
            num_nodes,
            edges,
            base_costs,
            capacities,
            max_edges: _,
        } = self;
        let nodes = num_nodes as usize;

        // A stable counting sort, so that edges keep their order within a node.
        // Edge counts fit u32 because add_edge enforces it.
        let mut offsets = vec![0u32; nodes + 1];
        for edge in &edges {
            offsets[edge.from.index() + 1] += 1;
        }
        for node in 0..nodes {
            offsets[node + 1] += offsets[node];
        }
        let mut next = offsets.clone();
        let mut targets = vec![NodeId(0); edges.len()];
        let mut step_costs = vec![Cost::ZERO; edges.len()];
        for edge in edges {
            let slot = &mut next[edge.from.index()];
            let position = *slot as usize;
            *slot += 1;
            targets[position] = edge.to;
            let base = base_costs
                .get(edge.to.index())
                .copied()
                .unwrap_or(Cost::ZERO);
            step_costs[position] = edge.cost + base;
        }
        drop(next);
        merge_edges(&mut offsets, &mut targets, &mut step_costs);

        Graph {
            num_nodes,
            offsets: offsets.into_boxed_slice(),
            targets: targets.into_boxed_slice(),
            step_costs: Column::compact(step_costs, Cost::ZERO),
            base_costs: Column::compact(base_costs, Cost::ZERO),
            capacities: Column::compact(capacities, DEFAULT_CAPACITY),
            bound: ZeroBound,
        }
    }

    fn check_node(&self, node: NodeId) -> Result<(), GraphError> {
        if node.0 < self.num_nodes {
            Ok(())
        } else {
            Err(GraphError::NodeOutOfRange {
                node,
                num_nodes: self.num_nodes,
            })
        }
    }
}

impl fmt::Debug for GraphBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GraphBuilder")
            .field("num_nodes", &self.num_nodes)
            .field("num_edges", &self.edges.len())
            .finish_non_exhaustive()
    }
}

/// Writes to a per-node column that stays empty for as long as every value is the default.
fn set_in_lazy_column<T: Copy + PartialEq>(
    column: &mut Vec<T>,
    num_nodes: u32,
    default: T,
    node: usize,
    value: T,
) {
    if column.is_empty() {
        if value == default {
            return;
        }
        column.resize(num_nodes as usize, default);
    }
    column[node] = value;
}

/// An immutable directed graph, where each node is a resource such as a wire.
///
/// Created through a [`GraphBuilder`].
///
/// # Costs
///
/// A step costs the edge's cost plus the base cost of the node it arrives at. A path
/// therefore pays for every node it enters and every edge it takes, like VPR's wire and
/// switch costs, and not for the node it starts from, which every path to or from there
/// has to use anyway. Sums saturate at [`Cost::MAX`].
///
/// Since the graph is immutable, step costs are worked out when it is built. Expansion
/// then doesn't have to read anything from the arrival nodes, each of which would
/// otherwise be a likely cache miss.
///
/// # Order
///
/// [`expand`](RoutingSpace::expand) produces edges in the order they were added to the
/// builder, among those out of the same node.
///
/// # Resources
///
/// Stepping to a node consumes that node, and nothing else. Parallel edges between two
/// nodes share the node they arrive at, and don't compete in any other way.
///
/// # Lower bound
///
/// The bound comes from the estimator `B`, which by default is [`ZeroBound`]. See
/// [`with_lower_bound`](Graph::with_lower_bound).
///
/// # Nets
///
/// There are no per-net rules yet, so every method ignores its `net` argument.
///
/// # Foreign nodes
///
/// A [`NodeId`] outside the graph has no steps, consumes no resources and has a capacity
/// of 0. Bounds involving one are [`Cost::ZERO`], and don't reach the estimator.
///
/// # Memory
///
/// Besides a fixed-size header and small tables, a graph takes these per element:
///
/// | Column | Per | All equal | At most 65536 distinct values | More distinct values |
/// | --- | --- | --- | --- | --- |
/// | Edge list offset | node | 4 | 4 | 4 |
/// | Base cost | node | 0 | 2 | 8 |
/// | Capacity | node | 0 | 2 | 4 |
/// | Target | edge | 4 | 4 | 4 |
/// | Step cost | edge | 0 | 2 | 8 |
///
/// So a graph takes 4 to 16 bytes per node and 4 to 12 per edge, and a typical routing
/// resource graph takes 4 to 8 per node and 6 per edge. The distinct step costs are the
/// distinct sums of an edge cost and a base cost. [`heap_bytes`](Graph::heap_bytes)
/// reports the actual total.
///
/// # Examples
///
/// ```
/// use guide::cost::Cost;
/// use guide::id::{NetId, NodeId};
/// use guide::space::graph::GraphBuilder;
/// use guide::space::{RoutingSpace, Step};
///
/// let mut builder = GraphBuilder::new(3);
/// builder.add_edge(NodeId(0), NodeId(2), Cost::from(4))?;
/// builder.add_edge(NodeId(0), NodeId(1), Cost::from(1))?;
/// builder.set_base_cost(NodeId(1), Cost::from(10))?;
/// builder.set_capacity(NodeId(2), 2)?;
/// let graph = builder.build();
///
/// let mut steps = Vec::new();
/// graph.expand(NetId(0), NodeId(0), &mut steps);
/// assert_eq!(
///     steps,
///     [
///         Step { to: NodeId(2), cost: Cost::from(4) },
///         Step { to: NodeId(1), cost: Cost::from(11) },
///     ],
/// );
/// assert_eq!(graph.capacity(NodeId(2)), 2);
/// assert_eq!(graph.lower_bound(NetId(0), NodeId(0), NodeId(2)), Cost::ZERO);
///
/// // Each node here sits at a position on a line, and each unit of distance costs
/// // at least 1.
/// let graph = graph.with_lower_bound(|from: NodeId, to: NodeId| Cost::from(from.0.abs_diff(to.0)));
/// assert_eq!(graph.lower_bound(NetId(0), NodeId(0), NodeId(2)), Cost::from(2));
/// # Ok::<(), guide::space::graph::GraphError>(())
/// ```
#[derive(Clone)]
pub struct Graph<B = ZeroBound> {
    num_nodes: u32,
    /// The edges of node `n` are `offsets[n]..offsets[n + 1]`.
    offsets: Box<[u32]>,
    targets: Box<[NodeId]>,
    /// The cost of each edge plus the base cost of its target.
    step_costs: Column<Cost>,
    base_costs: Column<Cost>,
    capacities: Column<u32>,
    bound: B,
}

impl<B> Graph<B> {
    /// Replaces the estimator, keeping the graph.
    ///
    /// The estimator must be admissible for this graph, meaning that it never exceeds the
    /// cheapest total of step costs between its arguments. The graph can't verify this,
    /// and a wrong estimator costs routes their optimality, not their validity.
    ///
    /// A closure needs its parameter types spelled out, as in the [`Graph`] example.
    #[must_use]
    pub fn with_lower_bound<E: LowerBound>(self, bound: E) -> Graph<E> {
        Graph {
            num_nodes: self.num_nodes,
            offsets: self.offsets,
            targets: self.targets,
            step_costs: self.step_costs,
            base_costs: self.base_costs,
            capacities: self.capacities,
            bound,
        }
    }

    /// Returns the number of nodes. Valid node IDs are below this.
    #[inline]
    pub const fn num_nodes(&self) -> u32 {
        self.num_nodes
    }

    /// Returns the number of edges.
    #[inline]
    pub fn num_edges(&self) -> u32 {
        self.offsets[self.num_nodes as usize]
    }

    /// Returns how many edges leave `node`, which is 0 for a node outside the graph.
    #[inline]
    pub fn out_degree(&self, node: NodeId) -> u32 {
        if self.contains(node) {
            self.offsets[node.index() + 1] - self.offsets[node.index()]
        } else {
            0
        }
    }

    /// Returns the cost of entering `node`, or `None` if it is not in the graph.
    #[inline]
    pub fn base_cost(&self, node: NodeId) -> Option<Cost> {
        self.contains(node)
            .then(|| self.base_costs.get(node.index()))
    }

    /// Returns the bytes this graph holds on the heap, not counting the estimator's.
    pub fn heap_bytes(&self) -> usize {
        size_of::<u32>() * self.offsets.len()
            + size_of::<NodeId>() * self.targets.len()
            + self.step_costs.heap_bytes()
            + self.base_costs.heap_bytes()
            + self.capacities.heap_bytes()
    }

    #[inline]
    const fn contains(&self, node: NodeId) -> bool {
        node.0 < self.num_nodes
    }
}

impl<B> fmt::Debug for Graph<B> {
    /// Leaves out the tables, which can hold billions of entries.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Graph")
            .field("num_nodes", &self.num_nodes)
            .field("num_edges", &self.num_edges())
            .finish_non_exhaustive()
    }
}

impl<B: LowerBound> RoutingSpace for Graph<B> {
    type Node = NodeId;
    type Resource = NodeId;
    type Slots = DenseSlots<NodeId>;

    #[inline]
    fn new_slots(&self) -> DenseSlots<NodeId> {
        DenseSlots::new()
    }

    #[inline]
    fn expand(&self, _net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
        if !self.contains(from) {
            return;
        }
        let start = self.offsets[from.index()] as usize;
        let end = self.offsets[from.index() + 1] as usize;
        let targets = &self.targets[start..end];

        // Choosing the form once, outside the loop, keeps the loop itself free of a branch
        // per edge.
        match &self.step_costs {
            Column::Uniform(cost) => {
                out.extend(targets.iter().map(|&to| Step { to, cost: *cost }));
            }
            Column::Table { table, index } => {
                out.extend(
                    targets
                        .iter()
                        .zip(&index[start..end])
                        .map(|(&to, &entry)| Step {
                            to,
                            cost: table[usize::from(entry)],
                        }),
                );
            }
            Column::Direct(costs) => {
                out.extend(
                    targets
                        .iter()
                        .zip(&costs[start..end])
                        .map(|(&to, &cost)| Step { to, cost }),
                );
            }
        }
    }

    #[inline]
    fn lower_bound(&self, _net: NetId, from: NodeId, to: NodeId) -> Cost {
        if self.contains(from) && self.contains(to) {
            self.bound.estimate(from, to)
        } else {
            Cost::ZERO
        }
    }

    #[inline]
    fn resources(&self, _net: NetId, _from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
        if self.contains(to) {
            out.push(to);
        }
    }

    #[inline]
    fn capacity(&self, resource: NodeId) -> u32 {
        if self.contains(resource) {
            self.capacities.get(resource.index())
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::NodeSlots;
    use proptest::prelude::*;

    const NET: NetId = NetId(0);

    fn cost(value: f64) -> Cost {
        Cost::new(value).unwrap()
    }

    fn steps<B: LowerBound>(graph: &Graph<B>, from: u32) -> Vec<(u32, f64)> {
        let mut out = Vec::new();
        graph.expand(NET, NodeId(from), &mut out);
        out.iter()
            .map(|step| (step.to.0, step.cost.get()))
            .collect()
    }

    fn sample() -> Graph {
        let mut builder = GraphBuilder::new(4);
        builder.add_edge(NodeId(2), NodeId(3), cost(1.5)).unwrap();
        builder.add_edge(NodeId(0), NodeId(3), cost(2.0)).unwrap();
        builder.add_edge(NodeId(0), NodeId(1), cost(1.0)).unwrap();
        builder.add_edge(NodeId(2), NodeId(0), cost(0.5)).unwrap();
        builder.add_edge(NodeId(0), NodeId(2), cost(4.0)).unwrap();
        builder.set_base_cost(NodeId(3), cost(10.0)).unwrap();
        builder.set_base_cost(NodeId(0), cost(0.25)).unwrap();
        builder.set_capacity(NodeId(1), 5).unwrap();
        builder.build()
    }

    #[test]
    fn step_cost_is_edge_cost_plus_arrival_base_cost_in_insertion_order() {
        let graph = sample();
        assert_eq!(graph.num_nodes(), 4);
        assert_eq!(graph.num_edges(), 5);
        assert_eq!(steps(&graph, 0), [(3, 12.0), (1, 1.0), (2, 4.0)]);
        assert_eq!(steps(&graph, 1), []);
        assert_eq!(steps(&graph, 2), [(3, 11.5), (0, 0.75)]);
        assert_eq!(steps(&graph, 3), []);
        assert_eq!(graph.out_degree(NodeId(0)), 3);
        assert_eq!(graph.out_degree(NodeId(1)), 0);
        assert_eq!(graph.out_degree(NodeId(4)), 0);
    }

    #[test]
    fn per_node_values_have_defaults() {
        let graph = sample();
        assert_eq!(graph.base_cost(NodeId(3)), Some(cost(10.0)));
        assert_eq!(graph.base_cost(NodeId(1)), Some(Cost::ZERO));
        assert_eq!(graph.base_cost(NodeId(4)), None);
        assert_eq!(graph.capacity(NodeId(1)), 5);
        assert_eq!(graph.capacity(NodeId(0)), 1);
    }

    #[test]
    fn untouched_graph_has_all_defaults() {
        let graph = GraphBuilder::new(2).build();
        assert_eq!(graph.base_cost(NodeId(1)), Some(Cost::ZERO));
        assert_eq!(graph.capacity(NodeId(1)), 1);
        assert_eq!(graph.num_edges(), 0);
    }

    #[test]
    fn later_settings_replace_earlier_ones() {
        let mut builder = GraphBuilder::new(2);
        builder.set_capacity(NodeId(0), 9).unwrap();
        builder.set_capacity(NodeId(0), 1).unwrap();
        builder.set_base_cost(NodeId(0), cost(9.0)).unwrap();
        builder.set_base_cost(NodeId(0), Cost::ZERO).unwrap();
        let graph = builder.build();
        assert_eq!(graph.capacity(NodeId(0)), 1);
        assert_eq!(graph.base_cost(NodeId(0)), Some(Cost::ZERO));
    }

    #[test]
    fn empty_graph_is_fine() {
        let graph = GraphBuilder::new(0).build();
        assert_eq!(graph.num_nodes(), 0);
        assert_eq!(graph.num_edges(), 0);
        assert_eq!(steps(&graph, 0), []);
        assert_eq!(graph.capacity(NodeId(0)), 0);
    }

    #[test]
    fn builder_rejects_nodes_out_of_range_and_stays_usable() {
        let mut builder = GraphBuilder::new(2);
        let error = GraphError::NodeOutOfRange {
            node: NodeId(2),
            num_nodes: 2,
        };
        assert_eq!(
            builder.add_edge(NodeId(2), NodeId(0), Cost::ONE),
            Err(error)
        );
        assert_eq!(
            builder.add_edge(NodeId(0), NodeId(2), Cost::ONE),
            Err(error)
        );
        assert_eq!(builder.set_base_cost(NodeId(2), Cost::ONE), Err(error));
        assert_eq!(builder.set_capacity(NodeId(2), 2), Err(error));
        assert_eq!(error.to_string(), "node 2 is out of range for 2 nodes");

        builder.add_edge(NodeId(0), NodeId(1), Cost::ONE).unwrap();
        assert_eq!(builder.build().num_edges(), 1);
    }

    #[test]
    fn builder_rejects_edges_past_the_limit() {
        let mut builder = GraphBuilder::new(1);
        builder.max_edges = 1;
        builder.add_edge(NodeId(0), NodeId(0), Cost::ONE).unwrap();
        assert_eq!(
            builder.add_edge(NodeId(0), NodeId(0), Cost::ONE),
            Err(GraphError::TooManyEdges)
        );
        assert_eq!(GraphBuilder::new(1).max_edges, u32::MAX as usize);
    }

    #[test]
    fn edge_capacity_is_only_a_reservation() {
        let mut builder = GraphBuilder::with_edge_capacity(2, 8);
        assert!(builder.edges.capacity() >= 8);
        assert_eq!(builder.edges.len(), 0);
        builder.add_edge(NodeId(0), NodeId(1), Cost::ONE).unwrap();
        assert_eq!(steps(&builder.build(), 0), [(1, 1.0)]);
    }

    #[test]
    fn self_loops_are_dropped_and_parallel_edges_merged_into_the_cheapest() {
        let mut builder = GraphBuilder::new(4);
        builder.add_edge(NodeId(0), NodeId(0), cost(3.0)).unwrap();
        builder.add_edge(NodeId(0), NodeId(1), cost(2.0)).unwrap();
        builder.add_edge(NodeId(0), NodeId(2), cost(5.0)).unwrap();
        builder.add_edge(NodeId(0), NodeId(1), cost(1.0)).unwrap();
        // The same target from another node is not a parallel edge.
        builder.add_edge(NodeId(3), NodeId(1), cost(9.0)).unwrap();
        builder.add_edge(NodeId(3), NodeId(2), cost(4.0)).unwrap();
        let graph = builder.build();
        assert_eq!(steps(&graph, 0), [(1, 1.0), (2, 5.0)]);
        assert_eq!(steps(&graph, 1), []);
        assert_eq!(steps(&graph, 3), [(1, 9.0), (2, 4.0)]);
        assert_eq!(graph.num_edges(), 4);
    }

    #[test]
    fn step_costs_saturate() {
        let mut builder = GraphBuilder::new(2);
        builder.add_edge(NodeId(0), NodeId(1), Cost::MAX).unwrap();
        builder.set_base_cost(NodeId(1), Cost::MAX).unwrap();
        assert_eq!(steps(&builder.build(), 0), [(1, f64::MAX)]);
    }

    #[test]
    fn expansion_appends_without_clearing_and_repeats_exactly() {
        let graph = sample();
        let sentinel = Step {
            to: NodeId(99),
            cost: Cost::MAX,
        };
        let mut first = vec![sentinel];
        graph.expand(NET, NodeId(0), &mut first);
        assert_eq!(first.len(), 4);
        assert_eq!(first[0], sentinel);

        let mut second = vec![sentinel];
        graph.clone().expand(NetId(5), NodeId(0), &mut second);
        assert_eq!(first, second);
    }

    #[test]
    fn resources_are_the_arrival_node_appended_to_out() {
        let graph = sample();
        let mut out = vec![NodeId(99)];
        graph.resources(NET, NodeId(0), NodeId(1), &mut out);
        assert_eq!(out, [NodeId(99), NodeId(1)]);
        graph.resources(NET, NodeId(0), NodeId(4), &mut out);
        assert_eq!(out, [NodeId(99), NodeId(1)]);
    }

    #[test]
    fn foreign_nodes_are_inert() {
        let graph = sample().with_lower_bound(|from: NodeId, to: NodeId| -> Cost {
            assert!(from.0 < 4 && to.0 < 4, "estimator saw a foreign node");
            Cost::ONE
        });
        assert_eq!(steps(&graph, 4), []);
        assert_eq!(steps(&graph, u32::MAX), []);
        assert_eq!(graph.capacity(NodeId(u32::MAX)), 0);
        assert_eq!(graph.lower_bound(NET, NodeId(4), NodeId(0)), Cost::ZERO);
        assert_eq!(graph.lower_bound(NET, NodeId(0), NodeId(4)), Cost::ZERO);
        assert_eq!(graph.lower_bound(NET, NodeId(0), NodeId(1)), Cost::ONE);
    }

    #[test]
    fn lower_bound_defaults_to_zero() {
        let graph = sample();
        assert_eq!(graph.lower_bound(NET, NodeId(0), NodeId(3)), Cost::ZERO);
    }

    #[test]
    fn named_estimators_are_inlinable_types() {
        struct Fixed;
        impl LowerBound for Fixed {
            fn estimate(&self, _from: NodeId, _to: NodeId) -> Cost {
                Cost::from(2)
            }
        }
        let graph: Graph<Fixed> = sample().with_lower_bound(Fixed);
        assert_eq!(graph.lower_bound(NET, NodeId(0), NodeId(3)), Cost::from(2));
        // The estimator doesn't change anything else.
        assert_eq!(steps(&graph, 0), [(3, 12.0), (1, 1.0), (2, 4.0)]);
    }

    #[test]
    fn uses_dense_slots() {
        assert_eq!(sample().new_slots().slot(NodeId(3)), 3);
    }

    #[test]
    fn debug_leaves_out_the_tables() {
        assert_eq!(
            format!("{:?}", sample()),
            "Graph { num_nodes: 4, num_edges: 5, .. }"
        );
        let text = format!("{:?}", GraphBuilder::new(3));
        assert_eq!(text, "GraphBuilder { num_nodes: 3, num_edges: 0, .. }");
    }

    #[test]
    fn errors_have_messages() {
        assert!(GraphError::TooManyEdges.to_string().contains("u32"));
    }

    #[test]
    fn uniform_graph_takes_four_bytes_per_node_and_edge() {
        let mut builder = GraphBuilder::new(100);
        for node in 0..100 {
            builder
                .add_edge(NodeId(node), NodeId((node + 1) % 100), Cost::ONE)
                .unwrap();
            builder
                .add_edge(NodeId(node), NodeId((node + 2) % 100), Cost::ONE)
                .unwrap();
        }
        let graph = builder.build();
        assert!(matches!(graph.step_costs, Column::Uniform(_)));
        assert!(matches!(graph.base_costs, Column::Uniform(_)));
        assert!(matches!(graph.capacities, Column::Uniform(_)));
        assert_eq!(graph.heap_bytes(), 4 * 101 + 4 * 200);
    }

    #[test]
    fn few_distinct_values_use_a_table() {
        let mut builder = GraphBuilder::new(100);
        for node in 0..100 {
            builder
                .add_edge(NodeId(node), NodeId((node + 1) % 100), Cost::from(node % 3))
                .unwrap();
            builder.set_base_cost(NodeId(node), Cost::ONE).unwrap();
            builder.set_capacity(NodeId(node), node % 2).unwrap();
        }
        let graph = builder.build();
        assert!(matches!(graph.step_costs, Column::Table { .. }));
        assert!(matches!(graph.base_costs, Column::Uniform(_)));
        assert!(matches!(graph.capacities, Column::Table { .. }));
        assert_eq!(
            graph.heap_bytes(),
            4 * 101 + (4 + 2) * 100 + 3 * 8 + 2 * 100 + 2 * 4
        );

        assert_eq!(steps(&graph, 5), [(6, 3.0)]);
        assert_eq!(steps(&graph, 6), [(7, 1.0)]);
        assert_eq!(graph.capacity(NodeId(5)), 1);
        assert_eq!(graph.capacity(NodeId(6)), 0);
    }

    #[test]
    fn table_holds_exactly_as_many_values_as_its_index_can_address() {
        let distinct = |count: u32| {
            // Every edge leads to one extra node, so that none of them is a self-loop.
            let mut builder = GraphBuilder::new(count + 1);
            for node in 0..count {
                builder
                    .add_edge(NodeId(node), NodeId(count), Cost::from(node))
                    .unwrap();
            }
            builder.build()
        };

        let largest_table = distinct(1 << 16);
        let Column::Table { table, .. } = &largest_table.step_costs else {
            panic!("65536 distinct values should fit a table");
        };
        assert_eq!(table.len(), 1 << 16);
        assert_eq!(steps(&largest_table, 65535), [(65536, 65535.0)]);

        let direct = distinct((1 << 16) + 1);
        assert!(matches!(direct.step_costs, Column::Direct(_)));
        assert_eq!(steps(&direct, 65536), [(65537, 65536.0)]);
        assert_eq!(steps(&direct, 1), [(65537, 1.0)]);
    }

    #[test]
    fn direct_columns_are_read_correctly() {
        let count = (1u32 << 16) + 1;
        let mut builder = GraphBuilder::new(count);
        for node in 0..count {
            builder
                .set_base_cost(NodeId(node), Cost::from(node))
                .unwrap();
            builder.set_capacity(NodeId(node), node).unwrap();
        }
        let graph = builder.build();
        assert!(matches!(graph.base_costs, Column::Direct(_)));
        assert!(matches!(graph.capacities, Column::Direct(_)));
        assert_eq!(graph.base_cost(NodeId(65536)), Some(Cost::from(65536)));
        assert_eq!(graph.capacity(NodeId(65536)), 65536);
        assert_eq!(
            graph.heap_bytes(),
            4 * (count as usize + 1) + (8 + 4) * count as usize
        );
    }

    /// Shortest distances from `source` by Bellman-Ford, over edges as the caller stated
    /// them rather than as the graph stored them.
    ///
    /// Distances accumulate through `Cost` addition, as they do in a search.
    fn bellman_ford(nodes: u32, edges: &[(u32, u32, Cost)], source: u32) -> Vec<Option<Cost>> {
        let mut distance = vec![None; nodes as usize];
        distance[source as usize] = Some(Cost::ZERO);
        loop {
            let mut changed = false;
            for &(from, to, step) in edges {
                let Some(reached) = distance[from as usize] else {
                    continue;
                };
                let candidate = reached + step;
                if distance[to as usize].is_none_or(|known: Cost| candidate < known) {
                    distance[to as usize] = Some(candidate);
                    changed = true;
                }
            }
            if !changed {
                return distance;
            }
        }
    }

    fn any_cost() -> impl Strategy<Value = Cost> {
        prop_oneof![
            Just(0.0),
            Just(f64::from_bits(1)),
            Just(0.1),
            Just(1.0),
            Just(1e300),
            Just(f64::MAX),
            0.0..10.0,
        ]
        .prop_map(cost)
    }

    /// A small random graph: node count, edges as `(from, to, cost)`, and base costs.
    fn any_graph() -> impl Strategy<Value = (u32, Vec<(u32, u32, Cost)>, Vec<Cost>)> {
        (1u32..=12).prop_flat_map(|nodes| {
            (
                Just(nodes),
                proptest::collection::vec((0..nodes, 0..nodes, any_cost()), 0..40),
                proptest::collection::vec(any_cost(), nodes as usize),
            )
        })
    }

    fn build(nodes: u32, edges: &[(u32, u32, Cost)], bases: &[Cost]) -> Graph {
        let mut builder = GraphBuilder::new(nodes);
        for &(from, to, edge_cost) in edges {
            builder
                .add_edge(NodeId(from), NodeId(to), edge_cost)
                .unwrap();
        }
        for (node, &base) in bases.iter().enumerate() {
            builder.set_base_cost(NodeId(node as u32), base).unwrap();
        }
        builder.build()
    }

    proptest! {
        /// Checks the storage against the edges as they were stated, after merging.
        #[test]
        fn expansion_follows_insertion_order((nodes, edges, bases) in any_graph()) {
            let graph = build(nodes, &edges, &bases);
            let mut out = Vec::new();
            let mut num_edges = 0;
            for node in 0..nodes {
                let mut expected: Vec<Step<NodeId>> = Vec::new();
                for &(_, to, edge_cost) in edges.iter().filter(|edge| edge.0 == node && edge.1 != node) {
                    let step_cost = edge_cost + bases[to as usize];
                    match expected.iter_mut().find(|step| step.to == NodeId(to)) {
                        Some(step) => step.cost = step.cost.min(step_cost),
                        None => expected.push(Step { to: NodeId(to), cost: step_cost }),
                    }
                }
                num_edges += expected.len();
                out.clear();
                graph.expand(NET, NodeId(node), &mut out);
                prop_assert_eq!(&out, &expected);
                prop_assert_eq!(graph.out_degree(NodeId(node)) as usize, expected.len());
                prop_assert_eq!(graph.base_cost(NodeId(node)), Some(bases[node as usize]));
            }
            prop_assert_eq!(graph.num_edges() as usize, num_edges);
        }

        #[test]
        fn zero_bound_is_admissible((nodes, edges, bases) in any_graph()) {
            let graph = build(nodes, &edges, &bases);
            let stated: Vec<_> = edges
                .iter()
                .map(|&(from, to, edge_cost)| (from, to, edge_cost + bases[to as usize]))
                .collect();
            for from in 0..nodes {
                let distances = bellman_ford(nodes, &stated, from);
                for to in 0..nodes {
                    if let Some(distance) = distances[to as usize] {
                        let bound = graph.lower_bound(NET, NodeId(from), NodeId(to));
                        prop_assert!(bound <= distance);
                    }
                }
            }
        }

        /// Models reticle's estimate: nodes sit on tiles, and each edge costs at least the
        /// Manhattan distance it covers, so that distance is an admissible estimate. The
        /// estimator must also survive being moved into the graph and called through it.
        #[test]
        fn supplied_estimator_is_passed_through_and_admissible(
            tiles in proptest::collection::vec((0u32..8, 0u32..8), 8),
            edges in proptest::collection::vec((0u32..8, 0u32..8, 0u32..4, 0.0..4.0f64), 0..40),
            bases in proptest::collection::vec(0u32..4, 8),
        ) {
            let manhattan = |from: u32, to: u32| {
                let (a, b) = (tiles[from as usize], tiles[to as usize]);
                a.0.abs_diff(b.0) + a.1.abs_diff(b.1)
            };
            let mut stated = Vec::new();
            let mut builder = GraphBuilder::new(8);
            for &(from, to, extra, fraction) in &edges {
                let edge_cost = cost(f64::from(manhattan(from, to) + extra) + fraction);
                builder.add_edge(NodeId(from), NodeId(to), edge_cost).unwrap();
                stated.push((from, to, edge_cost + Cost::from(bases[to as usize])));
            }
            for (node, &base) in bases.iter().enumerate() {
                builder.set_base_cost(NodeId(node as u32), Cost::from(base)).unwrap();
            }
            let estimate = |from: NodeId, to: NodeId| Cost::from(manhattan(from.0, to.0));
            let graph = builder.build().with_lower_bound(estimate);

            for from in 0..8 {
                let distances = bellman_ford(8, &stated, from);
                for to in 0..8 {
                    let bound = graph.lower_bound(NET, NodeId(from), NodeId(to));
                    prop_assert_eq!(bound, Cost::from(manhattan(from, to)));
                    if let Some(distance) = distances[to as usize] {
                        prop_assert!(bound <= distance);
                    }
                }
            }
        }
    }
}
