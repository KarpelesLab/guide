//! Throughput of `Search` on grids and on an FPGA-like routing graph.
//!
//! Every instance comes from a fixed [`Rng`] seed, so that the instances never vary. Each
//! benchmark runs a fixed batch of queries through a `Search` that has already seen them,
//! which is how a router uses one: with its buffers already grown.
//!
//! Throughput is in nodes expanded, read from `SearchStats`, so criterion's `elem/s` means
//! expansions per second. When comparing Dijkstra with A\*, look at the time per batch as
//! well: the point of a heuristic is to expand fewer nodes, each costing a little more.

use std::hint::black_box;
use std::time::Duration;

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, Throughput, criterion_group, criterion_main};
use guide::cost::Cost;
use guide::id::{NetId, NodeId};
use guide::rng::Rng;
use guide::search::{Search, Source};
use guide::space::graph::{Graph, GraphBuilder, LowerBound};
use guide::space::grid::{Connectivity, Grid};
use guide::space::{RoutingSpace, Step};

const NET: NetId = NetId(0);

/// Queries in each grid batch, enough to average out the shape of any one route.
const GRID_QUERIES: usize = 16;

/// A pair of nodes to route between.
type Query = (NodeId, NodeId);

/// Hides a space's lower bound, turning A\* into Dijkstra's algorithm on the same instance.
struct WithoutBound<'a, S>(&'a S);

impl<S: RoutingSpace> RoutingSpace for WithoutBound<'_, S> {
    type Node = S::Node;
    type Resource = S::Resource;
    type Slots = S::Slots;
    type ResourceSlots = S::ResourceSlots;

    #[inline]
    fn new_slots(&self) -> S::Slots {
        self.0.new_slots()
    }

    #[inline]
    fn new_resource_slots(&self) -> S::ResourceSlots {
        self.0.new_resource_slots()
    }

    #[inline]
    fn contains(&self, node: S::Node) -> bool {
        self.0.contains(node)
    }

    #[inline]
    fn expand(&self, net: NetId, from: S::Node, out: &mut Vec<Step<S::Node>>) {
        self.0.expand(net, from, out);
    }

    #[inline]
    fn lower_bound(&self, _net: NetId, _from: S::Node, _to: S::Node) -> Cost {
        Cost::ZERO
    }

    #[inline]
    fn resources(&self, net: NetId, from: S::Node, to: S::Node, out: &mut Vec<S::Resource>) {
        self.0.resources(net, from, to, out);
    }

    #[inline]
    fn capacity(&self, resource: S::Resource) -> u32 {
        self.0.capacity(resource)
    }
}

/// Runs a batch of queries, returning how many nodes they expanded.
fn run_batch<S: RoutingSpace<Node = NodeId>>(
    search: &mut Search<S>,
    space: &S,
    queries: &[Query],
) -> u64 {
    let mut expanded = 0;
    for &(source, target) in queries {
        let found = search.find_path(space, NET, [Source::new(source)], [target]);
        black_box(found.map(|found| found.cost));
        expanded += search.stats().expanded;
    }
    expanded
}

/// Benchmarks a batch of queries, with a throughput of nodes expanded.
fn bench_batch<S: RoutingSpace<Node = NodeId>>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    space: &S,
    queries: &[Query],
) {
    let mut search = Search::new(space);
    // This also grows the search's buffers, which keeps that out of the measurement.
    let expanded = run_batch(&mut search, space, queries);
    group.throughput(Throughput::Elements(expanded));
    group.bench_function(name, |b| {
        b.iter(|| run_batch(&mut search, space, black_box(queries)));
    });
}

/// Creates a grid with whole-number axis and via costs. An 8-connected one also has
/// diagonals of √2, which round, and so make the grid give up its exact bound.
fn make_grid(size: u32, layers: u32, connectivity: Connectivity, blocked: f64, seed: u64) -> Grid {
    let mut grid = Grid::builder(size, size, layers)
        .connectivity(connectivity)
        .via_cost(4.0)
        .build()
        .unwrap();
    let mut rng = Rng::new(seed);
    for layer in 0..layers {
        for y in 0..size {
            for x in 0..size {
                if rng.unit() < blocked {
                    grid.set_blocked(x, y, layer, true).unwrap();
                }
            }
        }
    }
    grid
}

/// Picks reachable pairs of free cells a good way apart, so that trivial routes don't
/// dominate a batch.
fn grid_queries(grid: &Grid, count: usize, seed: u64) -> Vec<Query> {
    let mut rng = Rng::new(seed);
    let mut search = Search::new(grid);
    let min_distance = (grid.width() + grid.height()) / 4;
    let cells = u64::from(grid.cell_count());
    let free_cell = |rng: &mut Rng| loop {
        let node = NodeId(rng.below(cells) as u32);
        if !grid.is_blocked(node) {
            return node;
        }
    };

    let mut queries = Vec::with_capacity(count);
    while queries.len() < count {
        let (source, target) = (free_cell(&mut rng), free_cell(&mut rng));
        let (a, b) = (
            grid.position(source).unwrap(),
            grid.position(target).unwrap(),
        );
        if a.0.abs_diff(b.0) + a.1.abs_diff(b.1) >= min_distance
            && search
                .find_path(grid, NET, [Source::new(source)], [target])
                .is_some()
        {
            queries.push((source, target));
        }
    }
    queries
}

fn connectivity_name(connectivity: Connectivity) -> &'static str {
    match connectivity {
        Connectivity::Orthogonal => "4-connected",
        Connectivity::Octilinear => "8-connected",
    }
}

const CONNECTIVITIES: [Connectivity; 2] = [Connectivity::Orthogonal, Connectivity::Octilinear];

/// One layer, with and without obstacles. 30% blocked cells make routes work their way
/// around pockets, and is still short of cutting a 4-connected grid into pieces.
fn single_layer_grids(c: &mut Criterion) {
    let mut group = c.benchmark_group("grid");
    for (obstacles, blocked) in [("open", 0.0), ("obstacles", 0.3)] {
        for size in [64, 256, 1024] {
            for connectivity in CONNECTIVITIES {
                let grid = make_grid(size, 1, connectivity, blocked, 1);
                let queries = grid_queries(&grid, GRID_QUERIES, 2);
                let name = format!(
                    "{obstacles}/{size}x{size}/{}",
                    connectivity_name(connectivity)
                );
                bench_batch(&mut group, &name, &grid, &queries);
            }
        }
    }
    group.finish();
}

fn multi_layer_grids(c: &mut Criterion) {
    let mut group = c.benchmark_group("multilayer_grid");
    for (obstacles, blocked) in [("open", 0.0), ("obstacles", 0.2)] {
        for (size, layers) in [(128, 4), (256, 8)] {
            for connectivity in CONNECTIVITIES {
                let grid = make_grid(size, layers, connectivity, blocked, 3);
                let queries = grid_queries(&grid, GRID_QUERIES, 4);
                let name = format!(
                    "{obstacles}/{size}x{size}x{layers}/{}",
                    connectivity_name(connectivity)
                );
                bench_batch(&mut group, &name, &grid, &queries);
            }
        }
    }
    group.finish();
}

/// Prints how many expansions a heuristic saved, which the timings only show indirectly, and
/// checks that it isn't saving them by getting wrong answers, or by not finding any.
fn report_expansions<S, T>(name: &str, guided: &S, plain: &T, queries: &[Query])
where
    S: RoutingSpace<Node = NodeId>,
    T: RoutingSpace<Node = NodeId>,
{
    let (mut with_bound, mut without_bound) = (Search::new(guided), Search::new(plain));
    for &(source, target) in queries {
        let sources = [Source::new(source)];
        let a = with_bound.find_path(guided, NET, sources, [target]);
        let b = without_bound.find_path(plain, NET, sources, [target]);
        let (a, b) = (a.expect("route exists").cost, b.expect("route exists").cost);
        // Rounding allows these to differ, but only in about the last place.
        assert!((a.get() - b.get()).abs() <= 1e-9 * a.get(), "{a} != {b}");
    }

    let a_star = run_batch(&mut Search::new(guided), guided, queries);
    let dijkstra = run_batch(&mut Search::new(plain), plain, queries);
    println!(
        "{name}: A* expands {a_star}, Dijkstra {dijkstra} ({:.1} times as many)",
        dijkstra as f64 / a_star as f64
    );
}

fn dijkstra_versus_a_star_on_grids(c: &mut Criterion) {
    let mut group = c.benchmark_group("dijkstra_vs_astar_grid");
    for (obstacles, blocked) in [("open", 0.0), ("obstacles", 0.3)] {
        for connectivity in CONNECTIVITIES {
            let grid = make_grid(512, 1, connectivity, blocked, 5);
            let queries = grid_queries(&grid, 4, 6);
            let name = format!("{obstacles}/512x512/{}", connectivity_name(connectivity));
            let plain = WithoutBound(&grid);
            report_expansions(&name, &grid, &plain, &queries);
            bench_batch(&mut group, &format!("{name}/astar"), &grid, &queries);
            bench_batch(&mut group, &format!("{name}/dijkstra"), &plain, &queries);
        }
    }
    group.finish();
}

// The fabric's costs are small multiples of powers of two, so that no sum of them rounds.

/// Covering a tile costs at least this much, whichever kind of wire does it.
const COST_PER_TILE: f64 = 0.75;
/// How many tiles each kind of wire spans.
const WIRE_SPAN: [u32; 2] = [1, 4];
/// What entering each kind of wire costs per tile it spans. Long wires are the cheaper way
/// to cover distance, as they tend to be on real devices.
const WIRE_COST_PER_TILE: [f64; 2] = [1.0, COST_PER_TILE];
/// What a switch costs, by switch type. Real devices have a handful of these.
const SWITCH_COST: [f64; 3] = [0.125, 0.1875, 0.25];
/// The two nodes of each tile's logic site: a source and a sink.
const SITE_NODES: u32 = 2;
/// The directions wires run in, as tile offsets.
const DIRECTIONS: [(i64, i64); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// The shape of an FPGA-like routing resource graph.
///
/// Each tile of a square array holds a logic site: a source that drives output pins, and
/// input pins that drive a sink. Directional wires start in every tile, in all four
/// directions, in two kinds that span different distances. A route therefore passes through
/// layers of node types: source, output pin, any number of wires, input pin, sink. As in
/// VPR's graphs, wires connect to each other only at their ends, here at the far end, and
/// the switch and wire costs come from short lists.
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
    ///
    /// Wires that would leave the array stop at its edge. One that starts at the edge and
    /// points outwards covers nothing, and stays unconnected.
    fn wire_end(self, x: u32, y: u32, index: u32) -> (u32, u32, u32) {
        let direction = DIRECTIONS[index as usize % DIRECTIONS.len()];
        let span = i64::from(WIRE_SPAN[Self::wire_kind(index)]);
        let limit = i64::from(self.tiles) - 1;
        let end_x = (i64::from(x) + direction.0 * span).clamp(0, limit);
        let end_y = (i64::from(y) + direction.1 * span).clamp(0, limit);
        let covered = end_x.abs_diff(i64::from(x)) + end_y.abs_diff(i64::from(y));
        (end_x as u32, end_y as u32, covered as u32)
    }

    /// Every node's position: its tile, or for a wire the tile at its far end, which is the
    /// only place it can connect to anything.
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

    /// Picks source-to-sink routes spanning at most `reach` tiles along each axis.
    fn queries(self, count: usize, reach: u32, seed: u64) -> Vec<Query> {
        let mut rng = Rng::new(seed);
        let tiles = u64::from(self.tiles);
        let near = |rng: &mut Rng, at: u32| {
            let low = at.saturating_sub(reach);
            let high = at.saturating_add(reach).min(self.tiles - 1);
            low + rng.below(u64::from(high - low + 1)) as u32
        };
        (0..count)
            .map(|_| {
                let (x, y) = (rng.below(tiles) as u32, rng.below(tiles) as u32);
                let (to_x, to_y) = (near(&mut rng, x), near(&mut rng, y));
                (self.source(x, y), self.sink(to_x, to_y))
            })
            .collect()
    }
}

/// The Manhattan distance times the cheapest cost of covering a tile, which is the simplest
/// form of a router's lookahead. Any step that changes a node's position pays for a wire
/// covering that distance, so this is admissible and consistent.
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

fn fpga_like_graphs(c: &mut Criterion) {
    let mut group = c.benchmark_group("fpga_graph");
    // A search of the whole graph is too slow for many samples, and 10 is the fewest allowed.
    group.sample_size(10);

    for tiles in [64, 128] {
        let fabric = Fabric {
            tiles,
            tracks: 8,
            pins: 8,
            output_flexibility: 8,
            switch_fanout: 6,
            input_flexibility: 2,
        };
        let graph = fabric.build(u64::from(tiles));
        println!(
            "{tiles}x{tiles} tiles: {} nodes, {} edges, {} MiB",
            graph.num_nodes(),
            graph.num_edges(),
            graph.heap_bytes() >> 20,
        );
        let guided = graph.with_lower_bound(TileBound {
            positions: fabric.positions(),
        });
        let plain = WithoutBound(&guided);

        // Most nets on a real design are short, and a few cross the whole device.
        for (scope, reach, count) in [("local", 8, 16), ("global", tiles, 4)] {
            let queries = fabric.queries(count, reach, u64::from(reach));
            let name = format!("{tiles}x{tiles}/{scope}");
            report_expansions(&name, &guided, &plain, &queries);
            bench_batch(&mut group, &format!("{name}/astar"), &guided, &queries);
            bench_batch(&mut group, &format!("{name}/dijkstra"), &plain, &queries);
        }
    }
    group.finish();
}

fn configured() -> Criterion {
    Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_millis(300))
        .measurement_time(Duration::from_millis(1500))
}

criterion_group! {
    name = benches;
    config = configured();
    targets = single_layer_grids, multi_layer_grids, dijkstra_versus_a_star_on_grids, fpga_like_graphs
}
criterion_main!(benches);
