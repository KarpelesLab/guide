//! FPGA placement and routing problems, read from and written back to VTR's file formats.
//!
//! This imports what VPR's placer and router work from, and writes what they produce:
//!
//! | File | Read or written by | Becomes |
//! | --- | --- | --- |
//! | Routing resource graph (`rr_graph.xml`) | [`read_rr_graph`] | [`RrGraph`], holding a [`Graph`] and its side tables |
//! | Packed netlist (`.net`) | [`read_netlist`] | [`PackedNetlist`] |
//! | Placement (`.place`) | [`read_placement`], [`write_placement`] | [`Placement`] |
//! | The graph, the netlist and the architecture's carry chains | [`build_place_problem`] | [`PlaceProblem`], holding a [`PlacementProblem`] that [`place_design`] places |
//! | The last three together | [`resolve_nets`] | [`Design`], holding [`Net`]s |
//! | Architecture (`arch.xml`), for the delays inside blocks | [`read_architecture`] | [`Architecture`], holding [`LogicDelays`] |
//! | The packed netlist again, and delays inside blocks | [`build_timing`] | [`DesignTiming`], holding a [`TimingGraph`] and the engine's [`Connection`]s |
//! | Routes (`.route`) | [`write_route`] | VPR's text format |
//!
//! Everything reads from a [`BufRead`] or writes to a [`Write`] passed in by the caller, and
//! nothing here opens a file.
//!
//! # Example
//!
//! Two I/O pads joined by a single wire, from files to a route file. Every step here is what
//! a real design goes through, with far smaller input.
//!
//! ```
//! use guide::fpga::{NetOptions, RouteHeader, read_netlist, read_placement, read_rr_graph, resolve_nets, write_route};
//! use guide::id::{NetId, NodeId};
//! use guide::net::{Route, RouteSet};
//! use guide::space::RoutingSpace;
//!
//! let rr_graph = r#"<rr_graph>
//!   <switches>
//!     <switch id="0" name="delayless" type="mux"><timing Tdel="0"/><sizing mux_trans_size="0" buf_size="0"/></switch>
//!     <switch id="1" name="wire" type="mux"><timing Tdel="1e-10"/><sizing mux_trans_size="0" buf_size="0"/></switch>
//!   </switches>
//!   <segments><segment id="0" name="L1" length="1"/></segments>
//!   <block_types>
//!     <block_type id="0" name="io" width="1" height="1">
//!       <pin_class type="OUTPUT"><pin ptc="0">io.inpad[0]</pin></pin_class>
//!       <pin_class type="INPUT"><pin ptc="1">io.outpad[0]</pin></pin_class>
//!     </block_type>
//!   </block_types>
//!   <grid>
//!     <grid_loc x="0" y="0" block_type_id="0" width_offset="0" height_offset="0"/>
//!     <grid_loc x="1" y="0" block_type_id="0" width_offset="0" height_offset="0"/>
//!   </grid>
//!   <rr_nodes>
//!     <node id="0" type="SOURCE" capacity="1"><loc xlow="0" ylow="0" xhigh="0" yhigh="0" ptc="0"/></node>
//!     <node id="1" type="OPIN" capacity="1"><loc xlow="0" ylow="0" xhigh="0" yhigh="0" side="RIGHT" ptc="0"/></node>
//!     <node id="2" type="CHANX" capacity="1"><loc xlow="0" ylow="0" xhigh="1" yhigh="0" ptc="0"/><segment segment_id="0"/></node>
//!     <node id="3" type="IPIN" capacity="1"><loc xlow="1" ylow="0" xhigh="1" yhigh="0" side="LEFT" ptc="1"/></node>
//!     <node id="4" type="SINK" capacity="1"><loc xlow="1" ylow="0" xhigh="1" yhigh="0" ptc="1"/></node>
//!   </rr_nodes>
//!   <rr_edges>
//!     <edge src_node="0" sink_node="1" switch_id="0"/>
//!     <edge src_node="1" sink_node="2" switch_id="1"/>
//!     <edge src_node="2" sink_node="3" switch_id="1"/>
//!     <edge src_node="3" sink_node="4" switch_id="0"/>
//!   </rr_edges>
//! </rr_graph>"#;
//! let netlist = r#"<block name="top.net" instance="FPGA_packed_netlist[0]">
//!   <inputs>a</inputs><outputs>out:b</outputs><clocks></clocks>
//!   <block name="a" instance="io[0]" mode="inpad">
//!     <outputs><port name="inpad">inpad[0].inpad[0]-&gt;x</port></outputs>
//!     <block name="a" instance="inpad[0]"><outputs><port name="inpad">a</port></outputs></block>
//!   </block>
//!   <block name="out:b" instance="io[1]" mode="outpad">
//!     <inputs><port name="outpad">a</port></inputs>
//!   </block>
//! </block>"#;
//! let placement = "Array size: 2 x 1 logic blocks\na 0 0 0\nout:b 1 0 0\n";
//!
//! let device = read_rr_graph(rr_graph.as_bytes())?;
//! let design = resolve_nets(
//!     &device,
//!     &read_netlist(netlist.as_bytes())?,
//!     &read_placement(placement.as_bytes())?,
//!     &NetOptions::default(),
//! )?;
//!
//! // The driver comes first, and the search has real costs to work with.
//! let net = &design.nets()[0];
//! assert_eq!(net.terminals[0].access_points()[0].node, NodeId(0));
//! assert_eq!(net.terminals[1].access_points()[0].node, NodeId(4));
//! assert!(device.graph().lower_bound(NetId(0), NodeId(0), NodeId(4)).get() > 0.0);
//!
//! // A router would find this path by searching `device.graph()`.
//! let mut route = Route::new();
//! route.add_path(&[NodeId(0), NodeId(1), NodeId(2), NodeId(3), NodeId(4)])?;
//! let mut routes = RouteSet::new();
//! routes.set_route(NetId(0), route);
//!
//! let header = RouteHeader { placement_file: "top.place", placement_id: "SHA256:0" };
//! let mut out = Vec::new();
//! write_route(&mut out, &device, &design, &routes, &header)?;
//! assert!(String::from_utf8(out)?.contains("Node:\t2\t CHANX (0,0,0) to (1,0,0)  Track: 0  Switch: 1\n"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # How well this matches VPR
//!
//! Each format was pinned against the real thing where one could be found, and this says
//! where it couldn't. See `testdata/vtr/README.md` for the samples. Since then, VPR 9.0.0 has
//! checked and timed this crate's routes of the 20 MCNC circuits of VTR's regression tests,
//! and of VTR's Verilog benchmarks on its flagship architecture, with fracturable LUTs, carry
//! chains, memories and multipliers over several tiles, on graphs and placements that it
//! wrote: see the FPGA routing section of `docs/benchmarks.md`, and `examples/vtr_route.rs`
//! for the code that does it.
//!
//! - **Routing resource graphs** were checked against the schema in VTR's source and a
//!   genuine device graph, the 96 MB Zero ASIC z1000 graph shipped with VTR, and are read
//!   from VPR 9.0.0's own output in the benchmarks. Base costs follow VPR's source, and have
//!   not been compared with the numbers VPR works with.
//! - **Packed netlists and placements** were checked against files VPR wrote, and against
//!   the net order, driver, pin indices and sinks in the route VPR wrote from them.
//! - **Route files** are reproduced byte for byte from a real VPR output, using a graph
//!   reconstructed from it. VPR 9.0.0 loads those of the benchmarks with `--analysis`, and
//!   finds every one legal. That took a change: VPR lets a net leave a logic block through
//!   only one output pin, which [`Design`](Design#one-output-pin-per-net) now arranges.
//! - **Delays** for timing-driven routing, from [`RrGraph::timed_graph`], are the Elmore delay
//!   that VPR's route tree gives a step through a buffered switch, fixed per step. That is an
//!   approximation behind unbuffered switches and where a tree branches: see
//!   [`RrGraph::step_delay`]. On the benchmarks, the worst slack that the engine works out
//!   with these is within 1.2% of VPR's timing analysis of the same routes, and mostly 0.1%.
//! - **Timing graphs** from [`build_timing`] follow the structure that the packed netlist
//!   records inside each block, which was checked against `mult_4x4`, `multiclock` and
//!   `unconnected`. The delays inside blocks come from [`read_architecture`], which was
//!   checked against `k6_N10_40nm.xml`, the architecture `mult_4x4` was packed for, and
//!   against VPR's source for what it makes of LUT and memory classes, and then against
//!   VPR's timing of the Verilog benchmarks on `k6_frac_N10_frac_chain_mem32K_40nm.xml`. See
//!   [what VPR does differently](build_timing#what-vpr-does-differently).
//! - **Constant nets**, which VPR doesn't route, are found by [`constant_nets`] as VPR finds
//!   them, and agreed with VPR's count on every Verilog benchmark.
//! - **Placements** that [`place_design`] makes were loaded by VPR 9.0.0 with every block fixed,
//!   which checks them as it checks its own, on all 41 circuits of both suites, carry chains,
//!   memories and multipliers included, and routed by it and by this crate. Their
//!   [`bb_estimate`](PlaceProblem::bb_estimate) equals what VPR prints for the same placement
//!   on every circuit. See the FPGA placement section of `docs/benchmarks.md`.
//!
//! # Limitations
//!
//! - **No Cap'n Proto.** VPR's binary form of the graph isn't read, only XML.
//! - **Pin names have to line up.** Pins are found by the names in the graph's
//!   `block_types`, which spares the architecture file. That works when a block's ports have
//!   the same names on the tile that it is placed on. A tile with several sub-tiles of
//!   different kinds, or `equivalent_sites` with a custom `pin_mapping`, needs the
//!   architecture file. Most such cases fail with [`ImportError::PinNotFound`], but a custom
//!   mapping that swaps pins while keeping their names can't be detected.
//! - **Non-clock globals need naming.** A clock is recognizable from the netlist, but a port
//!   declared `is_non_clock_global` is only known to the architecture file, so these have to
//!   be listed in [`NetOptions::global_ports`].
//! - **Two pins of one net in one class.** VPR can route to a shared `SINK` twice, once for
//!   each pin. A [`Route`] is a tree and can't pass a node twice, so [`write_route`] refuses
//!   these, and [`Design::shared_sink_nets`] lists them. This doesn't occur in VPR's clusters
//!   where pins in a class are equivalent, as a net then takes only one pin of the class.
//! - **Non-configurable edges** (`short` and `buffer` switches) are
//!   [non-configurable](crate::space::RoutingSpace::non_configurable) edges of the [`Graph`],
//!   so the engine takes a set of tied nodes as a whole, as VPR does, and the validator checks
//!   that routes do. [`RrStats::non_configurable_edges`] says if a graph has any. Where a
//!   configurable and a non-configurable edge join the same two nodes, the graph keeps one
//!   edge, through the configurable switch as [parallel edges](RrGraph#parallel-edges) says,
//!   and it is not tied. VPR would tie those nodes.
//! - **Base costs** match VPR except for two details, given at [`read_rr_graph`].
//! - **Digests are the caller's job.** VPR checks that a route file names the SHA-256 of the
//!   placement it was made from, and this crate has no SHA-256, so [`RouteHeader`] takes it
//!   as given.
//!
//! [`BufRead`]: std::io::BufRead
//! [`Write`]: std::io::Write
//! [`Graph`]: crate::space::graph::Graph
//! [`Net`]: crate::net::Net
//! [`Route`]: crate::net::Route
//! [`TimingGraph`]: crate::timing::TimingGraph
//! [`PlacementProblem`]: crate::place::PlacementProblem
//! [`Connection`]: crate::engine::Connection

use core::fmt;
use std::error::Error;
use std::io;

mod arch;
mod design;
mod lookahead;
mod netlist;
mod place;
mod placer;
mod route_file;
mod rr_graph;
mod timing;
mod xml;

#[cfg(test)]
mod tests;

pub use arch::{ArchDirect, ArchPrimitive, Architecture, PrimitiveKind, read_architecture};
pub use design::{Design, GlobalPort, NetInfo, NetOptions, NetPin, NetStatus, resolve_nets};
pub use lookahead::{LookaheadOptions, MapLookahead};
pub use netlist::{
    HopEnds, Interconnect, InternalPath, InternalPin, PackedBlock, PackedNet, PackedNetlist,
    PackedPin, PackedPrimitive, PortPin, read_netlist,
};
pub use place::{PlacedBlock, Placement, read_placement, write_placement};
pub use placer::{
    ChannelDemand, PlaceDelayModel, PlaceMethod, PlaceProblem, PlaceResult, PlacerOptions,
    ProblemOptions, build_place_problem, estimate_slack, place_design,
};
pub use route_file::{RouteFileError, RouteHeader, RouteSummary, write_route};
pub use rr_graph::{
    BlockType, ChannelDirection, GridTile, LocationBound, NodeKind, NodeLocation, Pin, PinClass,
    PinClassKind, RrDelayBound, RrGraph, RrStats, Segment, Switch, SwitchKind, read_rr_graph,
};
pub use timing::{
    ClockDomain, ClockSelection, DesignTiming, Fallback, LogicDelays, PrimitiveModel,
    SequentialDelays, TimingOptions, TimingReport, build_timing, constant_nets,
};

/// Why a file could not be imported.
///
/// Anything found while reading a file says which line it is on, counting from 1. Problems
/// only visible once a whole file has been read, or from combining files, have no line.
#[derive(Debug)]
#[non_exhaustive]
pub enum ImportError {
    /// Reading from the source failed.
    Io {
        /// The line reached when the read failed.
        line: u64,
        /// What the source reported.
        source: io::Error,
    },
    /// The file is not well-formed XML, or goes beyond what the parser accepts.
    Xml {
        /// The line of the fault.
        line: u64,
        /// What is wrong.
        kind: XmlErrorKind,
    },
    /// An element lacks an attribute that it needs.
    MissingAttribute {
        /// The line of the element.
        line: u64,
        /// The element's name.
        element: String,
        /// The attribute's name.
        attribute: &'static str,
    },
    /// An attribute or field has a value that can't be used.
    InvalidValue {
        /// The line of the value.
        line: u64,
        /// What the value is for.
        what: &'static str,
        /// The value, shortened if it is long.
        value: String,
    },
    /// The file doesn't have the structure of its format.
    Malformed {
        /// The line of the fault.
        line: u64,
        /// What is wrong.
        message: String,
    },
    /// A line of a text file is unreasonably long.
    LineTooLong {
        /// The line.
        line: u64,
        /// The longest line accepted, in bytes.
        limit: usize,
    },
    /// A whole section of the file is absent.
    MissingSection {
        /// The section's name.
        section: &'static str,
    },
    /// An element refers to a node that the graph doesn't have, or the graph's node IDs are
    /// not exactly `0..n` without gaps.
    BadNodeId {
        /// The line of the reference, or of the end of the file if nodes came after it.
        line: u64,
        /// The ID.
        id: u32,
    },
    /// An element refers to something that isn't defined, or defines something twice.
    BadReference {
        /// The line of the element, or of the end of the file if it came before the definition.
        line: u64,
        /// What kind of thing it is: a switch, a segment or a block type.
        what: &'static str,
        /// The ID.
        id: u32,
    },
    /// The device grid doesn't list each of its tiles exactly once.
    IncompleteGrid {
        /// How many tiles the grid's extent requires.
        expected: u64,
        /// How many it lists.
        found: u64,
    },
    /// The graph has more edges than a [`Graph`](crate::space::graph::Graph) can hold.
    TooManyEdges,
    /// The delay data can't give finite base costs.
    CostOverflow,
    /// A block in the netlist has no location in the placement.
    UnplacedBlock {
        /// The block's name.
        block: String,
    },
    /// A block is placed somewhere that isn't a tile of the device.
    BadLocation {
        /// The block's name.
        block: String,
        /// The column.
        x: u32,
        /// The row.
        y: u32,
    },
    /// A block's pin isn't on the tile it is placed on, under the names the graph uses.
    PinNotFound {
        /// The block's name.
        block: String,
        /// The tile type it is placed on.
        tile: String,
        /// The port on the block.
        port: String,
        /// The pin within the port.
        bit: u32,
        /// The block's position within the tile.
        sub_block: u32,
    },
    /// A pin's class has no `SOURCE` or `SINK` node at the block's location.
    NodeNotFound {
        /// The block's name.
        block: String,
        /// The kind of node looked for.
        kind: NodeKind,
        /// The column.
        x: u32,
        /// The row.
        y: u32,
        /// The pin class.
        class: u32,
    },
    /// Two different pins drive the same net.
    MultipleDrivers {
        /// The line in the netlist of the second driver.
        line: u64,
        /// The net's name.
        net: String,
    },
    /// A pin connects to its pin class in the wrong direction.
    PinDirection {
        /// The block's name.
        block: String,
        /// The port on the block.
        port: String,
    },
    /// A block's type is not a tile type of the device, so there is nowhere to place it.
    NoTileType {
        /// The block's name.
        block: String,
        /// Its type.
        pb_type: String,
    },
}

impl ImportError {
    /// Returns the line this error is on, if it's about a specific line of a file.
    pub fn line(&self) -> Option<u64> {
        match *self {
            ImportError::Io { line, .. }
            | ImportError::Xml { line, .. }
            | ImportError::MissingAttribute { line, .. }
            | ImportError::InvalidValue { line, .. }
            | ImportError::Malformed { line, .. }
            | ImportError::LineTooLong { line, .. }
            | ImportError::BadNodeId { line, .. }
            | ImportError::BadReference { line, .. }
            | ImportError::MultipleDrivers { line, .. } => Some(line),
            _ => None,
        }
    }
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::Io { line, source } => write!(f, "line {line}: read failed: {source}"),
            ImportError::Xml { line, kind } => write!(f, "line {line}: {kind}"),
            ImportError::MissingAttribute {
                line,
                element,
                attribute,
            } => write!(f, "line {line}: <{element}> has no {attribute} attribute"),
            ImportError::InvalidValue { line, what, value } => {
                write!(f, "line {line}: invalid {what}: {value:?}")
            }
            ImportError::Malformed { line, message } => write!(f, "line {line}: {message}"),
            ImportError::LineTooLong { line, limit } => {
                write!(f, "line {line}: longer than the limit of {limit} bytes")
            }
            ImportError::MissingSection { section } => {
                write!(f, "the file has no {section} section")
            }
            ImportError::BadNodeId { line, id } => {
                write!(f, "line {line}: node {id} is out of order or out of range")
            }
            ImportError::BadReference { line, what, id } => {
                write!(f, "line {line}: {what} {id} is undefined or defined twice")
            }
            ImportError::IncompleteGrid { expected, found } => {
                write!(f, "the grid has {found} tiles listed out of {expected}")
            }
            ImportError::TooManyEdges => f.write_str("the graph has too many edges"),
            ImportError::CostOverflow => {
                f.write_str("the timing data is too large to give finite base costs")
            }
            ImportError::UnplacedBlock { block } => {
                write!(f, "block {block} has no location in the placement")
            }
            ImportError::BadLocation { block, x, y } => {
                write!(
                    f,
                    "block {block} is placed at ({x},{y}), outside the device"
                )
            }
            ImportError::PinNotFound {
                block,
                tile,
                port,
                bit,
                sub_block,
            } => write!(
                f,
                "block {block} has pin {port}[{bit}], which position {sub_block} of tile {tile} doesn't"
            ),
            ImportError::NodeNotFound {
                block,
                kind,
                x,
                y,
                class,
            } => write!(
                f,
                "block {block} needs a {kind} node for class {class} at ({x},{y}), which the graph doesn't have"
            ),
            ImportError::MultipleDrivers { line, net } => {
                write!(f, "line {line}: net {net} has more than one driver")
            }
            ImportError::PinDirection { block, port } => write!(
                f,
                "port {port} of block {block} connects to a pin class of the opposite direction"
            ),
            ImportError::NoTileType { block, pb_type } => write!(
                f,
                "block {block} is a {pb_type}, and the device has no tile of that type"
            ),
        }
    }
}

impl Error for ImportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ImportError::Io { source, .. } => Some(source),
            ImportError::Xml { kind, .. } => Some(kind),
            _ => None,
        }
    }
}

/// What makes a file unacceptable as XML, or too demanding to read.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum XmlErrorKind {
    /// The input ended in the middle of something.
    UnexpectedEof {
        /// What the parser was looking for.
        expected: &'static str,
    },
    /// The input ended with an element still open.
    UnclosedElement(String),
    /// The input has no element at all.
    NoRootElement,
    /// A second element follows the root element.
    MultipleRoots,
    /// There is text before or after the root element.
    TextOutsideRoot,
    /// An end tag has no element to close.
    UnexpectedEndTag(String),
    /// An end tag closes an element other than the one that's open.
    MismatchedEndTag {
        /// The element that's open.
        expected: String,
        /// The name in the end tag.
        found: String,
    },
    /// The text isn't valid UTF-8.
    InvalidUtf8,
    /// An element or attribute has an unusable name.
    InvalidName(String),
    /// Two attributes run together with no space between them.
    MissingSpace,
    /// An attribute has no `="value"` after it, or the value never closes.
    MissingAttributeValue,
    /// An attribute's value contains a `<`.
    LessThanInAttribute,
    /// An attribute appears twice on the same element.
    DuplicateAttribute(String),
    /// An entity is unknown or badly formed, or names an invalid character.
    InvalidEntity(String),
    /// Markup that VTR's formats don't use, and which this parser therefore doesn't read.
    UnsupportedMarkup(&'static str),
    /// A tag is longer than the parser will hold.
    TagTooLong {
        /// The most bytes accepted.
        limit: usize,
    },
    /// Text is longer than the parser will hold.
    TextTooLong {
        /// The most bytes accepted.
        limit: usize,
    },
    /// Elements are nested deeper than the parser will follow.
    TooDeep {
        /// The most open elements accepted.
        limit: usize,
    },
    /// An element has more attributes than the parser will hold.
    TooManyAttributes {
        /// The most attributes accepted.
        limit: usize,
    },
}

impl fmt::Display for XmlErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            XmlErrorKind::UnexpectedEof { expected } => {
                write!(f, "input ended while looking for {expected}")
            }
            XmlErrorKind::UnclosedElement(name) => {
                write!(f, "input ended with <{name}> still open")
            }
            XmlErrorKind::NoRootElement => f.write_str("the document has no element"),
            XmlErrorKind::MultipleRoots => f.write_str("the document has a second root element"),
            XmlErrorKind::TextOutsideRoot => f.write_str("text outside of the root element"),
            XmlErrorKind::UnexpectedEndTag(name) => {
                write!(f, "</{name}> has nothing to close")
            }
            XmlErrorKind::MismatchedEndTag { expected, found } => {
                write!(f, "</{found}> where </{expected}> was expected")
            }
            XmlErrorKind::InvalidUtf8 => f.write_str("invalid UTF-8"),
            XmlErrorKind::InvalidName(name) => write!(f, "invalid name {name:?}"),
            XmlErrorKind::MissingSpace => f.write_str("no space between attributes"),
            XmlErrorKind::MissingAttributeValue => f.write_str("attribute has no quoted value"),
            XmlErrorKind::LessThanInAttribute => f.write_str("`<` in an attribute value"),
            XmlErrorKind::DuplicateAttribute(name) => write!(f, "duplicate attribute {name}"),
            XmlErrorKind::InvalidEntity(text) => write!(f, "invalid entity at {text:?}"),
            XmlErrorKind::UnsupportedMarkup(what) => write!(f, "unsupported markup: {what}"),
            XmlErrorKind::TagTooLong { limit } => {
                write!(f, "tag longer than the limit of {limit} bytes")
            }
            XmlErrorKind::TextTooLong { limit } => {
                write!(f, "text longer than the limit of {limit} bytes")
            }
            XmlErrorKind::TooDeep { limit } => {
                write!(f, "elements nested deeper than the limit of {limit}")
            }
            XmlErrorKind::TooManyAttributes { limit } => {
                write!(f, "more attributes on an element than the limit of {limit}")
            }
        }
    }
}

impl Error for XmlErrorKind {}
