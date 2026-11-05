//! Routing resource graphs: VPR's `rr_graph.xml`.
//!
//! The format is defined by `libs/librrgraph/src/io/rr_graph.xsd` in VTR's source. Sections
//! may come in any order, and do in principle, so nothing is resolved until the file ends.
//!
//! # Memory
//!
//! Parsing takes 12 bytes per edge and 20 per node, on top of the parser's small fixed
//! buffers. Finishing then briefly needs at most about 100 bytes per node and 40 per edge,
//! of which [`GraphBuilder`] accounts for 28 per edge. See [`RrGraph`] for what's kept.

use core::fmt;
use core::mem::size_of;
use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::sync::Arc;

use super::ImportError;
use super::xml::{Event, Tag, XmlReader};
use crate::cost::Cost;
use crate::id::{NetId, NodeId};
use crate::space::graph::{DelayBound, Graph, GraphBuilder, LowerBound, TimedGraph};
use crate::space::{DelaySpace, RoutingSpace, Step};

/// The value in [`NodeInfo::segment`] for a node with no segment.
const NO_SEGMENT: u16 = u16::MAX;

/// What a routing resource node is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum NodeKind {
    /// Where a net starts: an output pin class of a block.
    Source,
    /// Where a net ends: an input pin class of a block.
    Sink,
    /// An output pin of a block.
    Opin,
    /// An input pin of a block.
    Ipin,
    /// A horizontal wire.
    ChanX,
    /// A vertical wire.
    ChanY,
    /// A wire between the dies of a 3D device.
    ChanZ,
    /// A multiplexer modelled as a node of its own.
    Mux,
}

impl NodeKind {
    /// Returns the name VTR's files use.
    pub const fn name(self) -> &'static str {
        match self {
            NodeKind::Source => "SOURCE",
            NodeKind::Sink => "SINK",
            NodeKind::Opin => "OPIN",
            NodeKind::Ipin => "IPIN",
            NodeKind::ChanX => "CHANX",
            NodeKind::ChanY => "CHANY",
            NodeKind::ChanZ => "CHANZ",
            NodeKind::Mux => "MUX",
        }
    }

    /// Returns whether this is a wire.
    pub const fn is_wire(self) -> bool {
        matches!(self, NodeKind::ChanX | NodeKind::ChanY | NodeKind::ChanZ)
    }

    fn parse(name: &str) -> Option<NodeKind> {
        Some(match name {
            "SOURCE" => NodeKind::Source,
            "SINK" => NodeKind::Sink,
            "OPIN" => NodeKind::Opin,
            "IPIN" => NodeKind::Ipin,
            "CHANX" => NodeKind::ChanX,
            "CHANY" => NodeKind::ChanY,
            "CHANZ" => NodeKind::ChanZ,
            "MUX" => NodeKind::Mux,
            _ => return None,
        })
    }

    const COUNT: usize = 8;
}

impl fmt::Display for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Which way a wire carries signals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ChannelDirection {
    /// Towards higher coordinates.
    Increasing,
    /// Towards lower coordinates.
    Decreasing,
    /// Either way.
    Bidirectional,
}

/// Where a node is on the device, as a box of tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeLocation {
    /// The lowest column.
    pub xlow: u16,
    /// The lowest row.
    pub ylow: u16,
    /// The highest column.
    pub xhigh: u16,
    /// The highest row.
    pub yhigh: u16,
    /// The lowest die, on a 3D device.
    pub layer_low: u8,
    /// The highest die, on a 3D device.
    pub layer_high: u8,
}

/// What a switch is made of, which decides how it behaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SwitchKind {
    /// A configurable buffered multiplexer. This is the default when a file doesn't say.
    Mux,
    /// A configurable tri-state buffer.
    Tristate,
    /// A configurable pass transistor, which isn't buffered.
    PassGate,
    /// An electrical short, which is always on.
    Short,
    /// A buffer that's always on.
    Buffer,
}

impl SwitchKind {
    /// Returns whether a router can choose not to use this switch. Otherwise, both ends are
    /// electrically tied together, and using one uses the other.
    pub const fn is_configurable(self) -> bool {
        !matches!(self, SwitchKind::Short | SwitchKind::Buffer)
    }

    /// Returns whether this isolates its output from its input's load.
    pub const fn is_buffered(self) -> bool {
        matches!(
            self,
            SwitchKind::Mux | SwitchKind::Tristate | SwitchKind::Buffer
        )
    }
}

/// A switch type and its timing, shared by all the edges that use it.
#[derive(Clone, Debug, PartialEq)]
pub struct Switch {
    /// The switch's name.
    pub name: String,
    /// What the switch is.
    pub kind: SwitchKind,
    /// The resistance, in ohms.
    pub r: f32,
    /// The input capacitance, in farads.
    pub c_in: f32,
    /// The output capacitance, in farads.
    pub c_out: f32,
    /// The internal capacitance, in farads.
    pub c_internal: f32,
    /// The minimum intrinsic delay, in seconds.
    pub t_del_min: f32,
    /// The maximum intrinsic delay, in seconds.
    pub t_del_max: f32,
}

/// A wire type.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    /// The segment's name.
    pub name: String,
    /// The segment's length in tiles, if the file says. Not all do.
    pub length: Option<u32>,
    /// The resistance per meter, in ohms.
    pub r_per_meter: f32,
    /// The capacitance per meter, in farads.
    pub c_per_meter: f32,
}

/// Which way the pins of a class face.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PinClassKind {
    /// Signals enter the block here, and the class is a `SINK` node.
    Input,
    /// Signals leave the block here, and the class is a `SOURCE` node.
    Output,
    /// Unused.
    Open,
}

/// A pin of a block type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pin {
    /// The pin's number within its block type, which is the `ptc` of its `IPIN` or `OPIN`.
    pub ptc: u32,
    /// The pin's name in the graph, such as `io[3].inpad[0]` or `clb.I[12]`.
    pub name: String,
}

/// A group of pins that a router treats as interchangeable.
///
/// VPR gives each class one `SOURCE` or `SINK` node, whose `ptc` is the class's index in
/// [`BlockType::classes`], with an edge to or from each pin. That one node is how the graph
/// represents pin equivalence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinClass {
    /// Which way the pins face.
    pub kind: PinClassKind,
    /// The pins, in the order the file lists them.
    pub pins: Vec<Pin>,
}

/// A pin's position, as it's found from its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PinSlot {
    /// The class's index in [`BlockType::classes`].
    pub class: u32,
    /// The position within the tile that the pin belongs to.
    pub sub_block: u32,
    /// The port's name.
    pub port: String,
    /// The pin within the port.
    pub bit: u32,
}

/// A type of tile, and its pins.
#[derive(Clone, Debug)]
pub struct BlockType {
    /// The ID that the file gives the type.
    pub id: u32,
    /// The name of the type.
    pub name: String,
    /// How many columns it covers.
    pub width: u32,
    /// How many rows it covers.
    pub height: u32,
    /// The pin classes, whose indices are the `ptc` of the `SOURCE` and `SINK` nodes.
    pub classes: Vec<PinClass>,
    /// Pins by `ptc`, for looking them up from nodes.
    by_ptc: HashMap<u32, PinSlot>,
    /// Pins by position, port and pin number, for looking them up from a netlist.
    by_slot: HashMap<(u32, String, u32), u32>,
    is_io: bool,
}

impl BlockType {
    /// Returns whether VPR would call this an I/O tile, which it writes differently in a
    /// route file.
    ///
    /// VPR knows from the architecture file whether a block contains the `.input` or
    /// `.output` primitive, which the graph doesn't say. This goes by VPR's naming: a tile
    /// named `io`, or with a port named `inpad` or `outpad`.
    pub fn is_io(&self) -> bool {
        self.is_io
    }

    /// Finds the `ptc` of a pin from a block at `sub_block` of this tile.
    pub(crate) fn find_pin(&self, sub_block: u32, port: &str, bit: u32) -> Option<&PinSlot> {
        // Port names are short and pins are few per block, so a key per lookup costs little.
        let ptc = self.by_slot.get(&(sub_block, port.to_owned(), bit))?;
        self.by_ptc.get(ptc)
    }

    /// Finds a pin from its `ptc`.
    pub(crate) fn pin_slot(&self, ptc: u32) -> Option<&PinSlot> {
        self.by_ptc.get(&ptc)
    }

    /// Works out the tables that make pins findable, from the pins' names.
    ///
    /// VPR names a pin `tile.port[bit]`, with `tile[n]` when a sub-tile has more than one
    /// position. A tile made of several sub-tiles of one position each repeats its names, as
    /// the z1000 device does, so the position has to come from what's been seen so far: pins
    /// are in order of position, so a name seen a second time starts the next position.
    /// Sub-tiles with different ports but one position each can't be told apart this way.
    /// They end up as one position, which makes the later positions' pins unfindable, and
    /// never gives a wrong pin.
    fn index_pins(&mut self) {
        let mut pins: Vec<(u32, u32)> = self
            .classes
            .iter()
            .enumerate()
            .flat_map(|(class, group)| group.pins.iter().map(move |pin| (pin.ptc, class as u32)))
            .collect();
        pins.sort_unstable();

        let mut names: HashMap<u32, &str> = HashMap::with_capacity(pins.len());
        for class in &self.classes {
            for pin in &class.pins {
                names.insert(pin.ptc, &pin.name);
            }
        }

        let mut sub_block = 0u32;
        let mut last_instance = None;
        let mut seen: HashSet<(String, u32)> = HashSet::new();
        let mut is_io = self.name == "io";
        let mut by_ptc = HashMap::with_capacity(pins.len());
        let mut by_slot = HashMap::with_capacity(pins.len());
        for (ptc, class) in pins {
            let Some((instance, port, bit)) = names.get(&ptc).and_then(|name| parse_pin_name(name))
            else {
                continue;
            };
            is_io |= port == "inpad" || port == "outpad";
            let key = (port.to_owned(), bit);
            if (instance.is_some() && instance != last_instance && last_instance.is_some())
                || seen.contains(&key)
            {
                sub_block = sub_block.saturating_add(1);
                seen.clear();
            }
            last_instance = instance;
            seen.insert(key);
            by_slot.insert((sub_block, port.to_owned(), bit), ptc);
            by_ptc.insert(
                ptc,
                PinSlot {
                    class,
                    sub_block,
                    port: port.to_owned(),
                    bit,
                },
            );
        }
        self.is_io = is_io;
        self.by_ptc = by_ptc;
        self.by_slot = by_slot;
    }
}

/// Splits a pin's name from `block_types` into its instance, port and pin number.
///
/// Names of pins inside a block, which flat routing adds, have more parts and don't match.
fn parse_pin_name(name: &str) -> Option<(Option<u32>, &str, u32)> {
    let (tile, port) = name.split_once('.')?;
    let instance = match tile.split_once('[') {
        Some((_, rest)) => Some(rest.strip_suffix(']')?.parse().ok()?),
        None => None,
    };
    let (port, bit) = port.split_once('[')?;
    if port.contains('.') {
        return None;
    }
    Some((instance, port, bit.strip_suffix(']')?.parse().ok()?))
}

/// What's on one tile of the device grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GridTile {
    /// The tile's type, as an index in [`RrGraph::block_types`].
    pub block_type: u16,
    /// How far this tile is from the leftmost tile of its block.
    pub width_offset: u16,
    /// How far this tile is from the bottom tile of its block.
    pub height_offset: u16,
}

/// Facts about a graph that are lost in building it, and that a caller may want to report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RrStats {
    /// The nodes in the file.
    pub nodes: u32,
    /// The edges in the file.
    pub file_edges: u64,
    /// Edges from a node to itself, which were dropped.
    pub self_loops: u64,
    /// Edges that repeat a connection between two nodes, which were dropped.
    pub parallel_edges: u64,
    /// Pairs of nodes with more than one edge between them.
    pub parallel_pairs: u64,
    /// Of those pairs, the ones where the switches aren't all the same.
    pub parallel_pairs_with_different_switches: u64,
    /// Edges through a switch that isn't configurable.
    pub non_configurable_edges: u64,
}

/// One node, in 20 bytes: a graph can have hundreds of millions of these.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NodeInfo {
    xlow: u16,
    ylow: u16,
    xhigh: u16,
    yhigh: u16,
    /// The first `ptc` of the node, as VPR uses when it writes a route.
    ptc: u32,
    /// An index in the table of distinct timings, or `u32::MAX` for a node without any.
    rc: u32,
    segment: u16,
    kind: NodeKind,
    /// A [`ChannelDirection`] in the low 2 bits, where 0 is none, and sides from bit 2 up.
    packed: u8,
}

const SIDE_LEFT: u8 = 1 << 2;
const SIDE_RIGHT: u8 = 1 << 3;
const SIDE_TOP: u8 = 1 << 4;
const SIDE_BOTTOM: u8 = 1 << 5;
const DIRECTION_MASK: u8 = 0b11;

impl NodeInfo {
    /// Returns the Manhattan distance between two boxes of tiles.
    #[inline]
    fn gap(&self, other: &NodeInfo) -> u32 {
        let gap = |low_a: u16, high_a: u16, low_b: u16, high_b: u16| {
            u32::from(low_a.saturating_sub(high_b)).max(u32::from(low_b.saturating_sub(high_a)))
        };
        gap(self.xlow, self.xhigh, other.xlow, other.xhigh)
            + gap(self.ylow, self.yhigh, other.ylow, other.yhigh)
    }

    /// Returns how many tiles this box reaches beyond its first.
    #[inline]
    fn span(&self) -> u32 {
        u32::from(self.xhigh - self.xlow) + u32::from(self.yhigh - self.ylow)
    }

    /// Returns the box's position, as a major and a minor coordinate.
    ///
    /// Horizontal wires run along rows, so sorting them by row first puts each track's wires
    /// in order. For anything else, a column is as good as a row.
    #[inline]
    fn major_minor(&self) -> (u16, u16) {
        match self.kind {
            NodeKind::ChanX => (self.ylow, self.xlow),
            _ => (self.xlow, self.ylow),
        }
    }

    #[inline]
    fn covers(&self, x: u16, y: u16) -> bool {
        (self.xlow..=self.xhigh).contains(&x) && (self.ylow..=self.yhigh).contains(&y)
    }
}

/// A lower bound on the cost of going between nodes, from where they are on the device.
///
/// This is what the graph uses as its estimator, computed from the graph itself so that it
/// is admissible for any file, including one with long jumps.
///
/// Stepping along an edge from `u` to `v` gets at most `gap(u, v) + span(v)` tiles closer to
/// any target, where `gap` is the Manhattan distance between two nodes' boxes and `span` is
/// how far `v`'s box reaches. So the graph has a cost per tile: the smallest ratio of step
/// cost to that distance over its edges. No path can cost less than that ratio times the
/// distance it has to cover.
///
/// The end of a path is treated separately, as it would otherwise spoil the ratio for all the
/// rest. A `SINK` costs nothing to enter, and real devices have `OPIN`s wired directly to
/// `IPIN`s across the whole device, such as the clock spine of the z1000, whose reach is 32
/// tiles for the cost of a single wire. But nothing leaves a `SINK`, and an `IPIN` only leads
/// to `SINK`s, so these are only ever the last two steps. Such nodes are left out of the ratio.
/// Instead, each node has its own *tail reach*: how many tiles the steps into it can cover
/// beyond what the ratio accounts for, which is taken off the distance. It's 0 for any node
/// that leads on to something else, and is what a pin of a clock spine has to give up, without
/// costing other nets anything.
///
/// So, for a target `t`, the bound is `cost per tile × (gap(s, t) - tail reach of t)`.
///
/// The result is also shrunk by a factor of a billionth, since a search sums its costs one
/// step at a time and a distance computed in one go could otherwise round above that sum.
/// That is enough for paths of millions of steps.
#[derive(Clone)]
pub struct LocationBound {
    nodes: Arc<[NodeInfo]>,
    tail_reach: Arc<[u16]>,
    cost_per_tile: f64,
}

impl LocationBound {
    /// Returns the smallest cost of covering one tile.
    pub fn cost_per_tile(&self) -> f64 {
        self.cost_per_tile
    }

    /// Returns how many tiles the last steps into a node can cover that the cost per tile
    /// doesn't account for. This is 0 for a node outside the graph.
    pub fn tail_reach(&self, node: NodeId) -> u16 {
        self.tail_reach.get(node.index()).copied().unwrap_or(0)
    }
}

impl fmt::Debug for LocationBound {
    /// Leaves out the tables.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocationBound")
            .field("cost_per_tile", &self.cost_per_tile)
            .finish_non_exhaustive()
    }
}

impl LowerBound for LocationBound {
    #[inline]
    fn estimate(&self, from: NodeId, to: NodeId) -> Cost {
        // The graph only calls this with its own nodes, so these can't miss.
        let (Some(from), Some(target)) = (self.nodes.get(from.index()), self.nodes.get(to.index()))
        else {
            return Cost::ZERO;
        };
        let distance = from
            .gap(target)
            .saturating_sub(u32::from(self.tail_reach(to)));
        Cost::new(f64::from(distance) * self.cost_per_tile * (1.0 - 1e-9)).unwrap_or(Cost::ZERO)
    }
}

/// A lower bound on the delay of going between nodes, from where they are on the device.
///
/// This is [`LocationBound`] with delays in place of costs: the smallest ratio of a step's
/// delay to the tiles it can cover, over the same edges and with the same tail reach, so it is
/// admissible for the same reasons. It comes from [`RrGraph::timed_graph`], whose step delays
/// it bounds.
#[derive(Clone)]
pub struct RrDelayBound {
    location: LocationBound,
    picoseconds_per_tile: f64,
}

impl RrDelayBound {
    /// Returns the smallest delay of covering one tile, in picoseconds.
    pub fn picoseconds_per_tile(&self) -> f64 {
        self.picoseconds_per_tile
    }
}

impl fmt::Debug for RrDelayBound {
    /// Leaves out the tables.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RrDelayBound")
            .field("picoseconds_per_tile", &self.picoseconds_per_tile)
            .finish_non_exhaustive()
    }
}

impl DelayBound for RrDelayBound {
    #[inline]
    fn estimate(&self, from: NodeId, to: NodeId) -> u32 {
        let nodes = &self.location.nodes;
        let (Some(from), Some(target)) = (nodes.get(from.index()), nodes.get(to.index())) else {
            return 0;
        };
        let distance = from
            .gap(target)
            .saturating_sub(u32::from(self.location.tail_reach(to)));
        // Delays are whole picoseconds, so rounding down keeps the bound under their sum, and
        // the margin keeps a product that should be whole from rounding up past it.
        let estimate = (f64::from(distance) * self.picoseconds_per_tile * (1.0 - 1e-9)).floor();
        estimate as u32
    }
}

/// Converts a delay in seconds to whole picoseconds, rounding to the nearest and saturating.
fn picoseconds(seconds: f64) -> u32 {
    let value = (seconds * 1e12).round();
    if value.is_nan() || value <= 0.0 {
        0
    } else {
        // `as` saturates at the top.
        value as u32
    }
}

/// A routing resource graph, ready to route on, and what's needed to report on the routes.
///
/// # Costs
///
/// These follow VPR's default `--base_cost_type delay_normalized_length`, so that the costs
/// of a route are comparable to VPR's. Each node's base cost is one of these, where `f` is
/// the average delay per tile along a wire, in seconds:
///
/// | Node | Base cost |
/// | --- | --- |
/// | `SOURCE`, `OPIN`, `MUX` | `f` |
/// | `IPIN` | `0.875 f` |
/// | `SINK` | 0 |
/// | Wire | `f` × the length of its segment, in tiles |
///
/// `IPIN`s are slightly cheap to make the search reach for them early. VPR's cost at
/// criticality 0 is only the base cost of the node entered, with no charge for the switch,
/// so edges have a cost of 0, and a [`Graph`] step costs its target's base cost. Delays are
/// in the side tables for a timing-driven router to combine with these as VPR does.
///
/// `f` averages, over wire types by direction, `(T_linear + T_quadratic / length) / length`.
/// These use the median `R` and `C` of a type's nodes, and the switch values most common
/// among them, from a 10-bucket histogram of each node's average over its fan-in. A type with
/// a buffered switch has `T_linear = Tsw + Rsw (Cint + C) + 0.5 R (C + Cint)` and no
/// quadratic term, and an unbuffered one has `T_linear = Tsw + 0.5 Rsw C` and
/// `T_quadratic = 0.5 (Rsw + R) C`. All of this is in `alloc_and_load_rr_indexed_data.cpp` in
/// VPR, and is computed here in the same single-precision arithmetic. If no wire has delay,
/// `f` is 1 ns as it is in VPR.
///
/// [`read_rr_graph`] has the differences from VPR.
///
/// # Parallel edges
///
/// A [`Graph`] holds one edge per pair of nodes, and a route file needs to name a switch.
/// VPR's route reader looks at the first edge between two nodes, and rejects any other
/// switch unless told not to check. That first edge is the configurable one with the lowest
/// switch ID, as VPR sorts edges by source, then configurability, destination and switch.
/// So that's the switch this keeps, even if another has less delay. It's also the one that
/// VPR's timing analysis would then use, so a timing-driven router has to use the delays of
/// this switch for its numbers to match. The z1000 device has no parallel edges.
/// [`stats`](RrGraph::stats) has the numbers for a graph.
///
/// # Memory
///
/// Besides the [`Graph`], whose documentation gives its memory, this takes 20 bytes per node
/// for the table of types, locations and delays, 4 per node for the lookup, 2 per node for
/// the estimator, 4 per node and 2 per edge for the switch of each edge, plus small tables of
/// the distinct values.
/// [`heap_bytes`](RrGraph::heap_bytes) has the total.
#[derive(Clone)]
pub struct RrGraph {
    graph: Graph<LocationBound>,
    /// The same as the graph's estimator, which can't be got back from the graph.
    bound: LocationBound,
    nodes: Arc<[NodeInfo]>,
    /// The distinct `(R, C)` pairs, as `f32`s. Devices have a handful of wire types.
    rc: Box<[(f32, f32)]>,
    /// Layers of the nodes not on layer 0, which is all nodes on a 2D device, by ID.
    layers: Box<[(u32, u8, u8)]>,
    /// The start of each node's edges in `edge_switches`.
    edge_offsets: Box<[u32]>,
    /// The switch of each edge, in the order that [`RoutingSpace::expand`] produces them.
    edge_switches: Box<[u16]>,
    /// Node IDs by kind, `ptc` and position, to find nodes by binary search.
    lookup: Box<[u32]>,
    /// The most that nodes of a kind reach, as (major, minor) coordinates.
    max_reach: [(u16, u16); NodeKind::COUNT],
    switches: Box<[Option<Switch>]>,
    segments: Box<[Option<Segment>]>,
    block_types: Box<[BlockType]>,
    grid: Box<[GridTile]>,
    width: u32,
    height: u32,
    layer_count: u32,
    channel_width: Option<u32>,
    delay_normalization: f64,
    stats: RrStats,
}

impl fmt::Debug for RrGraph {
    /// Leaves out the tables, which can hold billions of entries.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RrGraph")
            .field("graph", &self.graph)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl RrGraph {
    /// Returns the graph, with base costs, capacities and a lower bound.
    #[inline]
    pub fn graph(&self) -> &Graph<LocationBound> {
        &self.graph
    }

    /// Returns the estimator that the graph uses.
    #[inline]
    pub fn bound(&self) -> &LocationBound {
        &self.bound
    }

    /// Returns the delay of stepping to `to` through the switch `switch`, in picoseconds.
    ///
    /// This is the Elmore delay that VPR's route tree gives a node reached through a buffered
    /// switch, when nothing else hangs off it: the switch's `Tdel` (its maximum, as VPR uses
    /// for setup timing), plus its resistance driving the node's capacitance, plus half of the
    /// node's own resistance driving that capacitance:
    ///
    /// ```text
    /// Tdel(switch) + R(switch) * C(to) + 0.5 * R(to) * C(to)
    /// ```
    ///
    /// See `load_route_tree_Tdel` in VPR's `route_tree.cpp`. **It is an approximation** in
    /// every other case, as a step can only have one delay whatever the tree around it:
    ///
    /// - Behind an unbuffered switch, such as a pass gate, VPR adds the resistance upstream of
    ///   the switch, back to the last buffer, so this is too low there.
    /// - The capacitance a node drives includes the internal capacitance of the switches out
    ///   of it that the tree uses, and through unbuffered ones everything beyond them. This
    ///   leaves those out, so it is too low for a node that the tree branches at.
    ///
    /// On an architecture whose switches are all buffered, which is usual for modern ones, the
    /// only difference is the internal capacitance of branches. A switch or node with no timing
    /// counts as 0, and so does an unknown switch.
    pub fn step_delay(&self, switch: u16, to: NodeId) -> u32 {
        let (r_switch, t_switch) = self
            .switch(switch)
            .map_or((0.0, 0.0), |switch| (switch.r, switch.t_del_max));
        let (r_node, c_node) = self.node_rc(to).unwrap_or((0.0, 0.0));
        let (r_switch, t_switch) = (f64::from(r_switch), f64::from(t_switch));
        let (r_node, c_node) = (f64::from(r_node), f64::from(c_node));
        picoseconds(t_switch + r_switch * c_node + 0.5 * r_node * c_node)
    }

    /// Returns the graph with the delay of every step, for timing-driven routing.
    ///
    /// Each step's delay is [`step_delay`](RrGraph::step_delay) through the switch that a route
    /// file would name, which is also the one VPR's timing analysis would use: see [parallel
    /// edges](RrGraph#parallel-edges). The estimator is an [`RrDelayBound`].
    ///
    /// The graph's base costs are in seconds, as VPR's are, so a cost of `1e-12` per
    /// picosecond of delay weighs the two as VPR's router does. This takes 2 bytes per edge in
    /// the usual case of at most 65536 distinct delays, and a pass over every edge to make.
    pub fn timed_graph(&self) -> TimedGraph<'_, LocationBound, RrDelayBound> {
        // Asked in the order of the edges, which is the order of `edge_switches`.
        let mut edge = 0usize;
        let timed = self.graph.with_step_delays(|_, to| {
            let switch = self.edge_switches.get(edge).copied().unwrap_or(u16::MAX);
            edge += 1;
            self.step_delay(switch, to)
        });

        // The same edges as `bound_tables` counts, so that the tail reach applies: none into a
        // dead end, or into a tail that only leads to dead ends.
        let mut steps = Vec::new();
        let is_dead_end = |node: NodeId| self.graph.out_degree(node) == 0;
        let is_tail: Vec<bool> = (0..self.graph.num_nodes())
            .map(|node| {
                steps.clear();
                self.graph.expand(NetId(0), NodeId(node), &mut steps);
                !steps.is_empty() && steps.iter().all(|step| is_dead_end(step.to))
            })
            .collect();

        let mut ratio = f64::INFINITY;
        let mut delays = Vec::new();
        for from in 0..self.graph.num_nodes() {
            let from = NodeId(from);
            steps.clear();
            delays.clear();
            self.graph.expand(NetId(0), from, &mut steps);
            timed.step_delays(NetId(0), from, &mut delays);
            for (step, &delay) in steps.iter().zip(&delays) {
                if is_dead_end(step.to) || is_tail[step.to.index()] {
                    continue;
                }
                let (Some(a), Some(b)) = (self.info(from), self.info(step.to)) else {
                    continue;
                };
                let tiles = a.gap(b) + b.span();
                if tiles > 0 {
                    ratio = ratio.min(f64::from(delay) / f64::from(tiles));
                }
            }
        }
        if !ratio.is_finite() {
            ratio = 0.0;
        }
        timed.with_delay_bound(RrDelayBound {
            location: self.bound.clone(),
            picoseconds_per_tile: ratio,
        })
    }

    /// Returns the facts about this graph from reading its file.
    #[inline]
    pub fn stats(&self) -> &RrStats {
        &self.stats
    }

    /// Returns the factor VPR calls `delay_normalization_fac`: the average delay per tile
    /// along a wire, in seconds. A timing-driven router needs it to weigh delay against cost.
    #[inline]
    pub fn delay_normalization(&self) -> f64 {
        self.delay_normalization
    }

    /// Returns the device's size in tiles, as a width and a height.
    #[inline]
    pub fn grid_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Returns the widest channel, if the file has a `channels` section.
    #[inline]
    pub fn channel_width(&self) -> Option<u32> {
        self.channel_width
    }

    /// Returns what's on a tile of the device.
    pub fn tile(&self, x: u32, y: u32, layer: u32) -> Option<&GridTile> {
        if x >= self.width || y >= self.height || layer >= self.layer_count {
            return None;
        }
        let index = (u64::from(layer) * u64::from(self.height) + u64::from(y))
            * u64::from(self.width)
            + u64::from(x);
        self.grid.get(usize::try_from(index).ok()?)
    }

    /// Returns the block types. [`GridTile::block_type`] is an index in this.
    #[inline]
    pub fn block_types(&self) -> &[BlockType] {
        &self.block_types
    }

    /// Returns the switch with an ID as used in edges and route files, if there is one.
    #[inline]
    pub fn switch(&self, id: u16) -> Option<&Switch> {
        self.switches.get(usize::from(id))?.as_ref()
    }

    /// Returns the segment with an ID, if there is one.
    #[inline]
    pub fn segment(&self, id: u16) -> Option<&Segment> {
        self.segments.get(usize::from(id))?.as_ref()
    }

    #[inline]
    fn info(&self, node: NodeId) -> Option<&NodeInfo> {
        self.nodes.get(node.index())
    }

    /// Returns what kind of node this is.
    #[inline]
    pub fn node_kind(&self, node: NodeId) -> Option<NodeKind> {
        Some(self.info(node)?.kind)
    }

    /// Returns where a node is.
    pub fn node_location(&self, node: NodeId) -> Option<NodeLocation> {
        let info = self.info(node)?;
        let (layer_low, layer_high) = self
            .layers
            .binary_search_by_key(&node.0, |entry| entry.0)
            .map_or((0, 0), |at| (self.layers[at].1, self.layers[at].2));
        Some(NodeLocation {
            xlow: info.xlow,
            ylow: info.ylow,
            xhigh: info.xhigh,
            yhigh: info.yhigh,
            layer_low,
            layer_high,
        })
    }

    /// Returns a node's `ptc`: its pin, class or track number, depending on its kind.
    ///
    /// A node can have a list of these in tileable graphs, in which case this is the first,
    /// which is the one that VPR puts in a route file.
    #[inline]
    pub fn node_ptc(&self, node: NodeId) -> Option<u32> {
        Some(self.info(node)?.ptc)
    }

    /// Returns which way a wire runs, or `None` for anything else or if the file doesn't say.
    pub fn node_direction(&self, node: NodeId) -> Option<ChannelDirection> {
        match self.info(node)?.packed & DIRECTION_MASK {
            1 => Some(ChannelDirection::Increasing),
            2 => Some(ChannelDirection::Decreasing),
            3 => Some(ChannelDirection::Bidirectional),
            _ => None,
        }
    }

    /// Returns the wire type of a wire, if the file gives one.
    #[inline]
    pub fn node_segment(&self, node: NodeId) -> Option<u16> {
        let segment = self.info(node)?.segment;
        (segment != NO_SEGMENT).then_some(segment)
    }

    /// Returns which sides of its tile a pin is on: left, right, top and bottom.
    pub fn node_sides(&self, node: NodeId) -> Option<[bool; 4]> {
        let packed = self.info(node)?.packed;
        Some([
            packed & SIDE_LEFT != 0,
            packed & SIDE_RIGHT != 0,
            packed & SIDE_TOP != 0,
            packed & SIDE_BOTTOM != 0,
        ])
    }

    /// Returns a node's resistance in ohms, and its capacitance in farads, as the `timing` of
    /// the node gives them. These are 0 if it has none.
    #[inline]
    pub fn node_rc(&self, node: NodeId) -> Option<(f32, f32)> {
        let info = self.info(node)?;
        Some(self.rc.get(info.rc as usize).copied().unwrap_or((0.0, 0.0)))
    }

    /// Returns the switch of each edge out of a node, in the order that
    /// [`RoutingSpace::expand`] produces its steps, so a router can look up the delay of
    /// a step from its position.
    ///
    /// This is empty for a node outside the graph.
    pub fn out_switches(&self, node: NodeId) -> &[u16] {
        let start = self.edge_offsets.get(node.index()).copied();
        let end = self.edge_offsets.get(node.index() + 1).copied();
        match (start, end) {
            (Some(start), Some(end)) => self
                .edge_switches
                .get(start as usize..end as usize)
                .unwrap_or(&[]),
            _ => &[],
        }
    }

    /// Returns the switch of the edge between two nodes, if they're connected.
    ///
    /// If the file has several, this is the one to name in a route file: see the [parallel
    /// edges](RrGraph#parallel-edges) section. This scans the edges out of `from`. A router
    /// that has the position from an expansion can use
    /// [`out_switches`](RrGraph::out_switches), which is faster.
    pub fn edge_switch(&self, from: NodeId, to: NodeId) -> Option<u16> {
        self.edge_switch_with(from, to, &mut Vec::new())
    }

    /// As [`edge_switch`](RrGraph::edge_switch), reusing a caller's buffer for the expansion.
    pub(crate) fn edge_switch_with(
        &self,
        from: NodeId,
        to: NodeId,
        steps: &mut Vec<Step<NodeId>>,
    ) -> Option<u16> {
        steps.clear();
        self.graph.expand(NetId(0), from, steps);
        let position = steps.iter().position(|step| step.to == to)?;
        self.out_switches(from).get(position).copied()
    }

    /// Finds the nodes of a kind with a `ptc`, whose boxes include a tile.
    ///
    /// Wires and blocks larger than one tile are found from any tile they cover. A `ptc` can
    /// match more than one node: VPR makes one `IPIN` or `OPIN` per side that a pin is on,
    /// for instance. The nodes are in order of ID.
    pub fn find_nodes(&self, kind: NodeKind, x: u32, y: u32, ptc: u32) -> Vec<NodeId> {
        let mut found = Vec::new();
        let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) else {
            return found;
        };
        let (major, minor) = if kind == NodeKind::ChanX {
            (y, x)
        } else {
            (x, y)
        };
        let (major_reach, minor_reach) = self.max_reach[kind as usize];
        let key = |&id: &u32| {
            let info = &self.nodes[id as usize];
            let (node_major, node_minor) = info.major_minor();
            (info.kind, info.ptc, node_major, node_minor)
        };
        for row in major.saturating_sub(major_reach)..=major {
            let end = self
                .lookup
                .partition_point(|id| key(id) <= (kind, ptc, row, minor));
            for &id in self.lookup[..end].iter().rev() {
                let (found_kind, found_ptc, found_row, found_minor) = key(&id);
                if (found_kind, found_ptc, found_row) != (kind, ptc, row)
                    || found_minor < minor.saturating_sub(minor_reach)
                {
                    break;
                }
                if self.nodes[id as usize].covers(x, y) {
                    found.push(NodeId(id));
                }
            }
        }
        found.sort_unstable();
        found
    }

    /// Returns the bytes this holds on the heap, including the graph's.
    pub fn heap_bytes(&self) -> usize {
        let pins: usize = self
            .block_types
            .iter()
            .flat_map(|block| &block.classes)
            .map(|class| class.pins.len() * size_of::<Pin>())
            .sum();
        self.graph.heap_bytes()
            + size_of::<NodeInfo>() * self.nodes.len()
            + size_of::<(f32, f32)>() * self.rc.len()
            + size_of::<(u32, u8, u8)>() * self.layers.len()
            + size_of::<u32>() * self.edge_offsets.len()
            + size_of::<u16>() * self.edge_switches.len()
            + size_of::<u32>() * self.lookup.len()
            + size_of::<u16>() * self.nodes.len()
            + size_of::<GridTile>() * self.grid.len()
            + pins
    }
}

/// An edge as the file gives it.
#[derive(Clone, Copy)]
struct FileEdge {
    from: u32,
    to: u32,
    switch: u16,
}

/// What a `<grid_loc>` said.
#[derive(Clone, Copy)]
struct GridEntry {
    layer: u16,
    x: u16,
    y: u16,
    block_type_id: u32,
    width_offset: u16,
    height_offset: u16,
}

/// The sections a graph needs, as they fill up while reading the file.
#[derive(Default)]
struct Sections {
    switches: Vec<Option<Switch>>,
    segments: Vec<Option<Segment>>,
    block_types: Vec<BlockType>,
    grid: Vec<GridEntry>,
    nodes: Vec<NodeInfo>,
    /// The ID of each node, once the nodes have turned out not to be in order of ID. Stays
    /// empty otherwise, which is the case in every file that VPR writes.
    node_ids: Vec<u32>,
    rc: Vec<(f32, f32)>,
    capacities: Vec<(u32, u32)>,
    layers: Vec<(u32, u8, u8)>,
    edges: Vec<FileEdge>,
    channel_width: Option<u32>,
    seen_switches: bool,
    seen_block_types: bool,
    seen_grid: bool,
    seen_nodes: bool,
    seen_edges: bool,
}

/// Which section the reader is in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Other,
    Channels,
    Switches,
    Segments,
    BlockTypes,
    Grid,
    Nodes,
    Edges,
}

fn invalid(line: u64, what: &'static str, value: &str) -> ImportError {
    ImportError::InvalidValue {
        line,
        what,
        value: value.chars().take(64).collect(),
    }
}

fn required<'a>(tag: &Tag<'a>, attribute: &'static str) -> Result<&'a str, ImportError> {
    tag.attr(attribute)
        .ok_or_else(|| ImportError::MissingAttribute {
            line: tag.line(),
            element: tag.name().to_owned(),
            attribute,
        })
}

fn parse_int<T: core::str::FromStr>(
    tag: &Tag<'_>,
    attribute: &'static str,
    value: &str,
) -> Result<T, ImportError> {
    value
        .parse()
        .map_err(|_| invalid(tag.line(), attribute, value))
}

fn required_int<T: core::str::FromStr>(
    tag: &Tag<'_>,
    attribute: &'static str,
) -> Result<T, ImportError> {
    parse_int(tag, attribute, required(tag, attribute)?)
}

/// Reads an optional physical quantity, which can't be negative and has to fit in the `f32`
/// that VPR keeps it in.
fn optional_quantity(tag: &Tag<'_>, attribute: &'static str) -> Result<Option<f32>, ImportError> {
    let Some(value) = tag.attr(attribute) else {
        return Ok(None);
    };
    let parsed: f32 = parse_int(tag, attribute, value)?;
    if parsed.is_finite() && parsed >= 0.0 {
        Ok(Some(parsed))
    } else {
        Err(invalid(tag.line(), attribute, value))
    }
}

/// Reads a switch's `timing` element, whose rules are in the schema: `Tdel` is both delays,
/// and a missing `Tdel_min` or `Tdel_max` is 0.
fn read_switch_timing(tag: &Tag<'_>, switch: &mut Switch) -> Result<(), ImportError> {
    switch.r = optional_quantity(tag, "R")?.unwrap_or(0.0);
    switch.c_in = optional_quantity(tag, "Cin")?.unwrap_or(0.0);
    switch.c_out = optional_quantity(tag, "Cout")?.unwrap_or(0.0);
    switch.c_internal = optional_quantity(tag, "Cinternal")?.unwrap_or(0.0);
    let single = optional_quantity(tag, "Tdel")?;
    let min = optional_quantity(tag, "Tdel_min")?;
    let max = optional_quantity(tag, "Tdel_max")?;
    if single.is_some() && (min.is_some() || max.is_some()) {
        return Err(invalid(
            tag.line(),
            "switch timing",
            "Tdel with Tdel_min or Tdel_max",
        ));
    }
    let (min, max) = match single {
        Some(delay) => (delay, delay),
        None => (min.unwrap_or(0.0), max.unwrap_or(0.0)),
    };
    if min > max {
        return Err(invalid(
            tag.line(),
            "switch timing",
            "Tdel_min above Tdel_max",
        ));
    }
    switch.t_del_min = min;
    switch.t_del_max = max;
    Ok(())
}

/// Turns the `side` attribute of a pin into its bits. It's a list such as `TOP_RIGHT`.
fn parse_side(line: u64, value: &str) -> Result<u8, ImportError> {
    let mut bits = 0;
    for side in value.split('_') {
        bits |= match side {
            "LEFT" => SIDE_LEFT,
            "RIGHT" => SIDE_RIGHT,
            "TOP" => SIDE_TOP,
            "BOTTOM" => SIDE_BOTTOM,
            _ => return Err(invalid(line, "side", value)),
        };
    }
    Ok(bits)
}

/// Puts a value at `id` of a table that has an ID up to 65534, which is the most that VPR's
/// `short` allows for anything here, and which rules out a huge table from a tiny file.
fn insert_by_id<T>(
    table: &mut Vec<Option<T>>,
    line: u64,
    what: &'static str,
    id: u32,
    value: T,
) -> Result<(), ImportError> {
    let index = usize::try_from(id).unwrap_or(usize::MAX);
    if index >= usize::from(NO_SEGMENT) {
        return Err(ImportError::BadReference { line, what, id });
    }
    if table.len() <= index {
        table.resize_with(index + 1, || None);
    }
    if table[index].is_some() {
        return Err(ImportError::BadReference { line, what, id });
    }
    table[index] = Some(value);
    Ok(())
}

/// Reads a routing resource graph, as in `rr_graph.xml`, from a stream.
///
/// The file is read as it arrives, with memory that depends on the size of the graph and not
/// on the amount of text it's written in. See [`RrGraph`] for what comes out.
///
/// # Differences from VPR
///
/// The base costs are VPR's, except in two ways that don't matter on regular devices:
///
/// - VPR gets a segment's length from the architecture file. The file doesn't need to have
///   it, so without a `length` attribute this uses the length of the longest wire of the
///   segment. This gives the same answer on any device with a segment fully on it, as VPR
///   caps a length at the device's width anyway.
/// - If a wire type appears as only `CHANX` or only `CHANY`, VPR averages a copy of the delay
///   of an orthogonal type into the normalization factor, for the type that doesn't occur.
///   Here, only the wire types that occur are averaged.
///
/// Self-loops are dropped, as they can't be in a route.
///
/// # Errors
///
/// Fails if the XML is malformed or a read fails, with the line where it happened. So
/// does anything that doesn't hold together as a graph: unknown switches or segments, node
/// IDs that aren't exactly `0..n`, negative coordinates, delays that are negative or not
/// finite, a grid that doesn't list every tile once, and IDs of switches or segments of
/// 65535 or more. See [`ImportError`].
///
/// # Examples
///
/// See the [module documentation](crate::fpga).
pub fn read_rr_graph<R: BufRead>(reader: R) -> Result<RrGraph, ImportError> {
    let mut xml = XmlReader::new(reader);
    let mut sections = Sections::default();

    let mut depth = 0u32;
    let mut section = Section::Other;
    let mut current_switch: Option<(u32, Switch)> = None;
    let mut current_segment: Option<(u32, Segment)> = None;
    let mut current_block: Option<BlockType> = None;
    let mut current_class: Option<PinClass> = None;
    let mut current_pin: Option<Pin> = None;
    let mut current_node: Option<PendingNode> = None;
    // Node after node has the same timing, so the last answer is usually the right one.
    let mut rc_index: HashMap<(u32, u32), u32> = HashMap::new();
    let mut last_rc: Option<((u32, u32), u32)> = None;

    while let Some(event) = xml.next_event()? {
        match event {
            Event::Start(tag) => {
                depth += 1;
                let line = tag.line();
                match depth {
                    1 => {
                        if tag.name() != "rr_graph" {
                            return Err(ImportError::Malformed {
                                line,
                                message: format!("expected <rr_graph>, found <{}>", tag.name()),
                            });
                        }
                    }
                    2 => {
                        section = match tag.name() {
                            "channels" => Section::Channels,
                            "switches" => {
                                sections.seen_switches = true;
                                Section::Switches
                            }
                            "segments" => Section::Segments,
                            "block_types" => {
                                sections.seen_block_types = true;
                                Section::BlockTypes
                            }
                            "grid" => {
                                sections.seen_grid = true;
                                Section::Grid
                            }
                            "rr_nodes" => {
                                sections.seen_nodes = true;
                                Section::Nodes
                            }
                            "rr_edges" => {
                                sections.seen_edges = true;
                                Section::Edges
                            }
                            _ => Section::Other,
                        };
                    }
                    _ => match section {
                        Section::Edges if depth == 3 && tag.name() == "edge" => {
                            read_edge(&tag, &mut sections)?;
                        }
                        Section::Nodes if depth == 3 && tag.name() == "node" => {
                            current_node = Some(PendingNode::start(&tag)?);
                        }
                        Section::Nodes if depth == 4 => {
                            if let Some(node) = &mut current_node {
                                match tag.name() {
                                    "loc" => node.read_loc(&tag)?,
                                    "timing" => {
                                        let bits = read_node_timing(&tag)?;
                                        let rc = match last_rc {
                                            Some((last, index)) if last == bits => index,
                                            _ => {
                                                let next = rc_index.len() as u32;
                                                let index = *rc_index.entry(bits).or_insert(next);
                                                if index == next {
                                                    sections.rc.push((
                                                        f32::from_bits(bits.0),
                                                        f32::from_bits(bits.1),
                                                    ));
                                                }
                                                last_rc = Some((bits, index));
                                                index
                                            }
                                        };
                                        node.info.rc = rc;
                                    }
                                    "segment" => {
                                        let id: u32 = required_int(&tag, "segment_id")?;
                                        if id >= u32::from(NO_SEGMENT) {
                                            return Err(ImportError::BadReference {
                                                line,
                                                what: "segment",
                                                id,
                                            });
                                        }
                                        // If it isn't defined yet, `finish` will check.
                                        if !sections.segments.is_empty()
                                            && sections
                                                .segments
                                                .get(id as usize)
                                                .is_none_or(Option::is_none)
                                        {
                                            return Err(ImportError::BadReference {
                                                line,
                                                what: "segment",
                                                id,
                                            });
                                        }
                                        node.info.segment = id as u16;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Section::Channels if depth == 3 && tag.name() == "channel" => {
                            sections.channel_width = Some(required_int(&tag, "chan_width_max")?);
                        }
                        Section::Switches if depth == 3 && tag.name() == "switch" => {
                            let id = required_int(&tag, "id")?;
                            let kind = match tag.attr("type") {
                                None | Some("mux") => SwitchKind::Mux,
                                Some("tristate") => SwitchKind::Tristate,
                                Some("pass_gate") => SwitchKind::PassGate,
                                Some("short") => SwitchKind::Short,
                                Some("buffer") => SwitchKind::Buffer,
                                Some(other) => return Err(invalid(line, "switch type", other)),
                            };
                            current_switch = Some((
                                id,
                                Switch {
                                    name: tag.attr("name").unwrap_or_default().to_owned(),
                                    kind,
                                    r: 0.0,
                                    c_in: 0.0,
                                    c_out: 0.0,
                                    c_internal: 0.0,
                                    t_del_min: 0.0,
                                    t_del_max: 0.0,
                                },
                            ));
                        }
                        Section::Switches if depth == 4 && tag.name() == "timing" => {
                            if let Some((_, switch)) = &mut current_switch {
                                read_switch_timing(&tag, switch)?;
                            }
                        }
                        Section::Segments if depth == 3 && tag.name() == "segment" => {
                            let id = required_int(&tag, "id")?;
                            let length = match tag.attr("length") {
                                Some(value) => Some(parse_int(&tag, "length", value)?),
                                None => None,
                            };
                            current_segment = Some((
                                id,
                                Segment {
                                    name: tag.attr("name").unwrap_or_default().to_owned(),
                                    length,
                                    r_per_meter: 0.0,
                                    c_per_meter: 0.0,
                                },
                            ));
                        }
                        Section::Segments if depth == 4 && tag.name() == "timing" => {
                            if let Some((_, segment)) = &mut current_segment {
                                segment.r_per_meter =
                                    optional_quantity(&tag, "R_per_meter")?.unwrap_or(0.0);
                                segment.c_per_meter =
                                    optional_quantity(&tag, "C_per_meter")?.unwrap_or(0.0);
                            }
                        }
                        Section::BlockTypes if depth == 3 && tag.name() == "block_type" => {
                            current_block = Some(BlockType {
                                id: required_int(&tag, "id")?,
                                name: required(&tag, "name")?.to_owned(),
                                width: required_int(&tag, "width")?,
                                height: required_int(&tag, "height")?,
                                classes: Vec::new(),
                                by_ptc: HashMap::new(),
                                by_slot: HashMap::new(),
                                is_io: false,
                            });
                        }
                        Section::BlockTypes if depth == 4 && tag.name() == "pin_class" => {
                            let kind = match required(&tag, "type")? {
                                "INPUT" => PinClassKind::Input,
                                "OUTPUT" => PinClassKind::Output,
                                "OPEN" => PinClassKind::Open,
                                other => return Err(invalid(line, "pin class type", other)),
                            };
                            current_class = Some(PinClass {
                                kind,
                                pins: Vec::new(),
                            });
                        }
                        Section::BlockTypes if depth == 5 && tag.name() == "pin" => {
                            current_pin = Some(Pin {
                                ptc: required_int(&tag, "ptc")?,
                                name: String::new(),
                            });
                        }
                        Section::Grid if depth == 3 && tag.name() == "grid_loc" => {
                            sections.grid.push(GridEntry {
                                layer: match tag.attr("layer") {
                                    Some(value) => parse_int(&tag, "layer", value)?,
                                    None => 0,
                                },
                                x: required_int(&tag, "x")?,
                                y: required_int(&tag, "y")?,
                                block_type_id: required_int(&tag, "block_type_id")?,
                                width_offset: required_int(&tag, "width_offset")?,
                                height_offset: required_int(&tag, "height_offset")?,
                            });
                        }
                        _ => {}
                    },
                }
            }
            Event::Text(text) => {
                if section == Section::BlockTypes
                    && let Some(pin) = &mut current_pin
                {
                    pin.name.push_str(text.trim());
                }
            }
            Event::End => {
                match (section, depth) {
                    (Section::Nodes, 3) => {
                        if let Some(node) = current_node.take() {
                            node.finish(&mut sections)?;
                        }
                    }
                    (Section::Switches, 3) => {
                        if let Some((id, switch)) = current_switch.take() {
                            insert_by_id(&mut sections.switches, xml.line(), "switch", id, switch)?;
                        }
                    }
                    (Section::Segments, 3) => {
                        if let Some((id, segment)) = current_segment.take() {
                            insert_by_id(
                                &mut sections.segments,
                                xml.line(),
                                "segment",
                                id,
                                segment,
                            )?;
                        }
                    }
                    (Section::BlockTypes, 5) => {
                        if let (Some(pin), Some(class)) = (current_pin.take(), &mut current_class) {
                            class.pins.push(pin);
                        }
                    }
                    (Section::BlockTypes, 4) => {
                        if let (Some(class), Some(block)) =
                            (current_class.take(), &mut current_block)
                        {
                            block.classes.push(class);
                        }
                    }
                    (Section::BlockTypes, 3) => {
                        if let Some(mut block) = current_block.take() {
                            block.index_pins();
                            sections.block_types.push(block);
                        }
                    }
                    _ => {}
                }
                if depth == 2 {
                    section = Section::Other;
                }
                depth -= 1;
            }
        }
    }

    finish(sections, xml.line())
}

/// A node whose `<loc>` may not have been read yet.
struct PendingNode {
    id: u32,
    capacity: u32,
    info: NodeInfo,
    has_location: bool,
    layers: (u8, u8),
    line: u64,
}

impl PendingNode {
    fn start(tag: &Tag<'_>) -> Result<PendingNode, ImportError> {
        let line = tag.line();
        let kind_name = required(tag, "type")?;
        let kind =
            NodeKind::parse(kind_name).ok_or_else(|| invalid(line, "node type", kind_name))?;
        let direction = match tag.attr("direction") {
            None | Some("NONE") => 0,
            Some("INC_DIR") => 1,
            Some("DEC_DIR") => 2,
            Some("BI_DIR") => 3,
            Some(other) => return Err(invalid(line, "node direction", other)),
        };
        Ok(PendingNode {
            id: required_int(tag, "id")?,
            capacity: required_int(tag, "capacity")?,
            info: NodeInfo {
                xlow: 0,
                ylow: 0,
                xhigh: 0,
                yhigh: 0,
                ptc: 0,
                rc: u32::MAX,
                segment: NO_SEGMENT,
                kind,
                packed: direction,
            },
            has_location: false,
            layers: (0, 0),
            line,
        })
    }

    fn read_loc(&mut self, tag: &Tag<'_>) -> Result<(), ImportError> {
        let line = tag.line();
        let info = &mut self.info;
        info.xlow = required_int(tag, "xlow")?;
        info.ylow = required_int(tag, "ylow")?;
        info.xhigh = required_int(tag, "xhigh")?;
        info.yhigh = required_int(tag, "yhigh")?;
        if info.xlow > info.xhigh || info.ylow > info.yhigh {
            return Err(invalid(line, "node location", required(tag, "xlow")?));
        }
        // A list in tileable graphs, of which VPR writes the first in a route.
        let ptc = required(tag, "ptc")?;
        let first = ptc.split_once(',').map_or(ptc, |(first, _)| first);
        info.ptc = parse_int(tag, "ptc", first.trim())?;
        if let Some(side) = tag.attr("side") {
            info.packed |= parse_side(line, side)?;
        }
        // `layer_low` and `layer_high` now, and `layer` in files from before 3D wires.
        let layer = match tag.attr("layer") {
            Some(value) => parse_int(tag, "layer", value)?,
            None => 0,
        };
        let low = match tag.attr("layer_low") {
            Some(value) => parse_int(tag, "layer_low", value)?,
            None => layer,
        };
        let high = match tag.attr("layer_high") {
            Some(value) => parse_int(tag, "layer_high", value)?,
            None => low.max(layer),
        };
        self.layers = (low, high);
        self.has_location = true;
        Ok(())
    }

    fn finish(self, sections: &mut Sections) -> Result<(), ImportError> {
        if !self.has_location {
            return Err(ImportError::MissingAttribute {
                line: self.line,
                element: "node".to_owned(),
                attribute: "loc",
            });
        }
        let next = sections.nodes.len() as u64;
        if !sections.node_ids.is_empty() || u64::from(self.id) != next {
            if sections.node_ids.is_empty() {
                sections.node_ids.extend(0..next as u32);
            }
            sections.node_ids.push(self.id);
        }
        if next >= u64::from(u32::MAX) {
            return Err(ImportError::BadNodeId {
                line: self.line,
                id: self.id,
            });
        }
        sections.nodes.push(self.info);
        if self.capacity != 1 {
            sections.capacities.push((self.id, self.capacity));
        }
        if self.layers != (0, 0) {
            sections
                .layers
                .push((self.id, self.layers.0, self.layers.1));
        }
        Ok(())
    }
}

/// Reads a node's `timing`, as bit patterns so that it can be a map's key. The values are
/// always finite, and are not negative zero, which [`optional_quantity`]'s check lets through
/// and which normalizes to the same bit pattern as zero here.
fn read_node_timing(tag: &Tag<'_>) -> Result<(u32, u32), ImportError> {
    let r = optional_quantity(tag, "R")?.unwrap_or(0.0) + 0.0;
    let c = optional_quantity(tag, "C")?.unwrap_or(0.0) + 0.0;
    Ok((r.to_bits(), c.to_bits()))
}

fn read_edge(tag: &Tag<'_>, sections: &mut Sections) -> Result<(), ImportError> {
    let line = tag.line();
    let from: u32 = required_int(tag, "src_node")?;
    let to: u32 = required_int(tag, "sink_node")?;
    let switch: u32 = required_int(tag, "switch_id")?;
    // Edges nearly always come after nodes and switches, so most checks can give a line. If
    // not, `finish` makes them with no better line than the end of the file.
    if sections.seen_nodes && !sections.nodes.is_empty() {
        let num_nodes = sections.nodes.len() as u64;
        for id in [from, to] {
            if u64::from(id) >= num_nodes {
                return Err(ImportError::BadNodeId { line, id });
            }
        }
    }
    if switch >= u32::from(u16::MAX) {
        return Err(ImportError::BadReference {
            line,
            what: "switch",
            id: switch,
        });
    }
    if sections.seen_switches
        && !sections.switches.is_empty()
        && sections
            .switches
            .get(switch as usize)
            .is_none_or(Option::is_none)
    {
        return Err(ImportError::BadReference {
            line,
            what: "switch",
            id: switch,
        });
    }
    if sections.edges.len() >= u32::MAX as usize {
        return Err(ImportError::TooManyEdges);
    }
    sections.edges.push(FileEdge {
        from,
        to,
        switch: switch as u16,
    });
    Ok(())
}

/// Puts the nodes in order of ID and checks that the IDs are exactly `0..n`.
fn order_nodes(sections: &mut Sections, line: u64) -> Result<(), ImportError> {
    if sections.node_ids.is_empty() {
        return Ok(());
    }
    let ids = core::mem::take(&mut sections.node_ids);
    // A slot for each node, which each ID has to fill exactly once. If an ID is out of range
    // or repeated, then a slot can't be filled, and that's reported here.
    let mut order = vec![u32::MAX; ids.len()];
    for (at, &id) in ids.iter().enumerate() {
        let Some(slot) = order.get_mut(id as usize) else {
            return Err(ImportError::BadNodeId { line, id });
        };
        if *slot != u32::MAX {
            return Err(ImportError::BadNodeId { line, id });
        }
        *slot = at as u32;
    }
    let nodes = core::mem::take(&mut sections.nodes);
    sections.nodes = order.iter().map(|&at| nodes[at as usize]).collect();
    Ok(())
}

/// Sorts edges by source into position, drops those that a [`Graph`] can't hold and keeps
/// the switch that's right to name in a route file. Returns offsets, targets and switches.
///
/// Stable, so that edges stay in file order within a node. The [`GraphBuilder`] then doesn't
/// have anything left to merge, so the order is the same in the graph as here.
fn sort_edges(
    num_nodes: usize,
    edges: Vec<FileEdge>,
    switches: &[Option<Switch>],
    stats: &mut RrStats,
) -> (Vec<u32>, Vec<u32>, Vec<u16>) {
    let mut offsets = vec![0u32; num_nodes + 1];
    for edge in &edges {
        offsets[edge.from as usize + 1] += 1;
    }
    for node in 0..num_nodes {
        offsets[node + 1] += offsets[node];
    }
    let mut next = offsets.clone();
    let mut targets = vec![0u32; edges.len()];
    let mut edge_switches = vec![0u16; edges.len()];
    for edge in edges {
        let slot = &mut next[edge.from as usize];
        targets[*slot as usize] = edge.to;
        edge_switches[*slot as usize] = edge.switch;
        *slot += 1;
    }
    drop(next);

    // VPR's route reader goes by the first edge in its own order, which puts configurable
    // edges first, and then the lowest switch. Lower is better here.
    let rank = |switch: u16| {
        let configurable = switches[usize::from(switch)]
            .as_ref()
            .is_some_and(|switch| switch.kind.is_configurable());
        (!configurable, switch)
    };

    // Where each target was last written. Positions before the node's start belong to other
    // nodes, so this never needs resetting.
    const UNSEEN: u32 = u32::MAX;
    let mut written_at = vec![UNSEEN; num_nodes];
    // The repeated targets of the current node, and whether each repeat changed the switch.
    let mut repeats: Vec<(u32, bool)> = Vec::new();
    let mut write = 0u32;
    let mut read = 0usize;
    for node in 0..num_nodes {
        let end = offsets[node + 1] as usize;
        let start = write;
        offsets[node] = start;
        while read < end {
            let (target, switch) = (targets[read], edge_switches[read]);
            read += 1;
            if rank(switch).0 {
                stats.non_configurable_edges += 1;
            }
            if target as usize == node {
                stats.self_loops += 1;
                continue;
            }
            let seen = written_at[target as usize];
            if seen != UNSEEN && seen >= start {
                let kept = &mut edge_switches[seen as usize];
                stats.parallel_edges += 1;
                repeats.push((target, switch != *kept));
                if rank(switch) < rank(*kept) {
                    *kept = switch;
                }
            } else {
                written_at[target as usize] = write;
                targets[write as usize] = target;
                edge_switches[write as usize] = switch;
                write += 1;
            }
        }
        if !repeats.is_empty() {
            repeats.sort_unstable_by_key(|repeat| repeat.0);
            for pair in repeats.chunk_by(|a, b| a.0 == b.0) {
                stats.parallel_pairs += 1;
                if pair.iter().any(|repeat| repeat.1) {
                    stats.parallel_pairs_with_different_switches += 1;
                }
            }
            repeats.clear();
        }
    }
    offsets[num_nodes] = write;
    targets.truncate(write as usize);
    edge_switches.truncate(write as usize);
    (offsets, targets, edge_switches)
}

/// Everything VPR's cost scheme needs for one wire type in one direction.
#[derive(Default)]
struct CostIndex {
    /// One over the length in tiles, as VPR keeps it.
    inverse_length: f32,
    /// The longest wire seen, for segments that don't state a length.
    longest: u32,
    r_values: Vec<f32>,
    c_values: Vec<f32>,
    /// The range of a node's average switch values: `R`, `Tdel` and `Cinternal`.
    low: [f32; 3],
    high: [f32; 3],
    /// How many nodes fall in each of 10 buckets of those ranges.
    buckets: [[u64; 10]; 3],
    buffered: bool,
    nodes: u64,
}

/// A node's fan-in, summed. Doubles, as VPR uses here, since it's many small values added.
#[derive(Clone, Copy, Default)]
struct FanIn {
    r: f64,
    t: f64,
    c_internal: f64,
    switches: u32,
    buffered: bool,
}

impl FanIn {
    /// Returns the average switch values as `f32`, in which VPR stores them.
    fn average(&self) -> [f32; 3] {
        let per = 1.0 / f64::from(self.switches);
        [
            (self.r * per) as f32,
            (self.t * per) as f32,
            (self.c_internal * per) as f32,
        ]
    }
}

/// VPR's histogram bucket edges: `count` increments of the width, with the last forced to
/// the maximum. Adding one at a time gives VPR's exact rounding.
fn bucket_edges(low: f32, high: f32) -> [f32; 10] {
    let width = (high - low) / 10.0;
    let mut edges = [0.0; 10];
    let mut edge = low;
    for slot in &mut edges {
        edge += width;
        *slot = edge;
    }
    edges[9] = high;
    edges
}

/// Returns VPR's `median_presorted`, once the values are sorted.
fn median(values: &mut [f32]) -> f32 {
    values.sort_unstable_by(f32::total_cmp);
    match values.len() {
        0 => 0.0,
        len if len % 2 == 1 => values[len / 2],
        len => (values[len / 2 - 1] + values[len / 2]) / 2.0,
    }
}

/// Which wire type and direction a node counts as, or `None` for what isn't a wire.
fn cost_index_of(info: &NodeInfo, segments: usize) -> Option<usize> {
    let axis = match info.kind {
        NodeKind::ChanX => 0,
        NodeKind::ChanY => 1,
        NodeKind::ChanZ => 2,
        _ => return None,
    };
    // One more than the highest segment, for wires that don't have one.
    let segment = if info.segment == NO_SEGMENT {
        segments
    } else {
        usize::from(info.segment)
    };
    Some(axis * (segments + 1) + segment)
}

/// Works out VPR's normalization factor and each node's base cost, as `f32` as VPR has them.
fn base_costs(
    nodes: &[NodeInfo],
    rc: &[(f32, f32)],
    edges: &[FileEdge],
    switches: &[Option<Switch>],
    segments: &[Option<Segment>],
    device_width: u32,
) -> Result<(f32, Vec<f32>), ImportError> {
    let mut fan_in = vec![FanIn::default(); nodes.len()];
    for edge in edges {
        let Some(switch) = switches
            .get(usize::from(edge.switch))
            .and_then(Option::as_ref)
        else {
            continue;
        };
        // VPR leaves shorts out here, as they aren't what's driving the wire.
        if switch.kind == SwitchKind::Short || !nodes[edge.to as usize].kind.is_wire() {
            continue;
        }
        let entry = &mut fan_in[edge.to as usize];
        entry.r += f64::from(switch.r);
        entry.t += f64::from(switch.t_del_max);
        entry.c_internal += f64::from(switch.c_internal);
        entry.switches += 1;
        entry.buffered |= switch.kind.is_buffered();
    }

    // Only the wire types that occur get an entry, so that a tiny file can't ask for a table
    // of every possible type.
    let table_size = 3 * (segments.len() + 1);
    let mut positions = vec![u32::MAX; table_size];
    let mut indices: Vec<CostIndex> = Vec::new();
    for (node, info) in nodes.iter().enumerate() {
        let Some(index) = cost_index_of(info, segments.len()) else {
            continue;
        };
        if positions[index] == u32::MAX {
            positions[index] = indices.len() as u32;
            indices.push(CostIndex {
                inverse_length: 1.0,
                low: [f32::INFINITY; 3],
                high: [f32::NEG_INFINITY; 3],
                ..CostIndex::default()
            });
        }
        let entry = &mut indices[positions[index] as usize];
        entry.longest = entry.longest.max(info.span() + 1);
        let fan = &fan_in[node];
        if fan.switches == 0 {
            continue;
        }
        let (r, c) = rc.get(info.rc as usize).copied().unwrap_or((0.0, 0.0));
        entry.r_values.push(r);
        entry.c_values.push(c);
        entry.buffered |= fan.buffered;
        entry.nodes += 1;
        for (kind, value) in fan.average().into_iter().enumerate() {
            entry.low[kind] = entry.low[kind].min(value);
            entry.high[kind] = entry.high[kind].max(value);
        }
    }

    // A second pass as the buckets depend on the ranges found by the first.
    let mut edges_of: Vec<[[f32; 10]; 3]> = Vec::with_capacity(indices.len());
    for entry in &indices {
        edges_of.push(core::array::from_fn(|kind| {
            bucket_edges(entry.low[kind], entry.high[kind])
        }));
    }
    for (node, info) in nodes.iter().enumerate() {
        let Some(index) = cost_index_of(info, segments.len()) else {
            continue;
        };
        let fan = &fan_in[node];
        if fan.switches == 0 {
            continue;
        }
        let position = positions[index] as usize;
        let entry = &mut indices[position];
        for (kind, value) in fan.average().into_iter().enumerate() {
            // VPR's lower_bound: the first bucket whose top isn't below the value.
            let bucket = edges_of[position][kind]
                .iter()
                .position(|&edge| edge >= value)
                .unwrap_or(9);
            entry.buckets[kind][bucket] += 1;
        }
    }
    drop(fan_in);

    let mut sum = 0.0f32;
    let mut count = 0u32;
    for (index, &position) in positions.iter().enumerate() {
        if position == u32::MAX {
            continue;
        }
        // The segment's stated length if there is one, but never more than the device.
        let segment = index % (segments.len() + 1);
        let stated = segments
            .get(segment)
            .and_then(Option::as_ref)
            .and_then(|segment| segment.length);
        let entry = &mut indices[position as usize];
        let length = stated
            .unwrap_or(entry.longest)
            .clamp(1, device_width.max(1));
        let inverse_length = 1.0 / length as f32;
        entry.inverse_length = inverse_length;
        if entry.nodes == 0 {
            continue;
        }

        // The modes come first because they don't change what's sorted for the medians.
        let mode = |kind: usize| -> f32 {
            let mut best = 0;
            let mut value = 0.0;
            for (bucket, &bucket_count) in entry.buckets[kind].iter().enumerate() {
                if bucket_count > best {
                    best = bucket_count;
                    value = edges_of[position as usize][kind][bucket];
                }
            }
            value
        };
        let (r_switch, t_switch, c_internal) = (mode(0), mode(1), mode(2));
        let c_node = median(&mut entry.c_values);
        let r_node = median(&mut entry.r_values);
        let (t_linear, t_quadratic) = if entry.buffered {
            (
                t_switch + r_switch * (c_internal + c_node) + 0.5 * r_node * (c_node + c_internal),
                0.0,
            )
        } else {
            (
                t_switch + 0.5 * r_switch * c_node,
                (r_switch + r_node) * 0.5 * c_node,
            )
        };
        let delay = t_linear * inverse_length + t_quadratic * inverse_length * inverse_length;
        if delay != 0.0 {
            sum += delay;
            count += 1;
        }
    }

    // What VPR falls back on when no wire has any delay, which would otherwise give costs of 0.
    let factor = if count == 0 { 1e-9 } else { sum / count as f32 };
    if !factor.is_finite() {
        return Err(ImportError::CostOverflow);
    }

    let mut costs = Vec::with_capacity(nodes.len());
    for info in nodes {
        let cost = match info.kind {
            NodeKind::Source | NodeKind::Opin | NodeKind::Mux => factor,
            NodeKind::Sink => 0.0,
            NodeKind::Ipin => 0.875 * factor,
            NodeKind::ChanX | NodeKind::ChanY | NodeKind::ChanZ => {
                let position = cost_index_of(info, segments.len())
                    .map(|index| positions[index])
                    .unwrap_or(u32::MAX);
                factor
                    / indices
                        .get(position as usize)
                        .map_or(1.0, |entry| entry.inverse_length)
            }
        };
        if !cost.is_finite() {
            return Err(ImportError::CostOverflow);
        }
        costs.push(cost);
    }
    Ok((factor, costs))
}

/// Works out the numbers for [`LocationBound`], from the edges that the graph will have. See
/// there for why this holds for any graph.
fn bound_tables(
    nodes: &[NodeInfo],
    offsets: &[u32],
    targets: &[u32],
    costs: &[f32],
) -> (f64, Vec<u16>) {
    let edges_of = |node: usize| &targets[offsets[node] as usize..offsets[node + 1] as usize];
    // A dead end has no way out, and a node that isn't one but only leads to dead ends is a
    // tail. A tail can't lead to another tail, so these are always the last two nodes of a
    // path, if they're on it at all, which is what lets the two get treated separately.
    let is_dead_end = |node: usize| edges_of(node).is_empty();
    let is_tail: Vec<bool> = (0..nodes.len())
        .map(|node| !is_dead_end(node) && edges_of(node).iter().all(|&to| is_dead_end(to as usize)))
        .collect();
    let reach = |from: usize, to: usize| nodes[from].gap(&nodes[to]) + nodes[to].span();

    let mut cost_per_tile = f64::INFINITY;
    let mut tail_reach = vec![0u32; nodes.len()];
    for from in 0..nodes.len() {
        for &to in edges_of(from) {
            let to = to as usize;
            if is_tail[to] {
                tail_reach[to] = tail_reach[to].max(reach(from, to));
            } else if !is_dead_end(to) {
                let tiles = reach(from, to);
                if tiles > 0 {
                    cost_per_tile = cost_per_tile.min(f64::from(costs[to]) / f64::from(tiles));
                }
            }
        }
    }
    // Dead ends last, as they add on what it took to get to the tail before them.
    let mut dead_end_reach = vec![0u32; nodes.len()];
    for (from, &reach_of_from) in tail_reach.iter().enumerate() {
        for &to in edges_of(from) {
            let to = to as usize;
            if is_dead_end(to) {
                let tiles = reach(from, to) + reach_of_from;
                dead_end_reach[to] = dead_end_reach[to].max(tiles);
            }
        }
    }
    let combined = tail_reach
        .iter()
        .zip(&dead_end_reach)
        .map(|(&tail, &dead_end)| u16::try_from(tail.max(dead_end)).unwrap_or(u16::MAX))
        .collect();
    if !cost_per_tile.is_finite() {
        cost_per_tile = 0.0;
    }
    (cost_per_tile, combined)
}

/// Assembles the graph once all of the file has been read.
fn finish(mut sections: Sections, end_line: u64) -> Result<RrGraph, ImportError> {
    for (seen, section) in [
        (sections.seen_switches, "switches"),
        (sections.seen_block_types, "block_types"),
        (sections.seen_grid, "grid"),
        (sections.seen_nodes, "rr_nodes"),
        (sections.seen_edges, "rr_edges"),
    ] {
        if !seen {
            return Err(ImportError::MissingSection { section });
        }
    }

    order_nodes(&mut sections, end_line)?;
    sections.capacities.sort_unstable_by_key(|entry| entry.0);
    sections.layers.sort_unstable_by_key(|entry| entry.0);

    let Sections {
        switches,
        segments,
        mut block_types,
        grid,
        nodes,
        rc,
        capacities,
        layers,
        edges,
        channel_width,
        ..
    } = sections;

    let num_nodes = u32::try_from(nodes.len()).map_err(|_| ImportError::BadNodeId {
        line: end_line,
        id: u32::MAX,
    })?;
    for edge in &edges {
        for id in [edge.from, edge.to] {
            if id >= num_nodes {
                return Err(ImportError::BadNodeId { line: end_line, id });
            }
        }
        if switches
            .get(usize::from(edge.switch))
            .is_none_or(Option::is_none)
        {
            return Err(ImportError::BadReference {
                line: end_line,
                what: "switch",
                id: u32::from(edge.switch),
            });
        }
    }
    for info in &nodes {
        if info.segment != NO_SEGMENT
            && segments
                .get(usize::from(info.segment))
                .is_none_or(Option::is_none)
        {
            return Err(ImportError::BadReference {
                line: end_line,
                what: "segment",
                id: u32::from(info.segment),
            });
        }
    }

    // The grid, which has to list each tile once. This has to hold before anything is
    // allocated from its extent, or a tiny file could claim to be an enormous device.
    block_types.sort_by_key(|block| block.id);
    for pair in block_types.windows(2) {
        if pair[0].id == pair[1].id {
            return Err(ImportError::BadReference {
                line: end_line,
                what: "block type",
                id: pair[0].id,
            });
        }
    }
    let width = grid
        .iter()
        .map(|entry| u32::from(entry.x) + 1)
        .max()
        .unwrap_or(0);
    let height = grid
        .iter()
        .map(|entry| u32::from(entry.y) + 1)
        .max()
        .unwrap_or(0);
    let layer_count = grid
        .iter()
        .map(|entry| u32::from(entry.layer) + 1)
        .max()
        .unwrap_or(0);
    let expected = u64::from(width) * u64::from(height) * u64::from(layer_count);
    if expected != grid.len() as u64 {
        return Err(ImportError::IncompleteGrid {
            expected,
            found: grid.len() as u64,
        });
    }
    let empty_tile = GridTile {
        block_type: u16::MAX,
        width_offset: 0,
        height_offset: 0,
    };
    let mut tiles = vec![empty_tile; grid.len()];
    for entry in &grid {
        let index = (u64::from(entry.layer) * u64::from(height) + u64::from(entry.y))
            * u64::from(width)
            + u64::from(entry.x);
        let block_type = block_types
            .binary_search_by_key(&entry.block_type_id, |block| block.id)
            .ok()
            .and_then(|at| u16::try_from(at).ok())
            .filter(|&at| at != u16::MAX)
            .ok_or(ImportError::BadReference {
                line: end_line,
                what: "block type",
                id: entry.block_type_id,
            })?;
        let tile = &mut tiles[index as usize];
        if tile.block_type != u16::MAX {
            return Err(ImportError::IncompleteGrid {
                expected,
                found: grid.len() as u64,
            });
        }
        *tile = GridTile {
            block_type,
            width_offset: entry.width_offset,
            height_offset: entry.height_offset,
        };
    }
    drop(grid);

    let (factor, costs) = base_costs(&nodes, &rc, &edges, &switches, &segments, width)?;

    let mut stats = RrStats {
        nodes: num_nodes,
        file_edges: edges.len() as u64,
        ..RrStats::default()
    };
    let (offsets, targets, edge_switches) = sort_edges(nodes.len(), edges, &switches, &mut stats);
    let (cost_per_tile, tail_reach) = bound_tables(&nodes, &offsets, &targets, &costs);

    let mut builder = GraphBuilder::with_edge_capacity(num_nodes, targets.len());
    for node in 0..nodes.len() {
        let edges = offsets[node] as usize..offsets[node + 1] as usize;
        for (&target, &switch) in targets[edges.clone()].iter().zip(&edge_switches[edges]) {
            let (from, to) = (NodeId(node as u32), NodeId(target));
            // VPR takes a node behind a short or a non-configurable buffer whenever it takes
            // the node in front, and so does a router on this graph.
            let tied = switches[usize::from(switch)]
                .as_ref()
                .is_some_and(|switch| !switch.kind.is_configurable());
            let added = if tied {
                builder.add_non_configurable_edge(from, to, Cost::ZERO)
            } else {
                builder.add_edge(from, to, Cost::ZERO)
            };
            added.map_err(|_| ImportError::TooManyEdges)?;
        }
    }
    drop(targets);
    for (node, &cost) in costs.iter().enumerate() {
        let cost = Cost::new(f64::from(cost)).ok_or(ImportError::CostOverflow)?;
        // Node IDs were checked, so neither of these can fail.
        builder
            .set_base_cost(NodeId(node as u32), cost)
            .map_err(|_| ImportError::CostOverflow)?;
    }
    drop(costs);
    for &(node, capacity) in &capacities {
        builder
            .set_capacity(NodeId(node), capacity)
            .map_err(|_| ImportError::BadNodeId {
                line: end_line,
                id: node,
            })?;
    }
    drop(capacities);

    // Node locations are sorted for looking up, and the reach bounds how far a lookup has
    // to look, which is usually not at all, except along a wire.
    let mut max_reach = [(0u16, 0u16); NodeKind::COUNT];
    for info in &nodes {
        let reach = &mut max_reach[info.kind as usize];
        let (major_reach, minor_reach) = match info.kind {
            NodeKind::ChanX => (info.yhigh - info.ylow, info.xhigh - info.xlow),
            _ => (info.xhigh - info.xlow, info.yhigh - info.ylow),
        };
        reach.0 = reach.0.max(major_reach);
        reach.1 = reach.1.max(minor_reach);
    }
    let mut lookup: Vec<u32> = (0..num_nodes).collect();
    lookup.sort_unstable_by_key(|&id| {
        let info = &nodes[id as usize];
        let (major, minor) = info.major_minor();
        (info.kind, info.ptc, major, minor)
    });

    let nodes: Arc<[NodeInfo]> = Arc::from(nodes);
    let bound = LocationBound {
        nodes: Arc::clone(&nodes),
        tail_reach: Arc::from(tail_reach),
        cost_per_tile,
    };
    let graph = builder.build().with_lower_bound(bound.clone());

    Ok(RrGraph {
        graph,
        bound,
        nodes,
        rc: rc.into_boxed_slice(),
        layers: layers.into_boxed_slice(),
        edge_offsets: offsets.into_boxed_slice(),
        edge_switches: edge_switches.into_boxed_slice(),
        lookup: lookup.into_boxed_slice(),
        max_reach,
        switches: switches.into_boxed_slice(),
        segments: segments.into_boxed_slice(),
        block_types: block_types.into_boxed_slice(),
        grid: tiles.into_boxed_slice(),
        width,
        height,
        layer_count,
        channel_width,
        delay_normalization: f64::from(factor),
        stats,
    })
}
