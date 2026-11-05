//! Full runs of the negotiated-congestion engine, and the sweeps behind its default options.
//!
//! Every instance comes from a fixed [`Rng`] seed, so that the instances never vary. See
//! `docs/benchmarks.md` for what the instances are and what has been measured on them.
//!
//! ```text
//! cargo bench --bench engine                          # timings, by criterion
//! GUIDE_ENGINE_SWEEP=all cargo bench --bench engine   # everything below, with no criterion
//! GUIDE_ENGINE_SWEEP=options cargo bench --bench engine
//! GUIDE_ENGINE_SWEEP=refine cargo bench --bench engine
//! GUIDE_ENGINE_SWEEP=expansion cargo bench --bench engine
//! GUIDE_ENGINE_SWEEP=timing cargo bench --bench engine
//! ```
//!
//! Criterion's throughput is in nodes expanded, so its `elem/s` means expansions per second.
//!
//! Apart from the columns marked as seconds or nanoseconds, the sweeps print counts and
//! costs that the engine guarantees to be the same on every run and every machine, so they
//! can be compared exactly and do not suffer from a busy machine. Instances run on several
//! threads to save time, which changes no result, but makes the times rougher still.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, Throughput, criterion_group};
use guide::cost::Cost;
use guide::engine::{Connection, EngineOptions, Outcome, TimingDriven, route, route_timed};
use guide::id::{NetId, NodeId};
use guide::net::{Net, NetOutcome, Route, RouteSet, Terminal};
use guide::rng::Rng;
use guide::search::{Search, SearchLimits, SearchOutcome, Source, Target};
use guide::space::graph::{DelayBound, Graph, GraphBuilder, LowerBound, TimedGraph};
use guide::space::grid::Grid;
use guide::space::{DelaySpace, RoutingSpace, Step};
use guide::steiner::{SteinerOptions, kmb_tree, refine_tree, takahashi_matsuyama};
use guide::timing::{Constraints, TimingAnalysis, TimingGraphBuilder};
use guide::validate::check;

/// The environment variable that selects a sweep in place of the timings.
const SWEEP_VARIABLE: &str = "GUIDE_ENGINE_SWEEP";

/// How many instances a sweep runs at once, to leave room on a shared machine.
const MAX_THREADS: usize = 16;

/// A space with plain node numbers for nodes and resources, as all the ones here have.
trait NodeSpace: RoutingSpace<Node = NodeId, Resource = NodeId> {}
impl<S: RoutingSpace<Node = NodeId, Resource = NodeId>> NodeSpace for S {}

fn cost(value: f64) -> Cost {
    Cost::new(value).expect("costs here are finite and not negative")
}

fn options_with(change: impl FnOnce(&mut EngineOptions<'static>)) -> EngineOptions<'static> {
    let mut options = EngineOptions::default();
    change(&mut options);
    options
}

fn net(id: usize, nodes: impl IntoIterator<Item = NodeId>) -> Net<NodeId> {
    Net {
        id: NetId(id as u32),
        terminals: nodes.into_iter().map(Terminal::at).collect(),
    }
}

// Instances on grids.

/// A dense problem: every row's left end to a shuffled row's right end, on layer 0 of a square
/// grid, so that nets have to cross. This is `permutation_problem` of the engine's tests.
fn permutation_problem(size: u32, layers: u32, seed: u64) -> (Grid, Vec<Net<NodeId>>) {
    let grid = Grid::new(size, size, layers).unwrap();
    let mut rows: Vec<u32> = (0..size).collect();
    Rng::new(seed).shuffle(&mut rows);
    let at = |x, y| grid.node_at(x, y, 0).unwrap();
    let nets = rows
        .iter()
        .enumerate()
        .map(|(y, &to)| net(y, [at(0, y as u32), at(size - 1, to)]))
        .collect();
    (grid, nets)
}

/// Nets of a driver and 1 to `max_sinks` sinks, on layer 0 of a square grid, anywhere at all.
/// No two terminals share a cell, as nothing could route a problem where they did.
fn multi_terminal_problem(
    size: u32,
    layers: u32,
    nets: usize,
    max_sinks: u64,
    seed: u64,
) -> (Grid, Vec<Net<NodeId>>) {
    let grid = Grid::new(size, size, layers).unwrap();
    let mut rng = Rng::new(seed);
    let mut cells: Vec<NodeId> = (0..size * size)
        .map(|cell| grid.node_at(cell % size, cell / size, 0).unwrap())
        .collect();
    rng.shuffle(&mut cells);
    let mut cells = cells.into_iter();
    let nets = (0..nets)
        .map(|id| {
            let sinks = 1 + rng.below(max_sinks) as usize;
            net(id, cells.by_ref().take(1 + sinks))
        })
        .collect();
    (grid, nets)
}

/// Two-terminal nets between random cells of a square grid, at least `min` apart.
fn distant_pairs(grid: &Grid, size: u32, count: usize, min: u32, seed: u64) -> Vec<Net<NodeId>> {
    let mut rng = Rng::new(seed);
    let bound = u64::from(size);
    let mut nets = Vec::with_capacity(count);
    while nets.len() < count {
        let a = (rng.below(bound) as u32, rng.below(bound) as u32);
        let b = (rng.below(bound) as u32, rng.below(bound) as u32);
        if a.0.abs_diff(b.0) + a.1.abs_diff(b.1) >= min {
            let at = |(x, y)| grid.node_at(x, y, 0).unwrap();
            nets.push(net(nets.len(), [at(a), at(b)]));
        }
    }
    nets
}

// An island-style fabric, small enough to be congested by a handful of nets.

/// A square of tiles, each with one pin and some horizontal and vertical wires. Every edge
/// goes both ways and costs 1, and every node has a capacity of 1.
#[derive(Clone, Copy, Debug)]
struct Island {
    size: u32,
    tracks: u32,
}

/// The Manhattan distance between tiles, which is what crossing them costs at least.
struct IslandBound(Island);

impl LowerBound for IslandBound {
    #[inline]
    fn estimate(&self, from: NodeId, to: NodeId) -> Cost {
        let (size, per_tile) = (self.0.size, self.0.per_tile());
        let (a, b) = (from.0 / per_tile, to.0 / per_tile);
        Cost::from((a % size).abs_diff(b % size) + (a / size).abs_diff(b / size))
    }
}

impl Island {
    fn per_tile(self) -> u32 {
        2 * self.tracks + 1
    }

    fn pin(self, x: u32, y: u32) -> u32 {
        (x + self.size * y) * self.per_tile()
    }

    fn wire(self, x: u32, y: u32, vertical: bool, track: u32) -> u32 {
        self.pin(x, y) + 1 + track * 2 + u32::from(vertical)
    }

    fn build(self) -> Graph<IslandBound> {
        let mut builder = GraphBuilder::new(self.size * self.size * self.per_tile());
        let mut both = |a: u32, b: u32| {
            builder
                .add_edge(NodeId(a), NodeId(b), Cost::ONE)
                .and_then(|()| builder.add_edge(NodeId(b), NodeId(a), Cost::ONE))
                .unwrap();
        };
        for y in 0..self.size {
            for x in 0..self.size {
                for track in 0..self.tracks {
                    let horizontal = self.wire(x, y, false, track);
                    let vertical = self.wire(x, y, true, track);
                    both(self.pin(x, y), horizontal);
                    both(self.pin(x, y), vertical);
                    both(horizontal, vertical);
                    both(horizontal, self.wire(x, y, true, (track + 1) % self.tracks));
                    if x + 1 < self.size {
                        both(horizontal, self.wire(x + 1, y, false, track));
                    }
                    if y + 1 < self.size {
                        both(vertical, self.wire(x, y + 1, true, track));
                    }
                }
            }
        }
        builder.build().with_lower_bound(IslandBound(self))
    }

    /// Nets of a driver and 1 to `fanout` sinks on the pins of shuffled tiles, so that no pin
    /// is used twice unless there are more terminals than tiles.
    fn nets(self, count: usize, fanout: u64, seed: u64) -> Vec<Net<NodeId>> {
        let mut rng = Rng::new(seed);
        let mut tiles: Vec<u32> = (0..self.size * self.size).collect();
        rng.shuffle(&mut tiles);
        let mut next = 0;
        (0..count)
            .map(|id| {
                let terminals = 2 + rng.below(fanout) as usize;
                let pins = (next..next + terminals).map(|index| {
                    let tile = tiles[index % tiles.len()];
                    NodeId(self.pin(tile % self.size, tile / self.size))
                });
                let net = net(id, pins);
                next += terminals;
                net
            })
            .collect()
    }
}

// The FPGA-like fabric of `benches/search.rs`, copied because benches cannot share code without
// a module that cargo would also take for a bench. Keep the two the same.
// Its costs are small multiples of powers of two, so that no sum of them rounds.

/// Covering a tile costs at least this much, whichever kind of wire does it.
const COST_PER_TILE: f64 = 0.75;
/// How many tiles each kind of wire spans.
const WIRE_SPAN: [u32; 2] = [1, 4];
/// What entering each kind of wire costs per tile it spans.
const WIRE_COST_PER_TILE: [f64; 2] = [1.0, COST_PER_TILE];
/// What a switch costs, by switch type.
const SWITCH_COST: [f64; 3] = [0.125, 0.1875, 0.25];
/// The two nodes of each tile's logic site: a source and a sink.
const SITE_NODES: u32 = 2;
/// The directions wires run in, as tile offsets.
const DIRECTIONS: [(i64, i64); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// The shape of an FPGA-like routing resource graph: a source drives output pins, which drive
/// directional wires of two spans, which connect at their far ends to more wires and to input
/// pins, which drive a sink. Connections are random, as far as that shape allows.
#[derive(Clone, Copy, Debug)]
struct Fabric {
    /// The array's size along each axis.
    tiles: u32,
    /// Wires of each kind starting in each direction in each tile.
    tracks: u32,
    /// How many output pins each site has, and how many input pins.
    pins: u32,
    /// How many wires each output pin can drive.
    output_flexibility: u32,
    /// How many wires each wire can drive at its far end.
    switch_fanout: u32,
    /// How many input pins each wire can drive at its far end.
    input_flexibility: u32,
}

impl Fabric {
    fn wires_per_tile(self) -> u32 {
        self.tracks * WIRE_SPAN.len() as u32 * DIRECTIONS.len() as u32
    }

    fn nodes_per_tile(self) -> u32 {
        SITE_NODES + 2 * self.pins + self.wires_per_tile()
    }

    fn num_nodes(self) -> u32 {
        self.tiles * self.tiles * self.nodes_per_tile()
    }

    fn tile_base(self, x: u32, y: u32) -> u32 {
        (x + self.tiles * y) * self.nodes_per_tile()
    }

    fn source(self, x: u32, y: u32) -> NodeId {
        NodeId(self.tile_base(x, y))
    }

    fn sink(self, x: u32, y: u32) -> NodeId {
        NodeId(self.tile_base(x, y) + 1)
    }

    fn output_pin(self, x: u32, y: u32, pin: u32) -> NodeId {
        NodeId(self.tile_base(x, y) + SITE_NODES + pin)
    }

    fn input_pin(self, x: u32, y: u32, pin: u32) -> NodeId {
        NodeId(self.tile_base(x, y) + SITE_NODES + self.pins + pin)
    }

    /// The `index`th wire starting in a tile.
    fn wire(self, x: u32, y: u32, index: u32) -> NodeId {
        NodeId(self.tile_base(x, y) + SITE_NODES + 2 * self.pins + index)
    }

    /// Which entry of [`WIRE_SPAN`] the `index`th wire of a tile is.
    fn wire_kind(index: u32) -> usize {
        ((index / DIRECTIONS.len() as u32) as usize) % WIRE_SPAN.len()
    }

    /// Where the `index`th wire starting in a tile ends, and how many tiles it covers.
    fn wire_end(self, x: u32, y: u32, index: u32) -> (u32, u32, u32) {
        let direction = DIRECTIONS[index as usize % DIRECTIONS.len()];
        let span = i64::from(WIRE_SPAN[Self::wire_kind(index)]);
        let limit = i64::from(self.tiles) - 1;
        let end_x = (i64::from(x) + direction.0 * span).clamp(0, limit);
        let end_y = (i64::from(y) + direction.1 * span).clamp(0, limit);
        let covered = end_x.abs_diff(i64::from(x)) + end_y.abs_diff(i64::from(y));
        (end_x as u32, end_y as u32, covered as u32)
    }

    /// Every node's position: its tile, or for a wire the tile at its far end.
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

    /// Builds the graph, with its connections chosen at random as far as its shape allows.
    fn build(self, seed: u64) -> Graph<TileBound> {
        let mut rng = Rng::new(seed);
        let wires = u64::from(self.tiles * self.tiles * self.wires_per_tile());
        let edges = wires * u64::from(self.switch_fanout + self.input_flexibility);
        let mut builder = GraphBuilder::with_edge_capacity(self.num_nodes(), edges as usize);
        let wires_per_tile = u64::from(self.wires_per_tile());
        let switch_cost =
            |rng: &mut Rng| cost(SWITCH_COST[rng.below(SWITCH_COST.len() as u64) as usize]);

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
                    builder
                        .set_base_cost(wire, cost(per_tile * f64::from(covered)))
                        .unwrap();
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
        builder.build().with_lower_bound(TileBound {
            positions: self.positions(),
        })
    }

    /// A random tile within `reach` of an axis position.
    fn near(self, rng: &mut Rng, at: u32, reach: u32) -> u32 {
        let low = at.saturating_sub(reach);
        let high = at.saturating_add(reach).min(self.tiles - 1);
        low + rng.below(u64::from(high - low + 1)) as u32
    }

    /// Two-terminal nets spanning at most `reach` tiles along each axis, which may share
    /// sources and sinks, and so may be impossible to route. This is the same as the queries
    /// of `benches/search.rs`.
    fn crowded_nets(self, count: usize, reach: u32, seed: u64) -> Vec<Net<NodeId>> {
        let mut rng = Rng::new(seed);
        let tiles = u64::from(self.tiles);
        (0..count)
            .map(|id| {
                let (x, y) = (rng.below(tiles) as u32, rng.below(tiles) as u32);
                let (to_x, to_y) = (self.near(&mut rng, x, reach), self.near(&mut rng, y, reach));
                net(id, [self.source(x, y), self.sink(to_x, to_y)])
            })
            .collect()
    }

    /// Up to `count` nets of a source and 1 to `max_sinks` sinks within `reach` tiles along each
    /// axis. Each source and each sink is used once at most, since each has a capacity of 1.
    /// Sinks are looked for at random, and a net goes without any that a few tries do not find.
    /// Sinks run out on a small array, and a net left with none at all is dropped, so there can
    /// be well under `count` nets.
    fn nets(self, count: usize, max_sinks: u64, reach: u32, seed: u64) -> Vec<Net<NodeId>> {
        let mut rng = Rng::new(seed);
        let side = self.tiles as usize;
        let mut sources: Vec<u32> = (0..self.tiles * self.tiles).collect();
        rng.shuffle(&mut sources);
        let mut sink_used = vec![false; side * side];
        let mut nets = Vec::with_capacity(count);
        for &tile in sources.iter().take(count) {
            let (x, y) = (tile % self.tiles, tile / self.tiles);
            let mut terminals = vec![self.source(x, y)];
            for _ in 0..1 + rng.below(max_sinks) {
                for _try in 0..16 {
                    let (to_x, to_y) =
                        (self.near(&mut rng, x, reach), self.near(&mut rng, y, reach));
                    let used = &mut sink_used[to_x as usize + side * to_y as usize];
                    if !*used {
                        *used = true;
                        terminals.push(self.sink(to_x, to_y));
                        break;
                    }
                }
            }
            if terminals.len() > 1 {
                nets.push(net(nets.len(), terminals));
            }
        }
        nets
    }
}

/// The Manhattan distance times the cheapest cost of covering a tile, which is the simplest
/// form of a router's lookahead.
struct TileBound {
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

/// The parameters of `benches/search.rs`, used here for the run measuring time per expansion.
const SEARCH_BENCH_FABRIC: Fabric = Fabric {
    tiles: 128,
    tracks: 8,
    pins: 8,
    output_flexibility: 8,
    switch_fanout: 6,
    input_flexibility: 2,
};

/// A fabric with few wires, so that nets have something to fight over. With one less switch per
/// wire, the engine solved none of the sweep's instances.
const fn narrow_fabric(tiles: u32) -> Fabric {
    Fabric {
        tiles,
        tracks: 1,
        pins: 4,
        output_flexibility: 4,
        switch_fanout: 4,
        input_flexibility: 2,
    }
}

// Timings.

fn bench_run<S: NodeSpace>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    space: &S,
    nets: &[Net<NodeId>],
    options: EngineOptions<'static>,
) {
    let outcome = route(space, nets, options).unwrap();
    let stats = outcome.routes.stats;
    println!(
        "{name}: {} nets, success {}, {} iterations, {} expansions, total cost {}",
        nets.len(),
        outcome.is_success(),
        stats.iterations,
        stats.nodes_expanded,
        outcome.total_cost,
    );
    group.throughput(Throughput::Elements(stats.nodes_expanded));
    group.bench_function(name, |b| {
        b.iter(|| route(space, black_box(nets), options).unwrap());
    });
}

/// Seed 500 is the first seed of the sweep's dense instances.
fn dense_permutations(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine_dense");
    for size in [24, 32, 40, 64] {
        let (grid, nets) = permutation_problem(size, 2, 500);
        let name = format!("{size}x{size}x2");
        bench_run(&mut group, &name, &grid, &nets, EngineOptions::default());
    }
    group.finish();
}

fn multi_terminal_nets(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine_multi_terminal");
    for (size, count) in MULTI_TERMINAL_SIZES {
        let (grid, nets) = multi_terminal_problem(size, 2, count, 4, 700);
        let name = format!("{size}x{size}x2/{count}_nets");
        bench_run(&mut group, &name, &grid, &nets, EngineOptions::default());
    }
    group.finish();
}

fn fabrics(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine_fabric");
    let island = Island {
        size: 16,
        tracks: 5,
    };
    let (graph, nets) = (island.build(), island.nets(80, 3, 0));
    let options = EngineOptions::default();
    bench_run(&mut group, "island/16x16/5_tracks", &graph, &nets, options);

    for (tiles, count) in FPGA_LIKE_SIZES {
        let fabric = narrow_fabric(tiles);
        let (graph, nets) = (
            fabric.build(u64::from(tiles)),
            fabric.nets(count, 3, 6, 900),
        );
        let name = format!("fpga_like/{tiles}x{tiles}/up_to_{count}_nets");
        bench_run(&mut group, &name, &graph, &nets, options);
    }
    group.finish();
}

/// The FPGA-like fabrics again, timing-driven, which is the default bench's measure of what
/// timing costs in time. See the timing sweep for what it does to the routes.
fn timed_fabrics(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine_timing");
    for (tiles, count) in FPGA_LIKE_SIZES {
        let fabric = narrow_fabric(tiles);
        let graph = fabric.build(u64::from(tiles));
        let nets = fabric.nets(count, 3, 6, 900);
        let timing = fabric.timing(&nets);
        let instance = TimedInstance {
            group: 0,
            fabric,
            graph,
            nets,
            timing,
        };
        let space = instance.space();
        let options = EngineOptions::default();
        let outcome =
            route_timed(&space, &instance.nets, options, instance.timing.clone()).unwrap();
        let stats = outcome.routes.stats;
        let name = format!("fpga_like/{tiles}x{tiles}/up_to_{count}_nets");
        println!(
            "{name}: {} nets, success {}, {} iterations, {} expansions, total cost {}, critical path {:?} ps",
            instance.nets.len(),
            outcome.is_success(),
            stats.iterations,
            stats.nodes_expanded,
            outcome.total_cost,
            outcome
                .timing
                .and_then(|timing| timing.metrics.critical_path_delay),
        );
        group.throughput(Throughput::Elements(stats.nodes_expanded));
        group.bench_function(&name, |b| {
            b.iter(|| {
                let timing = instance.timing.clone();
                route_timed(&space, black_box(&instance.nets), options, timing).unwrap()
            });
        });
    }
    group.finish();
}

/// The cheapest of the runs that the expansion sweep times, for an eye on the time per
/// expansion in the default bench.
fn expansion_cost(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine_expansion_cost");
    let (grid, nets) = sparse_grid_problem(40, 300, 1);
    let options = options_with(|options| options.max_iterations = 4);
    bench_run(&mut group, "grid512/light", &grid, &nets, options);
    group.finish();
}

fn sparse_grid_problem(count: usize, min: u32, seed: u64) -> (Grid, Vec<Net<NodeId>>) {
    let grid = Grid::new(512, 512, 1).unwrap();
    let nets = distant_pairs(&grid, 512, count, min, seed);
    (grid, nets)
}

/// A full run can take a while, so this takes the fewest samples criterion allows.
fn configured() -> Criterion {
    Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_millis(200))
        .measurement_time(Duration::from_secs(2))
}

criterion_group! {
    name = benches;
    config = configured();
    targets = dense_permutations, multi_terminal_nets, fabrics, timed_fabrics, expansion_cost
}

// Sweep instances.

/// Grid size and net count of the multi-terminal instances.
const MULTI_TERMINAL_SIZES: [(u32, usize); 3] = [(32, 28), (48, 42), (64, 56)];
/// Array size and most nets of the FPGA-like instances. See [`Fabric::nets`] for why most.
const FPGA_LIKE_SIZES: [(u32, usize); 2] = [(16, 200), (32, 800)];
/// Instances per group of a family of grids.
const GRID_SEEDS: u64 = 24;
/// Instances per group of a family of fabrics.
const FABRIC_SEEDS: u64 = 6;

struct Instance<S> {
    group: usize,
    space: S,
    nets: Vec<Net<NodeId>>,
}

/// Instances that are reported together, in named groups of a similar size.
struct Family<S> {
    name: &'static str,
    groups: Vec<String>,
    instances: Vec<Instance<S>>,
}

/// The sizes and seeds of the sweep that the option docs in `src/engine.rs` refer to, starting
/// from seed 500. Any other first seed gives instances that took no part in the tuning.
fn dense_family(name: &'static str, first_seed: u64) -> Family<Grid> {
    let sizes = [24, 32, 40];
    let mut instances = Vec::new();
    for (group, &size) in sizes.iter().enumerate() {
        for seed in 0..GRID_SEEDS {
            let (space, nets) = permutation_problem(size, 2, first_seed + seed);
            instances.push(Instance { group, space, nets });
        }
    }
    Family {
        name,
        groups: sizes
            .iter()
            .map(|size| format!("{size}x{size}x2"))
            .collect(),
        instances,
    }
}

fn multi_terminal_family() -> Family<Grid> {
    let mut instances = Vec::new();
    for (group, &(size, count)) in MULTI_TERMINAL_SIZES.iter().enumerate() {
        for seed in 0..GRID_SEEDS {
            let (space, nets) = multi_terminal_problem(size, 2, count, 4, 700 + seed);
            instances.push(Instance { group, space, nets });
        }
    }
    Family {
        name: "multi-terminal grid",
        groups: MULTI_TERMINAL_SIZES
            .iter()
            .map(|(size, count)| format!("{size}x{size}x2/{count}"))
            .collect(),
        instances,
    }
}

fn island_family(name: &'static str, tracks: &[u32]) -> Family<Graph<IslandBound>> {
    let mut instances = Vec::new();
    for (group, &tracks) in tracks.iter().enumerate() {
        let island = Island { size: 16, tracks };
        for seed in 0..FABRIC_SEEDS {
            instances.push(Instance {
                group,
                space: island.build(),
                nets: island.nets(80, 3, seed),
            });
        }
    }
    Family {
        name,
        groups: tracks.iter().map(|tracks| format!("{tracks}trk")).collect(),
        instances,
    }
}

fn fpga_like_family() -> Family<Graph<TileBound>> {
    let mut instances = Vec::new();
    for (group, &(tiles, count)) in FPGA_LIKE_SIZES.iter().enumerate() {
        let fabric = narrow_fabric(tiles);
        for seed in 0..FABRIC_SEEDS {
            instances.push(Instance {
                group,
                space: fabric.build(u64::from(tiles) + seed),
                nets: fabric.nets(count, 3, 6, 900 + seed),
            });
        }
    }
    Family {
        name: "FPGA-like fabric",
        groups: FPGA_LIKE_SIZES
            .iter()
            .map(|(tiles, count)| format!("{tiles}x{tiles}/up to {count}"))
            .collect(),
        instances,
    }
}

/// Runs `work` on each item, on several threads, returning the results in the items' order.
fn par_map<T: Sync, U: Send>(items: &[T], work: impl Fn(&T) -> U + Sync) -> Vec<U> {
    let next = AtomicUsize::new(0);
    let threads = thread::available_parallelism()
        .map_or(1, |count| count.get())
        .min(MAX_THREADS);
    let mut done: Vec<(usize, U)> = thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(index) else {
                            return out;
                        };
                        out.push((index, work(item)));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect()
    });
    done.sort_unstable_by_key(|&(index, _)| index);
    done.into_iter().map(|(_, result)| result).collect()
}

// The option sweep.

/// One option, and the values to try it at, one of which is the default.
struct OptionTable {
    option: &'static str,
    rows: Vec<(String, EngineOptions<'static>)>,
    default_row: usize,
}

impl OptionTable {
    /// Adds a row that changes more than the one option.
    fn add_row(&mut self, label: &str, options: EngineOptions<'static>) {
        self.rows.push((label.to_owned(), options));
    }
}

/// Builds a table from values of an option, marking the default.
fn table<T: Copy + PartialEq + std::fmt::Debug>(
    option: &'static str,
    values: &[T],
    default: T,
    set: impl Fn(&mut EngineOptions<'static>, T),
) -> OptionTable {
    OptionTable {
        option,
        rows: values
            .iter()
            .map(|&value| {
                (
                    format!("{value:?}"),
                    options_with(|options| set(options, value)),
                )
            })
            .collect(),
        default_row: values
            .iter()
            .position(|&value| value == default)
            .expect("the default is among the values"),
    }
}

fn option_tables() -> Vec<OptionTable> {
    // Some claims in the option docs were first measured with these settings and not with
    // today's defaults, so these rows show whether that makes the difference.
    let first_zero = |growth: f64| {
        options_with(|options| {
            options.first_present = Cost::ZERO;
            options.present_growth = cost(growth);
        })
    };
    let cap_200 = |change: fn(&mut EngineOptions<'static>)| {
        options_with(|options| {
            options.max_iterations = 200;
            change(options);
        })
    };

    let mut present_growth = table(
        "present_growth",
        &[1.1, 1.3, 1.5, 1.8, 2.2, 3.0],
        1.8,
        |options, value| options.present_growth = cost(value),
    );
    present_growth.add_row("1.8, first_present 0", first_zero(1.8));
    present_growth.add_row("1.3, first_present 0", first_zero(1.3));

    let mut reroute_all = table("reroute_all", &[false, true], false, |options, value| {
        options.reroute_all = value;
    });
    reroute_all.add_row("false, cap 200", cap_200(|_| ()));
    reroute_all.add_row(
        "true, cap 200",
        cap_200(|options| options.reroute_all = true),
    );

    let mut shuffle_seed = table(
        "shuffle_seed",
        &[None, Some(9), Some(1), Some(2), Some(3)],
        None,
        |options, value| options.shuffle_seed = value,
    );
    shuffle_seed.add_row("None, cap 200", cap_200(|_| ()));
    shuffle_seed.add_row(
        "Some(9), cap 200",
        cap_200(|options| options.shuffle_seed = Some(9)),
    );

    vec![
        iteration_table(),
        table(
            "first_present",
            &[0.0, 0.1, 0.25, 0.5, 1.0, 2.0],
            0.5,
            |options, value| options.first_present = cost(value),
        ),
        table(
            "initial_present",
            &[0.1, 0.25, 0.5, 1.0, 2.0],
            0.5,
            |options, value| options.initial_present = cost(value),
        ),
        present_growth,
        table(
            "history_factor",
            &[0.0, 0.2, 0.5, 1.0, 2.0, 5.0],
            1.0,
            |options, value| options.history_factor = cost(value),
        ),
        reroute_all,
        shuffle_seed,
    ]
}

fn iteration_table() -> OptionTable {
    table(
        "max_iterations",
        &[25, 50, 100, 200, 400],
        100,
        |options, value| options.max_iterations = value,
    )
}

/// What one run of the engine came to.
#[derive(Clone, Copy)]
struct Run {
    solved: bool,
    iterations: u64,
    expansions: u64,
    cost: f64,
    /// The sum over resources of how far each is beyond its capacity at the end.
    overuse: u64,
    seconds: f64,
}

fn run_engine<S: NodeSpace>(instance: &Instance<S>, options: EngineOptions<'static>) -> Run {
    let start = Instant::now();
    let outcome = route(&instance.space, &instance.nets, options).unwrap();
    let seconds = start.elapsed().as_secs_f64();
    Run {
        solved: outcome.is_success(),
        iterations: outcome.routes.stats.iterations,
        expansions: outcome.routes.stats.nodes_expanded,
        cost: outcome.total_cost.get(),
        overuse: outcome
            .overused
            .iter()
            .map(|entry| u64::from(entry.occupancy.saturating_sub(entry.capacity)))
            .sum(),
        seconds,
    }
}

/// `a / b` in three decimals, or a dash if that makes no sense.
fn ratio(a: f64, b: f64) -> String {
    if b > 0.0 {
        format!("{:.3}", a / b)
    } else {
        "-".to_owned()
    }
}

/// Prints one option's table for one family.
fn print_option_table<S: NodeSpace + Sync>(family: &Family<S>, table: &OptionTable) {
    let all: Vec<Vec<Run>> = table
        .rows
        .iter()
        .map(|&(_, options)| par_map(&family.instances, |instance| run_engine(instance, options)))
        .collect();
    let base = &all[table.default_row];
    let total_expansions = |runs: &[Run], group: Option<usize>| -> u64 {
        runs.iter()
            .zip(&family.instances)
            .filter(|(_, instance)| group.is_none_or(|group| instance.group == group))
            .map(|(run, _)| run.expansions)
            .sum()
    };

    println!(
        "\n### `{}` on {} ({} instances; groups {})\n",
        table.option,
        family.name,
        family.instances.len(),
        family.groups.join(", "),
    );
    println!(
        "| Value | Solved | By group | Mean iterations, solved | Iterations, all | Expansions | \
         Expansions / default | By group | Cost, solved | Cost / default, both solved | \
         Both solved | Overuse left | Seconds |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    for (index, ((label, _), runs)) in table.rows.iter().zip(&all).enumerate() {
        let solved: Vec<&Run> = runs.iter().filter(|run| run.solved).collect();
        let by_group: Vec<String> = (0..family.groups.len())
            .map(|group| {
                let solved = runs
                    .iter()
                    .zip(&family.instances)
                    .filter(|(run, instance)| run.solved && instance.group == group)
                    .count();
                solved.to_string()
            })
            .collect();
        let expansions_by_group: Vec<String> = (0..family.groups.len())
            .map(|group| {
                ratio(
                    total_expansions(runs, Some(group)) as f64,
                    total_expansions(base, Some(group)) as f64,
                )
            })
            .collect();
        let both: Vec<(&Run, &Run)> = runs
            .iter()
            .zip(base)
            .filter(|(run, base)| run.solved && base.solved)
            .collect();
        let iterations: u64 = solved.iter().map(|run| run.iterations).sum();
        println!(
            "| {label}{} | {}/{} | {} | {:.1} | {} | {} | {} | {} | {:.1} | {} | {} | {} | {:.2} |",
            if index == table.default_row {
                " (default)"
            } else {
                ""
            },
            solved.len(),
            runs.len(),
            by_group.join("+"),
            iterations as f64 / solved.len().max(1) as f64,
            runs.iter().map(|run| run.iterations).sum::<u64>(),
            total_expansions(runs, None),
            ratio(
                total_expansions(runs, None) as f64,
                total_expansions(base, None) as f64
            ),
            expansions_by_group.join(" "),
            // An empty sum of floats is -0, which looks odd in a table.
            solved.iter().map(|run| run.cost).sum::<f64>() + 0.0,
            ratio(
                both.iter().map(|(run, _)| run.cost).sum(),
                both.iter().map(|(_, base)| base.cost).sum(),
            ),
            both.len(),
            runs.iter().map(|run| run.overuse).sum::<u64>(),
            runs.iter().map(|run| run.seconds).sum::<f64>(),
        );
    }
}

fn print_option_tables<S: NodeSpace + Sync>(family: &Family<S>) {
    for table in option_tables() {
        print_option_table(family, &table);
    }
}

fn option_sweep() {
    println!("\n## Option sweep\n\nOne option at a time varied from the defaults.");
    print_option_tables(&dense_family("dense grids, tuning seeds", 500));
    print_option_tables(&dense_family("dense grids, other seeds", 1000));
    print_option_tables(&multi_terminal_family());
    print_option_tables(&island_family("island fabric", &[5, 6, 8]));
    print_option_tables(&fpga_like_family());
    // What giving up costs, so only the option that sets it.
    print_option_table(
        &island_family("tight island fabric", &[4]),
        &iteration_table(),
    );
}

// Refinement after negotiation.

/// Hides steps into any resource that other nets fill up, so that a tree refined in this space
/// cannot create overuse. Steps only ever go away, so the lower bound stays admissible.
struct Residual<'a, S> {
    space: &'a S,
    /// How many nets hold each resource. A net has to be taken out of this to refine it.
    occupancy: RefCell<HashMap<NodeId, u32>>,
    buffer: RefCell<Vec<NodeId>>,
}

impl<S: NodeSpace> Residual<'_, S> {
    /// Adds `delta` to the occupancy of every resource of a route.
    fn occupy(&self, net: NetId, route: &Route<NodeId>, delta: i32) {
        let mut occupancy = self.occupancy.borrow_mut();
        for resource in route_resources(self.space, net, route) {
            let held = occupancy.entry(resource).or_insert(0);
            *held = held.checked_add_signed(delta).expect("nets balance out");
        }
    }

    fn has_room(&self, net: NetId, from: NodeId, to: NodeId) -> bool {
        let mut buffer = self.buffer.borrow_mut();
        buffer.clear();
        self.space.resources(net, from, to, &mut buffer);
        let occupancy = self.occupancy.borrow();
        buffer.iter().all(|&resource| {
            occupancy.get(&resource).copied().unwrap_or(0) < self.space.capacity(resource)
        })
    }
}

impl<S: NodeSpace> RoutingSpace for Residual<'_, S> {
    type Node = NodeId;
    type Resource = NodeId;
    type Slots = S::Slots;
    type ResourceSlots = S::ResourceSlots;

    fn new_slots(&self) -> S::Slots {
        self.space.new_slots()
    }

    fn new_resource_slots(&self) -> S::ResourceSlots {
        self.space.new_resource_slots()
    }

    fn contains(&self, node: NodeId) -> bool {
        self.space.contains(node)
    }

    fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
        let start = out.len();
        self.space.expand(net, from, out);
        let mut index = 0;
        out.retain(|step| {
            index += 1;
            index <= start || self.has_room(net, from, step.to)
        });
    }

    fn lower_bound(&self, net: NetId, from: NodeId, to: NodeId) -> Cost {
        self.space.lower_bound(net, from, to)
    }

    fn resources(&self, net: NetId, from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
        self.space.resources(net, from, to, out);
    }

    fn capacity(&self, resource: NodeId) -> u32 {
        self.space.capacity(resource)
    }
}

/// The distinct resources a route holds, by the rule in the engine's docs: those of its root
/// and those of each edge.
fn route_resources<S: NodeSpace>(space: &S, net: NetId, route: &Route<NodeId>) -> Vec<NodeId> {
    let mut resources = Vec::new();
    if let Some(root) = route.root() {
        space.resources(net, root, root, &mut resources);
    }
    for edge in route.edges() {
        space.resources(net, edge.from, edge.to, &mut resources);
    }
    resources.sort_unstable();
    resources.dedup();
    resources
}

/// What refining a solved instance came to, by way of [`check`], which shares no code with the
/// engine. Costs are those of the plain space, with no congestion in them.
#[derive(Clone, Copy, Default)]
struct Refined {
    nets: usize,
    negotiated: f64,
    engine_expansions: u64,
    /// What each net costs by growth, with no other net in the way.
    alone_growth: f64,
    /// What each net costs by [`kmb_tree`], with no other net in the way.
    alone_kmb: f64,
    /// Every net refined with no regard for the others.
    blind_cost: f64,
    blind_legal: bool,
    blind_overused: usize,
    blind_expansions: u64,
    /// Every net refined in turn, around where the others are by then.
    residual_cost: f64,
    residual_legal: bool,
    residual_improved: usize,
    residual_expansions: u64,
}

fn refine_instance<S: NodeSpace>(instance: &Instance<S>) -> Option<Refined> {
    let (space, nets) = (&instance.space, instance.nets.as_slice());
    let outcome: Outcome<NodeId, NodeId> = route(space, nets, EngineOptions::default()).unwrap();
    let routes = outcome.legal_routes()?;
    let report = check(space, nets, routes);
    assert!(report.is_complete(), "{:?}", report.faults());
    let negotiated = report.summary().total_cost.get();
    // Summed in the same order, so these are the same unless one of them is wrong.
    assert_eq!(negotiated, outcome.total_cost.get());

    let options = SteinerOptions::default();
    let mut result = Refined {
        nets: nets.len(),
        negotiated,
        engine_expansions: routes.stats.nodes_expanded,
        ..Refined::default()
    };

    let mut search = Search::new(space);
    let mut blind = RouteSet::new();
    for net in nets {
        let route = routes.route(net.id).expect("the outcome is complete");
        let grown = takahashi_matsuyama(&mut search, space, net).unwrap();
        result.alone_growth += grown.cost.get();
        result.alone_kmb += kmb_tree(&mut search, space, net, &options)
            .unwrap()
            .cost
            .get();
        let tree = refine_tree(&mut search, space, net, route, &options).unwrap();
        result.blind_expansions += tree.nodes_expanded;
        blind.set_route(net.id, tree.route);
    }
    let report = check(space, nets, &blind);
    result.blind_cost = report.summary().total_cost.get();
    result.blind_legal = report.is_complete();
    result.blind_overused = report.summary().resources_overused;

    let residual = Residual {
        space,
        occupancy: RefCell::new(HashMap::new()),
        buffer: RefCell::new(Vec::new()),
    };
    for net in nets {
        let route = routes.route(net.id).expect("the outcome is complete");
        residual.occupy(net.id, route, 1);
    }
    let mut search = Search::new(&residual);
    let mut refined = RouteSet::new();
    for net in nets {
        let route = routes.route(net.id).expect("the outcome is complete");
        residual.occupy(net.id, route, -1);
        let before = route_cost(space, net, route);
        let tree = refine_tree(&mut search, &residual, net, route, &options).unwrap();
        residual.occupy(net.id, &tree.route, 1);
        result.residual_expansions += tree.nodes_expanded;
        result.residual_improved += usize::from(tree.cost.get() < before);
        refined.set_route(net.id, tree.route);
    }
    let report = check(space, nets, &refined);
    result.residual_cost = report.summary().total_cost.get();
    result.residual_legal = report.is_complete();
    Some(result)
}

/// The cost of one net's route, as the validator sees it on its own.
fn route_cost<S: NodeSpace>(space: &S, net: &Net<NodeId>, route: &Route<NodeId>) -> f64 {
    let mut set = RouteSet::new();
    set.set_route(net.id, route.clone());
    let report = check(space, std::slice::from_ref(net), &set);
    report.summary().total_cost.get()
}

fn print_refinement<S: NodeSpace + Sync>(family: &Family<S>) {
    let start = Instant::now();
    let runs = par_map(&family.instances, refine_instance);
    let seconds = start.elapsed().as_secs_f64();
    let solved: Vec<Refined> = runs.into_iter().flatten().collect();
    let sum = |field: fn(&Refined) -> f64| -> f64 { solved.iter().map(field).sum() };
    let count = |field: fn(&Refined) -> bool| solved.iter().filter(|run| field(run)).count();
    let negotiated = sum(|run| run.negotiated);
    // An empty sum of floats is -0, which looks odd in a table.
    let negotiated = negotiated + 0.0;
    let alone = sum(|run| run.alone_growth);
    let residual = sum(|run| run.residual_cost);
    let blind_overused: usize = solved.iter().map(|run| run.blind_overused).sum();
    println!(
        "| {} | {}/{} | {} | {negotiated:.1} | {} | {} | {} | {}/{} | {:.1} | {} | {}/{} | {}/{} | {} | {} | {} | {seconds:.1} |",
        family.name,
        solved.len(),
        family.instances.len(),
        solved.iter().map(|run| run.nets).sum::<usize>(),
        ratio(alone, negotiated),
        ratio(sum(|run| run.alone_kmb), negotiated),
        ratio(sum(|run| run.blind_cost), negotiated),
        count(|run| run.blind_legal),
        solved.len(),
        blind_overused as f64 / solved.len().max(1) as f64,
        ratio(residual, negotiated),
        count(|run| run.residual_legal),
        solved.len(),
        solved
            .iter()
            .map(|run| run.residual_improved)
            .sum::<usize>(),
        solved.iter().map(|run| run.nets).sum::<usize>(),
        ratio(negotiated - residual, negotiated - alone),
        ratio(
            sum(|run| run.blind_expansions as f64),
            sum(|run| run.engine_expansions as f64)
        ),
        ratio(
            sum(|run| run.residual_expansions as f64),
            sum(|run| run.engine_expansions as f64)
        ),
    );
}

fn refinement_sweep() {
    println!(
        "\n## Refinement after negotiation\n\n\
         Default options, solved instances only. Costs are divided by the negotiated cost. \
         \"Blind\" refines every net in the plain space, as if it were alone. \"Residual\" \
         refines each net in turn, with steps into resources that others fill taken away.\n"
    );
    println!(
        "| Family | Solved | Nets | Negotiated cost | Alone, growth | Alone, KMB | Blind | \
         Blind legal | Blind overused per instance | Residual | Residual legal | Nets improved | \
         Share of gap to alone recovered | Blind expansions / engine | \
         Residual expansions / engine | Seconds |"
    );
    println!(
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    );
    print_refinement(&dense_family("dense grids, tuning seeds", 500));
    print_refinement(&dense_family("dense grids, other seeds", 1000));
    print_refinement(&multi_terminal_family());
    print_refinement(&island_family("island fabric", &[5, 6, 8]));
    print_refinement(&fpga_like_family());
}

// Time per expansion.

/// FNV-1a over a result's routes, overuse and cost, to tell at a glance that two runs agree.
fn digest(outcome: &Outcome<NodeId, NodeId>) -> u64 {
    let mut hash = 0xCBF2_9CE4_8422_2325_u64;
    let mut add = |word: u64| {
        hash ^= word;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    };
    for (id, result) in outcome.routes.outcomes() {
        add(u64::from(id.0));
        if let NetOutcome::Routed(route) = result {
            for edge in route.edges() {
                add(u64::from(edge.from.0));
                add(u64::from(edge.to.0));
            }
        }
    }
    add(outcome.overused.len() as u64);
    add(outcome.total_cost.get().to_bits());
    hash
}

/// Times whole runs, one at a time, and prints the time per expansion.
fn print_expansion_cost<S: NodeSpace>(
    name: &str,
    space: &S,
    nets: &[Net<NodeId>],
    max_iterations: u32,
    repeats: u32,
) {
    let options = options_with(|options| options.max_iterations = max_iterations);
    let mut times = Vec::new();
    let mut last = None;
    for _ in 0..repeats {
        let start = Instant::now();
        let outcome = route(space, black_box(nets), options).unwrap();
        times.push(start.elapsed().as_secs_f64());
        last = Some(outcome);
    }
    let outcome = last.expect("at least one run");
    let expansions = outcome.routes.stats.nodes_expanded;
    times.sort_by(f64::total_cmp);
    let per_expansion = |seconds: f64| seconds * 1e9 / expansions as f64;
    println!(
        "| {name} | {} | {} | {} | {expansions} | {:016x} | {repeats} | {:.0} | {:.0} |",
        nets.len(),
        outcome.routes.stats.iterations,
        outcome.overused.len(),
        digest(&outcome),
        per_expansion(times[0]),
        per_expansion(times[times.len() / 2]),
    );
}

/// These runs stop early, with conflicts left, as they are only here to time expansions while
/// the resource table fills up. "Heavy" is about 20 and "fabric" about 60 seconds per run.
fn expansion_sweep() {
    println!(
        "\n## Time per expansion\n\nThese run one at a time, and stop at a low iteration cap.\n"
    );
    println!(
        "| Run | Nets | Iterations | Overused | Expansions | Digest | Repeats | Fastest, ns | Median, ns |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    let (grid, nets) = sparse_grid_problem(40, 300, 1);
    print_expansion_cost("grid512/light", &grid, &nets, 4, 12);
    let (grid, nets) = sparse_grid_problem(1000, 150, 2);
    print_expansion_cost("grid512/heavy", &grid, &nets, 2, 3);
    let graph = SEARCH_BENCH_FABRIC.build(128);
    let nets = SEARCH_BENCH_FABRIC.crowded_nets(4000, 16, 9);
    print_expansion_cost("fabric128", &graph, &nets, 6, 3);
}

// Timing-driven routing.

/// What entering a pin takes, in picoseconds.
const PIN_DELAY: u32 = 40;
/// What a switch onto a wire takes.
const SWITCH_DELAY: u32 = 70;
/// What a wire takes per tile, by kind, as in [`WIRE_SPAN`]. Long wires are faster per tile and
/// cheaper too, so delay and cost agree on which wires are good, but not on by how much.
const WIRE_DELAY_PER_TILE: [u32; 2] = [60, 30];
/// The fastest a tile can be covered, by a long wire: `(70 + 4 * 30) / 4`.
const FASTEST_PER_TILE: f64 = 47.5;
/// What a picosecond costs: one tile's worth of cost for one tile's worth of delay, about.
const DELAY_COST: f64 = 1.0 / 50.0;

impl Fabric {
    /// The delay of a step, by what it enters: a pin, or a wire with its switch.
    fn delay(self, to: NodeId) -> u32 {
        let per_tile = self.nodes_per_tile();
        let (tile, offset) = (to.0 / per_tile, to.0 % per_tile);
        let (x, y) = (tile % self.tiles, tile / self.tiles);
        if offset < SITE_NODES {
            0
        } else if offset < SITE_NODES + 2 * self.pins {
            PIN_DELAY
        } else {
            let index = offset - SITE_NODES - 2 * self.pins;
            let covered = self.wire_end(x, y, index).2;
            SWITCH_DELAY + WIRE_DELAY_PER_TILE[Self::wire_kind(index)] * covered
        }
    }

    /// The tile of a node: where its site is, for a site's nodes.
    fn tile_of(self, node: NodeId) -> usize {
        (node.0 / self.nodes_per_tile()) as usize
    }

    /// A netlist for these nets. A tile is a register one time in four, and otherwise a cell
    /// of 200 ps from the sink it is driven through to the source it drives. A pin that nothing
    /// reaches launches, and one that reaches nothing captures. The clock period is 0, so the
    /// fastest circuit is wanted. Net arcs start at an estimate of 50 ps per tile.
    fn timing(self, nets: &[Net<NodeId>]) -> TimingDriven {
        let tiles = (self.tiles * self.tiles) as usize;
        let mut drives = vec![false; tiles];
        let mut driven = vec![false; tiles];
        for net in nets {
            drives[self.tile_of(net.terminals[0].access_points()[0].node)] = true;
            for terminal in &net.terminals[1..] {
                driven[self.tile_of(terminal.access_points()[0].node)] = true;
            }
        }
        let mut builder = TimingGraphBuilder::new();
        let mut pins = Vec::with_capacity(tiles);
        for tile in 0..tiles {
            if tile % 4 == 0 {
                let register = builder.add_register(60, 40).unwrap();
                pins.push((register.q, register.d));
            } else {
                let (input, output) = (builder.add_point().unwrap(), builder.add_point().unwrap());
                builder.add_cell_arc(input, output, 200).unwrap();
                if !driven[tile] {
                    builder.set_launch(input, 0).unwrap();
                }
                if !drives[tile] {
                    builder.set_capture(output, 0).unwrap();
                }
                pins.push((output, input));
            }
        }
        let position = |node: NodeId| {
            let tile = self.tile_of(node) as u32;
            (tile % self.tiles, tile / self.tiles)
        };
        let mut connections = Vec::new();
        for net in nets {
            let driver = net.terminals[0].access_points()[0].node;
            for (terminal, sink) in net.terminals.iter().enumerate().skip(1) {
                let sink = sink.access_points()[0].node;
                let ((a, b), (c, d)) = (position(driver), position(sink));
                let estimate = 50 * (a.abs_diff(c) + b.abs_diff(d));
                let arc = builder
                    .add_net_arc(
                        pins[self.tile_of(driver)].0,
                        pins[self.tile_of(sink)].1,
                        estimate,
                    )
                    .unwrap();
                connections.push(Connection {
                    net: net.id,
                    terminal: terminal as u32,
                    arc,
                });
            }
        }
        TimingDriven::new(
            builder.build(),
            Constraints::single_clock(0).unwrap(),
            connections,
            cost(DELAY_COST),
        )
    }
}

/// The tile distance times the fastest delay of a tile, from the same positions as
/// [`TileBound`].
struct TileDelayBound {
    positions: Vec<(u16, u16)>,
}

impl DelayBound for TileDelayBound {
    fn estimate(&self, from: NodeId, to: NodeId) -> u32 {
        let (a, b) = (self.positions[from.index()], self.positions[to.index()]);
        let distance = u32::from(a.0.abs_diff(b.0)) + u32::from(a.1.abs_diff(b.1));
        (f64::from(distance) * FASTEST_PER_TILE).floor() as u32
    }
}

/// A space whose steps cost their delays, with no bound, for the fastest each connection could
/// be on its own.
struct DelaysAsCosts<'a, S>(&'a S);

impl<S: DelaySpace> RoutingSpace for DelaysAsCosts<'_, S> {
    type Node = S::Node;
    type Resource = S::Resource;
    type Slots = S::Slots;
    type ResourceSlots = S::ResourceSlots;

    fn new_slots(&self) -> S::Slots {
        self.0.new_slots()
    }
    fn new_resource_slots(&self) -> S::ResourceSlots {
        self.0.new_resource_slots()
    }
    fn contains(&self, node: S::Node) -> bool {
        self.0.contains(node)
    }
    fn expand(&self, net: NetId, from: S::Node, out: &mut Vec<Step<S::Node>>) {
        let start = out.len();
        self.0.expand(net, from, out);
        let mut delays = Vec::new();
        self.0.step_delays(net, from, &mut delays);
        for (step, delay) in out[start..].iter_mut().zip(delays) {
            step.cost = Cost::from(delay);
        }
    }
    fn lower_bound(&self, _net: NetId, _from: S::Node, _to: S::Node) -> Cost {
        Cost::ZERO
    }
    fn resources(&self, net: NetId, from: S::Node, to: S::Node, out: &mut Vec<S::Resource>) {
        self.0.resources(net, from, to, out);
    }
    fn capacity(&self, resource: S::Resource) -> u32 {
        self.0.capacity(resource)
    }
}

/// Analyses the routes from nothing, walking each one for its delays, and returns the
/// critical-path delay.
fn critical_path_of<S: DelaySpace<Node = NodeId>>(
    space: &S,
    nets: &[Net<NodeId>],
    routes: &RouteSet<NodeId>,
    timing: &TimingDriven,
) -> u64 {
    let mut graph = timing.graph.clone();
    for connection in &timing.connections {
        let net = &nets[connection.net.0 as usize];
        let Some(route) = routes.route(net.id) else {
            continue;
        };
        let mut arrival = HashMap::new();
        if let Some(root) = route.root() {
            arrival.insert(root, 0u32);
        }
        for edge in route.edges() {
            let (mut steps, mut delays) = (Vec::new(), Vec::new());
            space.expand(net.id, edge.from, &mut steps);
            space.step_delays(net.id, edge.from, &mut delays);
            let position = steps.iter().position(|step| step.to == edge.to).unwrap();
            arrival.insert(edge.to, arrival[&edge.from] + delays[position]);
        }
        let sink = net.terminals[connection.terminal as usize].access_points()[0].node;
        graph.set_delay(connection.arc, arrival[&sink]).unwrap();
    }
    let mut analysis = TimingAnalysis::new(&graph);
    analysis.run(&graph, &timing.constraints);
    analysis.critical_path().map_or(0, |path| path.arrival())
}

/// The critical path if every connection had the fastest route there is, on its own. No tree
/// can beat this for any sink, so it is a floor under what routing can reach.
fn alone_critical_path<S: DelaySpace<Node = NodeId>>(
    space: &S,
    nets: &[Net<NodeId>],
    timing: &TimingDriven,
) -> u64 {
    let delays_only = DelaysAsCosts(space);
    let mut search = Search::new(space);
    let mut graph = timing.graph.clone();
    for connection in &timing.connections {
        let net = &nets[connection.net.0 as usize];
        let from = net.terminals[0].access_points()[0].node;
        let to = net.terminals[connection.terminal as usize].access_points()[0].node;
        let sources = [Source::new(from)];
        let outcome = search.find_path_with(
            &delays_only,
            net.id,
            sources,
            [Target::new(to)],
            SearchLimits::NONE,
        );
        if let SearchOutcome::Found(found) = outcome {
            graph
                .set_delay(connection.arc, found.cost.get() as u32)
                .unwrap();
        }
    }
    let mut analysis = TimingAnalysis::new(&graph);
    analysis.run(&graph, &timing.constraints);
    analysis.critical_path().map_or(0, |path| path.arrival())
}

/// What one run came to, with or without timing.
#[derive(Clone, Copy)]
struct TimedRun {
    solved: bool,
    iterations: u64,
    expansions: u64,
    cost: f64,
    /// Steps in all routes, which is the number of nodes they use beyond their roots.
    wirelength: u64,
    critical: u64,
}

/// A fabric instance with delays.
struct TimedInstance {
    group: usize,
    fabric: Fabric,
    graph: Graph<TileBound>,
    nets: Vec<Net<NodeId>>,
    timing: TimingDriven,
}

impl TimedInstance {
    fn space(&self) -> TimedGraph<'_, TileBound, TileDelayBound> {
        let fabric = self.fabric;
        self.graph
            .with_step_delays(|_, to| fabric.delay(to))
            .with_delay_bound(TileDelayBound {
                positions: fabric.positions(),
            })
    }

    fn run(&self, options: EngineOptions<'static>, timed: bool) -> TimedRun {
        let space = self.space();
        let outcome = if timed {
            route_timed(&space, &self.nets, options, self.timing.clone()).unwrap()
        } else {
            route(&space, &self.nets, options).unwrap()
        };
        let critical = critical_path_of(&space, &self.nets, &outcome.routes, &self.timing);
        if let Some(reported) = &outcome.timing {
            assert_eq!(reported.metrics.critical_path_delay, Some(critical));
        }
        assert!(check(&space, &self.nets, &outcome.routes).is_complete() == outcome.is_success());
        TimedRun {
            solved: outcome.is_success(),
            iterations: outcome.routes.stats.iterations,
            expansions: outcome.routes.stats.nodes_expanded,
            cost: outcome.total_cost.get(),
            wirelength: outcome
                .routes
                .routes()
                .map(|(_, route)| route.edges().len() as u64)
                .sum(),
            critical,
        }
    }
}

/// The FPGA-like family, plus a wider fabric with 2 tracks that has room to spare.
fn timed_family() -> (Vec<String>, Vec<TimedInstance>) {
    let mut groups = Vec::new();
    let mut instances = Vec::new();
    let shapes = [
        (narrow_fabric(16), 200usize),
        (narrow_fabric(32), 800),
        (
            Fabric {
                tracks: 2,
                ..narrow_fabric(32)
            },
            800,
        ),
    ];
    for (group, &(fabric, count)) in shapes.iter().enumerate() {
        groups.push(format!(
            "{0}x{0}, {1} track{2}, up to {count} nets",
            fabric.tiles,
            fabric.tracks,
            if fabric.tracks == 1 { "" } else { "s" }
        ));
        for seed in 0..FABRIC_SEEDS {
            let graph = fabric.build(u64::from(fabric.tiles) + seed);
            let nets = fabric.nets(count, 3, 6, 900 + seed);
            let timing = fabric.timing(&nets);
            instances.push(TimedInstance {
                group,
                fabric,
                graph,
                nets,
                timing,
            });
        }
    }
    (groups, instances)
}

fn timing_sweep() {
    let (groups, instances) = timed_family();
    let improve = |iterations: u32, criticality: f64| {
        options_with(|o| {
            o.timing_iterations = iterations;
            o.timing_reroute_criticality = criticality;
        })
    };
    let settings: [(&str, EngineOptions<'static>, bool); 11] = [
        ("No timing (reference)", EngineOptions::default(), false),
        ("Timing", EngineOptions::default(), true),
        (
            "No timing, first_present 0",
            options_with(|o| o.first_present = Cost::ZERO),
            false,
        ),
        (
            "Timing, first_present 0",
            options_with(|o| o.first_present = Cost::ZERO),
            true,
        ),
        (
            "No timing, batch 32",
            options_with(|o| o.batch_size = 32),
            false,
        ),
        (
            "Timing, batch 32",
            options_with(|o| o.batch_size = 32),
            true,
        ),
        ("Timing, 10 more at 0.9", improve(10, 0.9), true),
        ("Timing, 30 more at 0.9", improve(30, 0.9), true),
        ("Timing, 10 more at 0.8", improve(10, 0.8), true),
        ("Timing, 10 more at 0.95", improve(10, 0.95), true),
        ("Timing, 10 more at 0.5", improve(10, 0.5), true),
    ];
    let runs: Vec<Vec<TimedRun>> = settings
        .iter()
        .map(|&(_, options, timed)| par_map(&instances, |instance| instance.run(options, timed)))
        .collect();
    let alone: Vec<u64> = par_map(&instances, |instance| {
        alone_critical_path(&instance.space(), &instance.nets, &instance.timing)
    });

    println!("\n## Timing-driven routing\n");
    println!(
        "Sums over each group's {FABRIC_SEEDS} instances. Ratios are to the first row, over the \
         instances both solved. \"Alone\" is the critical path with every connection on its \
         fastest route, ignoring the others.\n"
    );
    for (group, name) in groups.iter().enumerate() {
        let members: Vec<usize> = (0..instances.len())
            .filter(|&index| instances[index].group == group)
            .collect();
        let floor: u64 = members.iter().map(|&index| alone[index]).sum();
        println!("### {name}\n");
        println!(
            "| Setting | Solved | Critical path, ps | / reference | / alone | Wirelength | / reference | Cost / reference | Iterations | Expansions | / reference |"
        );
        println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
        let reference = &runs[0];
        for ((setting, _, _), these) in settings.iter().zip(&runs) {
            let both: Vec<usize> = members
                .iter()
                .copied()
                .filter(|&index| these[index].solved && reference[index].solved)
                .collect();
            let sum = |runs: &[TimedRun], field: fn(&TimedRun) -> f64, over: &[usize]| -> f64 {
                over.iter().map(|&index| field(&runs[index])).sum()
            };
            let critical = sum(these, |run| run.critical as f64, &members);
            let wirelength = sum(these, |run| run.wirelength as f64, &members);
            let expansions = sum(these, |run| run.expansions as f64, &members);
            println!(
                "| {setting} | {}/{} | {critical:.0} | {} | {} | {wirelength:.0} | {} | {} | {:.0} | {expansions:.0} | {} |",
                members.iter().filter(|&&index| these[index].solved).count(),
                members.len(),
                ratio(
                    sum(these, |run| run.critical as f64, &both),
                    sum(reference, |run| run.critical as f64, &both)
                ),
                ratio(critical, floor as f64),
                ratio(
                    sum(these, |run| run.wirelength as f64, &both),
                    sum(reference, |run| run.wirelength as f64, &both)
                ),
                ratio(
                    sum(these, |run| run.cost, &both),
                    sum(reference, |run| run.cost, &both)
                ),
                sum(these, |run| run.iterations as f64, &members),
                ratio(
                    sum(these, |run| run.expansions as f64, &both),
                    sum(reference, |run| run.expansions as f64, &both)
                ),
            );
        }
        println!("\nAlone: {floor} ps.\n");
    }
}

fn main() {
    let Ok(sweep) = std::env::var(SWEEP_VARIABLE) else {
        benches();
        Criterion::default().configure_from_args().final_summary();
        return;
    };
    let all = sweep == "all";
    if all || sweep == "options" {
        option_sweep();
    }
    if all || sweep == "refine" {
        refinement_sweep();
    }
    if all || sweep == "expansion" {
        expansion_sweep();
    }
    if all || sweep == "timing" {
        timing_sweep();
    }
    if !(all
        || matches!(
            sweep.as_str(),
            "options" | "refine" | "expansion" | "timing"
        ))
    {
        eprintln!(
            "{SWEEP_VARIABLE} must be all, options, refine, expansion or timing, not {sweep:?}"
        );
        std::process::exit(2);
    }
}
