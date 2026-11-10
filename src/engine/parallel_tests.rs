//! Tests and measurements of batches and threads.
//!
//! The tests compare whole outcomes and every iteration's report across thread counts, against
//! a run through [`Engine::step`] that has no thread in it. The measurements are `#[ignore]`d:
//! the [engine's documentation](super#batches-and-threads) has their results and how to run them.

use super::tests::{
    any_graph_problem, any_grid_problem, check_honest, check_legal, hash_outcome,
    permutation_problem, routable,
};
use super::*;
use crate::id::NodeId;
use crate::net::{AccessPoint, NetOutcome, Terminal};
use crate::rng::Rng;
use crate::space::graph::{Graph, GraphBuilder, LowerBound};
use crate::space::grid::Grid;
use crate::space::{DenseSlots, InternedSlots};
use core::cell::Cell;
use proptest::prelude::*;

fn net_of(id: u32, nodes: &[NodeId]) -> Net<NodeId> {
    Net {
        id: NetId(id),
        terminals: nodes.iter().map(|&node| Terminal::at(node)).collect(),
    }
}

/// Every row's left end to a shuffled row's right end: all nets cross the whole grid.
fn dense_grid(size: u32, layers: u32, seed: u64) -> (Grid, Vec<Net<NodeId>>) {
    let grid = Grid::new(size, size, layers).unwrap();
    let mut rows: Vec<u32> = (0..size).collect();
    Rng::new(seed).shuffle(&mut rows);
    let nets = rows
        .iter()
        .enumerate()
        .map(|(y, &to)| {
            let from = grid.node_at(0, y as u32, 0).unwrap();
            net_of(y as u32, &[from, grid.node_at(size - 1, to, 0).unwrap()])
        })
        .collect();
    (grid, nets)
}

/// Many nets of two to four terminals on unshared cells, each within `reach` of its first.
pub(super) fn local_grid(
    size: u32,
    layers: u32,
    count: u32,
    reach: u32,
    seed: u64,
) -> (Grid, Vec<Net<NodeId>>) {
    let grid = Grid::new(size, size, layers).unwrap();
    let mut rng = Rng::new(seed);
    let mut used = FastSet::default();
    let mut nets = Vec::new();
    let mut id = 0;
    while id < count {
        let (x, y) = (
            rng.below(u64::from(size)) as u32,
            rng.below(u64::from(size)) as u32,
        );
        let terminals = 2 + rng.below(3) as u32;
        let mut nodes = Vec::new();
        for _ in 0..terminals {
            let low_x = x.saturating_sub(reach);
            let low_y = y.saturating_sub(reach);
            let high_x = (x + reach).min(size - 1);
            let high_y = (y + reach).min(size - 1);
            let at_x = low_x + rng.below(u64::from(high_x - low_x + 1)) as u32;
            let at_y = low_y + rng.below(u64::from(high_y - low_y + 1)) as u32;
            let node = grid.node_at(at_x, at_y, 0).unwrap();
            if used.insert(node) {
                nodes.push(node);
            }
        }
        if nodes.len() >= 2 {
            nets.push(net_of(id, &nodes));
            id += 1;
        }
    }
    (grid, nets)
}

// The fabric of `benches/search.rs`.

pub(super) const COST_PER_TILE: f64 = 0.75;
pub(super) const WIRE_SPAN: [u32; 2] = [1, 4];
const WIRE_COST_PER_TILE: [f64; 2] = [1.0, COST_PER_TILE];
const SWITCH_COST: [f64; 3] = [0.125, 0.1875, 0.25];
pub(super) const SITE_NODES: u32 = 2;
const DIRECTIONS: [(i64, i64); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

#[derive(Clone, Copy, Debug)]
pub(super) struct BenchFabric {
    pub(super) tiles: u32,
    pub(super) tracks: u32,
    pub(super) pins: u32,
    output_flexibility: u32,
    switch_fanout: u32,
    input_flexibility: u32,
}

impl BenchFabric {
    pub(super) fn wires_per_tile(self) -> u32 {
        self.tracks * WIRE_SPAN.len() as u32 * DIRECTIONS.len() as u32
    }
    pub(super) fn nodes_per_tile(self) -> u32 {
        SITE_NODES + 2 * self.pins + self.wires_per_tile()
    }
    fn num_nodes(self) -> u32 {
        self.tiles * self.tiles * self.nodes_per_tile()
    }
    fn tile_base(self, x: u32, y: u32) -> u32 {
        (x + self.tiles * y) * self.nodes_per_tile()
    }
    pub(super) fn source(self, x: u32, y: u32) -> NodeId {
        NodeId(self.tile_base(x, y))
    }
    pub(super) fn sink(self, x: u32, y: u32) -> NodeId {
        NodeId(self.tile_base(x, y) + 1)
    }
    fn output_pin(self, x: u32, y: u32, pin: u32) -> NodeId {
        NodeId(self.tile_base(x, y) + SITE_NODES + pin)
    }
    fn input_pin(self, x: u32, y: u32, pin: u32) -> NodeId {
        NodeId(self.tile_base(x, y) + SITE_NODES + self.pins + pin)
    }
    fn wire(self, x: u32, y: u32, index: u32) -> NodeId {
        NodeId(self.tile_base(x, y) + SITE_NODES + 2 * self.pins + index)
    }
    pub(super) fn wire_kind(index: u32) -> usize {
        ((index / DIRECTIONS.len() as u32) as usize) % WIRE_SPAN.len()
    }
    pub(super) fn wire_end(self, x: u32, y: u32, index: u32) -> (u32, u32, u32) {
        let direction = DIRECTIONS[index as usize % DIRECTIONS.len()];
        let span = i64::from(WIRE_SPAN[Self::wire_kind(index)]);
        let limit = i64::from(self.tiles) - 1;
        let end_x = (i64::from(x) + direction.0 * span).clamp(0, limit);
        let end_y = (i64::from(y) + direction.1 * span).clamp(0, limit);
        let covered = end_x.abs_diff(i64::from(x)) + end_y.abs_diff(i64::from(y));
        (end_x as u32, end_y as u32, covered as u32)
    }
    fn positions(self) -> Vec<(u16, u16)> {
        let mut positions = vec![(0, 0); self.num_nodes() as usize];
        let site_nodes = (SITE_NODES + 2 * self.pins) as usize;
        for y in 0..self.tiles {
            for x in 0..self.tiles {
                let base = self.tile_base(x, y) as usize;
                positions[base..base + site_nodes].fill((x as u16, y as u16));
                for index in 0..self.wires_per_tile() {
                    let (end_x, end_y, _) = self.wire_end(x, y, index);
                    positions[self.wire(x, y, index).index()] = (end_x as u16, end_y as u16);
                }
            }
        }
        positions
    }
    fn build(self, seed: u64) -> Graph {
        let mut rng = Rng::new(seed);
        let wires = u64::from(self.tiles * self.tiles * self.wires_per_tile());
        let edges = wires * u64::from(self.switch_fanout + self.input_flexibility);
        let mut builder = GraphBuilder::with_edge_capacity(self.num_nodes(), edges as usize);
        let wires_per_tile = u64::from(self.wires_per_tile());
        let switch_cost = |rng: &mut Rng| {
            Cost::new(SWITCH_COST[rng.below(SWITCH_COST.len() as u64) as usize]).unwrap()
        };
        for y in 0..self.tiles {
            for x in 0..self.tiles {
                for pin in 0..self.pins {
                    let output = self.output_pin(x, y, pin);
                    let (source, sink) = (self.source(x, y), self.sink(x, y));
                    builder.add_edge(source, output, Cost::ZERO).unwrap();
                    builder
                        .add_edge(self.input_pin(x, y, pin), sink, Cost::ZERO)
                        .unwrap();
                    for _ in 0..self.output_flexibility {
                        let index = rng.below(wires_per_tile) as u32;
                        if self.wire_end(x, y, index).2 > 0 {
                            let cost = switch_cost(&mut rng);
                            builder
                                .add_edge(output, self.wire(x, y, index), cost)
                                .unwrap();
                        }
                    }
                }
                for index in 0..self.wires_per_tile() {
                    let (end_x, end_y, covered) = self.wire_end(x, y, index);
                    if covered == 0 {
                        continue;
                    }
                    let wire = self.wire(x, y, index);
                    let per_tile = WIRE_COST_PER_TILE[Self::wire_kind(index)];
                    let base = Cost::new(per_tile * f64::from(covered)).unwrap();
                    builder.set_base_cost(wire, base).unwrap();
                    for _ in 0..self.switch_fanout {
                        let next = rng.below(wires_per_tile) as u32;
                        if self.wire_end(end_x, end_y, next).2 > 0 {
                            let cost = switch_cost(&mut rng);
                            builder
                                .add_edge(wire, self.wire(end_x, end_y, next), cost)
                                .unwrap();
                        }
                    }
                    for _ in 0..self.input_flexibility {
                        let pin = rng.below(u64::from(self.pins)) as u32;
                        let cost = switch_cost(&mut rng);
                        builder
                            .add_edge(wire, self.input_pin(end_x, end_y, pin), cost)
                            .unwrap();
                    }
                }
            }
        }
        builder.build()
    }

    /// One net per used source, with a few sinks nearby and sometimes a far one.
    pub(super) fn nets(self, count: u32, reach: u32, seed: u64) -> Vec<Net<NodeId>> {
        let mut rng = Rng::new(seed);
        let tiles = u64::from(self.tiles);
        let mut sources = FastSet::default();
        let mut sinks = FastSet::default();
        let mut nets = Vec::new();
        while nets.len() < count as usize {
            let (x, y) = (rng.below(tiles) as u32, rng.below(tiles) as u32);
            if !sources.insert((x, y)) {
                continue;
            }
            let mut nodes = vec![self.source(x, y)];
            let fanout = 1 + rng.below(4);
            for _ in 0..fanout {
                // One sink in ten crosses the device, as real designs have a few such.
                let span = if rng.below(10) == 0 {
                    self.tiles
                } else {
                    reach
                };
                let (low_x, low_y) = (x.saturating_sub(span), y.saturating_sub(span));
                let (high_x, high_y) = (
                    (x + span).min(self.tiles - 1),
                    (y + span).min(self.tiles - 1),
                );
                let at_x = low_x + rng.below(u64::from(high_x - low_x + 1)) as u32;
                let at_y = low_y + rng.below(u64::from(high_y - low_y + 1)) as u32;
                if sinks.insert((at_x, at_y)) {
                    nodes.push(self.sink(at_x, at_y));
                }
            }
            if nodes.len() >= 2 {
                nets.push(net_of(nets.len() as u32, &nodes));
            }
        }
        nets
    }
}

pub(super) struct TileBound {
    positions: Vec<(u16, u16)>,
}

impl LowerBound for TileBound {
    #[inline]
    fn estimate(&self, from: NodeId, to: NodeId) -> Cost {
        let (a, b) = (self.positions[from.index()], self.positions[to.index()]);
        let distance = u32::from(a.0.abs_diff(b.0)) + u32::from(a.1.abs_diff(b.1));
        Cost::new(f64::from(distance) * COST_PER_TILE).unwrap_or(Cost::ZERO)
    }
}

// What has to hold.

pub(super) const THREAD_COUNTS: [usize; 5] = [1, 2, 3, 4, 8];

/// Runs on one thread with [`Engine::step`], which no thread is involved in, for comparing with.
fn run_stepped<S: RoutingSpace>(
    space: &S,
    nets: &[Net<S::Node>],
    options: EngineOptions<'_>,
) -> (Outcome<S::Node, S::Resource>, Vec<IterationReport>) {
    let mut engine = Engine::new(space, nets, options).unwrap();
    while engine.step().is_some() {}
    (engine.outcome(), engine.reports().to_vec())
}

fn run_threaded<S>(
    space: &S,
    nets: &[Net<S::Node>],
    options: EngineOptions<'_>,
    threads: usize,
) -> (Outcome<S::Node, S::Resource>, Vec<IterationReport>)
where
    S: RoutingSpace + Sync,
    S::Node: Send + Sync,
    S::Resource: Send + Sync,
    S::Slots: Send,
    S::ResourceSlots: Send + Sync,
{
    let mut engine = Engine::new(space, nets, options).unwrap();
    while engine.step_parallel(threads).is_some() {}
    (engine.outcome(), engine.reports().to_vec())
}

/// Requires routes, overuse, stop reason, costs and statistics, and every iteration's report,
/// to be the same at all thread counts, with the reference run on no thread but this one.
/// Returns the reference.
fn assert_same_for_all_threads<S>(
    space: &S,
    nets: &[Net<S::Node>],
    options: EngineOptions<'_>,
) -> (Outcome<S::Node, S::Resource>, Vec<IterationReport>)
where
    S: RoutingSpace + Sync,
    S::Node: Send + Sync,
    S::Resource: Send + Sync,
    S::Slots: Send,
    S::ResourceSlots: Send + Sync,
{
    let reference = run_stepped(space, nets, options);
    if let Err(problem) = check_honest(space, nets, &reference.0) {
        panic!("{problem}");
    }
    for threads in THREAD_COUNTS {
        let threaded = run_threaded(space, nets, options, threads);
        assert_eq!(threaded.0, reference.0, "outcome on {threads} threads");
        assert_eq!(threaded.1, reference.1, "reports on {threads} threads");
        // Equal costs could still differ in the sign of a zero, for all `==` can tell.
        assert_eq!(
            threaded.0.total_cost.get().to_bits(),
            reference.0.total_cost.get().to_bits(),
        );
    }
    reference
}

fn with_batch(batch_size: u32) -> EngineOptions<'static> {
    EngineOptions {
        batch_size,
        ..EngineOptions::default()
    }
}

/// Runs the batch machinery even at a batch size of 1, which `step` would not.
fn run_through_batches<S: RoutingSpace>(
    space: &S,
    nets: &[Net<S::Node>],
    options: EngineOptions<'_>,
) -> (Outcome<S::Node, S::Resource>, Vec<IterationReport>) {
    let mut engine = Engine::new(space, nets, options).unwrap();
    while engine.begin_iteration() {
        let before = engine.stats;
        let turns = engine.take_turns_in_batches(|engine, jobs| engine.route_batch_here(jobs));
        engine.finish_iteration(before, turns);
    }
    (engine.outcome(), engine.reports().to_vec())
}

pub(super) fn small_fabric(tiles: u32, tracks: u32) -> BenchFabric {
    BenchFabric {
        tiles,
        tracks,
        pins: 8,
        output_flexibility: 8,
        switch_fanout: 6,
        input_flexibility: 2,
    }
}

pub(super) fn fabric_graph(fabric: BenchFabric) -> Graph<TileBound> {
    fabric
        .build(u64::from(fabric.tiles))
        .with_lower_bound(TileBound {
            positions: fabric.positions(),
        })
}

// The batch size of 1.

/// The strongest statement about the batches that can be tested: with one net to a batch they
/// are the sequential router, to the last bit and the last statistic.
#[test]
fn a_batch_of_one_is_the_sequential_router() {
    let (grid, nets) = permutation_problem(16, 2, 42);
    let sequential = run_stepped(&grid, &nets, EngineOptions::default());
    assert_eq!(
        run_through_batches(&grid, &nets, EngineOptions::default()),
        sequential
    );

    // That includes the regression value of the sequential router itself.
    assert_eq!(
        hash_outcome(&run_through_batches(&grid, &nets, EngineOptions::default()).0),
        13615641185186696971
    );

    let fabric = small_fabric(12, 1);
    let (graph, nets) = (fabric_graph(fabric), fabric.nets(50, 6, 3));
    let sequential = run_stepped(&graph, &nets, EngineOptions::default());
    assert!(sequential.1.len() > 1, "this should take some negotiation");
    assert_eq!(
        run_through_batches(&graph, &nets, EngineOptions::default()),
        sequential
    );

    // And wherever a budget happens to run out.
    let spent = sequential.0.routes.stats.nodes_expanded;
    for budget in [0, 1, 7, spent / 10, spent / 2, spent - 1, spent, spent + 1] {
        let limited = EngineOptions {
            max_expansions: budget,
            ..EngineOptions::default()
        };
        assert_eq!(
            run_through_batches(&graph, &nets, limited),
            run_stepped(&graph, &nets, limited),
            "with a budget of {budget}",
        );
    }
}

#[test]
fn threads_alone_change_nothing_at_the_default_batch_size() {
    let (grid, nets) = permutation_problem(16, 2, 42);
    let (outcome, _) = assert_same_for_all_threads(&grid, &nets, EngineOptions::default());
    assert_eq!(hash_outcome(&outcome), 13615641185186696971);
}

#[test]
fn a_batch_size_of_zero_means_one_and_so_does_a_thread_count_of_zero() {
    let (grid, nets) = permutation_problem(10, 2, 4);
    let expected = run_stepped(&grid, &nets, EngineOptions::default());
    assert_eq!(run_stepped(&grid, &nets, with_batch(0)), expected);
    assert_eq!(run_threaded(&grid, &nets, with_batch(0), 4), expected);
    assert_eq!(run_threaded(&grid, &nets, with_batch(1), 0), expected);
    let batched = with_batch(4);
    assert_eq!(
        run_threaded(&grid, &nets, batched, 0),
        run_threaded(&grid, &nets, batched, 1)
    );
}

// Thread counts, across kinds of problem.

#[test]
fn dense_grids_are_the_same_on_any_number_of_threads() {
    let (grid, nets) = permutation_problem(24, 2, 5);
    for batch in [2, 3, 8, 32, 1000] {
        let (outcome, _) = assert_same_for_all_threads(&grid, &nets, with_batch(batch));
        assert!(outcome.is_success(), "at a batch size of {batch}");
    }
}

#[test]
fn multi_terminal_grids_are_the_same_on_any_number_of_threads() {
    let (grid, nets) = local_grid(32, 2, 40, 8, 6);
    for batch in [2, 16, 64] {
        let _ = assert_same_for_all_threads(&grid, &nets, with_batch(batch));
    }
}

#[test]
fn fabrics_are_the_same_on_any_number_of_threads() {
    let fabric = small_fabric(12, 1);
    let (graph, nets) = (fabric_graph(fabric), fabric.nets(50, 6, 3));
    for batch in [2, 16, 64] {
        let (outcome, _) = assert_same_for_all_threads(&graph, &nets, with_batch(batch));
        assert!(outcome.is_success(), "at a batch size of {batch}");
    }
}

#[test]
fn incremental_rerouting_is_the_same_on_any_number_of_threads() {
    let fabric = small_fabric(12, 1);
    let (graph, nets) = (fabric_graph(fabric), fabric.nets(50, 6, 3));
    let incremental = EngineOptions {
        incremental_reroute: true,
        ..EngineOptions::default()
    };
    // A batch of one is still the sequential router.
    let sequential = run_stepped(&graph, &nets, incremental);
    assert!(sequential.1.len() > 1, "this should take some negotiation");
    assert_eq!(run_through_batches(&graph, &nets, incremental), sequential);
    for batch in [2, 16] {
        let options = EngineOptions {
            batch_size: batch,
            ..incremental
        };
        let (outcome, _) = assert_same_for_all_threads(&graph, &nets, options);
        assert!(outcome.is_success(), "at a batch size of {batch}");
    }
}

#[test]
fn graphs_with_dead_roots_and_equivalent_access_points_are_the_same() {
    // Directed, so that some roots reach the nearest terminal and not the next, and nets have
    // several ways in that compete. The access costs are odd so that their sums are telling.
    let mut builder = GraphBuilder::new(30);
    let mut rng = Rng::new(9);
    for _ in 0..110 {
        let (from, to) = (rng.below(30) as u32, rng.below(30) as u32);
        let cost = Cost::from(rng.below(5) as u32);
        builder.add_edge(NodeId(from), NodeId(to), cost).unwrap();
    }
    let graph = builder.build();
    let point = |rng: &mut Rng| AccessPoint {
        node: NodeId(rng.below(32) as u32),
        cost: Cost::from(1 + 2 * rng.below(3) as u32),
    };
    let nets: Vec<_> = (0..12)
        .map(|id| {
            let count = 2 + rng.below(3);
            let terminals = (0..count)
                .map(|_| {
                    let mut points = vec![point(&mut rng), point(&mut rng)];
                    points.push(point(&mut rng));
                    Terminal::new(points).unwrap()
                })
                .collect();
            Net {
                id: NetId(id),
                terminals,
            }
        })
        .collect();
    for batch in [2, 5, 12] {
        let (outcome, reports) = assert_same_for_all_threads(&graph, &nets, with_batch(batch));
        assert!(!reports.is_empty());
        for net in &nets {
            let routed = outcome.routes.route(net.id).is_some();
            assert_eq!(routed, routable(&graph, net), "{:?}", net.id);
        }
    }
}

#[test]
fn shuffled_turns_are_the_same_on_any_number_of_threads() {
    let (grid, nets) = permutation_problem(16, 2, 2);
    let shuffled = EngineOptions {
        shuffle_seed: Some(11),
        reroute_all: true,
        max_iterations: 20,
        ..with_batch(6)
    };
    let _ = assert_same_for_all_threads(&grid, &nets, shuffled);
}

/// A grid whose resources are not numbered in advance, so that their slots depend on the
/// order they are first occupied in, and go through a hash map that all threads read.
struct Interned(Grid);

impl RoutingSpace for Interned {
    type Node = NodeId;
    type Resource = (u32, u32);
    type Slots = DenseSlots<NodeId>;
    type ResourceSlots = InternedSlots<(u32, u32)>;
    fn new_slots(&self) -> Self::Slots {
        DenseSlots::new()
    }
    fn new_resource_slots(&self) -> Self::ResourceSlots {
        InternedSlots::new()
    }
    fn contains(&self, node: NodeId) -> bool {
        self.0.contains(node)
    }
    fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
        self.0.expand(net, from, out);
    }
    fn lower_bound(&self, net: NetId, from: NodeId, to: NodeId) -> Cost {
        self.0.lower_bound(net, from, to)
    }
    fn resources(&self, net: NetId, from: NodeId, to: NodeId, out: &mut Vec<(u32, u32)>) {
        let mut cells = Vec::new();
        self.0.resources(net, from, to, &mut cells);
        out.extend(cells.iter().map(|cell: &NodeId| (cell.0, 7)));
    }
    fn capacity(&self, (cell, _): (u32, u32)) -> u32 {
        self.0.capacity(NodeId(cell))
    }
}

#[test]
fn interned_resources_are_the_same_on_any_number_of_threads() {
    let (grid, nets) = permutation_problem(16, 2, 9);
    let plain = assert_same_for_all_threads(&grid, &nets, with_batch(8));
    let interned = assert_same_for_all_threads(&Interned(grid), &nets, with_batch(8));
    // Slot numbering differs completely between the two, and must not show.
    assert_eq!(interned.0.routes, plain.0.routes);
    assert_eq!(interned.0.total_cost, plain.0.total_cost);
}

#[test]
fn the_thread_count_can_change_from_one_iteration_to_the_next() {
    let (grid, nets) = permutation_problem(16, 2, 6);
    let reference = run_stepped(&grid, &nets, with_batch(8));
    assert!(reference.1.len() > 3, "this should take some negotiation");
    let mut engine = Engine::new(&grid, &nets, with_batch(8)).unwrap();
    let mut counts = [4, 1, 8, 2, 3].iter().copied().cycle();
    while engine.step_parallel(counts.next().unwrap_or(1)).is_some() {}
    assert_eq!((engine.outcome(), engine.reports().to_vec()), reference);
}

#[test]
fn workers_are_kept_for_later_batches_and_only_as_many_as_needed() {
    let (grid, nets) = permutation_problem(16, 2, 6);
    let mut engine = Engine::new(&grid, &nets, with_batch(8)).unwrap();
    engine.step_parallel(3);
    assert_eq!(engine.workers.len(), 3);
    // A later iteration with fewer threads keeps what it has for one that wants them again.
    engine.step_parallel(2);
    assert_eq!(engine.workers.len(), 3);

    // More threads than a batch can use would waste a worker's memory for nothing.
    let mut engine = Engine::new(&grid, &nets, with_batch(2)).unwrap();
    engine.step_parallel(8);
    assert_eq!(engine.workers.len(), 2);

    // Nor does a sequential run ask for any more than the one.
    let mut engine = Engine::new(&grid, &nets, EngineOptions::default()).unwrap();
    engine.step_parallel(8);
    assert_eq!(engine.workers.len(), 1);
}

#[test]
fn problems_with_nothing_to_route_are_fine_on_any_number_of_threads() {
    let grid = Grid::new(4, 4, 1).unwrap();
    let _ = assert_same_for_all_threads(&grid, &[], with_batch(4));
    let lone = [net_of(0, &[NodeId(1)]), net_of(1, &[])];
    let (outcome, _) = assert_same_for_all_threads(&grid, &lone, with_batch(4));
    assert!(outcome.is_success());
    let outside = [
        net_of(0, &[NodeId(1), NodeId(99)]),
        net_of(1, &[NodeId(2), NodeId(3)]),
    ];
    let (outcome, _) = assert_same_for_all_threads(&grid, &outside, with_batch(4));
    assert_eq!(outcome.routes.unrouted().count(), 1);
}

// What a batch does.

/// Two nets that both prefer node 2, and one with a way around it.
fn bottleneck() -> (Graph, Vec<Net<NodeId>>) {
    let mut builder = GraphBuilder::new(7);
    for (from, to, cost) in [
        (0, 2, 1),
        (2, 4, 1),
        (0, 3, 2),
        (3, 4, 2),
        (1, 2, 1),
        (2, 5, 1),
    ] {
        builder
            .add_edge(NodeId(from), NodeId(to), Cost::from(cost))
            .unwrap();
    }
    let nets = vec![
        net_of(0, &[NodeId(1), NodeId(5)]),
        net_of(1, &[NodeId(0), NodeId(4)]),
    ];
    (builder.build(), nets)
}

#[test]
fn a_route_that_conflicts_with_an_earlier_commit_of_its_batch_is_found_again() {
    let (graph, nets) = bottleneck();
    // Sharing at this price is dearer than the way around, for a net that can see the sharing.
    let options = EngineOptions {
        first_present: Cost::from(100),
        ..with_batch(2)
    };
    let mut engine = Engine::new(&graph, &nets, options).unwrap();
    let first = engine.step().unwrap();
    // Neither could see the other. The second lost, was routed again, and saw the first.
    assert_eq!(first.discarded, 1);
    assert_eq!(first.batches, 2);
    assert_eq!(first.nets_rerouted, 2);
    assert_eq!(first.total_overuse, 0);
    assert_eq!(first.searches, 3);
    let outcome = engine.outcome();
    assert!(outcome.is_success());
    let route = outcome.routes.route(NetId(1)).unwrap();
    assert!(route.nodes().any(|node| node == NodeId(3)));
}

/// Runs the first iteration through the batch machinery, returning its report and the nets of
/// each batch.
fn first_batches<S: RoutingSpace>(
    space: &S,
    nets: &[Net<S::Node>],
    options: EngineOptions<'_>,
) -> (IterationReport, Vec<Vec<u32>>) {
    let mut engine = Engine::new(space, nets, options).unwrap();
    assert!(engine.begin_iteration());
    let before = engine.stats;
    let mut batches = Vec::new();
    let turns = engine.take_turns_in_batches(|engine, jobs| {
        batches.push(jobs.to_vec());
        engine.route_batch_here(jobs)
    });
    (engine.finish_iteration(before, turns), batches)
}

#[test]
fn a_dropped_net_routes_again_after_the_rest_of_its_batch() {
    // The bottleneck's two nets, then three that share nothing with anything.
    let (builder, mut nets) = {
        let (_, nets) = bottleneck();
        let mut builder = GraphBuilder::new(13);
        for (from, to, cost) in [
            (0, 2, 1),
            (2, 4, 1),
            (0, 3, 2),
            (3, 4, 2),
            (1, 2, 1),
            (2, 5, 1),
            (7, 8, 1),
            (9, 10, 1),
            (11, 12, 1),
        ] {
            builder
                .add_edge(NodeId(from), NodeId(to), Cost::from(cost))
                .unwrap();
        }
        (builder, nets)
    };
    for (id, from) in [(2, 7), (3, 9), (4, 11)] {
        nets.push(net_of(id, &[NodeId(from), NodeId(from + 1)]));
    }
    let graph = builder.build();
    let options = EngineOptions {
        first_present: Cost::from(100),
        ..with_batch(3)
    };
    let (first, batches) = first_batches(&graph, &nets, options);
    // The second net lost, and the third was committed all the same, so that the next batch
    // is full: the dropped net first, then the two nets after the third.
    assert_eq!(batches, [vec![0, 1, 2], vec![1, 3, 4]]);
    assert_eq!(
        (first.batches, first.discarded, first.nets_rerouted),
        (2, 1, 5)
    );
    assert_eq!(first.total_overuse, 0);
}

#[test]
fn a_net_of_many_terminals_only_routes_first_in_its_batch() {
    let grid = Grid::new(16, 16, 1).unwrap();
    let at = |x, y| grid.node_at(x, y, 0).unwrap();
    // Short nets in rows 8 and up, with one of 64 terminals in rows 0 to 3 among them.
    let mut nets: Vec<_> = (0..6)
        .map(|n| net_of(n, &[at(0, 8 + n), at(15, 8 + n)]))
        .collect();
    let big: Vec<_> = (0..4).flat_map(|y| (0..16).map(move |x| (x, y))).collect();
    let big: Vec<_> = big.into_iter().map(|(x, y)| at(x, y)).collect();
    assert_eq!(big.len(), LEADING_TERMINALS);
    nets.insert(3, net_of(6, &big));
    let (first, batches) = first_batches(&grid, &nets, with_batch(4));
    // The batch before it ends early, and it sees everything that batch committed.
    assert_eq!(batches, [vec![0, 1, 2], vec![3, 4, 5, 6]]);
    assert_eq!(first.discarded, 0);
    assert!(route(&grid, &nets, with_batch(4)).unwrap().is_success());
}

#[test]
fn sharing_within_capacity_is_not_a_conflict() {
    let (mut graph_builder, nets) = {
        let mut builder = GraphBuilder::new(7);
        for (from, to, cost) in [
            (0, 2, 1),
            (2, 4, 1),
            (0, 3, 2),
            (3, 4, 2),
            (1, 2, 1),
            (2, 5, 1),
        ] {
            builder
                .add_edge(NodeId(from), NodeId(to), Cost::from(cost))
                .unwrap();
        }
        (builder, bottleneck().1)
    };
    graph_builder.set_capacity(NodeId(2), 2).unwrap();
    let graph = graph_builder.build();
    let mut engine = Engine::new(&graph, &nets, with_batch(2)).unwrap();
    let first = engine.step().unwrap();
    assert_eq!(first.discarded, 0);
    assert_eq!(first.total_overuse, 0);
    assert!(engine.outcome().is_success());
}

#[test]
fn nets_that_share_nothing_are_never_routed_twice() {
    let grid = Grid::new(8, 4, 1).unwrap();
    let nets: Vec<_> = (0..4)
        .map(|row| {
            let at = |x| grid.node_at(x, row, 0).unwrap();
            net_of(row, &[at(0), at(7)])
        })
        .collect();
    let mut engine = Engine::new(&grid, &nets, with_batch(4)).unwrap();
    let first = engine.step_parallel(4).unwrap();
    assert_eq!((first.batches, first.discarded, first.searches), (1, 0, 4));
    assert!(engine.outcome().is_success());
}

#[test]
fn a_conflict_settled_by_an_earlier_commit_needs_no_new_route() {
    // The first net has a way around the bottleneck and the second has none, so the second
    // iteration sees both on it and both in a batch, and then the first moves away.
    let (graph, mut nets) = bottleneck();
    nets.swap(0, 1);
    let options = EngineOptions {
        initial_present: Cost::from(10),
        ..with_batch(2)
    };
    let mut engine = Engine::new(&graph, &nets, options).unwrap();
    let first = engine.step().unwrap();
    assert_eq!(
        first.total_overuse, 1,
        "at this price, both stay on it at first"
    );

    let second = engine.step().unwrap();
    assert_eq!(second.searches, 2);
    assert_eq!(
        second.nets_rerouted, 1,
        "the other was left with nothing to resolve"
    );
    assert_eq!(second.discarded, 1);
    assert_eq!(second.total_overuse, 0);
    assert!(engine.outcome().is_success());
}

#[test]
fn batch_size_does_change_results_and_they_stay_legal() {
    let (grid, nets) = permutation_problem(24, 2, 5);
    let sequential = run_stepped(&grid, &nets, EngineOptions::default()).0;
    let mut differs = false;
    for batch in [2, 4, 8, 16, 32] {
        let outcome = route(&grid, &nets, with_batch(batch)).unwrap();
        assert!(outcome.is_success(), "at a batch size of {batch}");
        if let Err(problem) = check_legal(&grid, &nets, &outcome.routes) {
            panic!("{problem}");
        }
        differs |= outcome.routes != sequential.routes;
    }
    assert!(differs, "if not, the option would mean nothing");
}

// Budgets.

#[test]
fn budgets_stop_at_the_same_place_on_any_number_of_threads() {
    let (grid, nets) = permutation_problem(16, 2, 3);
    for batch in [1, 2, 8, 32] {
        let spent = run_stepped(&grid, &nets, with_batch(batch))
            .0
            .routes
            .stats
            .nodes_expanded;
        for budget in [0, 1, 50, spent / 8, spent / 3, spent / 2, spent - 1] {
            let limited = EngineOptions {
                max_expansions: budget,
                ..with_batch(batch)
            };
            let (outcome, _) = assert_same_for_all_threads(&grid, &nets, limited);
            assert_eq!(outcome.stop, StopReason::BudgetExhausted, "budget {budget}");
            assert!(outcome.routes.stats.nodes_expanded <= budget);
        }
        // All that it takes is enough, and any more makes no difference.
        for budget in [spent, spent + 1] {
            let limited = EngineOptions {
                max_expansions: budget,
                ..with_batch(batch)
            };
            let (outcome, _) = assert_same_for_all_threads(&grid, &nets, limited);
            assert_eq!(outcome.stop, StopReason::Converged);
        }
    }
}

#[test]
fn a_budget_covers_the_work_of_discarded_routes_too() {
    let (grid, nets) = permutation_problem(12, 2, 3);
    let reports = run_stepped(&grid, &nets, with_batch(6)).1;
    let discarded: u32 = reports.iter().map(|report| report.discarded).sum();
    assert!(discarded > 0);
    // Each iteration's expansions are all charged, so the sum is the run's total.
    let (outcome, reports) = run_stepped(&grid, &nets, with_batch(6));
    let total: u64 = reports.iter().map(|report| report.nodes_expanded).sum();
    assert_eq!(total, outcome.routes.stats.nodes_expanded);
    // A discarded route does its share to end a run.
    let limited = EngineOptions {
        max_expansions: total - 1,
        ..with_batch(6)
    };
    let (limited, _) = run_stepped(&grid, &nets, limited);
    assert_eq!(limited.stop, StopReason::BudgetExhausted);
}

// Cancellation.

/// Sets a flag once enough nodes have been expanded, from whichever thread gets there.
struct CancelsAfter<'a> {
    grid: Grid,
    expansions: AtomicUsize,
    after: usize,
    flag: &'a AtomicBool,
}

impl RoutingSpace for CancelsAfter<'_> {
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
        self.grid.contains(node)
    }
    fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
        if self.expansions.fetch_add(1, Ordering::Relaxed) >= self.after {
            self.flag.store(true, Ordering::Relaxed);
        }
        self.grid.expand(net, from, out);
    }
    fn lower_bound(&self, net: NetId, from: NodeId, to: NodeId) -> Cost {
        self.grid.lower_bound(net, from, to)
    }
    fn resources(&self, net: NetId, from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
        self.grid.resources(net, from, to, out);
    }
    fn capacity(&self, resource: NodeId) -> u32 {
        self.grid.capacity(resource)
    }
}

#[test]
fn a_flag_that_is_never_set_changes_nothing() {
    let (grid, nets) = permutation_problem(16, 2, 3);
    let flag = AtomicBool::new(false);
    let plain = with_batch(8);
    let flagged = EngineOptions {
        cancel: Some(&flag),
        ..plain
    };
    let expected = run_stepped(&grid, &nets, plain);
    for threads in THREAD_COUNTS {
        assert_eq!(run_threaded(&grid, &nets, flagged, threads), expected);
    }
}

#[test]
fn a_flag_set_beforehand_stops_every_thread_count_at_the_start() {
    let (grid, nets) = permutation_problem(12, 2, 3);
    let flag = AtomicBool::new(true);
    let cancelled = EngineOptions {
        cancel: Some(&flag),
        ..with_batch(8)
    };
    let (outcome, _) = assert_same_for_all_threads(&grid, &nets, cancelled);
    assert_eq!(outcome.stop, StopReason::Cancelled);
    assert_eq!(outcome.best_iteration, 0);
    assert_eq!(outcome.routes.stats.nodes_expanded, 0);
}

#[test]
fn a_flag_set_along_the_way_stops_the_run_with_an_honest_result() {
    let (grid, nets) = permutation_problem(16, 2, 3);
    for threads in THREAD_COUNTS {
        for after in [0, 40, 400, 4000] {
            let flag = AtomicBool::new(false);
            let space = CancelsAfter {
                grid: grid.clone(),
                expansions: AtomicUsize::new(0),
                after,
                flag: &flag,
            };
            let options = EngineOptions {
                cancel: Some(&flag),
                ..with_batch(8)
            };
            let outcome = route_parallel(&space, &nets, options, threads).unwrap();
            assert_eq!(outcome.stop, StopReason::Cancelled);
            if let Err(problem) = check_honest(&grid, &nets, &outcome) {
                panic!("after {after} on {threads} threads: {problem}");
            }
        }
    }
}

// Panics.

/// Panics on expanding one node, which only one net's search will come to.
struct PanicsAt {
    grid: Grid,
    node: NodeId,
}

impl RoutingSpace for PanicsAt {
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
        self.grid.contains(node)
    }
    fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
        assert!(from != self.node, "the space gave up at {from:?}");
        self.grid.expand(net, from, out);
    }
    fn lower_bound(&self, net: NetId, from: NodeId, to: NodeId) -> Cost {
        self.grid.lower_bound(net, from, to)
    }
    fn resources(&self, net: NetId, from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
        self.grid.resources(net, from, to, out);
    }
    fn capacity(&self, resource: NodeId) -> u32 {
        self.grid.capacity(resource)
    }
}

#[test]
fn a_panic_in_a_search_reaches_the_caller_with_its_own_message() {
    // Rows of their own, so that the net that panics is one of many running at once, and in
    // different runs ends up on the calling thread or on another one.
    let grid = Grid::new(32, 16, 1).unwrap();
    let nets: Vec<_> = (0..16)
        .map(|row| {
            let at = |x| grid.node_at(x, row, 0).unwrap();
            net_of(row, &[at(0), at(31)])
        })
        .collect();
    let node = grid.node_at(5, 11, 0).unwrap();
    for threads in THREAD_COUNTS {
        for batch in [1, 4, 16] {
            let space = PanicsAt {
                grid: grid.clone(),
                node,
            };
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                route_parallel(&space, &nets, with_batch(batch), threads)
            }));
            let payload = run.expect_err("the search should have panicked");
            let message = payload
                .downcast_ref::<String>()
                .expect("the assertion's own");
            assert!(message.contains("the space gave up"), "{message}");
        }
    }
}

// Threads really being used, which results cannot show, as that is the point of them.

/// Holds each search at its first expansion until `expected` searches have got there at once,
/// which is only possible if that many threads are running them. It gives up after a while, so
/// that a failure to run in parallel is a failed test and not a hung one.
struct MeetsInTheMiddle {
    grid: Grid,
    arrived: AtomicUsize,
    expected: usize,
    gave_up: AtomicBool,
}

impl RoutingSpace for MeetsInTheMiddle {
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
        self.grid.contains(node)
    }
    fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
        // Each net starts from its own row's first cell, and comes to it just the once.
        if self.grid.position(from).is_some_and(|(x, _, _)| x == 0) {
            self.arrived.fetch_add(1, Ordering::SeqCst);
            let start = std::time::Instant::now();
            while self.arrived.load(Ordering::SeqCst) < self.expected {
                if start.elapsed().as_secs() >= 5 {
                    self.gave_up.store(true, Ordering::SeqCst);
                    break;
                }
                std::thread::yield_now();
            }
        }
        self.grid.expand(net, from, out);
    }
    fn lower_bound(&self, net: NetId, from: NodeId, to: NodeId) -> Cost {
        self.grid.lower_bound(net, from, to)
    }
    fn resources(&self, net: NetId, from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
        self.grid.resources(net, from, to, out);
    }
    fn capacity(&self, resource: NodeId) -> u32 {
        self.grid.capacity(resource)
    }
}

#[test]
fn the_searches_of_a_batch_really_do_run_at_the_same_time() {
    for threads in [2, 3, 4] {
        let grid = Grid::new(4, threads, 1).unwrap();
        let nets: Vec<_> = (0..threads)
            .map(|row| {
                let at = |x| grid.node_at(x, row, 0).unwrap();
                net_of(row, &[at(0), at(3)])
            })
            .collect();
        let space = MeetsInTheMiddle {
            grid,
            arrived: AtomicUsize::new(0),
            expected: threads as usize,
            gave_up: AtomicBool::new(false),
        };
        let outcome = route_parallel(&space, &nets, with_batch(threads), threads as usize).unwrap();
        assert!(outcome.is_success());
        assert!(
            !space.gave_up.load(Ordering::SeqCst),
            "{threads} searches never ran at once"
        );
    }
}

// Spaces that threads cannot share.

/// A grid that counts expansions in a `Cell`, which rules out sharing it between threads.
struct CountingGrid {
    grid: Grid,
    expansions: Cell<u64>,
}

impl RoutingSpace for CountingGrid {
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
        self.grid.contains(node)
    }
    fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
        self.expansions.set(self.expansions.get() + 1);
        self.grid.expand(net, from, out);
    }
    fn lower_bound(&self, net: NetId, from: NodeId, to: NodeId) -> Cost {
        self.grid.lower_bound(net, from, to)
    }
    fn resources(&self, net: NetId, from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
        self.grid.resources(net, from, to, out);
    }
    fn capacity(&self, resource: NodeId) -> u32 {
        self.grid.capacity(resource)
    }
}

#[test]
fn a_space_that_threads_cannot_share_gets_the_same_batches_on_one_thread() {
    let (grid, nets) = permutation_problem(16, 2, 3);
    for batch in [1, 8] {
        let space = CountingGrid {
            grid: grid.clone(),
            expansions: Cell::new(0),
        };
        let unshared = route(&space, &nets, with_batch(batch)).unwrap();
        assert_eq!(
            unshared,
            route_parallel(&grid, &nets, with_batch(batch), 4).unwrap()
        );
        // It really did do the searches with this space. Its count can be the higher one, as
        // an engine also asks the space about the steps of each path that it adopts.
        assert!(space.expansions.get() >= unshared.routes.stats.nodes_expanded);
    }
}

#[test]
fn the_spaces_of_this_crate_can_be_shared_between_threads() {
    fn assert_parallel<S>()
    where
        S: RoutingSpace + Sync,
        S::Node: Send + Sync,
        S::Resource: Send + Sync,
        S::Slots: Send,
        S::ResourceSlots: Send + Sync,
    {
    }
    assert_parallel::<Grid>();
    assert_parallel::<Graph>();
    assert_parallel::<Graph<TileBound>>();
    assert_parallel::<Interned>();
}

// Regression values.

/// FNV-1a over everything a result says, with each number as its little-endian bytes, so that
/// any platform computes the same. Unlike a derived `Hash` this does not depend on the
/// standard library's choices or on the width of `usize`.
fn hash_everything(outcome: &Outcome<NodeId, NodeId>, reports: &[IterationReport]) -> u64 {
    let mut hash = 0xCBF2_9CE4_8422_2325_u64;
    let mut add = |word: u64| {
        for byte in word.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
        }
    };
    for (id, result) in outcome.routes.outcomes() {
        add(u64::from(id.0));
        match result {
            NetOutcome::Routed(route) => {
                add(route.edges().len() as u64);
                add(route.root().map_or(u64::MAX, |node| u64::from(node.0)));
                for edge in route.edges() {
                    add(u64::from(edge.from.0));
                    add(u64::from(edge.to.0));
                }
            }
            NetOutcome::Unrouted(reason) => add(match reason {
                UnroutedReason::NoPath => 1,
                UnroutedReason::BudgetExhausted => 2,
                _ => 3,
            }),
        }
    }
    for entry in &outcome.overused {
        add(u64::from(entry.resource.0));
        add(u64::from(entry.occupancy));
        add(u64::from(entry.capacity));
        for net in &entry.nets {
            add(u64::from(net.0));
        }
    }
    add(outcome.total_cost.get().to_bits());
    add(u64::from(outcome.best_iteration));
    add(outcome.routes.stats.iterations);
    add(outcome.routes.stats.searches);
    add(outcome.routes.stats.nodes_expanded);
    for report in reports {
        add(report.present.get().to_bits());
        add(u64::from(report.nets_rerouted));
        add(report.searches);
        add(report.nodes_expanded);
        add(u64::from(report.unrouted));
        add(report.total_overuse);
        add(report.total_cost.get().to_bits());
        add(u64::from(report.batches));
        add(u64::from(report.discarded));
    }
    hash
}

/// Regression values and not reference ones: they came from this implementation, and they pin
/// the batches, the prices and the schedule to the bit, for routes, costs and statistics.
/// Costs only go through `f64` addition and multiplication in a fixed order, which IEEE 754
/// defines exactly, so CI must get these on Linux, macOS and Windows and at every thread count.
///
/// A deliberate change to routing will change them. Update them then, and not otherwise.
#[test]
fn batches_match_their_regression_values() {
    let (grid, nets) = permutation_problem(16, 2, 42);
    let (outcome, reports) = assert_same_for_all_threads(&grid, &nets, with_batch(8));
    assert!(outcome.is_success());
    assert_eq!(hash_everything(&outcome, &reports), 6790892157793294191);

    // A fabric's costs include fractions, and its nets have several terminals.
    let fabric = small_fabric(12, 1);
    let (graph, nets) = (fabric_graph(fabric), fabric.nets(50, 6, 3));
    let (outcome, reports) = assert_same_for_all_threads(&graph, &nets, with_batch(16));
    assert!(outcome.is_success());
    assert_eq!(hash_everything(&outcome, &reports), 8570704114041813701);

    // A budget that runs out part way through a negotiation.
    let limited = EngineOptions {
        max_expansions: 3000,
        ..with_batch(16)
    };
    let (outcome, reports) = assert_same_for_all_threads(&graph, &nets, limited);
    assert_eq!(outcome.stop, StopReason::BudgetExhausted);
    assert_eq!(hash_everything(&outcome, &reports), 5364956525097003118);
}

// Properties.

/// Same results at all thread counts, and honest ones, for a random problem and batch size.
fn check_threads<S>(
    space: &S,
    nets: &[Net<S::Node>],
    batch: u32,
    budget: u64,
) -> Result<(), TestCaseError>
where
    S: RoutingSpace + Sync,
    S::Node: Send + Sync,
    S::Resource: Send + Sync,
    S::Slots: Send,
    S::ResourceSlots: Send + Sync,
{
    let options = EngineOptions {
        max_iterations: 8,
        max_expansions: budget,
        ..with_batch(batch)
    };
    let reference = run_stepped(space, nets, options);
    let honest = check_honest(space, nets, &reference.0);
    prop_assert!(honest.is_ok(), "{:?}", honest);
    prop_assert!(reference.0.routes.stats.nodes_expanded <= budget);
    for threads in [2, 3, 8] {
        prop_assert_eq!(&run_threaded(space, nets, options, threads), &reference);
    }
    // At no budget to speak of, a net can still end up unrouted for that reason alone. If
    // there is one to spare, then none can, so what is left is what no path can connect.
    if budget == u64::MAX {
        for net in nets {
            match reference.0.routes.outcome(net.id) {
                Some(NetOutcome::Routed(_)) => prop_assert!(routable(space, net)),
                Some(NetOutcome::Unrouted(UnroutedReason::NoPath)) => {
                    prop_assert!(!routable(space, net));
                }
                other => prop_assert!(false, "{:?} ended up as {:?}", net.id, other),
            }
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn grid_problems_are_the_same_on_any_number_of_threads(
        (grid, nets) in any_grid_problem(),
        batch in 1u32..10,
    ) {
        check_threads(&grid, &nets, batch, u64::MAX)?;
    }

    #[test]
    fn graph_problems_are_the_same_on_any_number_of_threads(
        (graph, nets) in any_graph_problem(),
        batch in 1u32..10,
    ) {
        check_threads(&graph, &nets, batch, u64::MAX)?;
    }

    #[test]
    fn budgets_are_the_same_on_any_number_of_threads(
        (grid, nets) in any_grid_problem(),
        batch in 1u32..10,
        budget in 0u64..200,
    ) {
        check_threads(&grid, &nets, batch, budget)?;
    }

    /// The batch machinery with one net to a batch, on all that random problems can throw at it.
    #[test]
    fn a_batch_of_one_always_is_the_sequential_router(
        (graph, nets) in any_graph_problem(),
        budget in prop_oneof![Just(u64::MAX), 0u64..200],
    ) {
        let limited = EngineOptions {
            max_iterations: 8,
            max_expansions: budget,
            ..EngineOptions::default()
        };
        prop_assert_eq!(
            run_through_batches(&graph, &nets, limited),
            run_stepped(&graph, &nets, limited),
        );
    }
}

// Measurements.

fn env_num(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn peak_rss_mib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find(|line| line.starts_with("VmHWM"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|kb| kb.parse::<u64>().ok())
        .map_or(0, |kb| kb / 1024)
}

fn measure_one<S>(name: &str, space: &S, nets: &[Net<NodeId>])
where
    S: RoutingSpace<Node = NodeId> + Sync,
    S::Resource: Send + Sync,
    S::Slots: Send,
    S::ResourceSlots: Send + Sync,
{
    let threads = env_num("GUIDE_THREADS", 1);
    let options = EngineOptions {
        max_iterations: env_num("GUIDE_ITERS", 100) as u32,
        batch_size: env_num("GUIDE_BATCH", 1) as u32,
        ..EngineOptions::default()
    };
    let start = std::time::Instant::now();
    let mut engine = Engine::new(space, nets, options).unwrap();
    while engine.step_parallel(threads).is_some() {}
    let seconds = start.elapsed().as_secs_f64();
    let outcome = engine.outcome();
    let (batches, discarded) = engine
        .reports()
        .iter()
        .fold((0u64, 0u64), |(b, d), report| {
            (
                b + u64::from(report.batches),
                d + u64::from(report.discarded),
            )
        });
    let mut hash = 0xCBF2_9CE4_8422_2325u64;
    let mut add = |word: u64| {
        for byte in word.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
        }
    };
    for (id, result) in outcome.routes.outcomes() {
        add(u64::from(id.0));
        if let crate::net::NetOutcome::Routed(route) = result {
            for edge in route.edges() {
                add(u64::from(edge.from.0) << 32 | u64::from(edge.to.0));
            }
        }
    }
    add(outcome.routes.stats.nodes_expanded);
    add(outcome.total_cost.get().to_bits());
    // What no number of threads could beat, given the sizes of the searches in each batch.
    let total: u64 = engine.batch_work.iter().map(|work| work.1).sum();
    let limit = |threads: u64| {
        let span: u64 = engine
            .batch_work
            .iter()
            .map(|&(most, sum)| most.max(sum / threads))
            .sum();
        total as f64 / span.max(1) as f64
    };
    println!(
        "RESULT problem={name} threads={threads} batch={} secs={seconds:.4} iters={} ok={} overuse={} cost={} expanded={} searches={} hash={hash:016x} batches={batches} discarded={discarded} rss={} limit4={:.2} limit8={:.2} limit16={:.2}",
        options.batch_size,
        outcome.routes.stats.iterations,
        u8::from(outcome.is_success()),
        outcome.overused.len(),
        outcome.total_cost,
        outcome.routes.stats.nodes_expanded,
        outcome.routes.stats.searches,
        peak_rss_mib(),
        limit(4),
        limit(8),
        limit(16),
    );
}

/// Measures one configuration, chosen by environment variables. See the module docs.
#[test]
#[ignore = "a measurement, for release builds"]
fn measure() {
    let problem = std::env::var("GUIDE_PROBLEM").unwrap_or_default();
    let parts: Vec<&str> = problem.split(':').collect();
    let num = |i: usize| parts[i].parse::<u32>().unwrap();
    match parts[0] {
        "dense" => {
            let (grid, nets) = dense_grid(num(1), num(2), u64::from(num(3)));
            measure_one(&problem, &grid, &nets);
        }
        "local" => {
            let (grid, nets) = local_grid(num(1), num(2), num(3), num(4), u64::from(num(5)));
            measure_one(&problem, &grid, &nets);
        }
        "fabric" => {
            let fabric = BenchFabric {
                tiles: num(1),
                tracks: num(2),
                pins: 8,
                output_flexibility: 8,
                switch_fanout: 6,
                input_flexibility: 2,
            };
            let graph = fabric.build(u64::from(num(1))).with_lower_bound(TileBound {
                positions: fabric.positions(),
            });
            let nets = fabric.nets(num(3), num(4), u64::from(num(5)));
            measure_one(&problem, &graph, &nets);
        }
        other => panic!("unknown problem {other:?}"),
    }
}

/// One problem of [`measure_quality`]: its family and its parameters.
#[derive(Clone, Copy)]
enum QualityProblem {
    /// Rows and seed, on 2 layers.
    Dense(u32, u32),
    /// Tiles along a side and seed, at one track, with three nets for every ten tiles.
    Fabric(u32, u32),
    /// Cells along a side, nets and seed, on 2 layers, with a reach of 16.
    Local(u32, u32, u32),
}

impl QualityProblem {
    fn family(self) -> &'static str {
        match self {
            QualityProblem::Dense(..) => "Dense",
            QualityProblem::Fabric(..) => "Fabrics",
            QualityProblem::Local(..) => "Short nets",
        }
    }
}

/// What [`measure_quality`] keeps of a run.
#[derive(Clone, Copy, Default)]
struct QualityRun {
    solved: bool,
    cost: f64,
    expanded: u64,
    iterations: u64,
}

fn quality_run<S: RoutingSpace>(space: &S, nets: &[Net<S::Node>], batch: u32) -> QualityRun {
    let options = EngineOptions {
        max_iterations: 100,
        ..with_batch(batch)
    };
    let outcome = route(space, nets, options).unwrap();
    QualityRun {
        solved: outcome.is_success(),
        cost: outcome.total_cost.get(),
        expanded: outcome.routes.stats.nodes_expanded,
        iterations: outcome.routes.stats.iterations,
    }
}

/// Compares batch sizes with a batch size of 1 on three families of problems, for the table in
/// the [engine's documentation](super#quality-and-convergence). Problems run at once, one to
/// each thread of the machine, and each on one thread, which gives the same results as any
/// number would.
#[test]
#[ignore = "a measurement, for release builds"]
fn measure_quality() {
    const BATCHES: [u32; 5] = [1, 8, 32, 64, 256];
    let mut problems = Vec::new();
    for rows in [32, 48, 64, 96] {
        problems.extend((1..=6).map(|seed| QualityProblem::Dense(rows, seed)));
    }
    for tiles in [24, 32, 40, 48] {
        problems.extend((1..=5).map(|seed| QualityProblem::Fabric(tiles, seed)));
    }
    for (size, nets) in [(256, 800), (512, 3000)] {
        problems.extend((1..=10).map(|seed| QualityProblem::Local(size, nets, seed)));
    }
    let jobs: Vec<(usize, u32)> = (0..problems.len())
        .flat_map(|problem| BATCHES.map(|batch| (problem, batch)))
        .collect();
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut runs: Vec<((usize, u32), QualityRun)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    while let Some(&(problem, batch)) =
                        jobs.get(next.fetch_add(1, Ordering::Relaxed))
                    {
                        let run = match problems[problem] {
                            QualityProblem::Dense(rows, seed) => {
                                let (grid, nets) = dense_grid(rows, 2, u64::from(seed));
                                quality_run(&grid, &nets, batch)
                            }
                            QualityProblem::Fabric(tiles, seed) => {
                                let fabric = small_fabric(tiles, 1);
                                let nets = fabric.nets(tiles * tiles * 3 / 10, 8, u64::from(seed));
                                quality_run(&fabric_graph(fabric), &nets, batch)
                            }
                            QualityProblem::Local(size, count, seed) => {
                                let (grid, nets) = local_grid(size, 2, count, 16, u64::from(seed));
                                quality_run(&grid, &nets, batch)
                            }
                        };
                        done.push(((problem, batch), run));
                    }
                    done
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect()
    });
    runs.sort_by_key(|&(key, _)| key);
    let run_of = |problem: usize, batch: u32| {
        runs.iter()
            .find(|&&(key, _)| key == (problem, batch))
            .map(|&(_, run)| run)
            .unwrap_or_default()
    };
    let mean = |values: &[f64]| values.iter().sum::<f64>() / values.len().max(1) as f64;
    println!("| Batch size | Family | Solved | Cost | Worst cost | Expansions | Iterations |");
    println!("| --- | --- | --- | --- | --- | --- | --- |");
    for batch in BATCHES {
        for family in ["Dense", "Fabrics", "Short nets"] {
            let mine: Vec<usize> = (0..problems.len())
                .filter(|&p| problems[p].family() == family)
                .collect();
            let solved = mine.iter().filter(|&&p| run_of(p, batch).solved).count();
            let both: Vec<usize> = mine
                .iter()
                .copied()
                .filter(|&p| run_of(p, batch).solved && run_of(p, 1).solved)
                .collect();
            let ratio = |p: usize, of: fn(QualityRun) -> f64| {
                of(run_of(p, batch)) / of(run_of(p, 1)).max(f64::MIN_POSITIVE)
            };
            let costs: Vec<f64> = both.iter().map(|&p| ratio(p, |run| run.cost)).collect();
            let worst = costs.iter().copied().fold(0.0, f64::max);
            let expansions: Vec<f64> = mine
                .iter()
                .map(|&p| ratio(p, |run| run.expanded as f64))
                .collect();
            let iterations: Vec<f64> = both
                .iter()
                .map(|&p| ratio(p, |run| run.iterations as f64))
                .collect();
            println!(
                "| {batch} | {family} | {solved} of {} | {:.3} | {worst:.3} | {:.2} | {:.2} |",
                mine.len(),
                mean(&costs),
                mean(&expansions),
                mean(&iterations),
            );
        }
    }
}

/// A net's terminals' bounding box, on tile or cell positions.
type Box2 = (u32, u32, u32, u32);

fn boxes_overlap(a: Box2, b: Box2) -> bool {
    a.0 <= b.1 && b.0 <= a.1 && a.2 <= b.3 && b.2 <= a.3
}

/// Passes everything to a space, and notes for each net what its searches looked at.
struct Recording<'a, S: RoutingSpace> {
    inner: &'a S,
    reads: RefCell<FastMap<u32, FastSet<S::Resource>>>,
    expansions: RefCell<FastMap<u32, u64>>,
}

impl<S: RoutingSpace> RoutingSpace for Recording<'_, S> {
    type Node = S::Node;
    type Resource = S::Resource;
    type Slots = S::Slots;
    type ResourceSlots = S::ResourceSlots;
    fn new_slots(&self) -> S::Slots {
        self.inner.new_slots()
    }
    fn new_resource_slots(&self) -> S::ResourceSlots {
        self.inner.new_resource_slots()
    }
    fn contains(&self, node: S::Node) -> bool {
        self.inner.contains(node)
    }
    fn expand(&self, net: NetId, from: S::Node, out: &mut Vec<Step<S::Node>>) {
        *self.expansions.borrow_mut().entry(net.0).or_default() += 1;
        self.inner.expand(net, from, out);
    }
    fn lower_bound(&self, net: NetId, from: S::Node, to: S::Node) -> Cost {
        self.inner.lower_bound(net, from, to)
    }
    fn resources(&self, net: NetId, from: S::Node, to: S::Node, out: &mut Vec<S::Resource>) {
        let start = out.len();
        self.inner.resources(net, from, to, out);
        self.reads
            .borrow_mut()
            .entry(net.0)
            .or_default()
            .extend(out[start..].iter().copied());
    }
    fn capacity(&self, resource: S::Resource) -> u32 {
        self.inner.capacity(resource)
    }
}

/// What region-based batches could do on a problem: how often a net's route and its searches
/// leave its terminals' box, and how much a greedy packing into batches of disjoint boxes
/// could speed things up at best, given what each net's searches really cost.
fn measure_regions_of<S>(
    name: &str,
    space: &S,
    nets: &[Net<NodeId>],
    position: impl Fn(NodeId) -> (u32, u32),
) where
    S: RoutingSpace<Node = NodeId, Resource = NodeId>,
{
    let recording = Recording {
        inner: space,
        reads: RefCell::new(FastMap::default()),
        expansions: RefCell::new(FastMap::default()),
    };
    let outcome = route(&recording, nets, EngineOptions::default()).unwrap();
    let reads = recording.reads.borrow();
    let expansions = recording.expansions.borrow();
    let boxes: Vec<Box2> = nets
        .iter()
        .map(|net| {
            let mut bounds = (u32::MAX, 0, u32::MAX, 0);
            for terminal in &net.terminals {
                for point in terminal.access_points() {
                    let (x, y) = position(point.node);
                    bounds = (
                        bounds.0.min(x),
                        bounds.1.max(x),
                        bounds.2.min(y),
                        bounds.3.max(y),
                    );
                }
            }
            bounds
        })
        .collect();
    let inside = |bounds: Box2, node: NodeId| {
        let (x, y) = position(node);
        bounds.0 <= x && x <= bounds.1 && bounds.2 <= y && y <= bounds.3
    };
    let empty = FastSet::default();
    let reads_of = |net: &Net<NodeId>| reads.get(&net.id.0).unwrap_or(&empty);
    let (mut routes_out, mut reads_out, mut reads_total) = (0u64, 0u64, 0u64);
    let mut route_resources = Vec::new();
    for (net, &bounds) in nets.iter().zip(&boxes) {
        let route = outcome.routes.route(net.id);
        let mut held = FastSet::default();
        if let Some(route) = route {
            if route.nodes().any(|node| !inside(bounds, node)) {
                routes_out += 1;
            }
            held.extend(route.nodes());
        }
        route_resources.push(held);
        for &read in reads_of(net) {
            reads_total += 1;
            reads_out += u64::from(!inside(bounds, read));
        }
    }

    // Packs nets in order into the first batch where their box touches nothing.
    let mut batches: Vec<(Vec<Box2>, Vec<usize>)> = Vec::new();
    for (index, &bounds) in boxes.iter().enumerate() {
        match batches
            .iter_mut()
            .find(|(placed, _)| !placed.iter().any(|&other| boxes_overlap(other, bounds)))
        {
            Some((placed, members)) => {
                placed.push(bounds);
                members.push(index);
            }
            None => batches.push((vec![bounds], vec![index])),
        }
    }
    let work = |index: usize| expansions.get(&nets[index].id.0).copied().unwrap_or(0);
    let total: u64 = (0..nets.len()).map(work).sum();
    let limit = |threads: u64| {
        let span: u64 = batches
            .iter()
            .map(|(_, members)| {
                let sum: u64 = members.iter().map(|&m| work(m)).sum();
                let most = members.iter().map(|&m| work(m)).max().unwrap_or(0);
                most.max(sum / threads)
            })
            .sum();
        total as f64 / span.max(1) as f64
    };

    // How often two nets in a batch "cannot interact" and yet one searched what the other
    // ended up holding, which is what the disjoint boxes were supposed to rule out.
    let (mut pairs, mut clashes) = (0u64, 0u64);
    for (_, members) in &batches {
        for (place, &a) in members.iter().enumerate().take(64) {
            for &b in members.iter().skip(place + 1).take(64) {
                pairs += 1;
                let looked_at = reads_of(&nets[a]);
                if route_resources[b]
                    .iter()
                    .any(|resource| looked_at.contains(resource))
                {
                    clashes += 1;
                }
            }
        }
    }

    println!(
        "REGIONS {name}: nets={} routes_outside_box={:.1}% reads_outside_box={:.1}% batches={} largest_batch={} speedup_limit_t4={:.2} t8={:.2} t16={:.2} batch_pairs={pairs} pairs_searching_the_others_route={:.1}%",
        nets.len(),
        100.0 * routes_out as f64 / nets.len().max(1) as f64,
        100.0 * reads_out as f64 / reads_total.max(1) as f64,
        batches.len(),
        batches
            .iter()
            .map(|(_, members)| members.len())
            .max()
            .unwrap_or(0),
        limit(4),
        limit(8),
        limit(16),
        100.0 * clashes as f64 / pairs.max(1) as f64,
    );
}

/// Measures whether region-based batches could work, on the problem in `GUIDE_PROBLEM`.
#[test]
#[ignore = "a measurement, for release builds"]
fn measure_regions() {
    let problem = std::env::var("GUIDE_PROBLEM").unwrap_or_default();
    let parts: Vec<&str> = problem.split(':').collect();
    let num = |i: usize| parts[i].parse::<u32>().unwrap();
    let on_grid = |grid: &Grid, nets: &[Net<NodeId>]| {
        measure_regions_of(&problem, grid, nets, |node| {
            let (x, y, _) = grid.position(node).unwrap();
            (x, y)
        });
    };
    match parts[0] {
        "dense" => {
            let (grid, nets) = dense_grid(num(1), num(2), u64::from(num(3)));
            on_grid(&grid, &nets);
        }
        "local" => {
            let (grid, nets) = local_grid(num(1), num(2), num(3), num(4), u64::from(num(5)));
            on_grid(&grid, &nets);
        }
        "fabric" => {
            let fabric = BenchFabric {
                tiles: num(1),
                tracks: num(2),
                pins: 8,
                output_flexibility: 8,
                switch_fanout: 6,
                input_flexibility: 2,
            };
            let positions = fabric.positions();
            let graph = fabric.build(u64::from(num(1))).with_lower_bound(TileBound {
                positions: positions.clone(),
            });
            let nets = fabric.nets(num(3), num(4), u64::from(num(5)));
            measure_regions_of(&problem, &graph, &nets, |node| {
                let (x, y) = positions[node.index()];
                (u32::from(x), u32::from(y))
            });
        }
        other => panic!("unknown problem {other:?}"),
    }
}
