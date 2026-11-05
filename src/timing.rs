//! Static timing analysis.
//!
//! A domain builds a [`TimingGraph`] of timing points (pins) and arcs (cell delays and net
//! connections), and a [`TimingAnalysis`] computes arrival times, required times, slack and
//! criticality over it. Nothing here knows about routing: a router only supplies new net
//! delays and reads back criticalities.
//!
//! # Example
//!
//! Two registers with a gate between them, on a 1000 ps clock:
//!
//! ```
//! use guide::timing::{Constraints, CriticalityOptions, TimingAnalysis, TimingGraphBuilder};
//!
//! let mut builder = TimingGraphBuilder::new();
//! let launch = builder.add_register(100, 50)?; // clock-to-out 100, setup 50
//! let capture = builder.add_register(100, 50)?;
//! let gate_in = builder.add_point()?;
//! let gate_out = builder.add_point()?;
//! let first = builder.add_net_arc(launch.q, gate_in, 200)?;
//! builder.add_cell_arc(gate_in, gate_out, 300)?;
//! let second = builder.add_net_arc(gate_out, capture.d, 150)?;
//! let mut graph = builder.build();
//!
//! let constraints = Constraints::single_clock(1000)?;
//! let mut analysis = TimingAnalysis::new(&graph);
//! analysis.run(&graph, &constraints);
//! assert_eq!(analysis.arrival(capture.d), Some(750));
//! assert_eq!(analysis.required(capture.d), Some(950));
//! assert_eq!(analysis.worst_slack(), Some(200));
//!
//! // The router found a slower route for the first connection. Nothing is rebuilt.
//! graph.set_delay(first, 500)?;
//! analysis.run(&graph, &constraints);
//! assert_eq!(analysis.worst_slack(), Some(-100));
//! assert_eq!(analysis.total_negative_slack(), -100);
//!
//! let path = analysis.critical_path().expect("the registers are connected");
//! assert_eq!(path.start(), launch.q);
//! assert_eq!(path.end(), capture.d);
//! assert_eq!(path.arcs().last(), Some(&second));
//!
//! // Every arc here is on the one failing path, so all are as critical as allowed.
//! let options = CriticalityOptions::new(1, 0.75)?;
//! assert_eq!(analysis.arc_criticality(second, &options), 0.75);
//! # Ok::<(), guide::timing::TimingError>(())
//! ```
//!
//! # The model
//!
//! A timing path starts at a *launch point* and ends at a *capture point*:
//!
//! | Element | Launch point | Capture point |
//! | --- | --- | --- |
//! | Primary input | The input, offset by its input delay | |
//! | Primary output | | The output, with its output delay as a margin |
//! | Register | Q, offset by clock-to-out | D, with setup as a margin |
//!
//! With `T` the clock period, and delays added up along arcs:
//!
//! ```text
//! arrival(p)  = max(launch offset of p, arrival(from) + delay for each arc into p)
//! required(p) = min(T - capture margin of p, required(to) - delay for each arc out of p)
//! slack(p)    = required(p) - arrival(p)
//! slack(arc)  = required(to) - delay - arrival(from)
//! ```
//!
//! A register is two points with no arc between them, which is what ends one path and
//! starts the next, so feedback through a register is not a combinational loop. The clock
//! is ideal: it reaches every register at once, so clock nets do not belong in the graph.
//!
//! A launch point may have arcs coming in and a capture point may have arcs going out. The
//! formulas above then apply as written, which amounts to `set_input_delay` on an internal
//! pin that does not cut the paths through it. A point that is both forms a path with no
//! arcs.
//!
//! A point or arc is *constrained* if it lies on a path from a launch point to a capture
//! point. Anything else has no slack, reported as `None`, and a criticality of zero before
//! any exponent is applied. Unconstrained logic exerts no timing pressure on the router.
//!
//! # Combinational loops are broken and reported
//!
//! [`TimingGraphBuilder::build`] always succeeds. If the arcs contain cycles, it disables
//! enough arcs to make the graph acyclic and lists what it found in
//! [`TimingGraph::loops`]. VPR does the same, warning and disabling an arc per loop in its
//! `TimingGraphBuilder::fix_comb_loops`. The reason is that netlists with loops exist and
//! still have to be routed, and a router with no timing at all does worse on them than one
//! with timing for everything else. A caller that would rather refuse only has to check that
//! the list is empty.
//!
//! Results for such a graph are a *lower bound*. A disabled arc has no slack, and no path
//! through it is timed, so real paths may be slower than reported.
//!
//! The disabled arcs are the back edges of a depth-first search that visits launch points
//! first, then every other point, each set in ascending [`PointId`] order, following arcs
//! in ascending [`ArcId`] order. Starting from launch points means the arc given up is
//! usually the feedback one: in `input -> a -> b -> output` with `b -> a` added, it is
//! `b -> a` that goes, whichever of `a` and `b` happens to have the lower number, and the
//! path from the input to the output survives. The choice depends only on the structure of
//! the graph, never on its delays, so results do not jump between router iterations.
//!
//! # Criticality
//!
//! This follows VPR, from two of its functions as of 2026: `calc_relaxed_criticality` in
//! `vpr/src/timing/timing_util.cpp` and `get_net_pin_criticality` in
//! `vpr/src/route/route_net.cpp`. With `slack` from the point or arc in question,
//! `worst_slack` the lowest slack anywhere and `max_required` the largest required time of
//! any constrained point:
//!
//! ```text
//! shift = max(0, -worst_slack)                        // makes all shifted slacks >= 0
//! raw   = 1 - (slack + shift) / (max_required + shift)   // 1 if the denominator is 0
//! c     = max(raw - (1 - max_criticality), 0)
//! c     = c ^ criticality_exponent
//! c     = min(c, max_criticality)
//! ```
//!
//! The shift is Wainberg and Betz's "relaxed" criticality, which keeps the result within
//! `[0, 1]` when timing is failing. The defaults are VPR's, an exponent of 1 and a maximum
//! of 0.99. A maximum below 1 always leaves the router some weight on congestion, and the
//! subtraction means a connection needs a criticality above `1 - max` to get any delay
//! weight at all. An exponent of 0 treats everything alike, as VPR documents: even an
//! unconstrained arc gets `0^0 = 1` and so the maximum.
//!
//! The differences from VPR are all in arithmetic. VPR's `1 - slack / max_required` is
//! computed in `f32` and clamped against round-off. This computes
//! `(max_required - slack) / (max_required + shift)` in `f64`, whose numerator is an exact
//! integer between zero and the denominator. So the result needs no clamp, is exactly 0 at
//! a slack of `max_required`, and is exactly 1 on the worst path whenever that slack is zero
//! or negative. A worst path with positive slack stays below 1, as it does in VPR: nothing
//! is failing, so nothing needs the full delay weight.
//!
//! A clock period of 0 gives VPR's behaviour without a timing constraint, and pushes for the
//! fastest circuit. With no capture margins, the shift then turns the raw criticality of an
//! arc into `longest path through it / longest path of all`.
//!
//! # Clock domains
//!
//! Every launch and capture point is on a clock, a [`ClockId`]: `ClockId(0)` unless
//! [`set_launch_on`](TimingGraphBuilder::set_launch_on) or
//! [`set_capture_on`](TimingGraphBuilder::set_capture_on) says otherwise. What a path is
//! checked against depends on the [`Constraints`]:
//!
//! - [`Constraints::single_clock`] ignores the clocks, and checks every path against one
//!   period, as if they were all one clock. A graph with one clock gives the same results
//!   either way, to the bit.
//! - [`Constraints::per_clock`] gives each clock a period, and checks a path launched on one
//!   clock and captured by another only if that pair was constrained, with
//!   [`constrain`](Constraints::constrain) or
//!   [`constrain_by_edges`](Constraints::constrain_by_edges). By default, then, paths
//!   between two clocks are not timed, which is what VPR does without an SDC file. A pair's
//!   constraint takes the place of `T` in [the model](#the-model).
//!
//! Per clock, the analysis follows VPR's, as Tatum, its timing analyser, tags arrival times
//! by launching clock and required times by capturing clock:
//!
//! - Arrival times are propagated once for each clock that launches in a timed pair, and
//!   required times once for each clock that captures in one, against a constraint of 0,
//!   which a pair's constraint then shifts. So propagation costs a pass per such clock,
//!   and working out slacks and criticalities a pass per pair.
//! - The slack of a point or arc is the worst over the timed pairs, the worst slack is the
//!   worst over all of them, and the total negative slack adds up each pair's capture points
//!   on their own, as VPR's `find_setup_tns` does.
//! - **Criticality is per pair**: with the formula above, each pair's `worst_slack` and
//!   `max_required` are its own, and a point or arc takes the highest criticality over the
//!   pairs it is on. That is VPR's `calc_relaxed_criticality`, and it means each clock is
//!   pushed against its own constraint: the worst path of a slow clock is as critical as the
//!   worst path of a fast one.
//! - The critical path is the worst path of any pair, the lowest capture point winning ties
//!   as before and then the first pair in order of launch and capture clock.
//!   [`arrival`](TimingAnalysis::arrival) and [`required`](TimingAnalysis::required) are the
//!   latest and earliest over the pairs, so a point's slack can be higher than the one minus
//!   the other.
//!
//! # Determinism
//!
//! Results are a pure function of the graph, its delays and the constraints, and no hash
//! maps are involved. In particular:
//!
//! - **Delays are integers.** Arrival, required time, slack and their totals never touch a
//!   float.
//! - **Critical path ties.** Among capture points with the worst slack, the one with the
//!   lowest [`PointId`] is the end of the path. Walking back from it, the walk stops as
//!   soon as the current point's own launch offset accounts for its arrival time, and
//!   otherwise takes the lowest-numbered [`ArcId`] that accounts for it. So the path is the
//!   one whose arcs, read from the end, are lexicographically smallest.
//! - **Criticality uses only `-`, `*` and `/`, on inputs that convert exactly.** Slacks
//!   can exceed 2^53, where `i64 as f64` starts to round. Conversion would still be
//!   deterministic, but it is simple to avoid: numerator and denominator are shifted right
//!   by the same amount until the denominator fits in 53 bits, which is only ever needed
//!   beyond about 2.5 hours of delay, and the result moves by less than 2^-52 there.
//! - **Exponents are whole numbers.** [`f64::powf`] and [`f64::powi`] can differ across
//!   platforms, so `c ^ e` is square-and-multiply in a fixed order. A `u32` exponent means
//!   a fractional one cannot be requested and silently approximated. `crate::math` has
//!   `exp` but no logarithm, and `exp(e * ln(c))` would also be slower in this hot path and
//!   add rounding to the common exponent of 1. VPR's default is 1, so fractions wait until
//!   someone needs one.
//! - **Minimum and maximum use plain comparisons.** [`f64::max`] and [`f64::min`] may return
//!   either sign when given zeros of both signs, which a later division or bit comparison
//!   could tell apart.
//!
//! # Performance
//!
//! A run is a linear pass in each direction over one flat array of arcs sorted by level,
//! then one more to work out arc slacks. After [`TimingAnalysis::new`], a
//! [`run`](TimingAnalysis::run) on single-clock constraints does not allocate. Constraints per
//! clock take a pass per clock or pair, as [clock domains](#clock-domains) says, and allocate
//! on their first run. The measurements below are for one clock.
//!
//! There is no incremental mode, because a full run is not what limits a router. See the
//! [measurements](#measurements).
//!
//! ## Measurements
//!
//! `large_graph_timings`, in this file's tests, generates netlist-like graphs of 80 levels
//! in which every arc is on a timed path, and times them. Run it with
//! `cargo test --release --lib timing::tests::large_graph_timings -- --ignored --nocapture`.
//!
//! These are the lowest and highest medians of 5 runs, on one thread of a shared x86-64 Linux
//! server that other jobs kept busy, so treat them as rough. "Shuffled" numbers the points
//! and arcs at random, which is the worst case for the cache. "In order" numbers them roughly
//! by level.
//!
//! | Arcs | Points | Numbering | Build | Update all net delays | [`run`](TimingAnalysis::run) | All arc criticalities |
//! | --- | --- | --- | --- | --- | --- | --- |
//! | 1,000,012 | 632,920 | Shuffled | 186-419 ms | 0.3-0.6 ms | 11-22 ms | 1.7-2.3 ms |
//! | 1,000,012 | 632,920 | In order | 78-167 ms | 0.3-0.5 ms | 8-27 ms | 1.7-2.4 ms |
//! | 10,000,008 | 6,329,120 | Shuffled | 5.0-8.2 s | 9-14 ms | 376-486 ms | 18-24 ms |
//! | 10,000,008 | 6,329,120 | In order | 2.3-2.8 s | 9-14 ms | 111-140 ms | 18-23 ms |
//!
//! That settles incremental analysis for now. A million-arc graph here has half a million
//! connections for a router to search for. Even at an optimistic 10 microseconds per search
//! that is 5 seconds an iteration if every net is rerouted, against about 25 ms for a full
//! timing update. Incremental analysis would also have to be careful with exactly what a
//! negotiated router does, which is to change delays all over the graph at once.
//!
//! The gap between the two numberings at 10 million arcs shows where to look first if this
//! ever matters: numbering points internally by level would make every graph behave like the
//! in-order one, at the price of a second set of indices.
//!
//! # What is not handled
//!
//! - **Generated clocks, clock uncertainty, skew and phase.** Every clock is ideal, with an
//!   edge at 0 and then one every period, and reaches its registers at once. A constraint
//!   between two clocks can stand in for a known phase, as [`Constraints::constrain`] takes
//!   any number.
//! - **False paths and multicycle paths.** These would also arrive through [`Constraints`].
//!   Until then, the coarse way to remove a path is to leave its arcs out of the graph.
//! - **Hold analysis.** Only the maximum delay is propagated, so this finds setup violations
//!   and not hold violations.
//! - **Rise and fall, slew and load-dependent delay.** Each arc has one delay. Delay models
//!   that want these must reduce them to a number per arc.
//! - **Latches and time borrowing.** Registers are edge-triggered.
//! - **Fractional criticality exponents**, as explained above.
//! - **Incremental analysis**, as explained above.
//!
//! # Limits
//!
//! A graph has at most 2^29 points, and a clock period is at most
//! [`MAX_CLOCK_PERIOD`]. A path visits each point at most once, so with these limits every
//! time and slack fits in an `i64` without checks in the hot loops, and
//! [`total_negative_slack`](TimingAnalysis::total_negative_slack) is an `i128` because it
//! adds up a value per capture point.
//!
//! # How a router uses this
//!
//! ```
//! use guide::timing::{ArcId, Constraints, CriticalityOptions, TimingAnalysis, TimingGraph};
//!
//! /// One iteration's timing update, where `route_delay` holds the routed delay of every
//! /// arc, as the router's own table of connections would.
//! fn after_routing_iteration(
//!     graph: &mut TimingGraph,
//!     analysis: &mut TimingAnalysis,
//!     constraints: &Constraints,
//!     options: &CriticalityOptions,
//!     route_delay: &[u32],
//!     criticality: &mut Vec<f64>,
//! ) {
//!     graph.apply_net_delays(&|arc: ArcId| route_delay[arc.index()]);
//!     analysis.run(graph, constraints);
//!     // Indexed by ArcId, so each connection finds its sink's criticality directly.
//!     analysis.fill_arc_criticalities(options, criticality);
//! }
//! # let _ = after_routing_iteration;
//! ```
//!
//! Before the first iteration there are no routes, so a domain builds the graph with
//! estimated net delays and runs once to get starting criticalities. Each arc's
//! [`ArcId`] from [`add_net_arc`](TimingGraphBuilder::add_net_arc) is the handle for that
//! connection. A net arc has one sink, so its criticality is the sink's, and a router gets a
//! net's priority by taking the highest over its arcs.

use core::fmt;
use std::error::Error;

// `crate::id` holds the crate's other identifiers, and its macro is private to that module.
// These two have the same shape, so they could move there without any change to callers.
macro_rules! define_timing_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u32);

        impl $name {
            /// Returns the identifier as an index into a table.
            #[inline]
            pub const fn index(self) -> usize {
                self.0 as usize
            }
        }

        impl From<$name> for usize {
            #[inline]
            fn from(id: $name) -> usize {
                id.index()
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }
    };
}

define_timing_id! {
    /// Identifies a timing point of a [`TimingGraph`]: a pin.
    PointId
}

define_timing_id! {
    /// Identifies an arc of a [`TimingGraph`], numbered in the order they were added.
    ArcId
}

define_timing_id! {
    /// Identifies a clock domain: the clock that a launch or capture point is timed against.
    /// See [clock domains](self#clock-domains).
    ///
    /// The numbers are the caller's to choose. Points added without one are on `ClockId(0)`.
    ClockId
}

/// The largest clock period [`Constraints`] accepts, in picoseconds: 2^61 ps, about 27 days.
pub const MAX_CLOCK_PERIOD: u64 = 1 << 61;

/// The most points a graph can have, which is what bounds a path's total delay.
const MAX_POINTS: usize = 1 << 29;

/// The most arcs a graph can have. Offsets in `u32` have to reach this count.
const MAX_ARCS: usize = u32::MAX as usize;

// The unchecked arithmetic in `TimingAnalysis::run` relies on these bounds, so a change to
// the limits has to get past this first. The largest arrival is a launch offset plus a delay
// for each arc of a path. The lowest slack subtracts that from the lowest required time, and
// the largest denominator of a criticality is the largest required time minus that slack.
const _: () = {
    let largest_arrival = (MAX_POINTS as i128 - 1) * (u32::MAX as i128) + u32::MAX as i128;
    let largest_required = MAX_CLOCK_PERIOD as i128;
    let lowest_required = -largest_arrival - u32::MAX as i128;
    let lowest_slack = lowest_required - largest_arrival;
    let largest_span = largest_required - lowest_slack;
    assert!(lowest_slack > i64::MIN as i128);
    assert!(largest_span < i64::MAX as i128);
};

/// Why a change to a [`TimingGraphBuilder`] or [`TimingGraph`], or a set of options, was
/// rejected.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum TimingError {
    /// A point has not been added to the graph.
    UnknownPoint {
        /// The rejected point.
        point: PointId,
    },
    /// An arc has not been added to the graph.
    UnknownArc {
        /// The rejected arc.
        arc: ArcId,
    },
    /// A point already launches paths. A point launches on one clock, with one offset.
    DuplicateLaunch {
        /// The rejected point.
        point: PointId,
    },
    /// A point already captures paths. A point captures on one clock, with one margin.
    DuplicateCapture {
        /// The rejected point.
        point: PointId,
    },
    /// The graph would exceed the limit of 2^29 points.
    TooManyPoints,
    /// The graph would have more arcs than a `u32` can number.
    TooManyArcs,
    /// A clock period is above [`MAX_CLOCK_PERIOD`].
    ClockPeriodTooLarge {
        /// The rejected period, in picoseconds.
        period: u64,
    },
    /// A maximum criticality is not between 0 and 1, or is NaN or negative zero.
    InvalidMaxCriticality {
        /// The rejected value.
        max: f64,
    },
    /// A clock was given two periods.
    DuplicateClock {
        /// The clock.
        clock: ClockId,
    },
    /// A constraint needs the period of a clock that has none.
    UnknownClock {
        /// The clock.
        clock: ClockId,
    },
    /// A pair of clocks was constrained on [`Constraints::single_clock`], which times every
    /// path against one clock already.
    NotPerClock,
}

impl fmt::Display for TimingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TimingError::UnknownPoint { point } => {
                write!(f, "timing point {} is not in the graph", point.0)
            }
            TimingError::UnknownArc { arc } => {
                write!(f, "timing arc {} is not in the graph", arc.0)
            }
            TimingError::DuplicateLaunch { point } => {
                write!(f, "timing point {} already launches paths", point.0)
            }
            TimingError::DuplicateCapture { point } => {
                write!(f, "timing point {} already captures paths", point.0)
            }
            TimingError::TooManyPoints => {
                write!(f, "timing graph has more than {MAX_POINTS} points")
            }
            TimingError::TooManyArcs => {
                f.write_str("timing graph has more arcs than a u32 can number")
            }
            TimingError::ClockPeriodTooLarge { period } => {
                write!(
                    f,
                    "clock period {period} ps is above the limit of {MAX_CLOCK_PERIOD} ps"
                )
            }
            TimingError::InvalidMaxCriticality { max } => {
                write!(f, "maximum criticality {max} is not between 0 and 1")
            }
            TimingError::DuplicateClock { clock } => {
                write!(f, "clock {} has more than one period", clock.0)
            }
            TimingError::UnknownClock { clock } => {
                write!(f, "clock {} has no period", clock.0)
            }
            TimingError::NotPerClock => {
                f.write_str("single-clock constraints have no pairs of clocks to constrain")
            }
        }
    }
}

impl Error for TimingError {}

/// What an arc represents, which decides who updates its delay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ArcKind {
    /// A delay through a cell, from an input pin to an output pin. Usually fixed.
    Cell,
    /// A net connection, from a driver to a sink. The router changes these.
    Net,
}

/// The two pins of a register added with [`TimingGraphBuilder::add_register`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Register {
    /// The data input, which captures paths.
    pub d: PointId,
    /// The data output, which launches paths.
    pub q: PointId,
}

/// A point that paths start from, with the time they start at and the clock they start on.
#[derive(Clone, Copy, Debug)]
struct Launch {
    point: PointId,
    offset: u32,
    clock: ClockId,
}

/// A point that paths end at, with how long before the clock edge they have to, and the
/// clock they are captured by.
#[derive(Clone, Copy, Debug)]
struct Capture {
    point: PointId,
    margin: u32,
    clock: ClockId,
}

/// An arc waiting for [`TimingGraphBuilder::build`] to sort it into place.
#[derive(Clone, Copy)]
struct PendingArc {
    from: PointId,
    to: PointId,
    delay: u32,
    kind: ArcKind,
}

const IS_LAUNCH: u8 = 1;
const IS_CAPTURE: u8 = 2;

/// Collects the points and arcs of a [`TimingGraph`].
///
/// Each change is validated as it is made, so an error points at the call that caused it
/// and [`build`](TimingGraphBuilder::build) itself cannot fail.
///
/// # Examples
///
/// ```
/// use guide::timing::{PointId, TimingError, TimingGraphBuilder};
///
/// let mut builder = TimingGraphBuilder::new();
/// let input = builder.add_input(10)?;
/// let output = builder.add_output(20)?;
/// builder.add_net_arc(input, output, 5)?;
/// assert_eq!(
///     builder.add_net_arc(input, PointId(7), 5),
///     Err(TimingError::UnknownPoint { point: PointId(7) }),
/// );
/// assert_eq!(builder.build().num_arcs(), 1);
/// # Ok::<(), TimingError>(())
/// ```
#[derive(Clone)]
pub struct TimingGraphBuilder {
    /// `IS_LAUNCH` and `IS_CAPTURE` per point, whose length is the number of points.
    roles: Vec<u8>,
    arcs: Vec<PendingArc>,
    launches: Vec<Launch>,
    captures: Vec<Capture>,
    /// `MAX_POINTS` outside of tests, which can't afford to reach that many points.
    max_points: usize,
    /// `MAX_ARCS` outside of tests, which can't afford to reach that many arcs.
    max_arcs: usize,
}

impl Default for TimingGraphBuilder {
    fn default() -> Self {
        TimingGraphBuilder::new()
    }
}

impl fmt::Debug for TimingGraphBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TimingGraphBuilder")
            .field("points", &self.roles.len())
            .field("arcs", &self.arcs.len())
            .finish_non_exhaustive()
    }
}

impl TimingGraphBuilder {
    /// Starts an empty graph.
    pub fn new() -> TimingGraphBuilder {
        TimingGraphBuilder {
            roles: Vec::new(),
            arcs: Vec::new(),
            launches: Vec::new(),
            captures: Vec::new(),
            max_points: MAX_POINTS,
            max_arcs: MAX_ARCS,
        }
    }

    /// Like [`new`](TimingGraphBuilder::new), reserving room up front.
    ///
    /// On a large graph this avoids the extra peak memory of growing the lists.
    pub fn with_capacity(points: usize, arcs: usize) -> TimingGraphBuilder {
        let mut builder = TimingGraphBuilder::new();
        builder.roles.reserve_exact(points.min(MAX_POINTS));
        builder.arcs.reserve_exact(arcs.min(MAX_ARCS));
        builder
    }

    /// Adds a point with no role in constraints yet, such as an internal pin.
    ///
    /// # Errors
    ///
    /// Returns an error if the graph already has the most points allowed.
    pub fn add_point(&mut self) -> Result<PointId, TimingError> {
        if self.roles.len() >= self.max_points {
            return Err(TimingError::TooManyPoints);
        }
        // Below MAX_POINTS, so it fits.
        let point = PointId(self.roles.len() as u32);
        self.roles.push(0);
        Ok(point)
    }

    /// Adds a primary input, whose paths start `input_delay` picoseconds after the clock.
    ///
    /// # Errors
    ///
    /// Returns an error if the graph already has the most points allowed.
    pub fn add_input(&mut self, input_delay: u32) -> Result<PointId, TimingError> {
        let point = self.add_point()?;
        self.set_launch(point, input_delay)?;
        Ok(point)
    }

    /// Adds a primary output, whose paths must end `output_delay` picoseconds before the
    /// clock.
    ///
    /// # Errors
    ///
    /// Returns an error if the graph already has the most points allowed.
    pub fn add_output(&mut self, output_delay: u32) -> Result<PointId, TimingError> {
        let point = self.add_point()?;
        self.set_capture(point, output_delay)?;
        Ok(point)
    }

    /// Adds a register: a D point that captures paths with a margin of `setup`, and a Q
    /// point that launches paths with an offset of `clock_to_q`, with no arc between them.
    ///
    /// # Errors
    ///
    /// Returns an error if the graph would have more than the most points allowed. The
    /// builder is unchanged in that case.
    pub fn add_register(&mut self, clock_to_q: u32, setup: u32) -> Result<Register, TimingError> {
        if self.roles.len() + 2 > self.max_points {
            return Err(TimingError::TooManyPoints);
        }
        let d = self.add_output(setup)?;
        let q = self.add_input(clock_to_q)?;
        Ok(Register { d, q })
    }

    /// Makes an existing point start paths, `offset` picoseconds after the clock.
    ///
    /// The point keeps any arcs into it. See [the model](self#the-model).
    ///
    /// # Errors
    ///
    /// Returns an error if the point does not exist or already launches paths.
    pub fn set_launch(&mut self, point: PointId, offset: u32) -> Result<(), TimingError> {
        self.set_launch_on(point, ClockId(0), offset)
    }

    /// Like [`set_launch`](TimingGraphBuilder::set_launch), on a given clock's edge. See
    /// [clock domains](self#clock-domains).
    ///
    /// # Errors
    ///
    /// Returns an error if the point does not exist or already launches paths, on any clock.
    pub fn set_launch_on(
        &mut self,
        point: PointId,
        clock: ClockId,
        offset: u32,
    ) -> Result<(), TimingError> {
        self.check_point(point)?;
        if self.roles[point.index()] & IS_LAUNCH != 0 {
            return Err(TimingError::DuplicateLaunch { point });
        }
        self.roles[point.index()] |= IS_LAUNCH;
        self.launches.push(Launch {
            point,
            offset,
            clock,
        });
        Ok(())
    }

    /// Makes an existing point end paths, `margin` picoseconds before the clock.
    ///
    /// The point keeps any arcs out of it. See [the model](self#the-model).
    ///
    /// # Errors
    ///
    /// Returns an error if the point does not exist or already captures paths.
    pub fn set_capture(&mut self, point: PointId, margin: u32) -> Result<(), TimingError> {
        self.set_capture_on(point, ClockId(0), margin)
    }

    /// Like [`set_capture`](TimingGraphBuilder::set_capture), by a given clock's edge. See
    /// [clock domains](self#clock-domains).
    ///
    /// # Errors
    ///
    /// Returns an error if the point does not exist or already captures paths, on any clock.
    pub fn set_capture_on(
        &mut self,
        point: PointId,
        clock: ClockId,
        margin: u32,
    ) -> Result<(), TimingError> {
        self.check_point(point)?;
        if self.roles[point.index()] & IS_CAPTURE != 0 {
            return Err(TimingError::DuplicateCapture { point });
        }
        self.roles[point.index()] |= IS_CAPTURE;
        self.captures.push(Capture {
            point,
            margin,
            clock,
        });
        Ok(())
    }

    /// Adds an arc through a cell, with a delay in picoseconds.
    ///
    /// Parallel arcs are allowed, and an arc from a point to itself is a loop.
    ///
    /// # Errors
    ///
    /// Returns an error if either point does not exist, or if the graph already has as many
    /// arcs as a `u32` can number. The builder is unchanged in that case.
    pub fn add_cell_arc(
        &mut self,
        from: PointId,
        to: PointId,
        delay: u32,
    ) -> Result<ArcId, TimingError> {
        self.add_arc(from, to, delay, ArcKind::Cell)
    }

    /// Adds a net connection from a driver to a sink, with an estimated delay in picoseconds
    /// until the router knows better.
    ///
    /// Parallel arcs are allowed, and an arc from a point to itself is a loop.
    ///
    /// # Errors
    ///
    /// Returns an error if either point does not exist, or if the graph already has as many
    /// arcs as a `u32` can number. The builder is unchanged in that case.
    pub fn add_net_arc(
        &mut self,
        from: PointId,
        to: PointId,
        delay: u32,
    ) -> Result<ArcId, TimingError> {
        self.add_arc(from, to, delay, ArcKind::Net)
    }

    fn add_arc(
        &mut self,
        from: PointId,
        to: PointId,
        delay: u32,
        kind: ArcKind,
    ) -> Result<ArcId, TimingError> {
        self.check_point(from)?;
        self.check_point(to)?;
        if self.arcs.len() >= self.max_arcs {
            return Err(TimingError::TooManyArcs);
        }
        // Below MAX_ARCS, so it fits.
        let arc = ArcId(self.arcs.len() as u32);
        self.arcs.push(PendingArc {
            from,
            to,
            delay,
            kind,
        });
        Ok(arc)
    }

    fn check_point(&self, point: PointId) -> Result<(), TimingError> {
        if point.index() < self.roles.len() {
            Ok(())
        } else {
            Err(TimingError::UnknownPoint { point })
        }
    }

    /// Levelizes the graph, breaking and reporting any combinational loops.
    ///
    /// Takes linear time in the number of points and arcs.
    pub fn build(mut self) -> TimingGraph {
        // Sorted, because lookups search these and because the lowest point wins ties. The
        // points are unique, so the order does not depend on the order they were added in.
        self.launches.sort_unstable_by_key(|launch| launch.point);
        self.captures.sort_unstable_by_key(|capture| capture.point);

        let num_points = self.roles.len();
        let fanout = FanoutTable::new(num_points, &self.arcs);
        let (is_broken, loops) = break_loops(num_points, &self.arcs, &fanout, &self.launches);
        let (levels, num_levels) = levelize(num_points, &self.arcs, &fanout, &is_broken);
        let (steps, arc_step, num_live_steps) =
            sort_into_steps(&self.arcs, &is_broken, &levels, num_levels);
        let (fanin_offsets, fanin_arcs) = fanin_table(num_points, &self.arcs, &is_broken);

        let mut kinds = Vec::with_capacity(self.arcs.len());
        let mut net_arcs = Vec::new();
        for (arc, pending) in self.arcs.iter().enumerate() {
            kinds.push(pending.kind);
            if pending.kind == ArcKind::Net {
                // Below MAX_ARCS, so it fits.
                net_arcs.push(ArcId(arc as u32));
            }
        }

        TimingGraph {
            num_points: num_points as u32,
            num_levels,
            steps: steps.into_boxed_slice(),
            num_live_steps,
            arc_step: arc_step.into_boxed_slice(),
            kinds: kinds.into_boxed_slice(),
            net_arcs: net_arcs.into_boxed_slice(),
            fanin_offsets: fanin_offsets.into_boxed_slice(),
            fanin_arcs: fanin_arcs.into_boxed_slice(),
            launches: self.launches.into_boxed_slice(),
            captures: self.captures.into_boxed_slice(),
            loops,
        }
    }
}

/// The arcs out of each point, each list in ascending [`ArcId`] order. Only needed to build.
struct FanoutTable {
    offsets: Vec<u32>,
    arcs: Vec<u32>,
}

impl FanoutTable {
    fn new(num_points: usize, arcs: &[PendingArc]) -> FanoutTable {
        let mut offsets = vec![0u32; num_points + 1];
        for arc in arcs {
            offsets[arc.from.index() + 1] += 1;
        }
        for point in 0..num_points {
            offsets[point + 1] += offsets[point];
        }
        let mut next = offsets.clone();
        let mut table = vec![0u32; arcs.len()];
        for (index, arc) in arcs.iter().enumerate() {
            let slot = &mut next[arc.from.index()];
            // Below MAX_ARCS, so it fits.
            table[*slot as usize] = index as u32;
            *slot += 1;
        }
        FanoutTable {
            offsets,
            arcs: table,
        }
    }

    fn of(&self, point: usize) -> &[u32] {
        &self.arcs[self.offsets[point] as usize..self.offsets[point + 1] as usize]
    }
}

/// Finds strongly connected components by Tarjan's algorithm, and back edges along the way
/// because the two share a depth-first search. Returns a flag per arc saying whether it is
/// disabled, and the loops in ascending order of their lowest point.
///
/// Removing all the back edges of a depth-first search leaves a graph without cycles, and
/// each back edge lies within a component, so the loops reported are exactly the components
/// that contain one. See the module documentation for why launch points are visited first.
///
/// It keeps an explicit stack because a chain of millions of points is ordinary here and
/// would overflow the call stack.
fn break_loops(
    num_points: usize,
    arcs: &[PendingArc],
    fanout: &FanoutTable,
    launches: &[Launch],
) -> (Vec<bool>, Vec<CombinationalLoop>) {
    const UNVISITED: u32 = u32::MAX;

    let mut is_broken = vec![false; arcs.len()];
    let mut any_broken = false;

    let mut order = vec![UNVISITED; num_points];
    let mut low_link = vec![0u32; num_points];
    // Where each point is in its list of arcs out, which makes the search resumable.
    let mut cursor = vec![0u32; num_points];
    // On the current search path, which is what makes an arc a back edge.
    let mut on_path = vec![false; num_points];
    // Visited but not yet assigned to a component.
    let mut pending = vec![false; num_points];
    let mut component = vec![0u32; num_points];
    let mut num_components = 0u32;
    let mut next_order = 0u32;
    let mut path: Vec<u32> = Vec::new();
    let mut component_stack: Vec<u32> = Vec::new();

    let launch_roots = launches.iter().map(|launch| launch.point.0);
    let other_roots = 0..num_points as u32;
    for root in launch_roots.chain(other_roots) {
        if order[root as usize] != UNVISITED {
            continue;
        }
        let mut enter = root;
        // Each round either enters a new point or returns from the top of the path.
        loop {
            if enter != UNVISITED {
                let point = enter as usize;
                order[point] = next_order;
                low_link[point] = next_order;
                next_order += 1;
                on_path[point] = true;
                pending[point] = true;
                path.push(enter);
                component_stack.push(enter);
                enter = UNVISITED;
            }
            let Some(&top) = path.last() else { break };
            let point = top as usize;
            let out = fanout.of(point);
            if let Some(&arc) = out.get(cursor[point] as usize) {
                cursor[point] += 1;
                let to = arcs[arc as usize].to.index();
                if order[to] == UNVISITED {
                    enter = to as u32;
                } else {
                    if on_path[to] {
                        is_broken[arc as usize] = true;
                        any_broken = true;
                    }
                    if pending[to] {
                        low_link[point] = low_link[point].min(order[to]);
                    }
                }
            } else {
                path.pop();
                on_path[point] = false;
                if let Some(&parent) = path.last() {
                    let parent = parent as usize;
                    low_link[parent] = low_link[parent].min(low_link[point]);
                }
                if low_link[point] == order[point] {
                    while let Some(member) = component_stack.pop() {
                        pending[member as usize] = false;
                        component[member as usize] = num_components;
                        if member == top {
                            break;
                        }
                    }
                    num_components += 1;
                }
            }
        }
    }

    if !any_broken {
        return (is_broken, Vec::new());
    }

    // Every component with a broken arc is a loop. Going through the points in order numbers
    // the loops by their lowest point and leaves each one's points sorted.
    const NOT_A_LOOP: u32 = u32::MAX;
    const UNNUMBERED: u32 = u32::MAX - 1;
    let mut loop_of_component = vec![NOT_A_LOOP; num_components as usize];
    let broken_arcs = || (0..arcs.len()).filter(|&arc| is_broken[arc]);
    for arc in broken_arcs() {
        loop_of_component[component[arcs[arc].from.index()] as usize] = UNNUMBERED;
    }
    let mut loops: Vec<CombinationalLoop> = Vec::new();
    for (point, &component) in component.iter().enumerate() {
        let number = &mut loop_of_component[component as usize];
        if *number == NOT_A_LOOP {
            continue;
        }
        if *number == UNNUMBERED {
            *number = loops.len() as u32;
            loops.push(CombinationalLoop {
                points: Vec::new(),
                broken_arcs: Vec::new(),
            });
        }
        // Below MAX_POINTS, so it fits.
        loops[*number as usize].points.push(PointId(point as u32));
    }
    for arc in broken_arcs() {
        let number = loop_of_component[component[arcs[arc].from.index()] as usize];
        // Below MAX_ARCS, so it fits.
        loops[number as usize].broken_arcs.push(ArcId(arc as u32));
    }
    (is_broken, loops)
}

/// Gives each point its level: 0 with no live arcs in, otherwise one more than the highest
/// level with an arc to it. Returns the levels and how many there are.
fn levelize(
    num_points: usize,
    arcs: &[PendingArc],
    fanout: &FanoutTable,
    is_broken: &[bool],
) -> (Vec<u32>, u32) {
    let mut waiting_for = vec![0u32; num_points];
    for (arc, broken) in arcs.iter().zip(is_broken) {
        if !broken {
            waiting_for[arc.to.index()] += 1;
        }
    }
    let mut ready: Vec<u32> = (0..num_points as u32)
        .filter(|&point| waiting_for[point as usize] == 0)
        .collect();
    let mut levels = vec![0u32; num_points];
    let mut num_levels = u32::from(num_points > 0);
    // The level does not depend on the order points are taken in, so a stack serves.
    while let Some(point) = ready.pop() {
        let point = point as usize;
        let next_level = levels[point] + 1;
        for &arc in fanout.of(point) {
            if is_broken[arc as usize] {
                continue;
            }
            let to = arcs[arc as usize].to.index();
            if levels[to] < next_level {
                levels[to] = next_level;
                num_levels = num_levels.max(next_level + 1);
            }
            waiting_for[to] -= 1;
            if waiting_for[to] == 0 {
                ready.push(to as u32);
            }
        }
    }
    (levels, num_levels)
}

/// One arc as analysis sees it. Copies of the end points sit here so that a pass over arcs
/// reads one array in order, rather than following an index to a far-off table per arc.
#[derive(Clone, Copy, Debug)]
struct Step {
    from: PointId,
    to: PointId,
    delay: u32,
    arc: ArcId,
}

/// Puts arcs into steps, in ascending order of the level of their source and then of
/// [`ArcId`], with broken arcs last. Returns the steps, the step of each arc, and how many
/// are live.
///
/// Sorting by the source's level is all it takes for one pass to be a topological traversal:
/// every arc into a point has a source of a lower level, so comes before every arc out of
/// that point. In reverse, the same holds for a backward pass.
fn sort_into_steps(
    arcs: &[PendingArc],
    is_broken: &[bool],
    levels: &[u32],
    num_levels: u32,
) -> (Vec<Step>, Vec<u32>, usize) {
    let mut next = vec![0u32; num_levels as usize + 1];
    for (arc, broken) in arcs.iter().zip(is_broken) {
        if !broken {
            next[levels[arc.from.index()] as usize + 1] += 1;
        }
    }
    for level in 0..num_levels as usize {
        next[level + 1] += next[level];
    }
    let num_live = next[num_levels as usize] as usize;
    let mut next_broken = num_live;

    let placeholder = Step {
        from: PointId(0),
        to: PointId(0),
        delay: 0,
        arc: ArcId(0),
    };
    let mut steps = vec![placeholder; arcs.len()];
    let mut arc_step = vec![0u32; arcs.len()];
    for (index, (arc, &broken)) in arcs.iter().zip(is_broken).enumerate() {
        let slot = if broken {
            next_broken += 1;
            next_broken - 1
        } else {
            let slot = &mut next[levels[arc.from.index()] as usize];
            *slot += 1;
            *slot as usize - 1
        };
        // Below MAX_ARCS, so these fit.
        steps[slot] = Step {
            from: arc.from,
            to: arc.to,
            delay: arc.delay,
            arc: ArcId(index as u32),
        };
        arc_step[index] = slot as u32;
    }
    (steps, arc_step, num_live)
}

/// The live arcs into each point, each list in ascending [`ArcId`] order.
fn fanin_table(
    num_points: usize,
    arcs: &[PendingArc],
    is_broken: &[bool],
) -> (Vec<u32>, Vec<ArcId>) {
    let mut offsets = vec![0u32; num_points + 1];
    let mut num_live = 0;
    for (arc, broken) in arcs.iter().zip(is_broken) {
        if !broken {
            offsets[arc.to.index() + 1] += 1;
            num_live += 1;
        }
    }
    for point in 0..num_points {
        offsets[point + 1] += offsets[point];
    }
    let mut next = offsets.clone();
    let mut table = vec![ArcId(0); num_live];
    for (index, (arc, broken)) in arcs.iter().zip(is_broken).enumerate() {
        if !broken {
            let slot = &mut next[arc.to.index()];
            // Below MAX_ARCS, so it fits.
            table[*slot as usize] = ArcId(index as u32);
            *slot += 1;
        }
    }
    (offsets, table)
}

/// A combinational loop found in a graph: a strongly connected set of points.
///
/// Several overlapping cycles form one loop, since no arc can be charged to one of them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct CombinationalLoop {
    /// The points involved, in ascending order. A point with an arc to itself is the only
    /// loop with a single point.
    pub points: Vec<PointId>,
    /// The arcs disabled to break the loop, in ascending order. Analysis ignores them.
    pub broken_arcs: Vec<ArcId>,
}

/// One arc's details, as returned by [`TimingGraph::arc`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ArcInfo {
    /// The point the arc starts from.
    pub from: PointId,
    /// The point the arc ends at.
    pub to: PointId,
    /// The current delay, in picoseconds.
    pub delay: u32,
    /// What the arc represents.
    pub kind: ArcKind,
}

/// Supplies net connection delays to [`TimingGraph::apply_net_delays`].
///
/// This is the delay model's interface to the graph: a router would answer from the delays
/// of its current routes, and a placer from a table of estimates by distance. Closures
/// implement this, so a model usually doesn't need a type of its own.
pub trait DelayModel {
    /// Returns the delay, in picoseconds, of a net arc.
    fn net_delay(&self, arc: ArcId) -> u32;
}

impl<F: Fn(ArcId) -> u32> DelayModel for F {
    #[inline]
    fn net_delay(&self, arc: ArcId) -> u32 {
        self(arc)
    }
}

/// An immutable graph of timing points and arcs, except that delays can be changed.
///
/// Built by a [`TimingGraphBuilder`]. Storage is flat arrays with `u32` IDs, at 29 bytes per
/// arc or fewer, 4 per point, and 12 per launch or capture point.
#[derive(Clone)]
pub struct TimingGraph {
    num_points: u32,
    num_levels: u32,
    /// All arcs, as described at [`sort_into_steps`].
    steps: Box<[Step]>,
    num_live_steps: usize,
    /// The position in `steps` of each arc, by [`ArcId`].
    arc_step: Box<[u32]>,
    /// By [`ArcId`].
    kinds: Box<[ArcKind]>,
    net_arcs: Box<[ArcId]>,
    fanin_offsets: Box<[u32]>,
    fanin_arcs: Box<[ArcId]>,
    /// In ascending order of point.
    launches: Box<[Launch]>,
    /// In ascending order of point.
    captures: Box<[Capture]>,
    loops: Vec<CombinationalLoop>,
}

impl fmt::Debug for TimingGraph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TimingGraph")
            .field("points", &self.num_points)
            .field("arcs", &self.steps.len())
            .field("levels", &self.num_levels)
            .field("loops", &self.loops.len())
            .finish_non_exhaustive()
    }
}

impl TimingGraph {
    /// Returns the number of points.
    pub fn num_points(&self) -> u32 {
        self.num_points
    }

    /// Returns the number of arcs, including any disabled to break loops.
    pub fn num_arcs(&self) -> u32 {
        // Never above MAX_ARCS.
        self.steps.len() as u32
    }

    /// Returns the number of levels, which is also the most points any path can have.
    pub fn num_levels(&self) -> u32 {
        self.num_levels
    }

    /// Returns the combinational loops found, in ascending order of their lowest point.
    ///
    /// If this is not empty, results are a lower bound. See
    /// [the module documentation](self#combinational-loops-are-broken-and-reported).
    pub fn loops(&self) -> &[CombinationalLoop] {
        &self.loops
    }

    /// Returns whether an arc was disabled to break a loop. An unknown arc was not.
    pub fn is_broken(&self, arc: ArcId) -> bool {
        self.arc_step
            .get(arc.index())
            .is_some_and(|&step| step as usize >= self.num_live_steps)
    }

    /// Returns an arc's end points, current delay and kind, or `None` for an unknown arc.
    pub fn arc(&self, arc: ArcId) -> Option<ArcInfo> {
        let step = self.steps[*self.arc_step.get(arc.index())? as usize];
        Some(ArcInfo {
            from: step.from,
            to: step.to,
            delay: step.delay,
            kind: self.kinds[arc.index()],
        })
    }

    /// Returns the [net](ArcKind::Net) arcs, in ascending order.
    ///
    /// [`apply_net_delays`](TimingGraph::apply_net_delays) visits them in this order.
    pub fn net_arcs(&self) -> &[ArcId] {
        &self.net_arcs
    }

    /// Returns the arcs into a point, in ascending order, without any disabled to break
    /// loops. An unknown point has none.
    pub fn fanin(&self, point: PointId) -> &[ArcId] {
        let start = self.fanin_offsets.get(point.index());
        let end = self.fanin_offsets.get(point.index() + 1);
        match (start, end) {
            (Some(&start), Some(&end)) => &self.fanin_arcs[start as usize..end as usize],
            _ => &[],
        }
    }

    /// Returns a point's launch offset, if it launches paths.
    pub fn launch_offset(&self, point: PointId) -> Option<u32> {
        self.launch(point).map(|launch| launch.offset)
    }

    /// Returns a point's capture margin, if it captures paths.
    pub fn capture_margin(&self, point: PointId) -> Option<u32> {
        self.capture(point).map(|capture| capture.margin)
    }

    /// Returns the clock a point launches paths on, if it launches paths.
    pub fn launch_clock(&self, point: PointId) -> Option<ClockId> {
        self.launch(point).map(|launch| launch.clock)
    }

    /// Returns the clock a point captures paths by, if it captures paths.
    pub fn capture_clock(&self, point: PointId) -> Option<ClockId> {
        self.capture(point).map(|capture| capture.clock)
    }

    /// Returns the clocks that launch or capture any path, in ascending order.
    pub fn clocks(&self) -> Vec<ClockId> {
        let mut clocks: Vec<ClockId> = self
            .launches
            .iter()
            .map(|launch| launch.clock)
            .chain(self.captures.iter().map(|capture| capture.clock))
            .collect();
        clocks.sort_unstable();
        clocks.dedup();
        clocks
    }

    fn launch(&self, point: PointId) -> Option<&Launch> {
        let found = self.launches.binary_search_by_key(&point, |l| l.point);
        found.ok().map(|index| &self.launches[index])
    }

    fn capture(&self, point: PointId) -> Option<&Capture> {
        let found = self.captures.binary_search_by_key(&point, |c| c.point);
        found.ok().map(|index| &self.captures[index])
    }

    /// Changes one arc's delay. Takes effect at the next [`run`](TimingAnalysis::run).
    ///
    /// This takes any arc, so a domain can change cell delays too.
    ///
    /// # Errors
    ///
    /// Returns an error if the arc does not exist.
    #[inline]
    pub fn set_delay(&mut self, arc: ArcId, delay: u32) -> Result<(), TimingError> {
        match self.arc_step.get(arc.index()) {
            Some(&step) => {
                self.steps[step as usize].delay = delay;
                Ok(())
            }
            None => Err(TimingError::UnknownArc { arc }),
        }
    }

    /// Asks a delay model for the delay of every net arc, leaving cell arcs as they are.
    ///
    /// Takes effect at the next [`run`](TimingAnalysis::run), and does not allocate.
    pub fn apply_net_delays(&mut self, model: &impl DelayModel) {
        for &arc in &self.net_arcs {
            let delay = model.net_delay(arc);
            self.steps[self.arc_step[arc.index()] as usize].delay = delay;
        }
    }
}

/// What a [`TimingAnalysis`] checks a graph against: one ideal clock for every path, or a
/// constraint for each pair of [clock domains](self#clock-domains) that is timed.
///
/// Fields are private and this has constructors rather than public fields, so that false
/// paths and multicycle paths can be added through new methods. See [what is not
/// handled](self#what-is-not-handled).
///
/// # Examples
///
/// ```
/// use guide::timing::{ClockId, Constraints};
///
/// let (io, fast, slow) = (ClockId(0), ClockId(1), ClockId(2));
/// let constraints = Constraints::per_clock(&[(io, 0), (fast, 400), (slow, 1000)])?
///     // Paths from the slow clock to the fast one, at the closest pair of edges.
///     .constrain_by_edges(slow, fast)?;
/// assert_eq!(constraints.constraint(fast, fast), Some(400));
/// assert_eq!(constraints.constraint(slow, fast), Some(200));
/// // Not timed, as VPR's default has it.
/// assert_eq!(constraints.constraint(fast, slow), None);
/// # Ok::<(), guide::timing::TimingError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Constraints {
    clock_period: u64,
    /// `None` for one clock for everything.
    clocks: Option<PerClock>,
}

/// Constraints by clock domain.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PerClock {
    /// Each clock's period, in ascending order of clock.
    periods: Vec<(ClockId, u64)>,
    /// The timed pairs, as launch, capture and constraint, in ascending order of the pair.
    pairs: Vec<(ClockId, ClockId, u64)>,
}

fn check_period(period: u64) -> Result<(), TimingError> {
    if period > MAX_CLOCK_PERIOD {
        return Err(TimingError::ClockPeriodTooLarge { period });
    }
    Ok(())
}

/// The setup constraint between two ideal clocks whose edges are both at 0 and then every
/// period: the shortest time from a launch edge to the next capture edge after it, over all
/// edges. That is the greatest common divisor of the periods, or the period itself for one
/// clock with itself, and 0 if either period is 0.
fn edge_separation(launch: u64, capture: u64) -> u64 {
    if launch == 0 || capture == 0 {
        return 0;
    }
    let (mut a, mut b) = (launch, capture);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl Constraints {
    /// Checks every path against one clock, with a period in picoseconds, whatever clock
    /// its points are on.
    ///
    /// A period of 0 asks for the fastest possible circuit, as VPR does when it has no
    /// constraint: every path with any delay fails, and criticality measures how close each
    /// connection is to the longest path.
    ///
    /// # Errors
    ///
    /// Returns an error if the period is above [`MAX_CLOCK_PERIOD`].
    pub fn single_clock(period: u64) -> Result<Constraints, TimingError> {
        check_period(period)?;
        Ok(Constraints {
            clock_period: period,
            clocks: None,
        })
    }

    /// Checks the paths within each clock domain against its period, in picoseconds, and
    /// leaves every other path untimed, as VPR does by default for paths between netlist
    /// clocks. A clock that isn't listed times nothing. Paths between two clocks can then be
    /// timed with [`constrain`](Constraints::constrain) or
    /// [`constrain_by_edges`](Constraints::constrain_by_edges).
    ///
    /// # Errors
    ///
    /// Returns an error if a period is above [`MAX_CLOCK_PERIOD`], or a clock is listed
    /// twice.
    pub fn per_clock(periods: &[(ClockId, u64)]) -> Result<Constraints, TimingError> {
        let mut sorted = periods.to_vec();
        sorted.sort_unstable();
        for pair in sorted.windows(2) {
            if pair[0].0 == pair[1].0 {
                return Err(TimingError::DuplicateClock { clock: pair[0].0 });
            }
        }
        for &(_, period) in &sorted {
            check_period(period)?;
        }
        let pairs = sorted
            .iter()
            .map(|&(clock, period)| (clock, clock, period))
            .collect();
        Ok(Constraints {
            clock_period: 0,
            clocks: Some(PerClock {
                periods: sorted,
                pairs,
            }),
        })
    }

    /// Times the paths from `launch`'s registers to `capture`'s: each has to arrive
    /// `constraint` picoseconds after the launching edge, less the capture margin. For a
    /// clock with itself, this replaces its period. The clocks need no period of their own.
    ///
    /// # Errors
    ///
    /// Returns an error on [`single_clock`](Constraints::single_clock) constraints, or if
    /// the constraint is above [`MAX_CLOCK_PERIOD`].
    pub fn constrain(
        mut self,
        launch: ClockId,
        capture: ClockId,
        constraint: u64,
    ) -> Result<Constraints, TimingError> {
        check_period(constraint)?;
        let clocks = self.clocks.as_mut().ok_or(TimingError::NotPerClock)?;
        match clocks
            .pairs
            .binary_search_by_key(&(launch, capture), |&(l, c, _)| (l, c))
        {
            Ok(index) => clocks.pairs[index].2 = constraint,
            Err(index) => clocks.pairs.insert(index, (launch, capture, constraint)),
        }
        Ok(self)
    }

    /// Times the paths from `launch`'s registers to `capture`'s against the two periods, as
    /// VPR does for two ideal clocks: both have an edge at 0 and then one every period, and
    /// a path launched on one edge is captured by the next capture edge after it. The
    /// constraint is the closest that any two such edges come, which is the greatest common
    /// divisor of the periods: 200 for 400 and 1000. It is 0 if either period is 0.
    ///
    /// # Errors
    ///
    /// Returns an error on [`single_clock`](Constraints::single_clock) constraints, or if
    /// either clock has no period.
    pub fn constrain_by_edges(
        self,
        launch: ClockId,
        capture: ClockId,
    ) -> Result<Constraints, TimingError> {
        let clocks = self.clocks.as_ref().ok_or(TimingError::NotPerClock)?;
        let period = |clock| {
            clocks
                .periods
                .binary_search_by_key(&clock, |&(c, _)| c)
                .map(|index| clocks.periods[index].1)
                .map_err(|_| TimingError::UnknownClock { clock })
        };
        let separation = edge_separation(period(launch)?, period(capture)?);
        self.constrain(launch, capture, separation)
    }

    /// Returns whether these are constraints per clock, rather than one clock for all.
    pub fn is_per_clock(&self) -> bool {
        self.clocks.is_some()
    }

    /// Returns the clock period of [`single_clock`](Constraints::single_clock) constraints,
    /// in picoseconds. Constraints [per clock](Constraints::per_clock) have no single period,
    /// and return 0: see [`period`](Constraints::period) and
    /// [`constraint`](Constraints::constraint).
    pub fn clock_period(&self) -> u64 {
        self.clock_period
    }

    /// Returns a clock's period: the one of `single_clock` for any clock, or the one given to
    /// [`per_clock`](Constraints::per_clock).
    pub fn period(&self, clock: ClockId) -> Option<u64> {
        match &self.clocks {
            None => Some(self.clock_period),
            Some(clocks) => clocks
                .periods
                .binary_search_by_key(&clock, |&(c, _)| c)
                .ok()
                .map(|index| clocks.periods[index].1),
        }
    }

    /// Returns what paths from `launch`'s registers to `capture`'s are checked against, or
    /// `None` if they are not timed.
    pub fn constraint(&self, launch: ClockId, capture: ClockId) -> Option<u64> {
        match &self.clocks {
            None => Some(self.clock_period),
            Some(clocks) => clocks
                .pairs
                .binary_search_by_key(&(launch, capture), |&(l, c, _)| (l, c))
                .ok()
                .map(|index| clocks.pairs[index].2),
        }
    }
}

/// How slack turns into criticality. See [the formula](self#criticality).
///
/// # Examples
///
/// ```
/// use guide::timing::CriticalityOptions;
///
/// let options = CriticalityOptions::default();
/// assert_eq!(options.exponent(), 1);
/// assert_eq!(options.max(), 0.99);
/// assert!(CriticalityOptions::new(2, 1.5).is_err());
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CriticalityOptions {
    exponent: u32,
    max: f64,
}

impl CriticalityOptions {
    /// Sets the exponent (VPR's `--criticality_exp`) and maximum (VPR's `--max_criticality`).
    ///
    /// A higher exponent concentrates the router's delay effort on the most critical
    /// connections. A maximum of 1 lets critical connections ignore congestion entirely,
    /// which negotiated congestion needs a way out of, so it is best kept below 1.
    ///
    /// # Errors
    ///
    /// Returns an error if `max` is not in `[0, 1]`, or is NaN or negative zero.
    pub fn new(exponent: u32, max: f64) -> Result<CriticalityOptions, TimingError> {
        // Negative zero is out because the cap would hand it back as a result, and a sign on
        // a zero is something a later division or comparison of bits could tell apart.
        if !(0.0..=1.0).contains(&max) || max.is_sign_negative() {
            return Err(TimingError::InvalidMaxCriticality { max });
        }
        Ok(CriticalityOptions { exponent, max })
    }

    /// Returns the exponent.
    pub fn exponent(&self) -> u32 {
        self.exponent
    }

    /// Returns the maximum.
    pub fn max(&self) -> f64 {
        self.max
    }

    /// Applies the maximum and exponent to a criticality in `[0, 1]`.
    #[inline]
    fn sharpen(&self, raw: f64) -> f64 {
        let shifted = raw - (1.0 - self.max);
        let base = if shifted > 0.0 { shifted } else { 0.0 };
        let power = whole_power(base, self.exponent);
        if power < self.max { power } else { self.max }
    }
}

impl Default for CriticalityOptions {
    /// VPR's defaults: an exponent of 1 and a maximum of 0.99.
    fn default() -> Self {
        CriticalityOptions {
            exponent: 1,
            max: 0.99,
        }
    }
}

/// Computes `base ^ exponent` by square-and-multiply, with 0^0 = 1.
///
/// The sequence of operations depends only on the exponent, so the result does not depend on
/// the platform, unlike with [`f64::powi`]. An exponent of 1 returns `base` unchanged, since
/// multiplying by 1 is exact.
#[inline]
fn whole_power(base: f64, exponent: u32) -> f64 {
    let mut result = 1.0;
    let mut square = base;
    let mut remaining = exponent;
    while remaining > 0 {
        if remaining & 1 == 1 {
            result *= square;
        }
        square *= square;
        remaining >>= 1;
    }
    result
}

/// Marks a point with no arrival time. The lowest `i64`, so `max` needs no special case.
const NO_ARRIVAL: i64 = i64::MIN;
/// Marks a point with no required time. The highest `i64`, so `min` needs no special case.
const NO_REQUIRED: i64 = i64::MAX;
/// Marks an arc with no slack. Never a real slack, as the check on the limits proves.
const NO_SLACK: i64 = i64::MAX;

/// The results of analysing a [`TimingGraph`], and the buffers to compute them in.
///
/// Create one per graph and keep it between runs. It takes 16 bytes per point, 8 per arc, and
/// 8 per level. Results are a snapshot of the latest
/// [`run`](TimingAnalysis::run), and don't change if the graph's delays change after it. All
/// queries are constant time, and an unknown point or arc gets the same answer as an
/// unconstrained one.
#[derive(Clone)]
pub struct TimingAnalysis {
    arrival: Vec<i64>,
    required: Vec<i64>,
    /// By [`ArcId`].
    arc_slack: Vec<i64>,
    worst_slack: Option<i64>,
    total_negative_slack: i128,
    /// The largest required time of any constrained point.
    max_required: i64,
    /// `max_required` plus the shift, the denominator of every criticality.
    criticality_span: u64,
    path_points: Vec<PointId>,
    path_arcs: Vec<ArcId>,
    /// When the critical path reaches its end, on the clock it was launched by.
    path_arrival: u64,
    /// The rest is only used by a run on constraints per clock, as `per_clock` says.
    per_clock: bool,
    /// The worst slack of each point over the pairs of clocks, or `NO_SLACK`.
    point_slack: Vec<i64>,
    /// The highest raw criticality of each point and arc over the pairs of clocks.
    point_raw: Vec<f64>,
    arc_raw: Vec<f64>,
    /// The clocks that launch or capture in a timed pair, in ascending order.
    launch_clocks: Vec<ClockId>,
    capture_clocks: Vec<ClockId>,
    /// The arrival time of each point from each launching clock, a table of points after
    /// another, in the order of `launch_clocks`.
    clock_arrival: Vec<i64>,
    /// The required time of each point for each capturing clock, against a constraint of 0,
    /// in the order of `capture_clocks`.
    clock_required: Vec<i64>,
}

impl fmt::Debug for TimingAnalysis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TimingAnalysis")
            .field("worst_slack", &self.worst_slack)
            .field("total_negative_slack", &self.total_negative_slack)
            .finish_non_exhaustive()
    }
}

impl TimingAnalysis {
    /// Allocates for a graph. Until the first [`run`](TimingAnalysis::run), every query
    /// answers as if nothing were constrained.
    pub fn new(graph: &TimingGraph) -> TimingAnalysis {
        let levels = graph.num_levels as usize;
        TimingAnalysis {
            arrival: vec![NO_ARRIVAL; graph.num_points as usize],
            required: vec![NO_REQUIRED; graph.num_points as usize],
            arc_slack: vec![NO_SLACK; graph.steps.len()],
            worst_slack: None,
            total_negative_slack: 0,
            max_required: 0,
            criticality_span: 0,
            path_points: Vec::with_capacity(levels),
            path_arcs: Vec::with_capacity(levels),
            path_arrival: 0,
            per_clock: false,
            point_slack: Vec::new(),
            point_raw: Vec::new(),
            arc_raw: Vec::new(),
            launch_clocks: Vec::new(),
            capture_clocks: Vec::new(),
            clock_arrival: Vec::new(),
            clock_required: Vec::new(),
        }
    }

    /// Analyses a graph with its current delays, replacing the previous results.
    ///
    /// On [`single_clock`](Constraints::single_clock) constraints, this does not allocate
    /// when given the graph this was created for. Given another, it resizes its buffers and
    /// carries on. Constraints [per clock](Constraints::per_clock) take 16 bytes more per
    /// point and 8 per arc, and 8 per point for each clock in a timed pair, which the first
    /// such run allocates. See [clock domains](self#clock-domains).
    pub fn run(&mut self, graph: &TimingGraph, constraints: &Constraints) {
        match &constraints.clocks {
            None => self.run_single(graph, constraints.clock_period),
            Some(clocks) => self.run_per_clock(graph, clocks),
        }
    }

    fn run_single(&mut self, graph: &TimingGraph, period: u64) {
        self.per_clock = false;
        // At most MAX_CLOCK_PERIOD, which fits.
        let period = period as i64;
        let live_steps = &graph.steps[..graph.num_live_steps];

        // Paths with no launch point have no arrival time, and the sentinels let this keep
        // that apart from a real time of zero without a separate table of flags.
        let arrival = &mut self.arrival;
        arrival.clear();
        arrival.resize(graph.num_points as usize, NO_ARRIVAL);
        for launch in &graph.launches {
            arrival[launch.point.index()] = i64::from(launch.offset);
        }
        for step in live_steps {
            let from = arrival[step.from.index()];
            if from != NO_ARRIVAL {
                let candidate = from + i64::from(step.delay);
                let to = &mut arrival[step.to.index()];
                if candidate > *to {
                    *to = candidate;
                }
            }
        }

        let required = &mut self.required;
        required.clear();
        required.resize(graph.num_points as usize, NO_REQUIRED);
        for capture in &graph.captures {
            required[capture.point.index()] = period - i64::from(capture.margin);
        }
        for step in live_steps.iter().rev() {
            let to = required[step.to.index()];
            if to != NO_REQUIRED {
                let candidate = to - i64::from(step.delay);
                let from = &mut required[step.from.index()];
                if candidate < *from {
                    *from = candidate;
                }
            }
        }

        self.arc_slack.clear();
        self.arc_slack.resize(graph.steps.len(), NO_SLACK);
        for step in live_steps {
            let from = arrival[step.from.index()];
            let to = required[step.to.index()];
            if from != NO_ARRIVAL && to != NO_REQUIRED {
                self.arc_slack[step.arc.index()] = to - i64::from(step.delay) - from;
            }
        }

        // Worst slack and total negative slack count each capture point once, against its own
        // margin rather than its required time, since the latter can come from further on
        // when a capture point has arcs out. The worst slack of any point is always found
        // here: the worst path ends at a capture point, whose latest arrival makes it so.
        //
        // Likewise, the largest required time of any constrained point is at a capture point:
        // every other one has a constrained capture point after it, with a later time.
        let mut worst: Option<(i64, PointId)> = None;
        let mut total_negative_slack = 0i128;
        let mut max_required = i64::MIN;
        for capture in &graph.captures {
            let arrived = arrival[capture.point.index()];
            if arrived == NO_ARRIVAL {
                continue;
            }
            let slack = period - i64::from(capture.margin) - arrived;
            // Captures are in point order and this is strict, so the lowest point wins.
            if worst.is_none_or(|(lowest, _)| slack < lowest) {
                worst = Some((slack, capture.point));
            }
            if slack < 0 {
                total_negative_slack += i128::from(slack);
            }
            max_required = max_required.max(required[capture.point.index()]);
        }
        self.worst_slack = worst.map(|(slack, _)| slack);
        self.total_negative_slack = total_negative_slack;
        self.path_arcs.clear();
        self.path_points.clear();

        let Some((worst_slack, worst_end)) = worst else {
            self.max_required = 0;
            self.criticality_span = 0;
            return;
        };
        self.max_required = max_required;
        // `max_required + shift`, which the proof at `raw_criticality` shows is not negative.
        self.criticality_span = max_required.abs_diff(worst_slack.min(0));

        // Real arrival times are never negative.
        self.path_arrival = arrival[worst_end.index()] as u64;
        trace_back(arrival, graph, worst_end, &mut self.path_arcs, |point| {
            graph.launch_offset(point)
        });
        self.finish_path(graph, worst_end);
    }

    /// Fills in the critical path's points from its arcs, which `trace_back` left from the
    /// end.
    fn finish_path(&mut self, graph: &TimingGraph, end: PointId) {
        self.path_arcs.reverse();
        let start = match self.path_arcs.first() {
            Some(first) => graph.steps[graph.arc_step[first.index()] as usize].from,
            None => end,
        };
        self.path_points.push(start);
        for arc in &self.path_arcs {
            let step = &graph.steps[graph.arc_step[arc.index()] as usize];
            self.path_points.push(step.to);
        }
    }

    /// A run on constraints per clock. See [clock domains](self#clock-domains).
    fn run_per_clock(&mut self, graph: &TimingGraph, clocks: &PerClock) {
        self.per_clock = true;
        let points = graph.num_points as usize;
        let live_steps = &graph.steps[..graph.num_live_steps];

        self.launch_clocks.clear();
        self.capture_clocks.clear();
        for &(launch, capture, _) in &clocks.pairs {
            self.launch_clocks.push(launch);
            self.capture_clocks.push(capture);
        }
        self.launch_clocks.sort_unstable();
        self.launch_clocks.dedup();
        self.capture_clocks.sort_unstable();
        self.capture_clocks.dedup();

        // Arrival times from each launching clock, as for one clock but from its own
        // launch points only.
        self.clock_arrival.clear();
        self.clock_arrival
            .resize(points * self.launch_clocks.len(), NO_ARRIVAL);
        for (&clock, arrival) in self
            .launch_clocks
            .iter()
            .zip(self.clock_arrival.chunks_exact_mut(points.max(1)))
        {
            for launch in graph.launches.iter().filter(|l| l.clock == clock) {
                arrival[launch.point.index()] = i64::from(launch.offset);
            }
            for step in live_steps {
                let from = arrival[step.from.index()];
                if from != NO_ARRIVAL {
                    let candidate = from + i64::from(step.delay);
                    let to = &mut arrival[step.to.index()];
                    if candidate > *to {
                        *to = candidate;
                    }
                }
            }
        }

        // Required times for each capturing clock, against a constraint of 0. A pair's are
        // its constraint more, as the constraint only shifts every one of them.
        self.clock_required.clear();
        self.clock_required
            .resize(points * self.capture_clocks.len(), NO_REQUIRED);
        for (&clock, required) in self
            .capture_clocks
            .iter()
            .zip(self.clock_required.chunks_exact_mut(points.max(1)))
        {
            for capture in graph.captures.iter().filter(|c| c.clock == clock) {
                required[capture.point.index()] = -i64::from(capture.margin);
            }
            for step in live_steps.iter().rev() {
                let to = required[step.to.index()];
                if to != NO_REQUIRED {
                    let candidate = to - i64::from(step.delay);
                    let from = &mut required[step.from.index()];
                    if candidate < *from {
                        *from = candidate;
                    }
                }
            }
        }

        self.arrival.clear();
        self.arrival.resize(points, NO_ARRIVAL);
        for arrival in self.clock_arrival.chunks_exact(points.max(1)) {
            for (combined, &time) in self.arrival.iter_mut().zip(arrival) {
                *combined = (*combined).max(time);
            }
        }
        self.required.clear();
        self.required.resize(points, NO_REQUIRED);
        self.arc_slack.clear();
        self.arc_slack.resize(graph.steps.len(), NO_SLACK);
        self.arc_raw.clear();
        self.arc_raw.resize(graph.steps.len(), 0.0);
        self.point_slack.clear();
        self.point_slack.resize(points, NO_SLACK);
        self.point_raw.clear();
        self.point_raw.resize(points, 0.0);

        // The worst slack over all pairs, its end, and the pair's launching clock. Ties go to
        // the lowest end, as with one clock, and then to the first pair.
        let mut worst: Option<(i64, PointId, usize)> = None;
        let mut total_negative_slack = 0i128;
        for &(launch, capture, constraint) in &clocks.pairs {
            // Both are in the lists, which come from these pairs.
            let launch_index = self.launch_clocks.binary_search(&launch).unwrap_or(0);
            let capture_index = self.capture_clocks.binary_search(&capture).unwrap_or(0);
            let arrival = &self.clock_arrival[launch_index * points..][..points];
            let required = &self.clock_required[capture_index * points..][..points];
            // At most MAX_CLOCK_PERIOD, which fits.
            let constraint = constraint as i64;

            for (combined, &time) in self.required.iter_mut().zip(required) {
                if time != NO_REQUIRED {
                    *combined = (*combined).min(constraint + time);
                }
            }

            // As with one clock, each capture point counts once, against its own margin, and
            // the largest required time of the pair is at one of them.
            let mut pair_worst: Option<i64> = None;
            let mut max_required = i64::MIN;
            for capture_point in graph.captures.iter().filter(|c| c.clock == capture) {
                let arrived = arrival[capture_point.point.index()];
                if arrived == NO_ARRIVAL {
                    continue;
                }
                let slack = constraint - i64::from(capture_point.margin) - arrived;
                if pair_worst.is_none_or(|lowest| slack < lowest) {
                    pair_worst = Some(slack);
                }
                if worst.is_none_or(|(lowest, end, _)| {
                    slack < lowest || (slack == lowest && capture_point.point < end)
                }) {
                    worst = Some((slack, capture_point.point, launch_index));
                }
                if slack < 0 {
                    total_negative_slack += i128::from(slack);
                }
                max_required = max_required.max(constraint + required[capture_point.point.index()]);
            }
            let Some(pair_worst) = pair_worst else {
                continue;
            };
            // As for one clock, and for the same reasons, within the pair.
            let span = max_required.abs_diff(pair_worst.min(0));

            for step in live_steps {
                let from = arrival[step.from.index()];
                let to = required[step.to.index()];
                if from != NO_ARRIVAL && to != NO_REQUIRED {
                    let slack = constraint + to - i64::from(step.delay) - from;
                    let index = step.arc.index();
                    if slack < self.arc_slack[index] {
                        self.arc_slack[index] = slack;
                    }
                    let raw = raw_ratio(max_required, span, slack);
                    if raw > self.arc_raw[index] {
                        self.arc_raw[index] = raw;
                    }
                }
            }
            for (point, (&from, &to)) in arrival.iter().zip(required).enumerate() {
                if from != NO_ARRIVAL && to != NO_REQUIRED {
                    let slack = constraint + to - from;
                    if slack < self.point_slack[point] {
                        self.point_slack[point] = slack;
                    }
                    let raw = raw_ratio(max_required, span, slack);
                    if raw > self.point_raw[point] {
                        self.point_raw[point] = raw;
                    }
                }
            }
        }

        self.worst_slack = worst.map(|(slack, _, _)| slack);
        self.total_negative_slack = total_negative_slack;
        self.max_required = 0;
        self.criticality_span = 0;
        self.path_arcs.clear();
        self.path_points.clear();
        let Some((_, end, launch_index)) = worst else {
            return;
        };
        let clock = self.launch_clocks[launch_index];
        let arrival = &self.clock_arrival[launch_index * points..][..points];
        // Real arrival times are never negative.
        self.path_arrival = arrival[end.index()] as u64;
        trace_back(arrival, graph, end, &mut self.path_arcs, |point| {
            graph
                .launch(point)
                .filter(|launch| launch.clock == clock)
                .map(|launch| launch.offset)
        });
        self.finish_path(graph, end);
    }

    /// Returns the latest time a path reaches a point, or `None` if none does.
    pub fn arrival(&self, point: PointId) -> Option<u64> {
        let arrival = *self.arrival.get(point.index())?;
        // Real arrival times are never negative, and the sentinel always is.
        u64::try_from(arrival).ok()
    }

    /// Returns the latest time a point can be reached without any path from it being late,
    /// or `None` if no path from it reaches a capture point. Negative if it is already too
    /// late by the time the clock starts.
    pub fn required(&self, point: PointId) -> Option<i64> {
        let required = *self.required.get(point.index())?;
        (required != NO_REQUIRED).then_some(required)
    }

    /// Returns a point's slack, or `None` if the point is not constrained.
    ///
    /// With constraints per clock, this is the worst over the timed pairs of clocks, which
    /// may be more than [`required`](TimingAnalysis::required) minus
    /// [`arrival`](TimingAnalysis::arrival), as those can come from different pairs.
    pub fn slack(&self, point: PointId) -> Option<i64> {
        if self.per_clock {
            let slack = *self.point_slack.get(point.index())?;
            return (slack != NO_SLACK).then_some(slack);
        }
        let arrival = *self.arrival.get(point.index())?;
        let required = *self.required.get(point.index())?;
        (arrival != NO_ARRIVAL && required != NO_REQUIRED).then(|| required - arrival)
    }

    /// Returns the slack of the worst path through an arc, or `None` if the arc is not
    /// constrained or was disabled to break a loop.
    pub fn arc_slack(&self, arc: ArcId) -> Option<i64> {
        let slack = *self.arc_slack.get(arc.index())?;
        (slack != NO_SLACK).then_some(slack)
    }

    /// Returns the lowest slack in the graph, or `None` if no path is constrained.
    pub fn worst_slack(&self) -> Option<i64> {
        self.worst_slack
    }

    /// Returns the sum of the negative slacks of all capture points: zero if timing is met,
    /// and otherwise negative.
    pub fn total_negative_slack(&self) -> i128 {
        self.total_negative_slack
    }

    /// Returns the path with the worst slack, or `None` if no path is constrained.
    ///
    /// Ties are broken as [documented for the module](self#determinism).
    pub fn critical_path(&self) -> Option<CriticalPath<'_>> {
        let slack = self.worst_slack?;
        Some(CriticalPath {
            points: &self.path_points,
            arcs: &self.path_arcs,
            slack,
            arrival: self.path_arrival,
        })
    }

    /// Writes the arcs of the latest path to any point into `arcs`, from its start, breaking
    /// ties as [the critical path does](self#determinism). This reports on timing below the
    /// critical path, for a slow path that is not the slowest.
    ///
    /// The graph must be the one analysed, with the delays analysed, since this retraces the
    /// path from them. The result is `false` with `arcs` possibly empty if the point has no
    /// arrival time or the delays or graph have since changed, and `true` otherwise.
    ///
    /// With constraints per clock, the path is the latest from any clock that launches in a
    /// timed pair.
    pub fn trace_latest_path(
        &self,
        graph: &TimingGraph,
        point: PointId,
        arcs: &mut Vec<ArcId>,
    ) -> bool {
        arcs.clear();
        if self.arrival.len() != graph.num_points as usize || self.arrival(point).is_none() {
            return false;
        }
        let traced = if self.per_clock {
            trace_back(&self.arrival, graph, point, arcs, |point| {
                graph
                    .launch(point)
                    .filter(|launch| self.launch_clocks.binary_search(&launch.clock).is_ok())
                    .map(|launch| launch.offset)
            })
        } else {
            trace_back(&self.arrival, graph, point, arcs, |point| {
                graph.launch_offset(point)
            })
        };
        arcs.reverse();
        traced
    }

    /// Returns the criticality of a net's sink, or of any other arc: the one a router needs.
    /// An arc that is unconstrained or disabled counts as a raw criticality of zero.
    ///
    /// With constraints per clock, this is the highest over the timed pairs of clocks, each
    /// pair's against its own worst slack and largest required time, as VPR does.
    #[inline]
    pub fn arc_criticality(&self, arc: ArcId, options: &CriticalityOptions) -> f64 {
        if self.per_clock {
            let raw = self.arc_raw.get(arc.index()).copied().unwrap_or(0.0);
            return options.sharpen(raw);
        }
        options.sharpen(self.raw_criticality(self.arc_slack(arc)))
    }

    /// Returns the criticality of a point, which is the highest of any path through it. An
    /// unconstrained point counts as a raw criticality of zero.
    #[inline]
    pub fn point_criticality(&self, point: PointId, options: &CriticalityOptions) -> f64 {
        if self.per_clock {
            let raw = self.point_raw.get(point.index()).copied().unwrap_or(0.0);
            return options.sharpen(raw);
        }
        options.sharpen(self.raw_criticality(self.slack(point)))
    }

    /// Replaces the contents of `out` with the criticality of every arc, indexed by [`ArcId`].
    ///
    /// It does not allocate once `out` has room for every arc.
    pub fn fill_arc_criticalities(&self, options: &CriticalityOptions, out: &mut Vec<f64>) {
        out.clear();
        if self.per_clock {
            out.extend(self.arc_raw.iter().map(|&raw| options.sharpen(raw)));
            return;
        }
        out.extend(self.arc_slack.iter().map(|&slack| {
            let slack = (slack != NO_SLACK).then_some(slack);
            options.sharpen(self.raw_criticality(slack))
        }));
    }

    /// Computes `1 - (slack + shift) / (max_required + shift)`, in `[0, 1]`.
    ///
    /// Rearranged as `(max_required - slack) / (max_required + shift)`, the numerator is an
    /// exact integer and both of these hold without a clamp. For a constrained point or arc:
    ///
    /// - `slack <= max_required`, since arrivals and delays are not negative, and the slack
    ///   is at most the required time at the end of the arc or at the point. So the numerator
    ///   is not negative.
    /// - `slack >= worst_slack`, since it is the slack of some path, and `-shift` is at most
    ///   `worst_slack`, which makes the numerator at most the denominator. That also shows
    ///   the denominator is not negative.
    ///
    /// When the denominator is zero, the numerator is too, so every constrained slack equals
    /// `max_required` and is the worst there is. VPR also calls that fully critical.
    ///
    /// With constraints per clock, the same holds within each pair of clocks, with the
    /// pair's own worst slack and largest required time, and so for `raw_ratio` there.
    #[inline]
    fn raw_criticality(&self, slack: Option<i64>) -> f64 {
        let Some(slack) = slack else { return 0.0 };
        raw_ratio(self.max_required, self.criticality_span, slack)
    }
}

/// Computes `(max_required - slack) / span`, as [`TimingAnalysis::raw_criticality`] explains,
/// with 1 for a span of 0.
#[inline]
fn raw_ratio(max_required: i64, span: u64, slack: i64) -> f64 {
    if span == 0 {
        return 1.0;
    }
    // Shifting both the same way keeps the ratio, and leaves the two values exactly
    // convertible, since a `u64` of at most 53 significant bits is an exact `f64`.
    let excess = (u64::BITS - span.leading_zeros()).saturating_sub(f64::MANTISSA_DIGITS);
    let numerator = max_required.abs_diff(slack) >> excess;
    numerator as f64 / (span >> excess) as f64
}

/// Walks from `end` back along the arcs that decide its arrival time, pushing them onto `arcs`
/// starting with the last arc of the path. `launch` gives the offset of a point that starts
/// paths in `arrival`. Returns `false` if the walk could not get back to a launch point, which
/// only happens with delays that do not match the arrival times.
///
/// Levels fall with every step, so this ends, and the path holds at most one arc per level.
fn trace_back(
    arrival: &[i64],
    graph: &TimingGraph,
    end: PointId,
    arcs: &mut Vec<ArcId>,
    launch: impl Fn(PointId) -> Option<u32>,
) -> bool {
    let mut current = end;
    'walk: loop {
        let time = arrival[current.index()];
        if launch(current).is_some_and(|offset| i64::from(offset) == time) {
            return true;
        }
        for &arc in graph.fanin(current) {
            let step = &graph.steps[graph.arc_step[arc.index()] as usize];
            let from = arrival[step.from.index()];
            if from != NO_ARRIVAL && from + i64::from(step.delay) == time {
                arcs.push(arc);
                current = step.from;
                continue 'walk;
            }
        }
        return false;
    }
}

/// A worst path, from [`TimingAnalysis::critical_path`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CriticalPath<'a> {
    points: &'a [PointId],
    arcs: &'a [ArcId],
    slack: i64,
    arrival: u64,
}

impl<'a> CriticalPath<'a> {
    /// Returns the points of the path from start to end. There is always at least one.
    pub fn points(&self) -> &'a [PointId] {
        self.points
    }

    /// Returns the arcs of the path from start to end, one fewer than there are points.
    pub fn arcs(&self) -> &'a [ArcId] {
        self.arcs
    }

    /// Returns the launch point the path starts from.
    pub fn start(&self) -> PointId {
        self.points[0]
    }

    /// Returns the capture point the path ends at.
    pub fn end(&self) -> PointId {
        self.points[self.points.len() - 1]
    }

    /// Returns the path's slack, which is the worst in the graph.
    pub fn slack(&self) -> i64 {
        self.slack
    }

    /// Returns when the path reaches its end: its launch offset and all its delays.
    pub fn arrival(&self) -> u64 {
        self.arrival
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn run(graph: &TimingGraph, period: u64) -> TimingAnalysis {
        let mut analysis = TimingAnalysis::new(graph);
        analysis.run(graph, &Constraints::single_clock(period).unwrap());
        analysis
    }

    /// `input(10) -> a -> b -> output(3)`, with delays 5 and 20.
    #[test]
    fn a_chain_matches_hand_computation() {
        let mut builder = TimingGraphBuilder::new();
        let input = builder.add_input(10).unwrap();
        let a = builder.add_point().unwrap();
        let output = builder.add_output(3).unwrap();
        let first = builder.add_net_arc(input, a, 5).unwrap();
        let second = builder.add_cell_arc(a, output, 20).unwrap();
        let graph = builder.build();
        let analysis = run(&graph, 100);

        assert_eq!(analysis.arrival(input), Some(10));
        assert_eq!(analysis.arrival(a), Some(15));
        assert_eq!(analysis.arrival(output), Some(35));
        assert_eq!(analysis.required(output), Some(97));
        assert_eq!(analysis.required(a), Some(77));
        assert_eq!(analysis.required(input), Some(72));
        for point in [input, a, output] {
            assert_eq!(analysis.slack(point), Some(62));
        }
        assert_eq!(analysis.arc_slack(first), Some(62));
        assert_eq!(analysis.arc_slack(second), Some(62));
        assert_eq!(analysis.worst_slack(), Some(62));
        assert_eq!(analysis.total_negative_slack(), 0);

        let path = analysis.critical_path().unwrap();
        assert_eq!(path.points(), &[input, a, output]);
        assert_eq!(path.arcs(), &[first, second]);
        assert_eq!(path.start(), input);
        assert_eq!(path.end(), output);
        assert_eq!(path.slack(), 62);
        assert_eq!(path.arrival(), 35);
    }

    /// `i -> a -> o` at 1 + 2 against `i -> b -> o` at 5 + 7, meeting again at `o`.
    #[test]
    fn reconvergent_fanout_takes_the_slower_branch() {
        let mut builder = TimingGraphBuilder::new();
        let i = builder.add_input(0).unwrap();
        let a = builder.add_point().unwrap();
        let b = builder.add_point().unwrap();
        let o = builder.add_output(0).unwrap();
        let ia = builder.add_net_arc(i, a, 1).unwrap();
        let ao = builder.add_cell_arc(a, o, 2).unwrap();
        let ib = builder.add_net_arc(i, b, 5).unwrap();
        let bo = builder.add_cell_arc(b, o, 7).unwrap();
        let graph = builder.build();
        let analysis = run(&graph, 20);

        assert_eq!(analysis.arrival(o), Some(12));
        assert_eq!(analysis.required(i), Some(8));
        assert_eq!(analysis.required(a), Some(18));
        assert_eq!(analysis.required(b), Some(13));
        assert_eq!(analysis.slack(a), Some(17));
        assert_eq!(analysis.slack(b), Some(8));
        assert_eq!(analysis.slack(i), Some(8));
        assert_eq!(analysis.arc_slack(ia), Some(17));
        assert_eq!(analysis.arc_slack(ao), Some(17));
        assert_eq!(analysis.arc_slack(ib), Some(8));
        assert_eq!(analysis.arc_slack(bo), Some(8));
        assert_eq!(analysis.critical_path().unwrap().arcs(), &[ib, bo]);
    }

    #[test]
    fn multiple_sources_and_sinks_use_their_own_offsets_and_margins() {
        let mut builder = TimingGraphBuilder::new();
        let early = builder.add_input(0).unwrap();
        let late = builder.add_input(30).unwrap();
        let join = builder.add_point().unwrap();
        let tight = builder.add_output(40).unwrap();
        let loose = builder.add_output(0).unwrap();
        builder.add_net_arc(early, join, 10).unwrap();
        builder.add_net_arc(late, join, 5).unwrap();
        builder.add_net_arc(join, tight, 10).unwrap();
        builder.add_net_arc(join, loose, 10).unwrap();
        let graph = builder.build();
        let analysis = run(&graph, 50);

        assert_eq!(analysis.arrival(join), Some(35));
        assert_eq!(analysis.required(join), Some(0));
        assert_eq!(analysis.slack(early), Some(-10));
        assert_eq!(analysis.required(late), Some(-5));
        assert_eq!(analysis.slack(late), Some(-35));
        assert_eq!(analysis.slack(tight), Some(-35));
        assert_eq!(analysis.slack(loose), Some(5));
        assert_eq!(analysis.worst_slack(), Some(-35));
        assert_eq!(analysis.total_negative_slack(), -35);
        let path = analysis.critical_path().unwrap();
        assert_eq!(path.points(), &[late, join, tight]);
    }

    #[test]
    fn total_negative_slack_adds_up_failing_capture_points_only() {
        let mut builder = TimingGraphBuilder::new();
        let input = builder.add_input(0).unwrap();
        for (delay, margin) in [(30, 0), (25, 0), (5, 0), (20, 15)] {
            let output = builder.add_output(margin).unwrap();
            builder.add_net_arc(input, output, delay).unwrap();
        }
        let graph = builder.build();
        // Slacks are -10, -5, 15 and -15.
        assert_eq!(run(&graph, 20).total_negative_slack(), -30);
        assert_eq!(run(&graph, 20).worst_slack(), Some(-15));
    }

    #[test]
    fn registers_bound_paths_and_feedback_through_them_is_not_a_loop() {
        let mut builder = TimingGraphBuilder::new();
        let reg = builder.add_register(50, 30).unwrap();
        let gate = builder.add_point().unwrap();
        // A counter: the register feeds itself through a gate.
        let out = builder.add_net_arc(reg.q, gate, 100).unwrap();
        let back = builder.add_net_arc(gate, reg.d, 120).unwrap();
        let graph = builder.build();
        assert_eq!(graph.loops(), &[]);

        let analysis = run(&graph, 400);
        assert_eq!(analysis.arrival(reg.q), Some(50));
        assert_eq!(analysis.arrival(gate), Some(150));
        assert_eq!(analysis.arrival(reg.d), Some(270));
        assert_eq!(analysis.required(reg.d), Some(370));
        assert_eq!(analysis.required(reg.q), Some(150));
        assert_eq!(analysis.worst_slack(), Some(100));
        let path = analysis.critical_path().unwrap();
        assert_eq!(path.start(), reg.q);
        assert_eq!(path.end(), reg.d);
        assert_eq!(path.arcs(), &[out, back]);

        // At the minimum period, the slack is exactly zero.
        assert_eq!(run(&graph, 300).worst_slack(), Some(0));
    }

    #[test]
    fn register_to_register_paths_do_not_pass_through() {
        let mut builder = TimingGraphBuilder::new();
        let first = builder.add_register(10, 5).unwrap();
        let second = builder.add_register(20, 7).unwrap();
        let third = builder.add_register(0, 0).unwrap();
        builder.add_net_arc(first.q, second.d, 100).unwrap();
        builder.add_net_arc(second.q, third.d, 40).unwrap();
        let graph = builder.build();
        let analysis = run(&graph, 200);

        assert_eq!(analysis.arrival(second.d), Some(110));
        assert_eq!(analysis.arrival(second.q), Some(20));
        assert_eq!(analysis.arrival(third.d), Some(60));
        assert_eq!(analysis.slack(second.d), Some(83));
        assert_eq!(analysis.slack(third.d), Some(140));
    }

    #[test]
    fn unconstrained_points_and_arcs_have_no_slack() {
        let mut builder = TimingGraphBuilder::new();
        let input = builder.add_input(0).unwrap();
        let output = builder.add_output(0).unwrap();
        let dangling = builder.add_point().unwrap();
        let undriven = builder.add_point().unwrap();
        let isolated = builder.add_point().unwrap();
        let constrained = builder.add_net_arc(input, output, 5).unwrap();
        let to_dangling = builder.add_net_arc(input, dangling, 5).unwrap();
        let from_undriven = builder.add_net_arc(undriven, output, 5).unwrap();
        let graph = builder.build();
        let analysis = run(&graph, 100);

        assert_eq!(analysis.arrival(dangling), Some(5));
        assert_eq!(analysis.required(dangling), None);
        assert_eq!(analysis.slack(dangling), None);
        assert_eq!(analysis.arrival(undriven), None);
        assert_eq!(analysis.required(undriven), Some(95));
        assert_eq!(analysis.slack(undriven), None);
        assert_eq!(analysis.arrival(isolated), None);
        assert_eq!(analysis.required(isolated), None);
        assert_eq!(analysis.arc_slack(constrained), Some(95));
        assert_eq!(analysis.arc_slack(to_dangling), None);
        assert_eq!(analysis.arc_slack(from_undriven), None);

        let options = CriticalityOptions::default();
        assert_eq!(analysis.arc_criticality(to_dangling, &options), 0.0);
        assert_eq!(analysis.point_criticality(isolated, &options), 0.0);
    }

    #[test]
    fn nothing_constrained_reports_nothing() {
        let mut builder = TimingGraphBuilder::new();
        let a = builder.add_input(0).unwrap();
        let b = builder.add_point().unwrap();
        let arc = builder.add_net_arc(a, b, 5).unwrap();
        let graph = builder.build();
        let analysis = run(&graph, 100);
        assert_eq!(analysis.worst_slack(), None);
        assert_eq!(analysis.total_negative_slack(), 0);
        assert_eq!(analysis.critical_path(), None);
        assert_eq!(analysis.arc_slack(arc), None);
    }

    #[test]
    fn an_empty_graph_analyses_to_nothing() {
        let graph = TimingGraphBuilder::default().build();
        assert_eq!(graph.num_points(), 0);
        assert_eq!(graph.num_arcs(), 0);
        assert_eq!(graph.num_levels(), 0);
        assert_eq!(graph.loops(), &[]);
        let analysis = run(&graph, 100);
        assert_eq!(analysis.worst_slack(), None);
        assert_eq!(analysis.total_negative_slack(), 0);
        assert_eq!(analysis.critical_path(), None);
        assert_eq!(analysis.arrival(PointId(0)), None);
        assert_eq!(analysis.arc_slack(ArcId(0)), None);
        let mut out = vec![1.0];
        analysis.fill_arc_criticalities(&CriticalityOptions::default(), &mut out);
        assert_eq!(out, &[] as &[f64]);
    }

    #[test]
    fn a_new_analysis_answers_as_if_nothing_were_constrained() {
        let mut builder = TimingGraphBuilder::new();
        let input = builder.add_input(5).unwrap();
        let output = builder.add_output(0).unwrap();
        let arc = builder.add_net_arc(input, output, 5).unwrap();
        let analysis = TimingAnalysis::new(&builder.build());
        assert_eq!(analysis.arrival(input), None);
        assert_eq!(analysis.required(input), None);
        assert_eq!(analysis.slack(input), None);
        assert_eq!(analysis.arc_slack(arc), None);
        assert_eq!(analysis.worst_slack(), None);
        assert_eq!(analysis.critical_path(), None);
    }

    #[test]
    fn a_point_can_be_both_ends_of_a_path_with_no_arcs() {
        let mut builder = TimingGraphBuilder::new();
        let both = builder.add_input(10).unwrap();
        builder.set_capture(both, 5).unwrap();
        let graph = builder.build();
        let analysis = run(&graph, 8);
        assert_eq!(analysis.slack(both), Some(-7));
        assert_eq!(analysis.worst_slack(), Some(-7));
        let path = analysis.critical_path().unwrap();
        assert_eq!(path.points(), &[both]);
        assert_eq!(path.arcs(), &[]);
        assert_eq!(path.start(), both);
        assert_eq!(path.end(), both);
        assert_eq!(path.arrival(), 10);
    }

    #[test]
    fn launch_points_with_fanin_and_capture_points_with_fanout_keep_their_arcs() {
        let mut builder = TimingGraphBuilder::new();
        let input = builder.add_input(0).unwrap();
        let middle = builder.add_point().unwrap();
        let output = builder.add_output(0).unwrap();
        builder.add_net_arc(input, middle, 10).unwrap();
        builder.add_net_arc(middle, output, 10).unwrap();
        builder.set_launch(middle, 50).unwrap();
        builder.set_capture(middle, 25).unwrap();
        let graph = builder.build();
        let analysis = run(&graph, 100);

        // The offset beats the 10 arriving through the arc, so the critical path starts here.
        assert_eq!(analysis.arrival(middle), Some(50));
        assert_eq!(analysis.arrival(output), Some(60));
        // Its own 75 beats the 90 left by the next arc.
        assert_eq!(analysis.required(middle), Some(75));
        assert_eq!(analysis.required(input), Some(65));
        assert_eq!(analysis.worst_slack(), Some(25));
        let path = analysis.critical_path().unwrap();
        assert_eq!(path.points(), &[middle]);
    }

    #[test]
    fn critical_path_ties_go_to_the_lowest_capture_point_then_launch_then_arc() {
        let mut builder = TimingGraphBuilder::new();
        let i = builder.add_input(10).unwrap();
        let o_low = builder.add_output(0).unwrap();
        let o_high = builder.add_output(0).unwrap();
        let mid = builder.add_point().unwrap();
        builder.add_net_arc(i, o_high, 20).unwrap();
        let first_parallel = builder.add_net_arc(i, mid, 5).unwrap();
        let second_parallel = builder.add_net_arc(i, mid, 5).unwrap();
        let onward = builder.add_net_arc(mid, o_low, 15).unwrap();
        let graph = builder.build();
        let analysis = run(&graph, 100);
        let path = analysis.critical_path().unwrap();
        assert_eq!(path.end(), o_low);
        assert_eq!(path.arcs(), &[first_parallel, onward]);
        assert_ne!(first_parallel, second_parallel);

        // When an arc ties with a launch offset, the path stops at the launch point.
        let mut builder = TimingGraphBuilder::new();
        let i = builder.add_input(10).unwrap();
        let mid = builder.add_input(15).unwrap();
        let o = builder.add_output(0).unwrap();
        builder.add_net_arc(i, mid, 5).unwrap();
        let last = builder.add_net_arc(mid, o, 1).unwrap();
        let graph = builder.build();
        let analysis = run(&graph, 100);
        assert_eq!(analysis.critical_path().unwrap().arcs(), &[last]);
    }

    #[test]
    fn constraint_order_does_not_change_the_result() {
        let build = |reversed: bool| {
            let mut builder = TimingGraphBuilder::new();
            let points: Vec<_> = (0..4).map(|_| builder.add_point().unwrap()).collect();
            builder.add_net_arc(points[0], points[2], 5).unwrap();
            builder.add_net_arc(points[1], points[3], 5).unwrap();
            let launches = [(points[0], 1), (points[1], 1)];
            let captures = [(points[2], 0), (points[3], 0)];
            let order = if reversed { [1, 0] } else { [0, 1] };
            for index in order {
                builder
                    .set_launch(launches[index].0, launches[index].1)
                    .unwrap();
                builder
                    .set_capture(captures[index].0, captures[index].1)
                    .unwrap();
            }
            builder.build()
        };
        let forward = build(false);
        let backward = build(true);
        let (one, other) = (run(&forward, 100), run(&backward, 100));
        assert_eq!(
            one.critical_path().unwrap().points(),
            other.critical_path().unwrap().points(),
        );
    }

    #[test]
    fn trace_latest_path_follows_any_point() {
        let mut builder = TimingGraphBuilder::new();
        let i = builder.add_input(0).unwrap();
        let a = builder.add_point().unwrap();
        let unreached = builder.add_point().unwrap();
        let o_slow = builder.add_output(0).unwrap();
        let o_fast = builder.add_output(0).unwrap();
        let ia = builder.add_net_arc(i, a, 1).unwrap();
        builder.add_net_arc(a, o_slow, 100).unwrap();
        let af = builder.add_net_arc(a, o_fast, 2).unwrap();
        let mut graph = builder.build();
        let analysis = run(&graph, 100);

        let mut arcs = vec![ArcId(99)];
        assert!(analysis.trace_latest_path(&graph, o_fast, &mut arcs));
        assert_eq!(arcs, &[ia, af]);
        assert!(analysis.trace_latest_path(&graph, i, &mut arcs));
        assert_eq!(arcs, &[]);
        assert!(!analysis.trace_latest_path(&graph, unreached, &mut arcs));
        assert!(!analysis.trace_latest_path(&graph, PointId(99), &mut arcs));

        // After a change in delays there is nothing to retrace, which it says.
        graph.set_delay(af, 3).unwrap();
        assert!(!analysis.trace_latest_path(&graph, o_fast, &mut arcs));
        // A graph of another size is refused rather than misread.
        let empty = TimingGraphBuilder::new().build();
        assert!(!analysis.trace_latest_path(&empty, PointId(0), &mut arcs));
    }

    #[test]
    fn a_self_loop_is_reported_and_ignored() {
        let mut builder = TimingGraphBuilder::new();
        let input = builder.add_input(0).unwrap();
        let a = builder.add_point().unwrap();
        let output = builder.add_output(0).unwrap();
        builder.add_net_arc(input, a, 1).unwrap();
        let own = builder.add_cell_arc(a, a, 1000).unwrap();
        builder.add_net_arc(a, output, 2).unwrap();
        let graph = builder.build();
        assert_eq!(
            graph.loops(),
            &[CombinationalLoop {
                points: vec![a],
                broken_arcs: vec![own],
            }],
        );
        assert!(graph.is_broken(own));
        let analysis = run(&graph, 10);
        assert_eq!(analysis.arrival(output), Some(3));
        assert_eq!(analysis.arc_slack(own), None);
        assert_eq!(graph.fanin(a).len(), 1);
    }

    /// The numbering is chosen so that a search in plain point order would meet `b` first and
    /// disable the forward arc `a -> b`, disconnecting the input from the output.
    #[test]
    fn breaking_a_loop_keeps_the_path_from_launch_to_capture() {
        let mut builder = TimingGraphBuilder::new();
        let b = builder.add_point().unwrap();
        let a = builder.add_point().unwrap();
        let input = builder.add_input(1).unwrap();
        let output = builder.add_output(0).unwrap();
        builder.add_net_arc(input, a, 10).unwrap();
        let forward = builder.add_cell_arc(a, b, 100).unwrap();
        let feedback = builder.add_cell_arc(b, a, 1000).unwrap();
        builder.add_net_arc(b, output, 10).unwrap();
        let graph = builder.build();

        assert_eq!(
            graph.loops(),
            &[CombinationalLoop {
                points: vec![b, a],
                broken_arcs: vec![feedback],
            }],
        );
        assert!(!graph.is_broken(forward));
        let analysis = run(&graph, 200);
        assert_eq!(analysis.arrival(output), Some(121));
        assert_eq!(analysis.worst_slack(), Some(79));
        assert_eq!(analysis.arc_slack(feedback), None);
    }

    /// The search finishes with point 2 before it gets to `3 -> 2`. That arc stays inside the
    /// loop but closes no cycle of its own once `2 -> 0` is gone, so it has to stay enabled.
    #[test]
    fn only_back_edges_are_broken_and_not_other_arcs_within_a_loop() {
        let mut builder = TimingGraphBuilder::new();
        let start = builder.add_input(0).unwrap();
        let b = builder.add_point().unwrap();
        let c = builder.add_output(0).unwrap();
        let d = builder.add_point().unwrap();
        builder.add_cell_arc(start, b, 1).unwrap();
        builder.add_cell_arc(b, c, 1).unwrap();
        let back = builder.add_cell_arc(c, start, 1).unwrap();
        builder.add_cell_arc(b, d, 10).unwrap();
        let cross = builder.add_cell_arc(d, c, 10).unwrap();
        let graph = builder.build();

        assert_eq!(graph.loops().len(), 1);
        assert_eq!(graph.loops()[0].broken_arcs, &[back]);
        assert!(!graph.is_broken(cross));
        let analysis = run(&graph, 100);
        assert_eq!(analysis.arrival(c), Some(21));
        assert_eq!(analysis.arc_slack(cross), Some(79));
    }

    #[test]
    fn overlapping_cycles_are_one_loop_and_separate_ones_are_two() {
        let mut builder = TimingGraphBuilder::new();
        let points: Vec<_> = (0..7).map(|_| builder.add_point().unwrap()).collect();
        // 0 and 1 and 2 share two cycles, while 4 and 5 have their own. 3 is between the two
        // and on no cycle, as is 6 beyond them.
        builder.add_cell_arc(points[0], points[1], 1).unwrap();
        builder.add_cell_arc(points[1], points[0], 1).unwrap();
        builder.add_cell_arc(points[1], points[2], 1).unwrap();
        builder.add_cell_arc(points[2], points[0], 1).unwrap();
        builder.add_cell_arc(points[2], points[3], 1).unwrap();
        builder.add_cell_arc(points[3], points[4], 1).unwrap();
        builder.add_cell_arc(points[4], points[5], 1).unwrap();
        builder.add_cell_arc(points[5], points[4], 1).unwrap();
        builder.add_cell_arc(points[5], points[6], 1).unwrap();
        let graph = builder.build();

        let loops = graph.loops();
        assert_eq!(loops.len(), 2);
        assert_eq!(loops[0].points, &points[0..3]);
        assert_eq!(loops[0].broken_arcs, &[ArcId(1), ArcId(3)]);
        assert_eq!(loops[1].points, &points[4..6]);
        assert_eq!(loops[1].broken_arcs, &[ArcId(7)]);
    }

    #[test]
    fn a_very_long_chain_builds_and_traces_without_recursion() {
        let length = 1_000_000;
        let mut builder = TimingGraphBuilder::with_capacity(length + 1, length);
        let mut last = builder.add_input(0).unwrap();
        for _ in 0..length {
            let next = builder.add_point().unwrap();
            builder.add_net_arc(last, next, 1).unwrap();
            last = next;
        }
        builder.set_capture(last, 0).unwrap();
        let graph = builder.build();
        let analysis = run(&graph, 0);
        assert_eq!(analysis.arrival(last), Some(length as u64));
        assert_eq!(analysis.critical_path().unwrap().arcs().len(), length);
    }

    #[test]
    fn updating_delays_matches_a_fresh_build_and_does_not_reallocate() {
        let build = |net_delay: u32| {
            let mut builder = TimingGraphBuilder::new();
            let reg = builder.add_register(5, 7).unwrap();
            let input = builder.add_input(3).unwrap();
            let a = builder.add_point().unwrap();
            let b = builder.add_point().unwrap();
            builder.add_net_arc(reg.q, a, net_delay).unwrap();
            builder.add_net_arc(input, a, net_delay + 1).unwrap();
            builder.add_cell_arc(a, b, 11).unwrap();
            builder.add_net_arc(b, reg.d, net_delay + 2).unwrap();
            builder.build()
        };
        let mut graph = build(1);
        let constraints = Constraints::single_clock(30).unwrap();
        let mut analysis = TimingAnalysis::new(&graph);
        analysis.run(&graph, &constraints);

        let pointers = (
            analysis.arrival.as_ptr(),
            analysis.required.as_ptr(),
            analysis.arc_slack.as_ptr(),
            analysis.path_arcs.as_ptr(),
            analysis.path_points.as_ptr(),
        );
        for net_delay in [50, 0, 7, 1000] {
            let cell = ArcId(2);
            let before = graph.arc(cell).unwrap().delay;
            graph.apply_net_delays(&|arc: ArcId| net_delay + arc.0.min(3) - u32::from(arc.0 == 3));
            assert_eq!(graph.arc(cell).unwrap().delay, before);
            analysis.run(&graph, &constraints);

            let fresh = run(&build(net_delay), 30);
            assert_eq!(analysis.arrival, fresh.arrival);
            assert_eq!(analysis.required, fresh.required);
            assert_eq!(analysis.arc_slack, fresh.arc_slack);
            assert_eq!(analysis.worst_slack(), fresh.worst_slack());
            assert_eq!(
                analysis.total_negative_slack(),
                fresh.total_negative_slack()
            );
            assert_eq!(analysis.critical_path(), fresh.critical_path());
        }
        assert_eq!(
            pointers,
            (
                analysis.arrival.as_ptr(),
                analysis.required.as_ptr(),
                analysis.arc_slack.as_ptr(),
                analysis.path_arcs.as_ptr(),
                analysis.path_points.as_ptr(),
            ),
        );
    }

    #[test]
    fn set_delay_changes_any_arc_and_rejects_unknown_ones() {
        let mut builder = TimingGraphBuilder::new();
        let input = builder.add_input(0).unwrap();
        let output = builder.add_output(0).unwrap();
        let cell = builder.add_cell_arc(input, output, 5).unwrap();
        let mut graph = builder.build();

        graph.set_delay(cell, 9).unwrap();
        assert_eq!(
            graph.arc(cell),
            Some(ArcInfo {
                from: input,
                to: output,
                delay: 9,
                kind: ArcKind::Cell,
            }),
        );
        assert_eq!(graph.net_arcs(), &[]);
        assert_eq!(run(&graph, 10).worst_slack(), Some(1));
        assert_eq!(
            graph.set_delay(ArcId(1), 9),
            Err(TimingError::UnknownArc { arc: ArcId(1) }),
        );
        assert_eq!(graph.arc(ArcId(1)), None);
        assert!(!graph.is_broken(ArcId(1)));
        assert_eq!(graph.fanin(PointId(9)), &[]);
    }

    #[test]
    fn results_are_a_snapshot_of_the_last_run() {
        let mut builder = TimingGraphBuilder::new();
        let input = builder.add_input(0).unwrap();
        let output = builder.add_output(0).unwrap();
        let arc = builder.add_net_arc(input, output, 5).unwrap();
        let mut graph = builder.build();
        let analysis = run(&graph, 10);
        graph.set_delay(arc, 500).unwrap();
        assert_eq!(analysis.arc_slack(arc), Some(5));
        assert_eq!(analysis.critical_path().unwrap().arrival(), 5);
    }

    #[test]
    fn one_analysis_can_move_to_another_graph() {
        let mut builder = TimingGraphBuilder::new();
        let input = builder.add_input(0).unwrap();
        let output = builder.add_output(0).unwrap();
        builder.add_net_arc(input, output, 5).unwrap();
        let graph = builder.build();
        let empty = TimingGraphBuilder::new().build();

        let constraints = Constraints::single_clock(10).unwrap();
        let mut analysis = TimingAnalysis::new(&empty);
        analysis.run(&graph, &constraints);
        assert_eq!(analysis.worst_slack(), Some(5));
        analysis.run(&empty, &constraints);
        assert_eq!(analysis.worst_slack(), None);
        assert_eq!(analysis.critical_path(), None);
    }

    #[test]
    fn the_builder_rejects_what_it_should() {
        let mut builder = TimingGraphBuilder::new();
        let point = builder.add_point().unwrap();
        let unknown = PointId(1);
        let expected = TimingError::UnknownPoint { point: unknown };
        assert_eq!(builder.add_net_arc(unknown, point, 1), Err(expected));
        assert_eq!(builder.add_cell_arc(point, unknown, 1), Err(expected));
        assert_eq!(builder.set_launch(unknown, 1), Err(expected));
        assert_eq!(builder.set_capture(unknown, 1), Err(expected));

        builder.set_launch(point, 1).unwrap();
        assert_eq!(
            builder.set_launch(point, 2),
            Err(TimingError::DuplicateLaunch { point }),
        );
        builder.set_capture(point, 1).unwrap();
        assert_eq!(
            builder.set_capture(point, 2),
            Err(TimingError::DuplicateCapture { point }),
        );

        let mut limited = TimingGraphBuilder::new();
        limited.max_points = 1;
        limited.max_arcs = 1;
        let only = limited.add_point().unwrap();
        assert_eq!(limited.add_point(), Err(TimingError::TooManyPoints));
        assert_eq!(limited.add_input(1), Err(TimingError::TooManyPoints));
        assert_eq!(limited.add_output(1), Err(TimingError::TooManyPoints));
        assert_eq!(limited.add_register(1, 1), Err(TimingError::TooManyPoints));
        limited.max_points = 2;
        assert_eq!(limited.add_register(1, 1), Err(TimingError::TooManyPoints));
        assert_eq!(limited.roles.len(), 1, "a failed register adds no points");
        limited.add_net_arc(only, only, 1).unwrap();
        assert_eq!(
            limited.add_net_arc(only, only, 1),
            Err(TimingError::TooManyArcs),
        );
    }

    #[test]
    fn the_constraint_and_option_limits_are_enforced() {
        assert!(Constraints::single_clock(MAX_CLOCK_PERIOD).is_ok());
        assert_eq!(
            Constraints::single_clock(MAX_CLOCK_PERIOD + 1),
            Err(TimingError::ClockPeriodTooLarge {
                period: MAX_CLOCK_PERIOD + 1,
            }),
        );
        assert_eq!(Constraints::single_clock(5).unwrap().clock_period(), 5,);
        for max in [f64::NAN, -0.0, -0.5, 1.5, f64::INFINITY] {
            assert!(CriticalityOptions::new(1, max).is_err(), "{max}");
        }
        assert!(CriticalityOptions::new(0, 0.0).is_ok());
        assert!(CriticalityOptions::new(u32::MAX, 1.0).is_ok());
    }

    #[test]
    fn errors_describe_themselves() {
        let cases = [
            TimingError::UnknownPoint { point: PointId(1) },
            TimingError::UnknownArc { arc: ArcId(1) },
            TimingError::DuplicateLaunch { point: PointId(1) },
            TimingError::DuplicateCapture { point: PointId(1) },
            TimingError::TooManyPoints,
            TimingError::TooManyArcs,
            TimingError::ClockPeriodTooLarge { period: 1 },
            TimingError::InvalidMaxCriticality { max: 2.0 },
        ];
        for case in cases {
            assert!(!case.to_string().is_empty());
        }
        assert_eq!(format!("{:?}", PointId(3)), "PointId(3)");
        assert_eq!(format!("{:?}", ArcId(3)), "ArcId(3)");
        assert_eq!(usize::from(ArcId(4)), 4);
        assert_eq!(usize::from(PointId(4)), 4);
    }

    const RAW: CriticalityOptions = CriticalityOptions {
        exponent: 1,
        max: 1.0,
    };

    /// Slack 75 and 25 out of a `max_required` of 100, with no failing paths.
    fn two_branches() -> (TimingGraph, ArcId, ArcId) {
        let mut builder = TimingGraphBuilder::new();
        let input = builder.add_input(0).unwrap();
        let output_near = builder.add_output(0).unwrap();
        let output_far = builder.add_output(0).unwrap();
        let near = builder.add_net_arc(input, output_near, 25).unwrap();
        let far = builder.add_net_arc(input, output_far, 75).unwrap();
        (builder.build(), near, far)
    }

    #[test]
    fn criticality_with_positive_slack_is_one_minus_slack_over_max_required() {
        let (graph, near, far) = two_branches();
        let analysis = run(&graph, 100);
        assert_eq!(analysis.arc_criticality(near, &RAW), 0.25);
        assert_eq!(analysis.arc_criticality(far, &RAW), 0.75);
        assert_eq!(analysis.point_criticality(PointId(0), &RAW), 0.75);

        let mut all = Vec::new();
        analysis.fill_arc_criticalities(&RAW, &mut all);
        assert_eq!(all, &[0.25, 0.75]);
    }

    #[test]
    fn criticality_shifts_failing_slack_so_the_worst_is_one() {
        let (graph, near, far) = two_branches();
        // Slacks are 25 and -25, so the shift is 25 and the denominator 50 + 25, which gives
        // 1 - (25 + 25) / 75 = 1/3 and 1 - (-25 + 25) / 75 = 1.
        let analysis = run(&graph, 50);
        assert_eq!(analysis.arc_criticality(far, &RAW), 1.0);
        assert_eq!(analysis.arc_criticality(near, &RAW), 25.0 / 75.0);
    }

    #[test]
    fn a_period_of_zero_makes_criticality_arrival_over_the_longest_path() {
        let (graph, near, far) = two_branches();
        let analysis = run(&graph, 0);
        assert_eq!(analysis.worst_slack(), Some(-75));
        assert_eq!(analysis.arc_criticality(far, &RAW), 1.0);
        assert_eq!(analysis.arc_criticality(near, &RAW), 25.0 / 75.0);
    }

    #[test]
    fn criticality_of_a_graph_with_all_zero_times_is_one() {
        let mut builder = TimingGraphBuilder::new();
        let input = builder.add_input(0).unwrap();
        let output = builder.add_output(0).unwrap();
        let arc = builder.add_net_arc(input, output, 0).unwrap();
        let graph = builder.build();
        let analysis = run(&graph, 0);
        assert_eq!(analysis.worst_slack(), Some(0));
        assert_eq!(analysis.arc_criticality(arc, &RAW), 1.0);
    }

    #[test]
    fn criticality_of_a_slack_equal_to_max_required_is_zero() {
        let mut builder = TimingGraphBuilder::new();
        let input = builder.add_input(0).unwrap();
        let output = builder.add_output(0).unwrap();
        let arc = builder.add_net_arc(input, output, 0).unwrap();
        let graph = builder.build();
        assert_eq!(run(&graph, 100).arc_criticality(arc, &RAW), 0.0);
    }

    #[test]
    fn options_apply_in_vpr_order() {
        let (graph, near, far) = two_branches();
        let analysis = run(&graph, 100);

        // Shift down by 0.25, then square: (0.75 - 0.25)^2 = 0.25 and (0.25 - 0.25)^2 = 0.
        let squared = CriticalityOptions::new(2, 0.75).unwrap();
        assert_eq!(analysis.arc_criticality(far, &squared), 0.25);
        assert_eq!(analysis.arc_criticality(near, &squared), 0.0);

        // Capping: raw 1, shift 0.25, which gives 0.75 at most anyway.
        let capped = CriticalityOptions::new(1, 0.75).unwrap();
        assert_eq!(run(&graph, 75).arc_criticality(far, &capped), 0.75);

        // Maximum of zero: nothing has any timing weight.
        let none = CriticalityOptions::new(1, 0.0).unwrap();
        assert_eq!(run(&graph, 75).arc_criticality(far, &none), 0.0);

        // An exponent of zero treats everything alike, as VPR documents.
        let alike = CriticalityOptions::new(0, 0.75).unwrap();
        assert_eq!(analysis.arc_criticality(near, &alike), 0.75);
        assert_eq!(analysis.arc_criticality(far, &alike), 0.75);
    }

    #[test]
    fn an_exponent_of_zero_also_reaches_unconstrained_arcs() {
        let mut builder = TimingGraphBuilder::new();
        let point = builder.add_point().unwrap();
        let other = builder.add_point().unwrap();
        let arc = builder.add_net_arc(point, other, 0).unwrap();
        let graph = builder.build();
        let analysis = run(&graph, 0);
        let alike = CriticalityOptions::new(0, 0.75).unwrap();
        assert_eq!(analysis.arc_criticality(arc, &alike), 0.75);
        assert_eq!(analysis.arc_criticality(arc, &RAW), 0.0);
    }

    #[test]
    fn whole_power_matches_hand_values() {
        assert_eq!(whole_power(0.5, 0), 1.0);
        assert_eq!(whole_power(0.0, 0), 1.0);
        assert_eq!(whole_power(0.0, 3), 0.0);
        assert_eq!(whole_power(0.5, 1), 0.5);
        assert_eq!(whole_power(0.5, 2), 0.25);
        assert_eq!(whole_power(0.5, 3), 0.125);
        assert_eq!(whole_power(0.5, 10), 1.0 / 1024.0);
        assert_eq!(whole_power(1.0, u32::MAX), 1.0);
        assert_eq!(whole_power(0.5, u32::MAX), 0.0);
        // A value that does not represent exactly comes through an exponent of 1 untouched.
        assert_eq!(whole_power(0.1, 1), 0.1);
    }

    /// The criticality conversion has to stay exact and in range at the largest times.
    #[test]
    fn criticality_stays_in_range_with_times_too_large_for_a_float() {
        let mut builder = TimingGraphBuilder::new();
        let input = builder.add_input(1).unwrap();
        let output = builder.add_output(0).unwrap();
        let other_output = builder.add_output(u32::MAX).unwrap();
        let first = builder.add_net_arc(input, output, 0).unwrap();
        let second = builder.add_net_arc(input, other_output, u32::MAX).unwrap();
        let graph = builder.build();
        let analysis = run(&graph, MAX_CLOCK_PERIOD);

        // The largest required time is 2^61, which takes 62 bits, so both sides of the ratio
        // lose 9 bits to fit in 53. The first numerator of 1 becomes 0, and the second of
        // 2^33 - 1 becomes 2^24 - 1.
        let first_critical = analysis.arc_criticality(first, &RAW);
        let second_critical = analysis.arc_criticality(second, &RAW);
        assert_eq!(first_critical, 0.0);
        assert_eq!(
            second_critical,
            ((1u64 << 24) - 1) as f64 / (1u64 << 52) as f64
        );
        assert_eq!(analysis.worst_slack(), Some((1 << 61) - (1 << 33) + 1));
    }

    // Brute force, sharing nothing with the code under test: every path is listed explicitly.

    #[derive(Clone, Debug)]
    struct Case {
        points: u32,
        arcs: Vec<(u32, u32, u32, bool)>,
        launches: Vec<(u32, u32)>,
        captures: Vec<(u32, u32)>,
        period: u64,
    }

    impl Case {
        fn graph(&self) -> TimingGraph {
            let mut builder = TimingGraphBuilder::new();
            for _ in 0..self.points {
                builder.add_point().unwrap();
            }
            for &(from, to, delay, is_net) in &self.arcs {
                let (from, to) = (PointId(from), PointId(to));
                if is_net {
                    builder.add_net_arc(from, to, delay).unwrap();
                } else {
                    builder.add_cell_arc(from, to, delay).unwrap();
                }
            }
            for &(point, offset) in &self.launches {
                builder.set_launch(PointId(point), offset).unwrap();
            }
            for &(point, margin) in &self.captures {
                builder.set_capture(PointId(point), margin).unwrap();
            }
            builder.build()
        }

        fn offset(&self, point: u32) -> Option<u32> {
            self.launches.iter().find(|l| l.0 == point).map(|l| l.1)
        }

        fn margin(&self, point: u32) -> Option<u32> {
            self.captures.iter().find(|c| c.0 == point).map(|c| c.1)
        }

        /// Lists every path from every launch point, including those of no arcs, to anywhere.
        fn all_paths(&self) -> Vec<Path> {
            fn extend(case: &Case, path: &Path, out: &mut Vec<Path>) {
                out.push(path.clone());
                for (index, &(from, to, delay, _)) in case.arcs.iter().enumerate() {
                    if from == path.end {
                        let mut longer = path.clone();
                        longer.arcs.push(index as u32);
                        longer.end = to;
                        longer.total += u64::from(delay);
                        extend(case, &longer, out);
                    }
                }
            }
            let mut out = Vec::new();
            for &(point, offset) in &self.launches {
                let path = Path {
                    start: point,
                    arcs: Vec::new(),
                    end: point,
                    total: u64::from(offset),
                };
                extend(self, &path, &mut out);
            }
            out
        }

        fn period(&self) -> i64 {
            self.period as i64
        }

        /// Returns the points a path passes through, in order.
        fn points_of(&self, path: &Path) -> Vec<u32> {
            let mut points = vec![path.start];
            points.extend(path.arcs.iter().map(|&arc| self.arcs[arc as usize].1));
            points
        }
    }

    #[derive(Clone, Debug)]
    struct Path {
        start: u32,
        arcs: Vec<u32>,
        end: u32,
        /// The launch offset and all the delays.
        total: u64,
    }

    /// A random DAG: arcs go up in a random numbering of the points, so that point numbers
    /// don't give away the topological order. Small delays, so that ties are common.
    fn dag_case() -> impl Strategy<Value = Case> {
        (1u32..8)
            .prop_flat_map(|points| {
                let order = Just((0..points).collect::<Vec<_>>()).prop_shuffle();
                let arc = (0..points, 0..points, 0u32..12, any::<bool>());
                let arcs = prop::collection::vec(arc, 0..14);
                let launch = prop::collection::btree_map(0..points, 0u32..12, 0..=points as usize);
                let capture = prop::collection::btree_map(0..points, 0u32..12, 0..=points as usize);
                (Just(points), order, arcs, launch, capture, 0u64..80)
            })
            .prop_map(|(points, order, arcs, launch, capture, period)| {
                let arcs = arcs
                    .into_iter()
                    .filter(|arc| arc.0 != arc.1)
                    .map(|(a, b, delay, is_net)| {
                        // Rank by value, then rename by the random order.
                        let (low, high) = (a.min(b), a.max(b));
                        (order[low as usize], order[high as usize], delay, is_net)
                    })
                    .collect();
                Case {
                    points,
                    arcs,
                    launches: launch.into_iter().collect(),
                    captures: capture.into_iter().collect(),
                    period,
                }
            })
    }

    /// Any directed graph at all, with cycles, self-loops and parallel arcs.
    fn cyclic_case() -> impl Strategy<Value = Case> {
        (1u32..8)
            .prop_flat_map(|points| {
                let arc = (0..points, 0..points, 0u32..12, any::<bool>());
                let arcs = prop::collection::vec(arc, 0..16);
                let launch = prop::collection::btree_map(0..points, 0u32..12, 0..=points as usize);
                let capture = prop::collection::btree_map(0..points, 0u32..12, 0..=points as usize);
                (Just(points), arcs, launch, capture, 0u64..80)
            })
            .prop_map(|(points, arcs, launch, capture, period)| Case {
                points,
                arcs,
                launches: launch.into_iter().collect(),
                captures: capture.into_iter().collect(),
                period,
            })
    }

    /// Lists which points can reach which, by repeatedly extending a table.
    fn reachability(case: &Case, skip: &BTreeSet<usize>) -> Vec<Vec<bool>> {
        let n = case.points as usize;
        let mut reaches = vec![vec![false; n]; n];
        for (index, &(from, to, _, _)) in case.arcs.iter().enumerate() {
            if !skip.contains(&index) {
                reaches[from as usize][to as usize] = true;
            }
        }
        for via in 0..n {
            for from in 0..n {
                for to in 0..n {
                    if reaches[from][via] && reaches[via][to] {
                        reaches[from][to] = true;
                    }
                }
            }
        }
        reaches
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]

        #[test]
        fn arrival_required_and_slack_match_every_path(case in dag_case()) {
            let graph = case.graph();
            let analysis = run(&graph, case.period);
            let paths = case.all_paths();
            let period = case.period();

            // For each complete path, its slack against the clock and its capture margin.
            let complete = |path: &Path| {
                case.margin(path.end).map(|margin| period - i64::from(margin) - path.total as i64)
            };

            for point in 0..case.points {
                let id = PointId(point);
                let latest = paths.iter().filter(|p| p.end == point).map(|p| p.total).max();
                prop_assert_eq!(analysis.arrival(id), latest, "arrival at {}", point);

                // Paths after a point, from the point on, with no launch point needed.
                let required = required_by_brute_force(&case, point);
                prop_assert_eq!(analysis.required(id), required, "required at {}", point);

                // The slack of a point is the worst slack of the paths through it.
                let through = paths
                    .iter()
                    .filter(|p| case.points_of(p).contains(&point))
                    .filter_map(complete)
                    .min();
                prop_assert_eq!(analysis.slack(id), through, "slack at {}", point);
            }

            for arc in 0..case.arcs.len() {
                let through = paths
                    .iter()
                    .filter(|p| p.arcs.contains(&(arc as u32)))
                    .filter_map(complete)
                    .min();
                prop_assert_eq!(analysis.arc_slack(ArcId(arc as u32)), through, "arc {}", arc);
            }

            let worst = paths.iter().filter_map(complete).min();
            prop_assert_eq!(analysis.worst_slack(), worst);

            // One value per capture point, at its worst path.
            let mut total = 0i128;
            for &(point, _) in &case.captures {
                let lowest = paths
                    .iter()
                    .filter(|p| p.end == point)
                    .filter_map(complete)
                    .min();
                if let Some(lowest) = lowest {
                    total += i128::from(lowest.min(0));
                }
            }
            prop_assert_eq!(analysis.total_negative_slack(), total);
        }

        #[test]
        fn critical_path_is_valid_worst_and_follows_the_tie_break(case in dag_case()) {
            let graph = case.graph();
            let analysis = run(&graph, case.period);
            let paths = case.all_paths();
            let period = case.period();
            let complete = |path: &Path| {
                case.margin(path.end).map(|margin| period - i64::from(margin) - path.total as i64)
            };

            let Some(worst) = paths.iter().filter_map(complete).min() else {
                prop_assert!(analysis.critical_path().is_none());
                return Ok(());
            };
            let found = analysis.critical_path().unwrap();
            prop_assert_eq!(found.slack(), worst);

            // Walk the path: it has to be connected, from a launch point to a capture point,
            // and add up to its own reported slack.
            let mut at = found.start().0;
            let mut total = u64::from(case.offset(at).unwrap());
            for (index, arc) in found.arcs().iter().enumerate() {
                let (from, to, delay, _) = case.arcs[arc.index()];
                prop_assert_eq!(from, at);
                prop_assert_eq!(found.points()[index].0, at);
                total += u64::from(delay);
                at = to;
            }
            prop_assert_eq!(at, found.end().0);
            prop_assert_eq!(found.points().len(), found.arcs().len() + 1);
            prop_assert_eq!(found.arrival(), total);
            let margin = i64::from(case.margin(at).unwrap());
            prop_assert_eq!(period - margin - total as i64, worst);

            // The lowest end with a worst path, then the smallest arcs read from the end.
            let end = paths
                .iter()
                .filter(|p| complete(p) == Some(worst))
                .map(|p| p.end)
                .min()
                .unwrap();
            let expected = paths
                .iter()
                .filter(|p| p.end == end && complete(p) == Some(worst))
                .map(|p| p.arcs.iter().rev().copied().collect::<Vec<_>>())
                .min()
                .unwrap();
            let actual: Vec<_> = found.arcs().iter().rev().map(|arc| arc.0).collect();
            prop_assert_eq!(found.end().0, end);
            prop_assert_eq!(actual, expected);
        }

        #[test]
        fn trace_latest_path_reaches_the_arrival_time_of_every_point(case in dag_case()) {
            let graph = case.graph();
            let analysis = run(&graph, case.period);
            let mut arcs = Vec::new();
            for point in 0..case.points {
                let id = PointId(point);
                let traced = analysis.trace_latest_path(&graph, id, &mut arcs);
                let Some(arrival) = analysis.arrival(id) else {
                    prop_assert!(!traced);
                    continue;
                };
                prop_assert!(traced);
                let start = arcs.first().map_or(point, |arc| case.arcs[arc.index()].0);
                let mut total = u64::from(case.offset(start).unwrap());
                total += arcs
                    .iter()
                    .map(|arc| u64::from(case.arcs[arc.index()].2))
                    .sum::<u64>();
                prop_assert_eq!(total, arrival);
            }
        }

        #[test]
        fn criticality_matches_its_definition_and_stays_in_range(
            case in dag_case(),
            exponent in 0u32..5,
            max in prop_oneof![Just(0.0), Just(0.5), Just(0.99), Just(1.0)],
        ) {
            let graph = case.graph();
            let analysis = run(&graph, case.period);
            let options = CriticalityOptions::new(exponent, max).unwrap();
            let mut all = Vec::new();
            analysis.fill_arc_criticalities(&options, &mut all);
            prop_assert_eq!(all.len(), case.arcs.len());

            // The largest required time of any constrained point, from all of them.
            let max_required = (0..case.points)
                .filter(|&point| analysis.slack(PointId(point)).is_some())
                .filter_map(|point| analysis.required(PointId(point)))
                .max();

            let raw = |slack: Option<i64>| -> f64 {
                let Some(slack) = slack else { return 0.0 };
                let worst = analysis.worst_slack().unwrap();
                let shift = (-worst).max(0);
                let denominator = max_required.unwrap() + shift;
                if denominator == 0 {
                    1.0
                } else {
                    (max_required.unwrap() - slack) as f64 / denominator as f64
                }
            };
            for (arc, filled) in all.iter().enumerate() {
                let id = ArcId(arc as u32);
                let got = analysis.arc_criticality(id, &options);
                prop_assert_eq!(got.to_bits(), filled.to_bits());
                prop_assert!((0.0..=1.0).contains(&got), "{}", got);
                prop_assert!(got <= max);
                let expected_raw = raw(analysis.arc_slack(id));
                prop_assert!((0.0..=1.0).contains(&expected_raw));
                prop_assert_eq!(
                    analysis.arc_criticality(id, &CriticalityOptions::new(1, 1.0).unwrap()),
                    expected_raw,
                );
                // Shift, then power by repeated multiplication, then cap.
                let shifted = expected_raw - (1.0 - max);
                let mut power = 1.0f64;
                let base = if shifted > 0.0 { shifted } else { 0.0 };
                for _ in 0..exponent {
                    power *= base;
                }
                let capped = if power < max { power } else { max };
                prop_assert!((got - capped).abs() < 1e-12);
            }
            for point in 0..case.points {
                let id = PointId(point);
                prop_assert_eq!(
                    analysis.point_criticality(id, &CriticalityOptions::new(1, 1.0).unwrap()),
                    raw(analysis.slack(id)),
                );
            }
            // With no positive slack, the worst path is exactly as critical as can be.
            if let Some(path) = analysis.critical_path().filter(|path| path.slack() <= 0) {
                let end = analysis.point_criticality(
                    path.end(),
                    &CriticalityOptions::new(1, 1.0).unwrap(),
                );
                prop_assert_eq!(end, 1.0);
            }
        }

        #[test]
        fn criticality_rises_as_slack_falls(case in dag_case()) {
            let graph = case.graph();
            let analysis = run(&graph, case.period);
            let options = CriticalityOptions::new(1, 1.0).unwrap();
            let mut values: Vec<(i64, f64)> = (0..case.arcs.len() as u32)
                .filter_map(|arc| {
                    let id = ArcId(arc);
                    Some((analysis.arc_slack(id)?, analysis.arc_criticality(id, &options)))
                })
                .collect();
            values.sort_by_key(|value| value.0);
            for pair in values.windows(2) {
                prop_assert!(pair[0].1 >= pair[1].1, "{:?}", pair);
            }
        }

        #[test]
        fn loops_are_found_broken_and_equal_to_removing_the_arcs(case in cyclic_case()) {
            let graph = case.graph();

            // Mutual reachability gives the same loops as the graph's report.
            let reaches = reachability(&case, &BTreeSet::new());
            let n = case.points as usize;
            let on_cycle: Vec<usize> = (0..n).filter(|&point| reaches[point][point]).collect();
            let mut expected: Vec<Vec<u32>> = Vec::new();
            for &point in &on_cycle {
                let known = expected.iter().any(|group| group.contains(&(point as u32)));
                if !known {
                    expected.push(
                        on_cycle
                            .iter()
                            .copied()
                            .filter(|&other| reaches[point][other] && reaches[other][point])
                            .map(|other| other as u32)
                            .collect(),
                    );
                }
            }
            let reported: Vec<Vec<u32>> = graph
                .loops()
                .iter()
                .map(|group| group.points.iter().map(|point| point.0).collect())
                .collect();
            prop_assert_eq!(&reported, &expected);

            // What's left has no cycles, and each broken arc is inside its own loop.
            let broken: BTreeSet<usize> = graph
                .loops()
                .iter()
                .flat_map(|group| group.broken_arcs.iter().map(|arc| arc.index()))
                .collect();
            let remaining = reachability(&case, &broken);
            for (point, row) in remaining.iter().enumerate() {
                prop_assert!(!row[point], "point {} is still on a cycle", point);
            }
            for (arc, &(from, to, _, _)) in case.arcs.iter().enumerate() {
                prop_assert_eq!(graph.is_broken(ArcId(arc as u32)), broken.contains(&arc));
                // Every disabled arc costs timed paths, so each must have been needed: putting
                // it back alone would close a cycle through arcs that are still there.
                if broken.contains(&arc) {
                    prop_assert!(from == to || remaining[to as usize][from as usize]);
                }
            }

            // Analysis gives the same answers as a fresh graph without those arcs.
            let mut without = case.clone();
            without.arcs = case
                .arcs
                .iter()
                .enumerate()
                .filter(|(arc, _)| !broken.contains(arc))
                .map(|(_, arc)| *arc)
                .collect();
            let reference_graph = without.graph();
            prop_assert_eq!(reference_graph.loops(), &[]);
            let analysis = run(&graph, case.period);
            let reference = run(&reference_graph, case.period);
            prop_assert_eq!(&analysis.arrival, &reference.arrival);
            prop_assert_eq!(&analysis.required, &reference.required);
            prop_assert_eq!(analysis.worst_slack(), reference.worst_slack());
            prop_assert_eq!(analysis.total_negative_slack(), reference.total_negative_slack());
        }

        #[test]
        fn updated_delays_match_a_fresh_build(
            case in dag_case(),
            delays in prop::collection::vec(0u32..40, 14),
        ) {
            let mut graph = case.graph();
            let constraints = Constraints::single_clock(case.period).unwrap();
            let mut analysis = TimingAnalysis::new(&graph);
            analysis.run(&graph, &constraints);

            graph.apply_net_delays(&|arc: ArcId| delays[arc.index()]);
            analysis.run(&graph, &constraints);

            let mut updated = case.clone();
            for (index, arc) in updated.arcs.iter_mut().enumerate() {
                if arc.3 {
                    arc.2 = delays[index];
                }
            }
            let fresh = run(&updated.graph(), case.period);
            prop_assert_eq!(&analysis.arrival, &fresh.arrival);
            prop_assert_eq!(&analysis.required, &fresh.required);
            prop_assert_eq!(&analysis.arc_slack, &fresh.arc_slack);
            prop_assert_eq!(analysis.worst_slack(), fresh.worst_slack());
            prop_assert_eq!(analysis.total_negative_slack(), fresh.total_negative_slack());
            prop_assert_eq!(analysis.critical_path(), fresh.critical_path());
        }

        #[test]
        fn building_depends_only_on_the_graph_not_on_the_constraint_order(case in cyclic_case()) {
            let mut shuffled = case.clone();
            shuffled.launches.reverse();
            shuffled.captures.reverse();
            let (graph, shuffled_graph) = (case.graph(), shuffled.graph());
            prop_assert_eq!(graph.loops(), shuffled_graph.loops());
            let (one, other) = (run(&graph, case.period), run(&shuffled_graph, case.period));
            prop_assert_eq!(one.critical_path(), other.critical_path());
        }
    }

    /// Two registers on each of two clocks, with a path within each clock and one across.
    ///
    /// ```text
    /// a1 -(100)-> a2    on clock 1, registers of clock-to-Q 10 and setup 5
    /// b1 -(300)-> b2    on clock 2
    /// a1 -(50)--> b2    from clock 1 to clock 2
    /// ```
    fn two_clocks() -> (TimingGraph, [ArcId; 3], [PointId; 4]) {
        let mut builder = TimingGraphBuilder::new();
        let mut register = |clock: u32| {
            let d = builder.add_point().unwrap();
            let q = builder.add_point().unwrap();
            builder.set_capture_on(d, ClockId(clock), 5).unwrap();
            builder.set_launch_on(q, ClockId(clock), 10).unwrap();
            (d, q)
        };
        let (_, a1) = register(1);
        let (a2, _) = register(1);
        let (_, b1) = register(2);
        let (b2, _) = register(2);
        let within_a = builder.add_net_arc(a1, a2, 100).unwrap();
        let within_b = builder.add_net_arc(b1, b2, 300).unwrap();
        let across = builder.add_net_arc(a1, b2, 50).unwrap();
        (
            builder.build(),
            [within_a, within_b, across],
            [a1, a2, b1, b2],
        )
    }

    #[test]
    fn each_clock_is_timed_against_its_own_period_and_crossings_are_not() {
        let (graph, [within_a, within_b, across], [a1, a2, _, b2]) = two_clocks();
        assert_eq!(graph.clocks(), [ClockId(1), ClockId(2)]);
        assert_eq!(graph.launch_clock(a1), Some(ClockId(1)));
        assert_eq!(graph.capture_clock(b2), Some(ClockId(2)));
        assert_eq!(graph.capture_clock(a1), None);

        let constraints = Constraints::per_clock(&[(ClockId(1), 200), (ClockId(2), 500)]).unwrap();
        let mut analysis = TimingAnalysis::new(&graph);
        analysis.run(&graph, &constraints);
        // 200 - 5 - 110 and 500 - 5 - 310. The crossing isn't timed at all.
        assert_eq!(analysis.arc_slack(within_a), Some(85));
        assert_eq!(analysis.arc_slack(within_b), Some(185));
        assert_eq!(analysis.arc_slack(across), None);
        assert_eq!(analysis.worst_slack(), Some(85));
        assert_eq!(analysis.total_negative_slack(), 0);
        let path = analysis.critical_path().unwrap();
        assert_eq!(path.points(), &[a1, a2]);
        assert_eq!(path.arrival(), 110);
        // `b2` is reached at 310 from clock 2, and later than `a2`'s 110 from clock 1.
        assert_eq!(analysis.arrival(b2), Some(310));
        assert_eq!(analysis.slack(b2), Some(185));
        assert_eq!(analysis.slack(a1), Some(85));

        // Each clock's criticality is against its own largest required time: 195 and 495.
        let options = CriticalityOptions::new(1, 1.0).unwrap();
        assert_eq!(analysis.arc_criticality(within_a, &options), 110.0 / 195.0);
        assert_eq!(analysis.arc_criticality(within_b, &options), 310.0 / 495.0);
        assert_eq!(analysis.arc_criticality(across, &options), 0.0);
        let mut all = Vec::new();
        analysis.fill_arc_criticalities(&options, &mut all);
        assert_eq!(all, [110.0 / 195.0, 310.0 / 495.0, 0.0]);

        // As one clock of 200, the slower clock's path fails, and the crossing is timed.
        let mut merged = TimingAnalysis::new(&graph);
        merged.run(&graph, &Constraints::single_clock(200).unwrap());
        assert_eq!(merged.worst_slack(), Some(-115));
        assert_eq!(merged.arc_slack(across), Some(135));

        // Timing the crossing at the closest edges, 100 apart: 100 - 5 - 60.
        let crossing = constraints
            .constrain_by_edges(ClockId(1), ClockId(2))
            .unwrap();
        assert_eq!(crossing.constraint(ClockId(1), ClockId(2)), Some(100));
        analysis.run(&graph, &crossing);
        assert_eq!(analysis.arc_slack(across), Some(35));
        assert_eq!(analysis.worst_slack(), Some(35));
        assert_eq!(analysis.critical_path().unwrap().points(), &[a1, b2]);
        // `a1` launches into both clocks, and its worst is the crossing.
        assert_eq!(analysis.slack(a1), Some(35));
        // The crossing's own pair: `b2` is its only capture point, at 95.
        assert_eq!(analysis.arc_criticality(across, &options), 60.0 / 95.0);
        // `b2` is in two pairs, and the higher wins: 60 / 95 across, over 310 / 495 within.
        assert_eq!(analysis.point_criticality(b2, &options), 60.0 / 95.0);

        // Failing pairs add to the total each on their own.
        let tight = Constraints::per_clock(&[(ClockId(1), 100), (ClockId(2), 300)])
            .unwrap()
            .constrain(ClockId(1), ClockId(2), 0)
            .unwrap();
        analysis.run(&graph, &tight);
        // -15 within clock 1, -15 within clock 2 and -65 across, all at their own ends.
        assert_eq!(analysis.worst_slack(), Some(-65));
        assert_eq!(analysis.total_negative_slack(), -95);
        // Each pair's worst path is fully critical against its own worst slack.
        for arc in [within_a, within_b, across] {
            assert_eq!(analysis.arc_criticality(arc, &options), 1.0);
        }

        // A clock without a period, and nothing timed at all.
        let none = Constraints::per_clock(&[(ClockId(7), 10)]).unwrap();
        analysis.run(&graph, &none);
        assert_eq!(analysis.worst_slack(), None);
        assert_eq!(analysis.critical_path(), None);
        assert_eq!(analysis.arc_criticality(within_a, &options), 0.0);
        assert_eq!(analysis.slack(a1), None);
        // And back to one clock, which answers as before.
        analysis.run(&graph, &Constraints::single_clock(200).unwrap());
        assert_eq!(analysis.worst_slack(), Some(-115));
    }

    #[test]
    fn constraints_per_clock_are_checked_and_answer_queries() {
        let (one, two) = (ClockId(1), ClockId(2));
        assert_eq!(
            Constraints::per_clock(&[(one, 1), (two, 2), (one, 3)]),
            Err(TimingError::DuplicateClock { clock: one }),
        );
        assert_eq!(
            Constraints::per_clock(&[(one, MAX_CLOCK_PERIOD + 1)]),
            Err(TimingError::ClockPeriodTooLarge {
                period: MAX_CLOCK_PERIOD + 1
            }),
        );
        let single = Constraints::single_clock(40).unwrap();
        assert!(!single.is_per_clock());
        assert_eq!(single.period(two), Some(40));
        assert_eq!(single.constraint(one, two), Some(40));
        assert_eq!(
            single.clone().constrain(one, two, 5),
            Err(TimingError::NotPerClock)
        );
        assert_eq!(
            single.constrain_by_edges(one, two),
            Err(TimingError::NotPerClock)
        );

        let per = Constraints::per_clock(&[(two, 600), (one, 0)]).unwrap();
        assert!(per.is_per_clock());
        assert_eq!(per.clock_period(), 0);
        assert_eq!((per.period(one), per.period(two)), (Some(0), Some(600)));
        assert_eq!(per.period(ClockId(3)), None);
        assert_eq!(per.constraint(two, one), None);
        assert_eq!(
            per.clone().constrain_by_edges(two, ClockId(3)),
            Err(TimingError::UnknownClock { clock: ClockId(3) })
        );
        assert_eq!(
            per.clone().constrain(one, two, MAX_CLOCK_PERIOD + 1),
            Err(TimingError::ClockPeriodTooLarge {
                period: MAX_CLOCK_PERIOD + 1
            })
        );
        // A period of 0 asks for the fastest, across clocks too.
        let edges = per.clone().constrain_by_edges(two, one).unwrap();
        assert_eq!(edges.constraint(two, one), Some(0));
        // A pair of a clock with itself can be overridden.
        let own = per.constrain(two, two, 50).unwrap();
        assert_eq!(own.constraint(two, two), Some(50));
    }

    #[test]
    fn edges_are_closest_at_the_greatest_common_divisor() {
        for (launch, capture, expected) in [
            (10, 10, 10),
            (10, 4, 2),
            (4, 10, 2),
            (400, 1000, 200),
            (20, 10, 10),
            (10, 20, 10),
            (7, 13, 1),
            (0, 5, 0),
            (5, 0, 0),
        ] {
            assert_eq!(edge_separation(launch, capture), expected);
            // By listing every pair of edges over the least common multiple.
            if launch > 0 && capture > 0 {
                let lcm = launch * capture / expected;
                let closest = (0..lcm / launch)
                    .map(|k| {
                        let edge = k * launch;
                        let next = (edge / capture + 1) * capture;
                        next - edge
                    })
                    .min()
                    .unwrap();
                assert_eq!(closest, expected, "{launch} and {capture}");
            }
        }
    }

    /// A [`Case`] with a clock on each of its launch and capture points, and the timed pairs.
    #[derive(Clone, Debug)]
    struct ClockedCase {
        case: Case,
        /// By launch point, then capture point, in the order of the case's lists.
        launch_clocks: Vec<u32>,
        capture_clocks: Vec<u32>,
        pairs: BTreeMap<(u32, u32), u64>,
    }

    impl ClockedCase {
        fn graph(&self) -> TimingGraph {
            let case = &self.case;
            let mut builder = TimingGraphBuilder::new();
            for _ in 0..case.points {
                builder.add_point().unwrap();
            }
            for &(from, to, delay, is_net) in &case.arcs {
                let (from, to) = (PointId(from), PointId(to));
                if is_net {
                    builder.add_net_arc(from, to, delay).unwrap();
                } else {
                    builder.add_cell_arc(from, to, delay).unwrap();
                }
            }
            for (&(point, offset), &clock) in case.launches.iter().zip(&self.launch_clocks) {
                builder
                    .set_launch_on(PointId(point), ClockId(clock), offset)
                    .unwrap();
            }
            for (&(point, margin), &clock) in case.captures.iter().zip(&self.capture_clocks) {
                builder
                    .set_capture_on(PointId(point), ClockId(clock), margin)
                    .unwrap();
            }
            builder.build()
        }

        fn constraints(&self) -> Constraints {
            let mut constraints = Constraints::per_clock(&[]).unwrap();
            for (&(launch, capture), &constraint) in &self.pairs {
                constraints = constraints
                    .constrain(ClockId(launch), ClockId(capture), constraint)
                    .unwrap();
            }
            constraints
        }

        fn launch_clock(&self, point: u32) -> Option<u32> {
            let index = self.case.launches.iter().position(|l| l.0 == point)?;
            Some(self.launch_clocks[index])
        }

        fn capture_clock(&self, point: u32) -> Option<u32> {
            let index = self.case.captures.iter().position(|c| c.0 == point)?;
            Some(self.capture_clocks[index])
        }

        /// A path's pair of clocks and its slack, if it ends at a capture point and the pair
        /// is timed.
        fn complete(&self, path: &Path) -> Option<((u32, u32), i64)> {
            let launch = self.launch_clock(path.start)?;
            let capture = self.capture_clock(path.end)?;
            let constraint = *self.pairs.get(&(launch, capture))?;
            let margin = self.case.margin(path.end)?;
            let slack = constraint as i64 - i64::from(margin) - path.total as i64;
            Some(((launch, capture), slack))
        }

        /// The required time at a point for a pair, from every path on to its capture clock.
        fn required(&self, point: u32, capture: u32, constraint: u64) -> Option<i64> {
            fn walk(
                this: &ClockedCase,
                at: u32,
                spent: i64,
                wanted: (u32, i64),
                best: &mut Option<i64>,
            ) {
                if this.capture_clock(at) == Some(wanted.0) {
                    let margin = i64::from(this.case.margin(at).unwrap());
                    let required = wanted.1 - margin - spent;
                    *best = Some(best.map_or(required, |known: i64| known.min(required)));
                }
                for &(from, to, delay, _) in &this.case.arcs {
                    if from == at {
                        walk(this, to, spent + i64::from(delay), wanted, best);
                    }
                }
            }
            let mut best = None;
            walk(self, point, 0, (capture, constraint as i64), &mut best);
            best
        }
    }

    fn clocked_case() -> impl Strategy<Value = ClockedCase> {
        let pairs = prop::collection::btree_map((0u32..3, 0u32..3), 0u64..80, 0..7);
        (
            dag_case(),
            prop::collection::vec(0u32..3, 8),
            prop::collection::vec(0u32..3, 8),
            pairs,
        )
            .prop_map(|(case, launch_clocks, capture_clocks, pairs)| ClockedCase {
                launch_clocks: launch_clocks[..case.launches.len()].to_vec(),
                capture_clocks: capture_clocks[..case.captures.len()].to_vec(),
                case,
                pairs,
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]

        /// With every point on one clock, constraints for that clock alone give exactly what
        /// a single clock does, to the bit.
        #[test]
        fn one_clock_per_clock_matches_a_single_clock(case in dag_case(), clock in 0u32..3) {
            let mut clocked = ClockedCase {
                launch_clocks: vec![clock; case.launches.len()],
                capture_clocks: vec![clock; case.captures.len()],
                pairs: BTreeMap::new(),
                case,
            };
            clocked.pairs.insert((clock, clock), clocked.case.period);
            let graph = clocked.graph();
            let single = run(&graph, clocked.case.period);
            let mut per = TimingAnalysis::new(&graph);
            per.run(&graph, &clocked.constraints());

            prop_assert_eq!(per.worst_slack(), single.worst_slack());
            prop_assert_eq!(per.total_negative_slack(), single.total_negative_slack());
            prop_assert_eq!(per.critical_path(), single.critical_path());
            for options in [CriticalityOptions::default(), CriticalityOptions::new(3, 1.0).unwrap()] {
                let (mut a, mut b) = (Vec::new(), Vec::new());
                per.fill_arc_criticalities(&options, &mut a);
                single.fill_arc_criticalities(&options, &mut b);
                let bits = |values: &[f64]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
                prop_assert_eq!(bits(&a), bits(&b));
                for point in 0..clocked.case.points {
                    let id = PointId(point);
                    prop_assert_eq!(
                        per.point_criticality(id, &options).to_bits(),
                        single.point_criticality(id, &options).to_bits()
                    );
                }
            }
            let (mut a, mut b) = (Vec::new(), Vec::new());
            for point in 0..clocked.case.points {
                let id = PointId(point);
                prop_assert_eq!(per.arrival(id), single.arrival(id));
                prop_assert_eq!(per.required(id), single.required(id));
                prop_assert_eq!(per.slack(id), single.slack(id));
                prop_assert_eq!(
                    per.trace_latest_path(&graph, id, &mut a),
                    single.trace_latest_path(&graph, id, &mut b)
                );
                prop_assert_eq!(&a, &b);
            }
            for arc in 0..graph.num_arcs() {
                prop_assert_eq!(per.arc_slack(ArcId(arc)), single.arc_slack(ArcId(arc)));
            }
        }

        /// Every result against every path, listed with its pair of clocks.
        #[test]
        fn clock_pairs_match_every_path(clocked in clocked_case()) {
            let graph = clocked.graph();
            let mut analysis = TimingAnalysis::new(&graph);
            analysis.run(&graph, &clocked.constraints());
            let paths = clocked.case.all_paths();
            let timed: Vec<((u32, u32), i64, &Path)> = paths
                .iter()
                .filter_map(|path| clocked.complete(path).map(|(pair, slack)| (pair, slack, path)))
                .collect();

            for point in 0..clocked.case.points {
                let through = timed
                    .iter()
                    .filter(|(_, _, path)| clocked.case.points_of(path).contains(&point))
                    .map(|&(_, slack, _)| slack)
                    .min();
                prop_assert_eq!(analysis.slack(PointId(point)), through, "point {}", point);
            }
            for arc in 0..clocked.case.arcs.len() as u32 {
                let through = timed
                    .iter()
                    .filter(|(_, _, path)| path.arcs.contains(&arc))
                    .map(|&(_, slack, _)| slack)
                    .min();
                prop_assert_eq!(analysis.arc_slack(ArcId(arc)), through, "arc {}", arc);
            }
            let worst = timed.iter().map(|&(_, slack, _)| slack).min();
            prop_assert_eq!(analysis.worst_slack(), worst);

            // Per pair, one value per capture point, at its worst path.
            let mut total = 0i128;
            for &pair in clocked.pairs.keys() {
                for &(point, _) in &clocked.case.captures {
                    let lowest = timed
                        .iter()
                        .filter(|(p, _, path)| *p == pair && path.end == point)
                        .map(|&(_, slack, _)| slack)
                        .min();
                    total += i128::from(lowest.map_or(0, |slack| slack.min(0)));
                }
            }
            prop_assert_eq!(analysis.total_negative_slack(), total);

            // The critical path is a worst one, of a timed pair, ending at the lowest such end.
            match (analysis.critical_path(), worst) {
                (None, None) => {}
                (Some(path), Some(worst)) => {
                    prop_assert_eq!(path.slack(), worst);
                    let end = timed
                        .iter()
                        .filter(|&&(_, slack, _)| slack == worst)
                        .map(|(_, _, path)| path.end)
                        .min()
                        .unwrap();
                    prop_assert_eq!(path.end().0, end);
                    let start = path.start().0;
                    let mut total = u64::from(clocked.case.offset(start).unwrap());
                    for arc in path.arcs() {
                        total += u64::from(clocked.case.arcs[arc.index()].2);
                    }
                    prop_assert_eq!(path.arrival(), total);
                    let found = Path {
                        start,
                        arcs: path.arcs().iter().map(|arc| arc.0).collect(),
                        end,
                        total,
                    };
                    prop_assert_eq!(clocked.complete(&found).map(|c| c.1), Some(worst));
                }
                (found, expected) => prop_assert!(false, "{:?} against {:?}", found, expected),
            }

            // Raw criticality: the highest over pairs, each against its own worst slack and
            // largest required time.
            let raw = |on_path: &dyn Fn(&Path) -> bool| -> f64 {
                let mut highest = 0.0f64;
                for (&(launch, capture), &constraint) in &clocked.pairs {
                    let of_pair: Vec<_> = timed
                        .iter()
                        .filter(|(pair, _, _)| *pair == (launch, capture))
                        .collect();
                    let Some(pair_worst) = of_pair.iter().map(|&&(_, slack, _)| slack).min() else {
                        continue;
                    };
                    let max_required = of_pair
                        .iter()
                        .map(|(_, _, path)| clocked.required(path.end, capture, constraint).unwrap())
                        .max()
                        .unwrap();
                    let Some(slack) = of_pair
                        .iter()
                        .filter(|(_, _, path)| on_path(path))
                        .map(|&&(_, slack, _)| slack)
                        .min()
                    else {
                        continue;
                    };
                    let denominator = max_required + (-pair_worst).max(0);
                    let value = if denominator == 0 {
                        1.0
                    } else {
                        (max_required - slack) as f64 / denominator as f64
                    };
                    highest = highest.max(value);
                }
                highest
            };
            let options = CriticalityOptions::new(1, 1.0).unwrap();
            for arc in 0..clocked.case.arcs.len() as u32 {
                let expected = raw(&|path: &Path| path.arcs.contains(&arc));
                prop_assert_eq!(analysis.arc_criticality(ArcId(arc), &options), expected, "arc {}", arc);
            }
            for point in 0..clocked.case.points {
                let expected = raw(&|path: &Path| clocked.case.points_of(path).contains(&point));
                prop_assert_eq!(
                    analysis.point_criticality(PointId(point), &options),
                    expected,
                    "point {}",
                    point
                );
            }
        }
    }

    /// The required time at a point, by listing every path from it to a capture point.
    fn required_by_brute_force(case: &Case, point: u32) -> Option<i64> {
        fn walk(case: &Case, at: u32, spent: i64, best: &mut Option<i64>) {
            if let Some(margin) = case.margin(at) {
                let required = case.period() - i64::from(margin) - spent;
                *best = Some(best.map_or(required, |known: i64| known.min(required)));
            }
            for &(from, to, delay, _) in &case.arcs {
                if from == at {
                    walk(case, to, spent + i64::from(delay), best);
                }
            }
        }
        let mut best = None;
        walk(case, point, 0, &mut best);
        best
    }

    /// Generates a netlist-like graph of at least the requested number of arcs: a stack of
    /// levels of cells, where each cell's output drives a few inputs of later levels. Shuffling
    /// the IDs is the worst case for the memory access patterns of analysis.
    fn large_graph(arcs: usize, shuffle: bool) -> (TimingGraphBuilder, usize) {
        use crate::rng::Rng;

        const LEVELS: u64 = 40;
        const FANIN: usize = 4;
        let mut rng = Rng::new(0x5EED);

        // Each LUT is an output pin with FANIN input pins, FANIN cell arcs and FANIN net arcs.
        // The first level has no net arcs, so make up for those: 1/80 of them.
        let luts = (arcs / (2 * FANIN) + 1) * 80 / 79 + 1;
        let points = luts * (FANIN + 1);
        let mut names: Vec<u32> = (0..points as u32).collect();
        if shuffle {
            rng.shuffle(&mut names);
        }
        // The inputs of the first level launch paths, as if from registers.
        let lut_level: Vec<u64> = (0..luts)
            .map(|lut| lut as u64 * LEVELS / luts as u64)
            .collect();
        let output_pin = |lut: usize| PointId(names[lut * (FANIN + 1)]);
        let input_pin = |lut: usize, pin: usize| PointId(names[lut * (FANIN + 1) + 1 + pin]);

        let mut builder = TimingGraphBuilder::with_capacity(points, arcs);
        for _ in 0..points {
            builder.add_point().unwrap();
        }
        let mut arc_list = Vec::with_capacity(luts * 2 * FANIN);
        let mut has_fanout = vec![false; luts];
        let level_start = |level: u64| (level * luts as u64).div_ceil(LEVELS) as usize;
        for (lut, &level) in lut_level.iter().enumerate() {
            for pin in 0..FANIN {
                let input = input_pin(lut, pin);
                arc_list.push((input, output_pin(lut), false, 100 + rng.below(200) as u32));
                if level == 0 {
                    builder.set_launch(input, 100).unwrap();
                } else {
                    // Drivers from a few levels back, to get paths that reconverge.
                    let back = 1 + rng.below(level.min(3));
                    let start = level_start(level - back);
                    let end = level_start(level - back + 1);
                    let driver = start + rng.below((end - start).max(1) as u64) as usize;
                    has_fanout[driver] = true;
                    arc_list.push((output_pin(driver), input, true, 100 + rng.below(900) as u32));
                }
            }
        }
        // Whatever drives nothing goes to a register, so every arc is on a timed path and none
        // gets out of doing its share of the work.
        for (lut, drives_something) in has_fanout.iter().enumerate() {
            if !drives_something {
                builder.set_capture(output_pin(lut), 50).unwrap();
            }
        }
        if shuffle {
            rng.shuffle(&mut arc_list);
        }
        for (from, to, is_net, delay) in arc_list {
            if is_net {
                builder.add_net_arc(from, to, delay).unwrap();
            } else {
                builder.add_cell_arc(from, to, delay).unwrap();
            }
        }
        (builder, points)
    }

    /// Reports how long analysis takes on large graphs. Run it with `--ignored --nocapture`
    /// and in release mode; the module documentation has the command and the results.
    #[test]
    #[ignore = "a measurement rather than a check, and only meaningful in release mode"]
    fn large_graph_timings() {
        use std::time::Instant;

        fn median_of(runs: usize, mut work: impl FnMut()) -> f64 {
            let mut times: Vec<f64> = (0..runs)
                .map(|_| {
                    let start = Instant::now();
                    work();
                    start.elapsed().as_secs_f64() * 1e3
                })
                .collect();
            times.sort_by(f64::total_cmp);
            times[times.len() / 2]
        }

        for (arcs, shuffle) in [
            (1_000_000, true),
            (1_000_000, false),
            (10_000_000, true),
            (10_000_000, false),
        ] {
            let (builder, points) = large_graph(arcs, shuffle);
            let start = Instant::now();
            let mut graph = builder.build();
            let build_ms = start.elapsed().as_secs_f64() * 1e3;
            assert!(graph.loops().is_empty());
            assert!(graph.num_arcs() as usize >= arcs);

            let delays: Vec<u32> = (0..graph.num_arcs()).map(|arc| 100 + arc % 900).collect();
            let apply_ms = median_of(11, || {
                graph.apply_net_delays(&|arc: ArcId| delays[arc.index()]);
            });

            let constraints = Constraints::single_clock(20_000).unwrap();
            let mut analysis = TimingAnalysis::new(&graph);
            let run_ms = median_of(11, || analysis.run(&graph, &constraints));
            for arc in 0..graph.num_arcs() {
                assert!(analysis.arc_slack(ArcId(arc)).is_some());
            }

            let options = CriticalityOptions::default();
            let mut criticalities = Vec::with_capacity(graph.num_arcs() as usize);
            let fill_ms = median_of(11, || {
                analysis.fill_arc_criticalities(&options, &mut criticalities);
            });

            println!(
                "{} arcs, {points} points, {} levels, shuffled {shuffle}: \
                 build {build_ms:.0} ms, update net delays \
                 {apply_ms:.1} ms, run {run_ms:.1} ms, criticalities {fill_ms:.1} ms, \
                 worst slack {:?}",
                graph.num_arcs(),
                graph.num_levels(),
                analysis.worst_slack(),
            );
        }
    }
}
