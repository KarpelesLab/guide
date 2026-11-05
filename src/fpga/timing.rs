//! Timing graphs from VTR's packed netlists, for timing-driven routing.
//!
//! [`build_timing`] turns a [`PackedNetlist`] into a [`TimingGraph`] with a point for every
//! pin that timing passes through, and a net arc for every connection between blocks.
//! [`DesignTiming::connections`] then names the arc of each connection that the engine
//! routes, and [`DesignTiming::timing_driven`] puts it all together for
//! [`route_timed`](crate::engine::route_timed). The documentation of [`build_timing`] says
//! what the graph holds, and how it differs from VPR's.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::design::Design;
use super::netlist::{InternalPin, PackedNetlist, PackedPin, PackedPrimitive, PortPin};
use crate::cost::Cost;
use crate::engine::{Connection, TimingDriven};
use crate::id::{NetId, NodeId};
use crate::space::DelaySpace;
use crate::timing::{
    ArcId, ClockId, Constraints, PointId, TimingError, TimingGraph, TimingGraphBuilder,
};

/// The delays of a register, in picoseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SequentialDelays {
    /// From the clock edge to the output, VPR's `T_clock_to_Q`.
    pub clock_to_q: u32,
    /// How long before the clock edge an input has to settle, VPR's `T_setup`.
    pub setup: u32,
}

/// Delays of a combinational primitive's type.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CombinationalDelays {
    any: Option<u32>,
    /// By input port, then output port.
    pins: BTreeMap<String, BTreeMap<String, u32>>,
    /// By input port and pin, then output port and pin.
    bits: BTreeMap<(String, u32), BTreeMap<(String, u32), u32>>,
}

/// What a primitive does with its ports: which are clocked, and which inputs reach which
/// outputs without a clock in between. This is what an architecture's `<models>` say, and
/// [`read_architecture`](super::read_architecture) fills it in from them.
///
/// A primitive with a clocked port is a register: its clocked inputs capture paths, and its
/// clocked outputs launch them. Paths go through it only from an input to an output that the
/// model connects, whether it is a register or not.
///
/// # Examples
///
/// ```
/// use guide::fpga::PrimitiveModel;
///
/// // A RAM with a registered write and an asynchronous read.
/// let ram = PrimitiveModel::new()
///     .with_clocked("we")
///     .with_clocked("data")
///     .with_path("addr", "out");
/// assert!(ram.is_sequential());
/// assert!(ram.has_path("addr", "out"));
/// assert!(!ram.has_path("data", "out"));
/// assert!(PrimitiveModel::combinational().has_path("in", "out"));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct PrimitiveModel {
    clocked: BTreeSet<String>,
    paths: BTreeSet<(String, String)>,
    every_path: bool,
}

impl PrimitiveModel {
    /// Starts a model with nothing clocked and no path through it.
    pub fn new() -> PrimitiveModel {
        PrimitiveModel::default()
    }

    /// A combinational primitive with a path from every input to every output, such as a
    /// LUT: VPR's `.names`.
    pub fn combinational() -> PrimitiveModel {
        PrimitiveModel {
            every_path: true,
            ..PrimitiveModel::default()
        }
    }

    /// Makes a port clocked: an input captures paths, and an output launches them.
    pub fn with_clocked(mut self, port: &str) -> PrimitiveModel {
        self.clocked.insert(port.to_owned());
        self
    }

    /// Adds a path from an input port to an output port, VPR's `combinational_sink_ports`.
    pub fn with_path(mut self, input: &str, output: &str) -> PrimitiveModel {
        self.paths.insert((input.to_owned(), output.to_owned()));
        self
    }

    /// Returns whether any port is clocked, which makes the primitive a register.
    pub fn is_sequential(&self) -> bool {
        !self.clocked.is_empty()
    }

    /// Returns whether a port is clocked.
    pub fn is_clocked(&self, port: &str) -> bool {
        self.clocked.contains(port)
    }

    /// Returns whether a path goes from an input port to an output port.
    pub fn has_path(&self, input: &str, output: &str) -> bool {
        self.every_path
            || self
                .paths
                .iter()
                .any(|(from, to)| from == input && to == output)
    }
}

/// The delays inside blocks, in picoseconds, by the names the architecture file gives them.
///
/// See [the delay model](build_timing#the-delay-model). Everything is 0 unless set, and every lookup
/// that falls back on one of the `default_` fields is reported.
///
/// # Examples
///
/// ```
/// use guide::fpga::{LogicDelays, SequentialDelays};
///
/// let mut delays = LogicDelays::new();
/// delays.set_interconnect("clb", "crossbar", 80);
/// delays.set_combinational("lut", 250);
/// delays.set_sequential("ff", SequentialDelays { clock_to_q: 120, setup: 60 });
/// assert_eq!(delays.interconnect("clb", "crossbar"), Some(80));
/// assert_eq!(delays.combinational("lut", "in", "out"), Some(250));
/// assert_eq!(delays.interconnect("clb", "clbouts1"), None);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct LogicDelays {
    /// By the holder's type, then name.
    interconnect: BTreeMap<String, BTreeMap<String, u32>>,
    /// By the holder's type and name, then the ends, each written `type.port`.
    interconnect_ports: BTreeMap<(String, String), BTreeMap<(String, String), u32>>,
    combinational: BTreeMap<String, CombinationalDelays>,
    sequential: BTreeMap<String, SequentialDelays>,
    hold: BTreeMap<String, u32>,
    models: BTreeMap<String, PrimitiveModel>,
    /// The delay of an interconnect that isn't listed.
    pub default_interconnect: u32,
    /// The delay through a combinational primitive whose type isn't listed.
    pub default_combinational: u32,
    /// The delays of a register whose type isn't listed.
    pub default_sequential: SequentialDelays,
    /// When paths start at an input pad, after the clock edge.
    pub input_delay: u32,
    /// How long before the clock edge paths have to reach an output pad.
    pub output_delay: u32,
}

impl LogicDelays {
    /// Starts with every delay at 0.
    pub fn new() -> LogicDelays {
        LogicDelays::default()
    }

    /// Sets the delay of an interconnect: `container` is the type of the block that holds
    /// it, and `name` its name, as in `clb` and `crossbar`. This is VPR's `delay_constant`
    /// on that `<interconnect>`.
    pub fn set_interconnect(&mut self, container: &str, name: &str, delay: u32) {
        self.interconnect
            .entry(container.to_owned())
            .or_default()
            .insert(name.to_owned(), delay);
    }

    /// Returns the delay set for an interconnect, if one was.
    pub fn interconnect(&self, container: &str, name: &str) -> Option<u32> {
        self.interconnect.get(container)?.get(name).copied()
    }

    /// Sets the delay of an interconnect from one port to another, as a `delay_constant`
    /// with `in_port` and `out_port` does in VPR. `from` and `to` are written `type.port`,
    /// as in `clb.I` and `fle.in`, without the indices that VPR allows there.
    pub fn set_interconnect_ports(
        &mut self,
        container: &str,
        name: &str,
        from: &str,
        to: &str,
        delay: u32,
    ) {
        self.interconnect_ports
            .entry((container.to_owned(), name.to_owned()))
            .or_default()
            .insert((from.to_owned(), to.to_owned()), delay);
    }

    /// Returns the delay of an interconnect from one port to another, by ports first and
    /// then for the interconnect as a whole, as
    /// [`set_interconnect`](LogicDelays::set_interconnect) gives it.
    pub fn interconnect_ports(
        &self,
        container: &str,
        name: &str,
        from: &str,
        to: &str,
    ) -> Option<u32> {
        self.interconnect_ports
            .get(&(container.to_owned(), name.to_owned()))
            .and_then(|ports| ports.get(&(from.to_owned(), to.to_owned())))
            .copied()
            .or_else(|| self.interconnect(container, name))
    }

    /// Sets the delay through a combinational primitive's type, from any input to any
    /// output, unless [`set_combinational_ports`](LogicDelays::set_combinational_ports) says
    /// otherwise for a pair of ports.
    pub fn set_combinational(&mut self, pb_type: &str, delay: u32) {
        self.combinational
            .entry(pb_type.to_owned())
            .or_default()
            .any = Some(delay);
    }

    /// Sets the delay through a combinational primitive's type from one input port to one
    /// output port, as a `delay_constant` with `in_port` and `out_port` does in VPR.
    pub fn set_combinational_ports(
        &mut self,
        pb_type: &str,
        input: &str,
        output: &str,
        delay: u32,
    ) {
        self.combinational
            .entry(pb_type.to_owned())
            .or_default()
            .pins
            .entry(input.to_owned())
            .or_default()
            .insert(output.to_owned(), delay);
    }

    /// Sets the delay through a combinational primitive's type from one pin to another, each
    /// a port and a position in it, as a `delay_matrix` does in VPR.
    ///
    /// A position is where the pin is in its port as the `.net` file writes it, which is the
    /// physical pin, and so a row or column of the matrix. VPR's `port_rotation_map` says
    /// which logical input each physical pin of a LUT carries, which doesn't matter here.
    pub fn set_combinational_pins(
        &mut self,
        pb_type: &str,
        (input, input_bit): (&str, u32),
        (output, output_bit): (&str, u32),
        delay: u32,
    ) {
        self.combinational
            .entry(pb_type.to_owned())
            .or_default()
            .bits
            .entry((input.to_owned(), input_bit))
            .or_default()
            .insert((output.to_owned(), output_bit), delay);
    }

    /// Returns the delay set from one pin to another of a combinational type, if one was, by
    /// pins first, then by ports, then for the type.
    pub fn combinational_pins(
        &self,
        pb_type: &str,
        (input, input_bit): (&str, u32),
        (output, output_bit): (&str, u32),
    ) -> Option<u32> {
        self.combinational
            .get(pb_type)?
            .bits
            .get(&(input.to_owned(), input_bit))
            .and_then(|outputs| outputs.get(&(output.to_owned(), output_bit)))
            .copied()
            .or_else(|| self.combinational(pb_type, input, output))
    }

    /// Returns the delay set from an input port to an output port of a combinational type, if
    /// one was, by ports first and then for the type.
    pub fn combinational(&self, pb_type: &str, input: &str, output: &str) -> Option<u32> {
        let delays = self.combinational.get(pb_type)?;
        delays
            .pins
            .get(input)
            .and_then(|outputs| outputs.get(output))
            .copied()
            .or(delays.any)
    }

    /// Makes a primitive type a register, with these delays.
    pub fn set_sequential(&mut self, pb_type: &str, delays: SequentialDelays) {
        self.sequential.insert(pb_type.to_owned(), delays);
    }

    /// Returns the delays of a register type, if they were set.
    pub fn sequential(&self, pb_type: &str) -> Option<SequentialDelays> {
        self.sequential.get(pb_type).copied()
    }

    /// Sets a register type's hold time, VPR's `T_hold`: how long after the clock edge its
    /// inputs have to stay. It is kept for a hold analysis, which there isn't yet, and has no
    /// effect on the timing graph.
    pub fn set_hold(&mut self, pb_type: &str, hold: u32) {
        self.hold.insert(pb_type.to_owned(), hold);
    }

    /// Returns a register type's hold time, if it was set.
    pub fn hold(&self, pb_type: &str) -> Option<u32> {
        self.hold.get(pb_type).copied()
    }

    /// Says what a primitive type does with its ports, so that [`build_timing`] doesn't have
    /// to guess. See [which primitive is what](build_timing#which-primitive-is-what).
    pub fn set_model(&mut self, pb_type: &str, model: PrimitiveModel) {
        self.models.insert(pb_type.to_owned(), model);
    }

    /// Returns the model of a primitive type, if one was set.
    pub fn model(&self, pb_type: &str) -> Option<&PrimitiveModel> {
        self.models.get(pb_type)
    }
}

/// Which clocks' registers to time.
///
/// Either way, each register is on its clock's [`ClockId`], and the pads are on a clock of
/// their own: see [clock domains](build_timing#clock-domains). The
/// [`Constraints`] decide which of those are timed against what.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ClockSelection {
    /// All of them: as if they were one clock with [`Constraints::single_clock`], and each
    /// on its own with [`DesignTiming::vpr_constraints`] or other constraints per clock.
    #[default]
    All,
    /// Only the registers of the clock net with this name, and the pads. Other registers
    /// neither launch nor capture, and so are not timed at all.
    Only(String),
}

/// How to build a timing graph.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TimingOptions {
    /// Which registers to time. The default is all of them, each on its own clock domain.
    pub clocks: ClockSelection,
}

/// A delay that wasn't given, and fell back on a default.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Fallback {
    /// An interconnect, which took [`LogicDelays::default_interconnect`].
    Interconnect {
        /// The type of the block that holds it.
        container: String,
        /// Its name.
        name: String,
    },
    /// A combinational primitive type, from an input port to an output port, which took
    /// [`LogicDelays::default_combinational`].
    Combinational {
        /// The type.
        pb_type: String,
        /// The input port.
        input: String,
        /// The output port.
        output: String,
    },
    /// A register type, which took [`LogicDelays::default_sequential`].
    Sequential {
        /// The type.
        pb_type: String,
    },
}

/// A clock, and how many registers it drives.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ClockDomain {
    /// The clock: the name of the net on the registers' clock pins. A clock made inside a
    /// block, which is rare, is named after the primitive that makes it.
    pub name: String,
    /// How many registers it drives.
    pub registers: u32,
    /// Whether these registers are timed.
    pub timed: bool,
}

/// What [`build_timing`] found and assumed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TimingReport {
    /// How many input pads there are.
    pub input_pads: u32,
    /// How many output pads there are.
    pub output_pads: u32,
    /// How many registers there are, timed or not.
    pub registers: u32,
    /// How many combinational primitives there are.
    pub combinational: u32,
    /// How many primitives have no input, and so drive a constant.
    pub constant_generators: u32,
    /// The clocks, by name.
    pub clocks: Vec<ClockDomain>,
    /// How many registers have no clock connected, which are timed only with
    /// [`ClockSelection::All`].
    pub unclocked_registers: u32,
    /// The delays that weren't given, each once, in order.
    pub fallbacks: Vec<Fallback>,
    /// How many pins inside blocks could not be traced: see
    /// [`PackedNetlist::unresolved_pins`].
    pub unresolved_pins: u64,
}

/// A timing graph of a packed netlist, and which of its arcs stands for each connection.
///
/// Made by [`build_timing`].
#[derive(Clone, Debug)]
pub struct DesignTiming {
    graph: TimingGraph,
    /// The pin of each point.
    pins: Vec<InternalPin>,
    /// The net arcs of net `n`'s sinks are `net_arcs[net_offsets[n]..net_offsets[n + 1]]`.
    net_offsets: Vec<u32>,
    net_arcs: Vec<ArcId>,
    report: TimingReport,
    /// Every clock's name and domain, in order of name.
    clock_ids: Vec<(String, ClockId)>,
    /// The domain of registers without a clock, if there are any.
    unclocked: Option<ClockId>,
}

/// The clock domain of the pads: VPR's `virtual_io_clock`.
const IO_CLOCK: ClockId = ClockId(0);

impl DesignTiming {
    /// Returns the timing graph.
    pub fn graph(&self) -> &TimingGraph {
        &self.graph
    }

    /// Returns the clock domain of the pads, which is VPR's `virtual_io_clock`: always
    /// `ClockId(0)`.
    pub fn io_clock(&self) -> ClockId {
        IO_CLOCK
    }

    /// Returns the clock domain of the registers on a clock net, by the net's name as
    /// [`TimingReport::clocks`] has it.
    pub fn clock(&self, name: &str) -> Option<ClockId> {
        self.clock_ids
            .binary_search_by(|(known, _)| known.as_str().cmp(name))
            .ok()
            .map(|index| self.clock_ids[index].1)
    }

    /// Returns the clock domain of the registers that have no clock, if there are any. No
    /// constraints from [`vpr_constraints`](DesignTiming::vpr_constraints) time it.
    pub fn unclocked(&self) -> Option<ClockId> {
        self.unclocked
    }

    /// Returns the constraints that VPR uses without an SDC file, with every clock at
    /// `period` picoseconds. VPR's default is 0, the fastest circuit.
    ///
    /// | Netlist clocks | Constraints |
    /// | --- | --- |
    /// | None | The pads, on the I/O clock |
    /// | One | The pads and the registers, as one clock |
    /// | More | Each clock on its own, and the pads on the I/O clock, which is timed with each, at the [closest edges](Constraints::constrain_by_edges). Paths between two netlist clocks are not timed |
    ///
    /// That is VPR's documented default, from its `timing_constraints` page. Only the clocks
    /// that [`TimingReport::clocks`] says are timed count, so with
    /// [`ClockSelection::Only`], one clock and the pads are timed together, as VPR's
    /// single-clock default has it.
    ///
    /// # Errors
    ///
    /// Fails if the period is above [`MAX_CLOCK_PERIOD`](crate::timing::MAX_CLOCK_PERIOD).
    pub fn vpr_constraints(&self, period: u64) -> Result<Constraints, TimingError> {
        let clocks: Vec<ClockId> = self
            .report
            .clocks
            .iter()
            .filter(|clock| clock.timed)
            .filter_map(|clock| self.clock(&clock.name))
            .collect();
        let mut periods = vec![(IO_CLOCK, period)];
        periods.extend(clocks.iter().map(|&clock| (clock, period)));
        let mut constraints = Constraints::per_clock(&periods)?;
        // With one period for all, the closest edges are a period apart, so the pads are timed
        // with one netlist clock exactly as if they were on it.
        for &clock in &clocks {
            constraints = constraints
                .constrain_by_edges(IO_CLOCK, clock)?
                .constrain_by_edges(clock, IO_CLOCK)?;
        }
        Ok(constraints)
    }

    /// Returns the timing graph, to change its delays.
    pub fn graph_mut(&mut self) -> &mut TimingGraph {
        &mut self.graph
    }

    /// Returns what was found and assumed while building.
    pub fn report(&self) -> &TimingReport {
        &self.report
    }

    /// Returns the pin that a point stands for.
    pub fn pin(&self, point: PointId) -> Option<InternalPin> {
        self.pins.get(point.0 as usize).copied()
    }

    /// Returns the net arc from a net's driver to one of its sinks, numbered as in a route
    /// file's `Net_pin_index`: the first sink is 1.
    ///
    /// Every sink of a net with a driver has one, routed or not.
    pub fn net_arc(&self, net: NetId, pin: u32) -> Option<ArcId> {
        let net = net.0 as usize;
        let start = *self.net_offsets.get(net)? as usize;
        let end = *self.net_offsets.get(net + 1)? as usize;
        let index = start.checked_add(pin.checked_sub(1)? as usize)?;
        (index < end).then(|| self.net_arcs[index])
    }

    /// Returns a [`Connection`] for every sink of every net that `design` routes.
    ///
    /// `design` has to come from the netlist this was built from. A sink with no arc, which
    /// only another netlist could have, is left out, and so has a criticality of 0.
    pub fn connections(&self, design: &Design) -> Vec<Connection> {
        let mut connections = Vec::new();
        for net in design.nets() {
            for terminal in 1..net.terminals.len() as u32 {
                if let Some(arc) = self.net_arc(net.id, terminal) {
                    connections.push(Connection {
                        net: net.id,
                        terminal,
                        arc,
                    });
                }
            }
        }
        connections
    }

    /// Sets the delay of every connection that `design` routes to the space's lower bound on
    /// it, between the closest access points of its driver and its sink.
    ///
    /// These are what the first iteration of a timing-driven run computes criticalities
    /// from. Arcs of nets that aren't routed keep their delays, which start at 0: global
    /// nets are ideal.
    pub fn estimate_delays<S: DelaySpace<Node = NodeId>>(&mut self, space: &S, design: &Design) {
        for net in design.nets() {
            let Some(driver) = net.terminals.first() else {
                continue;
            };
            for (terminal, sink) in net.terminals.iter().enumerate().skip(1) {
                let Some(arc) = self.net_arc(net.id, terminal as u32) else {
                    continue;
                };
                let mut best: Option<u32> = None;
                for from in driver.access_points() {
                    for to in sink.access_points() {
                        let bound = space.delay_lower_bound(net.id, from.node, to.node);
                        best = Some(best.map_or(bound, |best| best.min(bound)));
                    }
                }
                // The arc is one of this graph's, so this can't fail.
                let _ = self.graph.set_delay(arc, best.unwrap_or(0));
            }
        }
    }

    /// Sets the delays of connections, such as those of [`TimingOutcome`] with the
    /// connections they were routed for, so that the graph can be analysed again. A delay of
    /// `None` leaves the arc as it is.
    ///
    /// [`TimingOutcome`]: crate::engine::TimingOutcome
    pub fn set_connection_delays(&mut self, connections: &[Connection], delays: &[Option<u32>]) {
        for (connection, delay) in connections.iter().zip(delays) {
            if let Some(delay) = *delay {
                let _ = self.graph.set_delay(connection.arc, delay);
            }
        }
    }

    /// Sets up timing-driven routing of `design` against `constraints`, with this graph as
    /// it is, all of the design's connections, and VPR's weighing of delay against the base
    /// costs of an [`RrGraph`](super::RrGraph), which are in seconds.
    pub fn timing_driven(&self, design: &Design, constraints: Constraints) -> TimingDriven {
        TimingDriven::new(
            self.graph.clone(),
            constraints,
            self.connections(design),
            Cost::new(1e-12).unwrap_or(Cost::ZERO),
        )
    }
}

/// What a primitive is, for timing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    InputPad,
    OutputPad,
    Register,
    Constant,
    Combinational,
}

/// Builds a timing graph of a packed netlist, with an arc for every connection between
/// blocks.
///
/// # The timing graph
///
/// | Point | Role |
/// | --- | --- |
/// | An output pin of an input pad | Launches paths at [`LogicDelays::input_delay`] |
/// | An input pin of an output pad | Captures paths with a margin of [`LogicDelays::output_delay`] |
/// | An input pin of a register | Captures paths with a margin of its setup time |
/// | An output pin of a register | Launches paths at its clock-to-Q delay |
/// | Any other pin of a primitive, or a pin of a placed block | Only passes paths on |
///
/// and its arcs are:
///
/// | Arc | Kind | Delay |
/// | --- | --- | --- |
/// | From a pin of a placed block or a primitive's output to a primitive's input or a pin of the block, through the block's interconnect | Cell | The sum of [`LogicDelays::interconnect`] along the way |
/// | From each input to each output of a combinational primitive | Cell | [`LogicDelays::combinational`] |
/// | From a net's driver pin to each of its sink pins, on placed blocks | Net | 0, until [`DesignTiming::estimate_delays`] or the router sets it |
///
/// A register has no arc through it, which is what ends one path and starts the next. Clock
/// pins have no points, because the clock is ideal: see below. Unconnected pins have no
/// points either.
///
/// # Which primitive is what
///
/// A `.net` file doesn't say what a primitive does. That is in the architecture file's
/// models, which [`read_architecture`](super::read_architecture) turns into a
/// [`PrimitiveModel`] for each primitive type. Primitives are told apart like this, in this
/// order:
///
/// 1. A primitive named in the root's `<inputs>` is an **input pad**, and one named in its
///    `<outputs>` an **output pad**. VPR names pads after the circuit's ports, so these lists
///    find exactly the pads.
/// 2. A primitive whose type has a [model](LogicDelays::set_model) is a **register** if the
///    model has a clocked port. Without a model, it is one if it has a connected clock pin,
///    or if its type is given delays with [`LogicDelays::set_sequential`]. Its clock is the
///    net that reaches its clock pin, traced through the block.
/// 3. A primitive with no connected input is a **constant generator**, which launches
///    nothing. VPR doesn't time constant nets either.
/// 4. Anything else is **combinational**.
///
/// With a model, a register's clocked inputs capture and its clocked outputs launch, and
/// paths go through any primitive only from an input to an output that the model connects.
/// Without one, a register's inputs all capture and its outputs all launch, with nothing
/// through it, and a combinational primitive connects every input to every output.
///
/// # The delay model
///
/// Every delay inside a block comes from a [`LogicDelays`], in picoseconds, and nothing else.
/// [`read_architecture`](super::read_architecture) fills one in from VPR's architecture file,
/// which is how VPR gets them. Without one, the default is **zero for everything**: then only
/// the routing between blocks has delay, and the netlist still decides which connections are
/// on a path between which registers.
/// Anything that was looked up and not found took the matching default of the
/// [`LogicDelays`] and is listed in [`TimingReport::fallbacks`], so a missing number never
/// goes unnoticed.
///
/// The names to give delays under are those the architecture file and the `.net` file share:
/// an interconnect is named by the `pb_type` that holds it and its `name` (`clb` and
/// `crossbar` in `clb.I[5]->crossbar`), and by the ports on either side of each hop through
/// it (`clb.I` and `fle.in`) for [`LogicDelays::interconnect_ports`]. A primitive is named by
/// its `pb_type` and port names, and pin positions for
/// [`LogicDelays::combinational_pins`].
/// [`PackedNetlist::interconnects`] lists every interconnect a design uses. A block used as a
/// wire, such as a LUT in VPR's `wire` mode, is interconnect like any other, under the name
/// the netlist gives it (`lut6` and `complete:lut6` in `k6_N10` architectures).
///
/// # What VPR does differently
///
/// - **Its numbers always come from the architecture file.** Here they come from whatever
///   [`LogicDelays`] is given, which [`read_architecture`](super::read_architecture) reads
///   from that file with the differences it lists: chiefly that interconnect delays are by
///   port and not by pin. A primitive's delays can differ by pin, as with VPR's
///   `delay_matrix`. Only maximum delays are used, for setup timing, and there is no hold
///   analysis.
/// - **It always knows what each primitive is**, from the architecture's models. Here that
///   takes a [`PrimitiveModel`], and without one a primitive with a clock is a register on
///   all of its ports, so a block with both registered and combinational paths, such as a
///   memory with an asynchronous read, loses the combinational ones. An unmodelled
///   combinational primitive connects every input to every output, which is right for a LUT
///   but may be pessimistic for a hard block.
/// - **Clocks take constraints to be told apart.** VPR's default without an SDC file times
///   each netlist clock separately, and does not time paths between two of them. That is
///   what [`DesignTiming::vpr_constraints`] gives: see [clock domains](#clock-domains).
///   [`Constraints::single_clock`] instead times everything as one clock, paths between
///   clocks included, which is pessimistic, never optimistic, for unrelated clocks.
/// - **The clock is ideal**, as with VPR's default `--clock_modeling ideal`: it reaches every
///   register at once, and clock nets have no delay. Like other global nets, they are not
///   routed by default; see [`NetOptions`](super::NetOptions).
/// - **Initial delays** of the connections are VPR's router's to choose with
///   `--router_initial_timing`. Here they are 0, or the delay lower bound of the space after
///   [`estimate_delays`](DesignTiming::estimate_delays). A lower bound is optimistic, so the
///   first iteration's criticalities are a little low.
///
/// # Clock domains
///
/// Each register launches and captures on the [`ClockId`] of its clock, and the pads on
/// `ClockId(0)`, VPR's `virtual_io_clock`: see [`DesignTiming::io_clock`] and
/// [`DesignTiming::clock`]. Clocks are numbered from 1 in order of name, which is the order of
/// [`TimingReport::clocks`], and registers without a clock come after them, as
/// [`DesignTiming::unclocked`] says.
///
/// What is timed against what is then up to the [`Constraints`]:
///
/// - [`Constraints::single_clock`] times every path against one period, whatever its clocks,
///   which is how everything was timed before there were clock domains.
/// - [`DesignTiming::vpr_constraints`] gives VPR's defaults without an SDC file: each clock
///   on its own, the pads with each, and nothing between two netlist clocks.
/// - [`Constraints::per_clock`] takes any other periods, and pairs of clocks can be
///   constrained on top, at the [closest edges](Constraints::constrain_by_edges) or by hand.
///
/// [`ClockSelection::Only`] times one clock's registers and the pads, whatever the
/// constraints, and leaves the other registers untimed. [`TimingReport::clocks`] says which
/// clocks there are.
///
/// # Determinism
///
/// Points and arcs are numbered in netlist order: the primitives' pins first, then the paths
/// inside blocks, then the nets. Hash maps are only used to look up, never to order.
///
/// # Errors
///
/// Fails only if the graph would be too large for a [`TimingGraph`].
///
/// # Examples
///
/// An input pad, through a LUT, to an output pad:
///
/// ```
/// use guide::fpga::{LogicDelays, TimingOptions, build_timing, read_netlist};
/// use guide::id::NetId;
/// use guide::timing::{Constraints, TimingAnalysis};
///
/// let text = r#"<block name="top.net" instance="FPGA_packed_netlist[0]">
///   <inputs>a</inputs><outputs>out:b</outputs><clocks></clocks>
///   <block name="a" instance="io[0]" mode="inpad">
///     <outputs><port name="inpad">inpad[0].inpad[0]-&gt;ipad</port></outputs>
///     <block name="a" instance="inpad[0]"><outputs><port name="inpad">a</port></outputs></block>
///   </block>
///   <block name="n" instance="clb[0]" mode="default">
///     <inputs><port name="I">a</port></inputs>
///     <outputs><port name="O">lut[0].out[0]-&gt;out</port></outputs>
///     <block name="n" instance="lut[0]">
///       <inputs><port name="in">clb.I[0]-&gt;in</port></inputs>
///       <outputs><port name="out">n</port></outputs>
///     </block>
///   </block>
///   <block name="out:b" instance="io[1]" mode="outpad">
///     <inputs><port name="outpad">n</port></inputs>
///     <block name="out:b" instance="outpad[0]">
///       <inputs><port name="outpad">io.outpad[0]-&gt;opad</port></inputs>
///     </block>
///   </block>
/// </block>"#;
/// let netlist = read_netlist(text.as_bytes())?;
/// let mut delays = LogicDelays::new();
/// delays.set_combinational("lut", 300);
/// delays.set_interconnect("clb", "in", 20);
/// delays.set_interconnect("clb", "out", 10);
/// let mut timing = build_timing(&netlist, &delays, &TimingOptions::default())?;
///
/// // Two routed connections, which the router would decide the delays of.
/// let into_lut = timing.net_arc(NetId(0), 1).unwrap();
/// let out_of_lut = timing.net_arc(NetId(1), 1).unwrap();
/// timing.graph_mut().set_delay(into_lut, 100)?;
/// timing.graph_mut().set_delay(out_of_lut, 200)?;
///
/// let mut analysis = TimingAnalysis::new(timing.graph());
/// analysis.run(timing.graph(), &Constraints::single_clock(1000)?);
/// // 100 + 20 + 300 + 10 + 200, and the pads' own interconnect, which wasn't given.
/// assert_eq!(analysis.worst_slack(), Some(1000 - 630));
/// assert_eq!(timing.report().fallbacks.len(), 2);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn build_timing(
    netlist: &PackedNetlist,
    delays: &LogicDelays,
    options: &TimingOptions,
) -> Result<DesignTiming, TimingError> {
    let mut report = TimingReport {
        unresolved_pins: netlist.unresolved_pins(),
        ..TimingReport::default()
    };
    let mut fallbacks = BTreeSet::new();
    let ports = netlist.ports();
    let port_name = |pin: PortPin| ports.get(pin.port as usize).map_or("", String::as_str);

    // The delay of each interconnect between each pair of ends it is used with. Only looked
    // up, never iterated.
    let mut hop_delays: HashMap<(u32, u32), u32> = HashMap::new();
    let mut hop_delay = |hop: u32, ends: u32| -> u32 {
        if let Some(&delay) = hop_delays.get(&(hop, ends)) {
            return delay;
        }
        let (Some(interconnect), Some(end)) = (
            netlist.interconnects().get(hop as usize),
            netlist.hop_ends().get(ends as usize),
        ) else {
            return 0;
        };
        let from = format!("{}.{}", end.from_type, end.from_port);
        let to = format!("{}.{}", end.to_type, end.to_port);
        let delay = delays
            .interconnect_ports(&interconnect.container, &interconnect.name, &from, &to)
            .unwrap_or_else(|| {
                fallbacks.insert(Fallback::Interconnect {
                    container: interconnect.container.clone(),
                    name: interconnect.name.clone(),
                });
                delays.default_interconnect
            });
        hop_delays.insert((hop, ends), delay);
        delay
    };
    let mut path_delays = Vec::with_capacity(netlist.internal_paths().len());
    // Clock paths are looked up too, though the clock is ideal, so that every interconnect
    // without a delay is reported.
    for path in netlist.internal_paths() {
        let ends = netlist.path_hop_ends(path);
        let delay = netlist
            .hops(path)
            .iter()
            .zip(ends)
            .fold(0u32, |sum, (&hop, &ends)| {
                sum.saturating_add(hop_delay(hop, ends))
            });
        path_delays.push(delay);
    }

    // The net on each pin of a placed block, to name clocks with.
    let mut block_pin_nets: HashMap<PackedPin, u32> = HashMap::new();
    for (index, net) in netlist.nets().iter().enumerate() {
        for &pin in net.driver.iter().chain(&net.sinks) {
            block_pin_nets.insert(pin, index as u32);
        }
    }

    // The clock of each primitive with one, by name.
    let mut clock_of: Vec<Option<String>> = vec![None; netlist.primitives().len()];
    for path in netlist.internal_paths() {
        let (true, InternalPin::Primitive { primitive, .. }) = (path.clock, path.to) else {
            continue;
        };
        let name = match path.from {
            InternalPin::Block(pin) => block_pin_nets
                .get(&pin)
                .and_then(|&net| netlist.nets().get(net as usize))
                .map(|net| net.name.clone()),
            InternalPin::Primitive { primitive, .. } => netlist
                .primitives()
                .get(primitive as usize)
                .map(|primitive| primitive.name.clone()),
        };
        if let Some(slot) = clock_of.get_mut(primitive as usize) {
            *slot = slot.take().or(name);
        }
    }

    let input_pads: BTreeSet<&str> = netlist
        .primary_inputs()
        .iter()
        .map(String::as_str)
        .collect();
    let output_pads: BTreeSet<&str> = netlist
        .primary_outputs()
        .iter()
        .map(String::as_str)
        .collect();

    let mut builder = TimingGraphBuilder::new();
    let mut pins: Vec<InternalPin> = Vec::new();
    let mut points: HashMap<InternalPin, PointId> = HashMap::new();
    let mut point_of =
        |builder: &mut TimingGraphBuilder, pin: InternalPin| -> Result<PointId, TimingError> {
            if let Some(&point) = points.get(&pin) {
                return Ok(point);
            }
            let point = builder.add_point()?;
            points.insert(pin, point);
            pins.push(pin);
            Ok(point)
        };

    // Arcs through a primitive, from each input to each output that its model connects, or to
    // every output without a model.
    let add_paths = |builder: &mut TimingGraphBuilder,
                     fallbacks: &mut BTreeSet<Fallback>,
                     primitive: &PackedPrimitive,
                     inputs: &[PointId],
                     outputs: &[PointId]|
     -> Result<(), TimingError> {
        let model = delays.model(&primitive.pb_type);
        for (&input, &from) in primitive.inputs.iter().zip(inputs) {
            for (&output, &to) in primitive.outputs.iter().zip(outputs) {
                let (input_port, output_port) = (port_name(input), port_name(output));
                if model.is_some_and(|model| !model.has_path(input_port, output_port)) {
                    continue;
                }
                let delay = delays
                    .combinational_pins(
                        &primitive.pb_type,
                        (input_port, input.bit),
                        (output_port, output.bit),
                    )
                    .unwrap_or_else(|| {
                        fallbacks.insert(Fallback::Combinational {
                            pb_type: primitive.pb_type.clone(),
                            input: input_port.to_owned(),
                            output: output_port.to_owned(),
                        });
                        delays.default_combinational
                    });
                builder.add_cell_arc(from, to, delay)?;
            }
        }
        Ok(())
    };
    let sequential_delays = |fallbacks: &mut BTreeSet<Fallback>, primitive: &PackedPrimitive| {
        delays.sequential(&primitive.pb_type).unwrap_or_else(|| {
            fallbacks.insert(Fallback::Sequential {
                pb_type: primitive.pb_type.clone(),
            });
            delays.default_sequential
        })
    };

    // The pads are on clock 0, each clock by name on the next, and registers without one on
    // the clock after those.
    let clock_names: BTreeSet<&str> = clock_of.iter().flatten().map(String::as_str).collect();
    let clock_ids: Vec<(String, ClockId)> = clock_names
        .iter()
        .zip(1..)
        .map(|(&name, id)| (name.to_owned(), ClockId(id)))
        .collect();
    let unclocked_id = ClockId(clock_ids.len() as u32 + 1);
    let clock_id = |name: Option<&str>| {
        name.and_then(|name| {
            clock_ids
                .binary_search_by(|(known, _)| known.as_str().cmp(name))
                .ok()
        })
        .map_or(unclocked_id, |index| clock_ids[index].1)
    };

    let mut clocks: BTreeMap<String, (u32, bool)> = BTreeMap::new();
    for (index, primitive) in netlist.primitives().iter().enumerate() {
        let index = index as u32;
        let at = |pin: PortPin| InternalPin::Primitive {
            primitive: index,
            pin,
        };
        let clock = clock_of[index as usize].as_deref();
        let domain = clock_id(clock);
        let sequential = delays.sequential(&primitive.pb_type);
        let model = delays.model(&primitive.pb_type);
        let role = if input_pads.contains(primitive.name.as_str()) {
            Role::InputPad
        } else if output_pads.contains(primitive.name.as_str()) {
            Role::OutputPad
        } else if model.map_or(
            !primitive.clocks.is_empty() || sequential.is_some(),
            PrimitiveModel::is_sequential,
        ) {
            Role::Register
        } else if primitive.inputs.is_empty() {
            Role::Constant
        } else {
            Role::Combinational
        };

        let mut inputs = Vec::with_capacity(primitive.inputs.len());
        for &pin in &primitive.inputs {
            inputs.push(point_of(&mut builder, at(pin))?);
        }
        let mut outputs = Vec::with_capacity(primitive.outputs.len());
        for &pin in &primitive.outputs {
            outputs.push(point_of(&mut builder, at(pin))?);
        }

        match role {
            Role::InputPad => {
                report.input_pads += 1;
                for &point in &outputs {
                    builder.set_launch_on(point, IO_CLOCK, delays.input_delay)?;
                }
            }
            Role::OutputPad => {
                report.output_pads += 1;
                for &point in &inputs {
                    builder.set_capture_on(point, IO_CLOCK, delays.output_delay)?;
                }
            }
            Role::Register => {
                report.registers += 1;
                let timed = match (&options.clocks, clock) {
                    (ClockSelection::All, _) => true,
                    (ClockSelection::Only(only), Some(clock)) => only == clock,
                    (ClockSelection::Only(_), None) => false,
                };
                match clock {
                    Some(clock) => {
                        clocks.entry(clock.to_owned()).or_insert((0, timed)).0 += 1;
                    }
                    None => report.unclocked_registers += 1,
                }
                // Only a model says which paths go through a register, and which of its ports
                // are clocked. Without one, all are clocked and none go through.
                if let Some(model) = model {
                    add_paths(&mut builder, &mut fallbacks, primitive, &inputs, &outputs)?;
                    if !timed {
                        continue;
                    }
                    let sequential = sequential_delays(&mut fallbacks, primitive);
                    for (&pin, &point) in primitive.inputs.iter().zip(&inputs) {
                        if model.is_clocked(port_name(pin)) {
                            builder.set_capture_on(point, domain, sequential.setup)?;
                        }
                    }
                    for (&pin, &point) in primitive.outputs.iter().zip(&outputs) {
                        if model.is_clocked(port_name(pin)) {
                            builder.set_launch_on(point, domain, sequential.clock_to_q)?;
                        }
                    }
                    continue;
                }
                if !timed {
                    continue;
                }
                let sequential = sequential_delays(&mut fallbacks, primitive);
                for &point in &inputs {
                    builder.set_capture_on(point, domain, sequential.setup)?;
                }
                for &point in &outputs {
                    builder.set_launch_on(point, domain, sequential.clock_to_q)?;
                }
            }
            Role::Constant => report.constant_generators += 1,
            Role::Combinational => {
                report.combinational += 1;
                add_paths(&mut builder, &mut fallbacks, primitive, &inputs, &outputs)?;
            }
        }
    }

    for (path, &delay) in netlist.internal_paths().iter().zip(&path_delays) {
        if path.clock {
            continue;
        }
        let from = point_of(&mut builder, path.from)?;
        let to = point_of(&mut builder, path.to)?;
        builder.add_cell_arc(from, to, delay)?;
    }

    let mut net_offsets = Vec::with_capacity(netlist.nets().len() + 1);
    let mut net_arcs = Vec::new();
    for net in netlist.nets() {
        net_offsets.push(net_arcs.len() as u32);
        let Some(driver) = net.driver else {
            continue;
        };
        let from = point_of(&mut builder, InternalPin::Block(driver))?;
        for &sink in &net.sinks {
            let to = point_of(&mut builder, InternalPin::Block(sink))?;
            net_arcs.push(builder.add_net_arc(from, to, 0)?);
        }
    }
    net_offsets.push(net_arcs.len() as u32);

    report.clocks = clocks
        .into_iter()
        .map(|(name, (registers, timed))| ClockDomain {
            name,
            registers,
            timed,
        })
        .collect();
    report.fallbacks = fallbacks.into_iter().collect();

    Ok(DesignTiming {
        graph: builder.build(),
        pins,
        net_offsets,
        net_arcs,
        unclocked: (report.unclocked_registers > 0).then_some(unclocked_id),
        report,
        clock_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fpga::read_netlist;
    use crate::timing::{ArcKind, CriticalityOptions, TimingAnalysis};

    const MULT: &str = include_str!("../../testdata/vtr/mult_4x4.net");
    const MULTICLOCK: &str = include_str!("../../testdata/vtr/multiclock.net");
    const UNCONNECTED: &str = include_str!("../../testdata/vtr/unconnected.net");

    fn primitive(netlist: &PackedNetlist, name: &str, pb_type: &str) -> u32 {
        netlist
            .primitives()
            .iter()
            .position(|primitive| primitive.name == name && primitive.pb_type == pb_type)
            .unwrap() as u32
    }

    /// The points of a primitive's pins on a port.
    fn points_of(
        timing: &DesignTiming,
        netlist: &PackedNetlist,
        primitive: u32,
        port: &str,
    ) -> Vec<PointId> {
        (0..timing.graph().num_points())
            .map(PointId)
            .filter(|&point| {
                matches!(
                    timing.pin(point),
                    Some(InternalPin::Primitive { primitive: p, pin })
                        if p == primitive && netlist.ports()[pin.port as usize] == port
                )
            })
            .collect()
    }

    fn set_all_net_delays(timing: &mut DesignTiming, delay: u32) {
        let arcs = timing.graph().net_arcs().to_vec();
        for arc in arcs {
            timing.graph_mut().set_delay(arc, delay).unwrap();
        }
    }

    #[test]
    fn times_a_genuine_combinational_netlist_from_pad_to_pad() {
        let netlist = read_netlist(MULT.as_bytes()).unwrap();
        let mut delays = LogicDelays::new();
        delays.set_combinational("lut", 100);
        let mut timing = build_timing(&netlist, &delays, &TimingOptions::default()).unwrap();
        let report = timing.report();
        assert_eq!((report.input_pads, report.output_pads), (8, 8));
        assert_eq!(report.registers, 0);
        assert_eq!(report.constant_generators, 0);
        assert_eq!(
            report.combinational as usize,
            netlist.primitives().len() - 16
        );
        assert!(report.clocks.is_empty());
        assert!(timing.graph().loops().is_empty());
        // Every interconnect fell back, and no LUT did.
        assert_eq!(report.fallbacks.len(), netlist.interconnects().len());
        assert!(
            report
                .fallbacks
                .iter()
                .all(|fallback| matches!(fallback, Fallback::Interconnect { .. }))
        );

        // A net arc for every sink of every net.
        let sinks: usize = netlist.nets().iter().map(|net| net.sinks.len()).sum();
        assert_eq!(timing.graph().net_arcs().len(), sinks);
        for (index, net) in netlist.nets().iter().enumerate() {
            let id = NetId(index as u32);
            for pin in 1..=net.sinks.len() as u32 {
                assert!(timing.net_arc(id, pin).is_some());
            }
            assert_eq!(timing.net_arc(id, 0), None);
            assert_eq!(timing.net_arc(id, net.sinks.len() as u32 + 1), None);
        }

        // With a delay of 1 on every connection, the critical path goes from pad to pad, and
        // its delay counts its LUTs and its connections.
        set_all_net_delays(&mut timing, 1);
        let mut analysis = TimingAnalysis::new(timing.graph());
        analysis.run(timing.graph(), &Constraints::single_clock(0).unwrap());
        let path = analysis.critical_path().unwrap();
        let primitive_of = |point| match timing.pin(point).unwrap() {
            InternalPin::Primitive { primitive, .. } => Some(primitive),
            InternalPin::Block(_) => None,
        };
        let kind = |point| {
            primitive_of(point).map_or("block", |primitive| {
                netlist.primitives()[primitive as usize].pb_type.as_str()
            })
        };
        assert_eq!(kind(path.start()), "inpad");
        assert_eq!(kind(path.end()), "outpad");
        let arcs: Vec<_> = path
            .arcs()
            .iter()
            .map(|&arc| timing.graph().arc(arc).unwrap())
            .collect();
        // Through a LUT, which isn't from one LUT's output to the next one's input.
        let luts = arcs
            .iter()
            .filter(|arc| kind(arc.from) == "lut" && primitive_of(arc.from) == primitive_of(arc.to))
            .count() as u64;
        let nets = arcs.iter().filter(|arc| arc.kind == ArcKind::Net).count() as u64;
        assert!(luts >= 2, "a 4x4 multiplier is more than one LUT deep");
        assert_eq!(path.arrival(), luts * 100 + nets);
    }

    #[test]
    fn registers_start_and_end_paths_on_each_clock() {
        let netlist = read_netlist(MULTICLOCK.as_bytes()).unwrap();
        let mut delays = LogicDelays::new();
        delays.set_sequential(
            "ff",
            SequentialDelays {
                clock_to_q: 30,
                setup: 20,
            },
        );
        delays.input_delay = 5;
        delays.output_delay = 7;
        let timing = build_timing(&netlist, &delays, &TimingOptions::default()).unwrap();
        let report = timing.report();
        assert_eq!(report.registers, 4);
        assert_eq!(report.unclocked_registers, 0);
        let clock = |name: &str| ClockDomain {
            name: name.into(),
            registers: 2,
            timed: true,
        };
        assert_eq!(report.clocks, [clock("clk"), clock("clk2")]);
        // The clocks' own pads are pads like any other, whose nets are ideal.
        assert_eq!(report.input_pads, 5);
        assert_eq!(report.output_pads, 3);
        assert!(
            !report
                .fallbacks
                .iter()
                .any(|fallback| matches!(fallback, Fallback::Sequential { .. }))
        );

        for name in ["FFA", "FFB", "FFC", "FFD"] {
            let ff = primitive(&netlist, name, "ff");
            let d = points_of(&timing, &netlist, ff, "D");
            let q = points_of(&timing, &netlist, ff, "Q");
            assert_eq!((d.len(), q.len()), (1, 1));
            assert_eq!(timing.graph().capture_margin(d[0]), Some(20));
            assert_eq!(timing.graph().launch_offset(q[0]), Some(30));
            assert_eq!(
                timing.graph().fanin(q[0]),
                &[],
                "nothing goes through a register"
            );
        }
        let pad = primitive(&netlist, "in1", "inpad");
        let out = points_of(&timing, &netlist, pad, "inpad");
        assert_eq!(timing.graph().launch_offset(out[0]), Some(5));
        let pad = primitive(&netlist, "out:out1", "outpad");
        let input = points_of(&timing, &netlist, pad, "outpad");
        assert_eq!(timing.graph().capture_margin(input[0]), Some(7));
    }

    #[test]
    fn one_clock_can_be_timed_on_its_own() {
        let netlist = read_netlist(MULTICLOCK.as_bytes()).unwrap();
        let options = TimingOptions {
            clocks: ClockSelection::Only("clk".into()),
        };
        let timing = build_timing(&netlist, &LogicDelays::new(), &options).unwrap();
        let report = timing.report();
        let timed: Vec<bool> = report.clocks.iter().map(|clock| clock.timed).collect();
        assert_eq!(timed, [true, false]);
        // Without delays for `ff`, the registers on `clk` fell back, and that is reported.
        assert!(report.fallbacks.contains(&Fallback::Sequential {
            pb_type: "ff".into()
        }));
        for (name, timed) in [("FFA", true), ("FFC", true), ("FFB", false), ("FFD", false)] {
            let ff = primitive(&netlist, name, "ff");
            let d = points_of(&timing, &netlist, ff, "D")[0];
            let q = points_of(&timing, &netlist, ff, "Q")[0];
            assert_eq!(timing.graph().capture_margin(d).is_some(), timed, "{name}");
            assert_eq!(timing.graph().launch_offset(q).is_some(), timed, "{name}");
        }
    }

    #[test]
    fn unconnected_pins_have_no_points_and_hard_blocks_are_combinational() {
        let netlist = read_netlist(UNCONNECTED.as_bytes()).unwrap();
        let mut delays = LogicDelays::new();
        delays.set_combinational_ports("dsp_bel", "a", "out", 400);
        delays.default_combinational = 900;
        delays.default_interconnect = 10;
        let timing = build_timing(&netlist, &delays, &TimingOptions::default()).unwrap();
        let report = timing.report();
        assert_eq!(report.combinational, 1);
        assert_eq!(report.unresolved_pins, 0);
        // `b` to `out` wasn't given, and `a` to `out` was.
        assert!(report.fallbacks.contains(&Fallback::Combinational {
            pb_type: "dsp_bel".into(),
            input: "b".into(),
            output: "out".into(),
        }));
        assert!(!report.fallbacks.iter().any(
            |fallback| matches!(fallback, Fallback::Combinational { input, .. } if input == "a")
        ));

        let bel = primitive(&netlist, "inst", "dsp_bel");
        let a = points_of(&timing, &netlist, bel, "a");
        let b = points_of(&timing, &netlist, bel, "b");
        let out = points_of(&timing, &netlist, bel, "out");
        assert_eq!((a.len(), b.len(), out.len()), (4, 8, 8));
        assert!(points_of(&timing, &netlist, bel, "addsub").is_empty());
        for &point in &out {
            let delays: Vec<u32> = timing
                .graph()
                .fanin(point)
                .iter()
                .map(|&arc| timing.graph().arc(arc).unwrap().delay)
                .collect();
            assert_eq!(delays.iter().filter(|&&delay| delay == 400).count(), 4);
            assert_eq!(delays.iter().filter(|&&delay| delay == 900).count(), 8);
        }
    }

    #[test]
    fn registers_without_a_clock_and_constants_are_reported() {
        let text = "<block name='t' instance='FPGA_packed_netlist[0]'>\
            <block name='r' instance='ff[0]'>\
            <inputs><port name='D'>x</port></inputs>\
            <outputs><port name='Q'>r</port></outputs></block>\
            <block name='k' instance='lut[0]'><outputs><port name='out'>k</port></outputs></block>\
            </block>";
        let netlist = read_netlist(text.as_bytes()).unwrap();
        let mut delays = LogicDelays::new();
        delays.set_sequential("ff", SequentialDelays::default());
        let timing = build_timing(&netlist, &delays, &TimingOptions::default()).unwrap();
        let report = timing.report();
        assert_eq!(report.registers, 1);
        assert_eq!(report.unclocked_registers, 1);
        assert_eq!(report.constant_generators, 1);
        assert!(report.clocks.is_empty());

        let only = TimingOptions {
            clocks: ClockSelection::Only("clk".into()),
        };
        let untimed = build_timing(&netlist, &delays, &only).unwrap();
        let mut analysis = TimingAnalysis::new(untimed.graph());
        analysis.run(untimed.graph(), &Constraints::single_clock(100).unwrap());
        assert_eq!(analysis.worst_slack(), None);
    }

    const ARCH: &str = include_str!("../../testdata/vtr/k6_N10_40nm.xml");

    #[test]
    fn the_genuine_architecture_gives_every_delay_mult_4x4_needs() {
        let arch = crate::fpga::read_architecture(ARCH.as_bytes()).unwrap();
        let delays = arch.delays();
        // Spot checks against the file, in picoseconds.
        assert_eq!(
            delays.interconnect_ports("clb", "crossbar", "clb.I", "fle.in"),
            Some(95)
        );
        assert_eq!(
            delays.interconnect_ports("clb", "crossbar", "fle.out", "fle.in"),
            Some(75)
        );
        assert_eq!(
            delays.interconnect_ports("ble6", "mux1", "ff.Q", "ble6.out"),
            Some(45)
        );
        assert_eq!(
            delays.interconnect_ports("io", "inpad", "inpad.inpad", "io.inpad"),
            Some(42)
        );
        assert_eq!(
            delays.interconnect_ports("lut6", "complete:lut6", "lut6.in", "lut6.out"),
            Some(398)
        );
        let lut: Vec<_> = (0..6)
            .map(|bit| delays.combinational_pins("lut", ("in", bit), ("out", 0)))
            .collect();
        let expected = [82, 173, 261, 263, 398, 397].map(Some);
        assert_eq!(lut, expected);
        assert!(delays.model("lut").unwrap().has_path("in", "out"));
        assert!(delays.model("ff").unwrap().is_sequential());

        let netlist = read_netlist(MULT.as_bytes()).unwrap();
        let timing = build_timing(&netlist, delays, &TimingOptions::default()).unwrap();
        assert_eq!(timing.report().fallbacks, []);
        assert_eq!(timing.report().combinational, 21);

        // From a CLB's input to a LUT's input: the crossbar's 95, then three delayless
        // directs. Out of a LUT to the CLB's output: `mux1`'s 25 from the LUT, then two more
        // directs. From one LUT to another in the same CLB: 25 and the crossbar's 75 for a
        // feedback, the 100 ps that the file's comments give. Pads: 42 and 14. The rest are
        // LUT inputs.
        let mut cell_delays = BTreeSet::new();
        for arc in 0..timing.graph().num_arcs() {
            let arc = timing.graph().arc(ArcId(arc)).unwrap();
            if arc.kind == ArcKind::Cell {
                cell_delays.insert(arc.delay);
            }
        }
        let expected: BTreeSet<u32> = [14, 25, 42, 82, 95, 100, 173, 261, 263, 397, 398].into();
        assert_eq!(cell_delays, expected);
    }

    /// `multiclock` has two clocks, and paths between them both ways: from `FFA` on `clk`
    /// through `to_FFD` to `FFD` on `clk2`, and from `FFB` on `clk2` through `to_FFC` to `FFC`
    /// on `clk`. Its blocks have the names of `k6_N10_40nm`, though it wasn't packed against
    /// that very file, so those delays stand in for its own.
    #[test]
    fn each_clock_of_multiclock_is_its_own_domain() {
        let netlist = read_netlist(MULTICLOCK.as_bytes()).unwrap();
        let arch = crate::fpga::read_architecture(ARCH.as_bytes()).unwrap();
        let mut timing = build_timing(&netlist, arch.delays(), &TimingOptions::default()).unwrap();
        assert_eq!(timing.report().fallbacks, []);
        set_all_net_delays(&mut timing, 100);
        let (clk, clk2) = (timing.clock("clk").unwrap(), timing.clock("clk2").unwrap());
        assert_eq!(
            (timing.io_clock(), clk, clk2),
            (ClockId(0), ClockId(1), ClockId(2))
        );
        assert_eq!(timing.clock("in1"), None);
        assert_eq!(timing.unclocked(), None);
        let graph = timing.graph();
        for (name, clock) in [("FFA", clk), ("FFC", clk), ("FFB", clk2), ("FFD", clk2)] {
            let ff = primitive(&netlist, name, "ff");
            let d = points_of(&timing, &netlist, ff, "D")[0];
            let q = points_of(&timing, &netlist, ff, "Q")[0];
            assert_eq!(graph.capture_clock(d), Some(clock), "{name}");
            assert_eq!(graph.launch_clock(q), Some(clock), "{name}");
        }
        let pad = primitive(&netlist, "in1", "inpad");
        let pad = points_of(&timing, &netlist, pad, "inpad")[0];
        assert_eq!(graph.launch_clock(pad), Some(timing.io_clock()));

        let vpr = timing.vpr_constraints(1000).unwrap();
        assert_eq!(vpr.constraint(clk, clk), Some(1000));
        assert_eq!(vpr.constraint(clk, clk2), None);
        assert_eq!(vpr.constraint(clk2, clk), None);
        assert_eq!(vpr.constraint(timing.io_clock(), clk2), Some(1000));
        assert_eq!(vpr.constraint(clk2, timing.io_clock()), Some(1000));
        let single = Constraints::single_clock(1000).unwrap();
        let both = vpr
            .clone()
            .constrain_by_edges(clk, clk2)
            .unwrap()
            .constrain_by_edges(clk2, clk)
            .unwrap();

        let analyse = |constraints: &Constraints| {
            let mut analysis = TimingAnalysis::new(graph);
            analysis.run(graph, constraints);
            analysis
        };
        let (single, vpr, both) = (analyse(&single), analyse(&vpr), analyse(&both));
        // The worst path is within `clk`, from `FFA` to `FFC`, whatever the constraints.
        for analysis in [&single, &vpr, &both] {
            assert_eq!(analysis.worst_slack(), Some(292));
            let path = analysis.critical_path().unwrap();
            let end = timing.pin(path.end()).unwrap();
            let ffc = primitive(&netlist, "FFC", "ff");
            assert!(matches!(end, InternalPin::Primitive { primitive, .. } if primitive == ffc));
        }

        // The two connections between the logic blocks only carry paths between the clocks.
        // VPR's default leaves them untimed, and so uncritical.
        let options = CriticalityOptions::default();
        let crossing: Vec<ArcId> = graph
            .net_arcs()
            .iter()
            .copied()
            .filter(|&arc| vpr.arc_slack(arc).is_none() && single.arc_slack(arc).is_some())
            .collect();
        assert_eq!(crossing.len(), 2);
        for &arc in &crossing {
            let info = graph.arc(arc).unwrap();
            let blocks = [info.from, info.to].map(|point| match timing.pin(point) {
                Some(InternalPin::Block(pin)) => {
                    netlist.blocks()[pin.block as usize].pb_type.clone()
                }
                _ => String::new(),
            });
            assert_eq!(blocks, ["clb", "clb"]);
            assert_eq!(vpr.arc_criticality(arc, &options), 0.0);
            assert!(single.arc_criticality(arc, &options) > 0.0);
            // Timed again once the crossings are constrained: 1000 - 66 - 627. Its pair's
            // only path is this one, whose capture point is required at 1000 - 66, and the
            // criticality is against that, not against the worst path of `clk`.
            assert_eq!(both.arc_slack(arc), Some(307));
            let raw = CriticalityOptions::new(1, 1.0).unwrap();
            assert_eq!(both.arc_criticality(arc, &raw), 627.0 / 934.0);
            // As one clock, against the output pads, which are required at 1000.
            assert_eq!(single.arc_criticality(arc, &raw), 693.0 / 1000.0);
        }
        // With the arcs inside the blocks on those paths, 8 arcs are only timed as one clock.
        // Every other arc is timed alike either way, as all the periods are the same.
        let mut untimed = 0;
        for arc in 0..graph.num_arcs() {
            let arc = ArcId(arc);
            match (vpr.arc_slack(arc), single.arc_slack(arc)) {
                (None, Some(_)) => untimed += 1,
                (per_clock, one_clock) => assert_eq!(per_clock, one_clock, "{arc:?}"),
            }
        }
        assert_eq!(untimed, 8);

        // `Only` leaves `clk2`'s registers untimed, and then VPR's default for one clock
        // times the pads with `clk`.
        let only = TimingOptions {
            clocks: ClockSelection::Only("clk".into()),
        };
        let timing = build_timing(&netlist, arch.delays(), &only).unwrap();
        let constraints = timing.vpr_constraints(0).unwrap();
        assert_eq!(constraints.constraint(clk, timing.io_clock()), Some(0));
        assert_eq!(constraints.constraint(clk2, clk2), None);
    }

    #[test]
    fn a_model_decides_what_is_clocked_and_what_goes_through() {
        // A register whose `en` input goes straight through to `out` as well.
        let text = "<block name='t' instance='FPGA_packed_netlist[0]'>\
            <block name='r' instance='reg[0]'>\
            <inputs><port name='d'>x</port><port name='en'>y</port></inputs>\
            <outputs><port name='q'>r</port><port name='out'>s</port></outputs></block>\
            </block>";
        let netlist = read_netlist(text.as_bytes()).unwrap();
        let mut delays = LogicDelays::new();
        delays.set_model(
            "reg",
            PrimitiveModel::new()
                .with_clocked("d")
                .with_clocked("q")
                .with_path("en", "out"),
        );
        delays.set_combinational_ports("reg", "en", "out", 7);
        let timing = build_timing(&netlist, &delays, &TimingOptions::default()).unwrap();
        assert_eq!(timing.report().registers, 1);
        let reg = primitive(&netlist, "r", "reg");
        let point = |port| points_of(&timing, &netlist, reg, port)[0];
        let graph = timing.graph();
        assert!(graph.capture_margin(point("d")).is_some());
        assert!(graph.capture_margin(point("en")).is_none());
        assert!(graph.launch_offset(point("q")).is_some());
        assert!(graph.launch_offset(point("out")).is_none());
        assert_eq!(graph.fanin(point("q")), &[]);
        let through = graph.fanin(point("out"));
        assert_eq!(through.len(), 1);
        assert_eq!(graph.arc(through[0]).unwrap().delay, 7);
        assert_eq!(graph.arc(through[0]).unwrap().from, point("en"));
        // Without the model, the clock pin is what made it a register, and there is none.
        let unmodelled = build_timing(&netlist, &LogicDelays::new(), &TimingOptions::default());
        assert_eq!(unmodelled.unwrap().report().combinational, 1);
    }

    #[test]
    fn the_same_netlist_gives_the_same_graph() {
        let netlist = read_netlist(MULTICLOCK.as_bytes()).unwrap();
        let build = || {
            let timing =
                build_timing(&netlist, &LogicDelays::new(), &TimingOptions::default()).unwrap();
            let arcs: Vec<_> = (0..timing.graph().num_arcs())
                .map(|arc| timing.graph().arc(ArcId(arc)))
                .collect();
            (arcs, timing.pins.clone(), timing.report().clone())
        };
        assert_eq!(build(), build());
    }
}
