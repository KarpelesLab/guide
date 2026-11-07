//! A map lookahead for [`RrGraph`]: tables of what it costs to get from a wire to a `SINK`
//! some tiles away, from searches of the graph itself, as VPR's `map` lookahead has.
//!
//! See [`MapLookahead`].

use core::cmp::Reverse;
use core::fmt;
use std::collections::BinaryHeap;

use super::rr_graph::{DIRECTION_MASK, NodeInfo, NodeKind, RrGraph};
use crate::id::{NetId, NodeId};
use crate::space::{RoutingSpace, Step};

/// What [`RrGraph::use_map_lookahead`] builds its tables from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LookaheadOptions {
    /// How many places to start searches from along each side of the device, for each type
    /// of wire. The default is 2: near two opposite corners, which is the fewest that sees
    /// every offset across the device. A value of 0 means 1, the middle of the device.
    pub samples_per_axis: u32,
}

impl Default for LookaheadOptions {
    fn default() -> Self {
        LookaheadOptions {
            samples_per_axis: 2,
        }
    }
}

/// Marks a node that isn't a wire of a type with a table.
const NO_CLASS: u8 = u8::MAX;
/// The most types of wire with a table of their own. Others get the location bound.
const MAX_CLASSES: usize = NO_CLASS as usize;

/// In [`MapLookahead::only_way`]: a node that is not a tail, so goes on to other nodes.
const NOT_TAIL: u32 = u32::MAX;
/// In [`MapLookahead::only_way`]: a dead end, which leads nowhere.
const DEAD_END: u32 = u32::MAX - 1;
/// In [`MapLookahead::only_way`]: a tail with more than one dead end after it.
const MANY_WAYS: u32 = u32::MAX - 2;

/// What the tables say about getting from one node to another.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Lookup {
    /// There is no way there at all.
    Unreachable,
    /// The least cost and delay that the searches found for a wire of this type at this
    /// offset, which are 0 where they found nothing.
    Table { cost: f64, delay: u32 },
    /// No table applies.
    Unknown,
}

/// Tables of the least cost and delay from a wire to a `SINK`, by type of wire and offset.
///
/// This is VPR's `map` lookahead (`router_lookahead_map.cpp`), made to bound costs rather
/// than estimate them. For each type of wire, by axis, direction and segment, a few places on
/// the device are picked, and from every wire of that type driven there, on all tracks at
/// once, one search finds the least sum of base costs to every `SINK`, and another the least
/// sum of step delays. The least over the places, at each offset from the wire's driven end to
/// the `SINK`'s tile, goes in the table. A search from a wire then looks up its type and offset.
///
/// **This is not exactly a lower bound.** A wire of the same type at another place, near an
/// edge of the device or on a track that the places picked do not have, may do better than
/// any wire that was searched from. On a device that looks the same everywhere, which an
/// island-style FPGA does but for its edges, it is close. The graph's location bound is
/// always a lower bound, so the larger of the two is used, and where a table has no entry,
/// the location bound is all there is.
///
/// Besides the tables, a node that can only lead to dead ends, such as an `IPIN`, cannot reach
/// a `SINK` it has no edge to, and a dead end cannot reach anything. Those are exact, and they
/// spare a search the pins of every block it passes.
///
/// `OPIN`s and `SOURCE`s, which only start a route, and other kinds of node get the location
/// bound.
pub struct MapLookahead {
    /// The type of each node, or [`NO_CLASS`].
    class_of: Box<[u8]>,
    /// For a tail, the only node it leads to, or [`MANY_WAYS`]. [`DEAD_END`] and [`NOT_TAIL`]
    /// for the others.
    only_way: Box<[u32]>,
    /// Offsets run from `-(width - 1)` to `width - 1`, and the same for heights.
    width: u32,
    height: u32,
    /// By type, then offset in y, then in x.
    costs: Box<[f64]>,
    delays: Box<[u32]>,
    classes: usize,
}

impl fmt::Debug for MapLookahead {
    /// Leaves out the tables.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MapLookahead")
            .field("classes", &self.classes)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// The tile a wire is driven from: its low end, unless it runs towards lower coordinates.
fn driven_end(info: &NodeInfo) -> (u16, u16) {
    // A direction of 2 is `DEC_DIR`.
    if info.packed & DIRECTION_MASK == 2 {
        (info.xhigh, info.yhigh)
    } else {
        (info.xlow, info.ylow)
    }
}

impl MapLookahead {
    /// Builds the tables for `graph`.
    pub(super) fn build(graph: &RrGraph, nodes: &[NodeInfo], options: &LookaheadOptions) -> Self {
        let count = nodes.len();
        let (width, height) = graph.grid_size();
        let (width, height) = (width.max(1), height.max(1));

        // Types of wire, numbered in the order first seen.
        let mut keys: Vec<(NodeKind, u8, u16)> = Vec::new();
        let mut class_of = vec![NO_CLASS; count];
        for (node, info) in nodes.iter().enumerate() {
            if !matches!(info.kind, NodeKind::ChanX | NodeKind::ChanY) {
                continue;
            }
            let key = (info.kind, info.packed & DIRECTION_MASK, info.segment);
            let class = match keys.iter().position(|&k| k == key) {
                Some(class) => class,
                None if keys.len() < MAX_CLASSES => {
                    keys.push(key);
                    keys.len() - 1
                }
                None => continue,
            };
            class_of[node] = class as u8;
        }
        let classes = keys.len();

        // Tails and dead ends.
        let graph_space = graph.graph();
        let mut steps: Vec<Step<NodeId>> = Vec::new();
        let mut is_dead_end = vec![false; count];
        for (node, dead) in is_dead_end.iter_mut().enumerate() {
            *dead = graph_space.out_degree(NodeId(node as u32)) == 0;
        }
        let mut only_way = vec![NOT_TAIL; count];
        for (node, way) in only_way.iter_mut().enumerate() {
            if is_dead_end[node] {
                *way = DEAD_END;
                continue;
            }
            steps.clear();
            graph_space.expand(NetId(0), NodeId(node as u32), &mut steps);
            if steps.iter().all(|step| is_dead_end[step.to.index()]) {
                let first = steps[0].to;
                *way = if steps.iter().all(|step| step.to == first) {
                    first.0
                } else {
                    MANY_WAYS
                };
            }
        }

        let cells = ((2 * width - 1) * (2 * height - 1)) as usize;
        let mut costs = vec![f64::INFINITY; classes * cells];
        let mut delays = vec![u32::MAX; classes * cells];
        let edge_delays = edge_delays(graph);
        let mut dijkstra = Dijkstra::new(count);

        for class in 0..classes {
            // Every place that some wire of this type is driven from.
            let mut places: Vec<(u16, u16)> = class_of
                .iter()
                .enumerate()
                .filter(|&(_, &c)| usize::from(c) == class)
                .map(|(node, _)| driven_end(&nodes[node]))
                .collect();
            places.sort_unstable();
            places.dedup();
            for place in pick_places(&places, options.samples_per_axis.max(1)) {
                let sources: Vec<u32> = (0..count as u32)
                    .filter(|&node| {
                        usize::from(class_of[node as usize]) == class
                            && driven_end(&nodes[node as usize]) == place
                    })
                    .collect();
                let table = class * cells;
                let cell_of = |sink: &NodeInfo| {
                    let dx = i64::from(sink.xlow) - i64::from(place.0) + i64::from(width) - 1;
                    let dy = i64::from(sink.ylow) - i64::from(place.1) + i64::from(height) - 1;
                    let in_range = (0..i64::from(2 * width - 1)).contains(&dx)
                        && (0..i64::from(2 * height - 1)).contains(&dy);
                    in_range.then(|| table + (dy * i64::from(2 * width - 1) + dx) as usize)
                };

                dijkstra.run(&sources, |from, out| {
                    steps.clear();
                    graph_space.expand(NetId(0), NodeId(from), &mut steps);
                    out.extend(steps.iter().map(|step| (step.to.0, step.cost.get())));
                });
                for (node, info) in nodes.iter().enumerate() {
                    if info.kind == NodeKind::Sink
                        && let Some(cost) = dijkstra.reached(node)
                        && let Some(cell) = cell_of(info)
                    {
                        costs[cell] = costs[cell].min(cost);
                    }
                }

                dijkstra.run(&sources, |from, out| {
                    let range = graph.edge_range(NodeId(from));
                    steps.clear();
                    graph_space.expand(NetId(0), NodeId(from), &mut steps);
                    out.extend(
                        steps
                            .iter()
                            .zip(&edge_delays[range])
                            .map(|(step, &delay)| (step.to.0, f64::from(delay))),
                    );
                });
                for (node, info) in nodes.iter().enumerate() {
                    if info.kind == NodeKind::Sink
                        && let Some(delay) = dijkstra.reached(node)
                        && let Some(cell) = cell_of(info)
                    {
                        // Sums of whole picoseconds, which `f64` holds exactly.
                        delays[cell] = delays[cell].min(delay as u32);
                    }
                }
            }
        }

        // No entry is no information, which the location bound then stands in for.
        for cost in &mut costs {
            if !cost.is_finite() {
                *cost = 0.0;
            }
        }
        for delay in &mut delays {
            if *delay == u32::MAX {
                *delay = 0;
            }
        }

        MapLookahead {
            class_of: class_of.into_boxed_slice(),
            only_way: only_way.into_boxed_slice(),
            width,
            height,
            costs: costs.into_boxed_slice(),
            delays: delays.into_boxed_slice(),
            classes,
        }
    }

    /// Looks up the way from `from` to `to`, whose infos are given.
    #[inline]
    pub(super) fn lookup(
        &self,
        from: NodeId,
        to: NodeId,
        from_info: &NodeInfo,
        to_info: &NodeInfo,
    ) -> Lookup {
        if from == to {
            return Lookup::Table {
                cost: 0.0,
                delay: 0,
            };
        }
        match self.only_way.get(from.index()).copied() {
            Some(DEAD_END) => return Lookup::Unreachable,
            Some(NOT_TAIL | MANY_WAYS) | None => {}
            Some(way) if way == to.0 => {
                return Lookup::Table {
                    cost: 0.0,
                    delay: 0,
                };
            }
            Some(_) => return Lookup::Unreachable,
        }
        let class = self.class_of.get(from.index()).copied().unwrap_or(NO_CLASS);
        if class == NO_CLASS || to_info.kind != NodeKind::Sink {
            return Lookup::Unknown;
        }
        let (x, y) = driven_end(from_info);
        let dx = i64::from(to_info.xlow) - i64::from(x) + i64::from(self.width) - 1;
        let dy = i64::from(to_info.ylow) - i64::from(y) + i64::from(self.height) - 1;
        let row = i64::from(2 * self.width - 1);
        if !(0..row).contains(&dx) || !(0..i64::from(2 * self.height - 1)).contains(&dy) {
            return Lookup::Unknown;
        }
        let cells = (row * i64::from(2 * self.height - 1)) as usize;
        let cell = usize::from(class) * cells + (dy * row + dx) as usize;
        Lookup::Table {
            cost: self.costs[cell],
            delay: self.delays[cell],
        }
    }

    /// Returns the number of types of wire with a table.
    pub fn classes(&self) -> usize {
        self.classes
    }

    /// Returns the bytes this holds on the heap.
    pub fn heap_bytes(&self) -> usize {
        self.class_of.len()
            + size_of::<u32>() * self.only_way.len()
            + size_of::<f64>() * self.costs.len()
            + size_of::<u32>() * self.delays.len()
    }
}

/// The delay of every edge, in the order of the graph's edges.
fn edge_delays(graph: &RrGraph) -> Vec<u32> {
    let space = graph.graph();
    let mut delays = Vec::with_capacity(space.num_edges() as usize);
    let mut steps = Vec::new();
    for node in 0..space.num_nodes() {
        steps.clear();
        space.expand(NetId(0), NodeId(node), &mut steps);
        let switches = graph.out_switches(NodeId(node));
        for (position, step) in steps.iter().enumerate() {
            let switch = switches.get(position).copied().unwrap_or(u16::MAX);
            delays.push(graph.step_delay(switch, step.to));
        }
    }
    delays
}

/// Picks up to `per_axis` squared places to search from, spread over the box of `places`,
/// each the nearest of `places` to a point of an even grid over that box. `places` is sorted.
fn pick_places(places: &[(u16, u16)], per_axis: u32) -> Vec<(u16, u16)> {
    let (Some(&first), false) = (places.first(), places.is_empty()) else {
        return Vec::new();
    };
    let (mut low, mut high) = (first, first);
    for &(x, y) in places {
        low = (low.0.min(x), low.1.min(y));
        high = (high.0.max(x), high.1.max(y));
    }
    let along = |low: u16, high: u16, index: u32| -> i64 {
        let (low, high) = (i64::from(low), i64::from(high));
        if per_axis == 1 {
            (low + high) / 2
        } else {
            low + (high - low) * i64::from(index) / i64::from(per_axis - 1)
        }
    };
    let mut picked = Vec::new();
    for j in 0..per_axis {
        for i in 0..per_axis {
            let point = (along(low.0, high.0, i), along(low.1, high.1, j));
            // The first of the nearest, in sorted order, so that ties are settled the same way.
            let nearest = places.iter().copied().min_by_key(|&(x, y)| {
                (i64::from(x) - point.0).abs() + (i64::from(y) - point.1).abs()
            });
            if let Some(nearest) = nearest
                && !picked.contains(&nearest)
            {
                picked.push(nearest);
            }
        }
    }
    picked
}

/// A plain Dijkstra over node numbers, for building the tables.
struct Dijkstra {
    distance: Vec<f64>,
    heap: BinaryHeap<Reverse<(u64, u32)>>,
    out: Vec<(u32, f64)>,
}

impl Dijkstra {
    fn new(count: usize) -> Self {
        Dijkstra {
            distance: vec![f64::INFINITY; count],
            heap: BinaryHeap::new(),
            out: Vec::new(),
        }
    }

    /// Finds the least distance to every node from `sources`, with `expand` giving the steps
    /// out of a node and their lengths, which are not negative.
    fn run(&mut self, sources: &[u32], mut expand: impl FnMut(u32, &mut Vec<(u32, f64)>)) {
        self.distance.fill(f64::INFINITY);
        self.heap.clear();
        for &source in sources {
            self.distance[source as usize] = 0.0;
            self.heap.push(Reverse((0f64.to_bits(), source)));
        }
        while let Some(Reverse((key, node))) = self.heap.pop() {
            // The bits of a float that is not negative order as the float does.
            let here = f64::from_bits(key);
            if here > self.distance[node as usize] {
                continue;
            }
            let mut out = core::mem::take(&mut self.out);
            out.clear();
            expand(node, &mut out);
            for &(to, length) in &out {
                let there = here + length;
                if there < self.distance[to as usize] {
                    self.distance[to as usize] = there;
                    self.heap.push(Reverse((there.to_bits(), to)));
                }
            }
            self.out = out;
        }
    }

    /// The distance to `node` in the last run, if it was reached.
    fn reached(&self, node: usize) -> Option<f64> {
        let distance = self.distance[node];
        distance.is_finite().then_some(distance)
    }
}
