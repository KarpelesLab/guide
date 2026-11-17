//! Reticle's timing model of a placed design (`place::PlaceTiming`, what `--timing-driven`
//! places and routes by) as a guide [`TimingGraph`], so that guide can route by it.
//!
//! A point is a netlist pin. It is timed as reticle's own analysis
//! (`place::path_criticality`) times it:
//!
//! - An instance's `Through` inputs reach its `Through` outputs (only the outputs `reaches`
//!   says, for a table holding two functions) through a cell arc of the pin's own delay to
//!   that output (`PlaceTiming::through_at`): a carry chain's carry in reaches its carry out
//!   far sooner than a select input does.
//! - A `Start` output, or a `Through` output with a launch of its own, launches paths at its
//!   own launch (`launch_at`), its site's and its clock's arrival included. An `End` input, or
//!   a `Through` input with a capture of its own (`ends_at`), captures them at its own
//!   capture (`capture_at`), its clock's arrival included. False paths (`untimed_*`) neither.
//! - Each **path group** of reticle's is a pair of guide clocks, each timed and weighed on its
//!   own as reticle weighs each group against its own latest path: per domain, a launch
//!   clock for registers and one for pads, and a capture clock for registers and one for
//!   pads per number of periods a path may take (`cycles`), the pair constrained to that many
//!   periods. Paths between two domains are not timed, as reticle's analysis has it. A pin in
//!   every domain (`PathDomain::Every`) launches or captures in each, through a point of its
//!   own for each domain after the first.
//!
//! A connection's net arc starts at reticle's estimate of it from the placement
//! (`place::estimated_wire`) and the router replaces it.
//!
//! What is left out, so that guide's analysis is only close to reticle's: the margin on a
//! path between two clock regions (`region_margin`), a capture margin below zero (counted as
//! zero), and a branch's load on the connections past it. Results are timed by reticle's own
//! analysis afterwards in any case.

use std::collections::BTreeMap;

use guide::engine::{Connection, TimingDriven};
use guide::id::NetId;
use guide::timing::{
    ArcId, ClockId, Constraints, PointId, TimingAnalysis, TimingGraph, TimingGraphBuilder,
};
use reticle::fpga::arch::RoutingGraph;
use reticle::fpga::place::{Netlist, PathDomain, PinTiming, PlaceTiming, Placement};

fn ps(ns: f64) -> u32 {
    if ns.is_finite() && ns > 0.0 {
        (ns * 1000.0).round().min(f64::from(u32::MAX)) as u32
    } else {
        0
    }
}

/// A guide clock: which side of a path, its domain, whether at a pad, and for a capture the
/// periods its paths may take.
type ClockKey = (bool, usize, bool, u32);

/// Reticle's model as a guide timing graph, with a net arc for each `(driver, sink)` pin
/// pair, of `delay(index)` picoseconds, and an arc of no delay for each `(pin, alias)`, a sink
/// pin that arrives when another does; returns the graph, the net arcs in order and the
/// constraints.
pub fn graph_of(
    netlist: &Netlist,
    timing: &PlaceTiming,
    connections: &[(usize, usize)],
    aliases: &[(usize, usize)],
    delay: impl Fn(usize) -> u32,
) -> Result<(TimingGraph, Vec<ArcId>, Constraints), String> {
    let err = |e: guide::timing::TimingError| format!("timing graph: {e}");
    let mut builder = TimingGraphBuilder::with_capacity(netlist.pins.len(), netlist.pins.len() * 2);
    let mut points: Vec<PointId> = Vec::with_capacity(netlist.pins.len());
    for _ in &netlist.pins {
        points.push(builder.add_point().map_err(err)?);
    }
    let domains = timing.periods.len();
    let domains_of = |pin: usize, instance: usize| -> Vec<usize> {
        let domain = timing
            .pin_domain
            .get(pin)
            .copied()
            .flatten()
            .unwrap_or_else(|| {
                timing
                    .domain
                    .get(instance)
                    .copied()
                    .unwrap_or(PathDomain::Untimed)
            });
        match domain {
            PathDomain::Untimed => Vec::new(),
            PathDomain::Clock(c) if c < domains => vec![c],
            PathDomain::Clock(_) => Vec::new(),
            PathDomain::Every => (0..domains).collect(),
        }
    };
    let flag = |flags: &[bool], instance: usize| flags.get(instance).copied().unwrap_or(false);
    let mut clocks: BTreeMap<ClockKey, ClockId> = BTreeMap::new();
    let mut clock = |key: ClockKey| -> ClockId {
        let next = ClockId(clocks.len() as u32);
        *clocks.entry(key).or_insert(next)
    };
    for (instance, inst) in netlist.instances.iter().enumerate() {
        let role = |pin: usize| timing.pins.get(pin).copied().unwrap_or(PinTiming::Ignored);
        let pad = inst.kind == "io";
        let cycles = timing.cycles.get(instance).copied().unwrap_or(1).max(1);
        let outputs: Vec<usize> = inst
            .pins
            .iter()
            .copied()
            .filter(|p| netlist.pins[*p].output && role(*p) == PinTiming::Through)
            .collect();
        for &pin in &inst.pins {
            let output = netlist.pins[pin].output;
            if role(pin) == PinTiming::Through && !output {
                let reach = timing.reaches.get(pin).copied().unwrap_or(u8::MAX);
                for (j, out) in outputs.iter().enumerate() {
                    if j >= 8 || reach >> j & 1 != 0 {
                        let delay = ps(timing.through_at(instance, pin, j));
                        builder
                            .add_cell_arc(points[pin], points[*out], delay)
                            .map_err(err)?;
                    }
                }
            }
            let own_launch = matches!(timing.pin_delays.launch.get(pin), Some(Some(_)));
            let launches = output
                && match role(pin) {
                    PinTiming::Start => true,
                    PinTiming::Through => own_launch,
                    _ => false,
                };
            if launches && !flag(&timing.untimed_from, instance) {
                let launch = ps(timing.launch_at(instance, pin));
                for (k, domain) in domains_of(pin, instance).into_iter().enumerate() {
                    let at = if k == 0 {
                        points[pin]
                    } else {
                        let extra = builder.add_point().map_err(err)?;
                        builder.add_cell_arc(extra, points[pin], 0).map_err(err)?;
                        extra
                    };
                    builder
                        .set_launch_on(at, clock((false, domain, pad, 1)), launch)
                        .map_err(err)?;
                }
            }
            if !output && timing.ends_at(pin) && !flag(&timing.untimed_to, instance) {
                let margin = ps(timing.capture_at(instance, pin));
                for (k, domain) in domains_of(pin, instance).into_iter().enumerate() {
                    let at = if k == 0 {
                        points[pin]
                    } else {
                        let extra = builder.add_point().map_err(err)?;
                        builder.add_cell_arc(points[pin], extra, 0).map_err(err)?;
                        extra
                    };
                    builder
                        .set_capture_on(at, clock((true, domain, pad, cycles)), margin)
                        .map_err(err)?;
                }
            }
        }
    }
    let mut arcs = Vec::with_capacity(connections.len());
    for (index, &(driver, sink)) in connections.iter().enumerate() {
        arcs.push(
            builder
                .add_net_arc(points[driver], points[sink], delay(index))
                .map_err(err)?,
        );
    }
    for &(pin, alias) in aliases {
        builder
            .add_cell_arc(points[pin], points[alias], 0)
            .map_err(err)?;
    }
    // Every launch clock of a domain with every capture clock of it.
    let mut constraints = Constraints::per_clock(&[]).map_err(err)?;
    for (&(capture, domain, _, cycles), &to) in &clocks {
        if !capture {
            continue;
        }
        let period = u64::from(ps(timing.periods[domain])).max(1) * u64::from(cycles);
        for (&(other, from_domain, _, _), &from) in &clocks {
            if !other && from_domain == domain {
                constraints = constraints.constrain(from, to, period).map_err(err)?;
            }
        }
    }
    Ok((builder.build(), arcs, constraints))
}

/// The timing graph, constraints and connections for guide, the net of index `i` in `sinks`
/// being guide's net `i`, whose terminal `k` is the sink pin `sinks[i][k - 1]`, and each of
/// `aliases` a sink pin that arrives when another does.
#[allow(clippy::too_many_arguments)]
pub fn driven(
    netlist: &Netlist,
    graph: &RoutingGraph,
    placement: &Placement,
    timing: &PlaceTiming,
    drivers: &[usize],
    sinks: &[Vec<usize>],
    aliases: &[(usize, usize)],
    delay_cost: f64,
) -> Result<TimingDriven, String> {
    let estimate = reticle::fpga::place::estimated_wire(netlist, graph, placement, timing);
    let pairs: Vec<(usize, usize)> = drivers
        .iter()
        .zip(sinks)
        .flat_map(|(driver, pins)| pins.iter().map(|pin| (*driver, *pin)))
        .collect();
    let (timing_graph, arcs, constraints) = graph_of(netlist, timing, &pairs, aliases, |index| {
        ps(estimate.get(pairs[index].1).copied().unwrap_or(0.0))
    })?;
    let mut connections = Vec::with_capacity(arcs.len());
    let mut at = 0;
    for (net, pins) in sinks.iter().enumerate() {
        for k in 0..pins.len() {
            connections.push(Connection {
                net: NetId(net as u32),
                terminal: k as u32 + 1,
                arc: arcs[at],
            });
            at += 1;
        }
    }
    let cost = guide::cost::Cost::new(delay_cost).ok_or("a delay cost that is not a cost")?;
    Ok(TimingDriven::new(
        timing_graph,
        constraints,
        connections,
        cost,
    ))
}

/// The latest path by guide's analysis of this model, with every routed signal's connection
/// at `wire` (nanoseconds by sink pin, as `route::connection_delays` gives it), in
/// picoseconds, and the worst slack: for checking the model against reticle's own analysis.
pub fn latest(
    netlist: &Netlist,
    timing: &PlaceTiming,
    wire: &[f64],
) -> Result<(u64, Option<i64>), String> {
    let pairs: Vec<(usize, usize)> = netlist
        .routable()
        .into_iter()
        .flat_map(|signal| {
            let s = &netlist.signals[signal];
            let driver = s.driver.unwrap_or(0);
            s.sinks.iter().map(move |sink| (driver, *sink))
        })
        .collect();
    let (graph, _, constraints) = graph_of(netlist, timing, &pairs, &[], |index| {
        ps(wire.get(pairs[index].1).copied().unwrap_or(0.0))
    })?;
    let mut analysis = TimingAnalysis::new(&graph);
    analysis.run(&graph, &constraints);
    Ok((
        analysis.critical_path().map_or(0, |path| path.arrival()),
        analysis.worst_slack(),
    ))
}
