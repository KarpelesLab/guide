//! Reticle's timing model of a placed design (`place::PlaceTiming`, what `--timing-driven`
//! places and routes by) as a guide [`TimingGraph`], so that guide can route by it.
//!
//! A point is a netlist pin. An instance's `Through` inputs reach its `Through` outputs
//! through a cell arc of its `cell` delay (only the outputs `reaches` says, for a table
//! holding two functions); a `Start` output launches paths `launch` after its clock, its
//! clock's arrival on its site (`skew`) included; an `End` input captures them `capture`
//! before its clock's, its arrival again included. Each domain is a clock with its own period,
//! and paths between two are not timed, as reticle's analysis has it. A connection's net arc
//! starts at reticle's estimate of it from the placement (`place::estimated_wire`) and the
//! router replaces it.
//!
//! What is left out, so that guide's analysis is only close to reticle's: single pins'
//! delays (`pin_delays`), multicycle and false paths (`cycles`, `untimed_*`), the margin on a
//! path between two clock regions, a pad's paths in every domain (they are put in the first),
//! and a branch's load on the connections past it. Results are timed by reticle's own
//! analysis afterwards in any case.

use guide::engine::{Connection, TimingDriven};
use guide::id::NetId;
use guide::timing::{ClockId, Constraints, PointId, TimingGraphBuilder};
use reticle::fpga::arch::RoutingGraph;
use reticle::fpga::place::{Netlist, PathDomain, PinTiming, PlaceTiming, Placement};

fn ps(ns: f64) -> u32 {
    if ns.is_finite() && ns > 0.0 {
        (ns * 1000.0).round().min(f64::from(u32::MAX)) as u32
    } else {
        0
    }
}

/// The timing graph, constraints and connections for guide, the net of index `i` in `sinks`
/// being guide's net `i`, whose terminal `k` is the sink pin `sinks[i][k - 1]`.
pub fn driven(
    netlist: &Netlist,
    graph: &RoutingGraph,
    placement: &Placement,
    timing: &PlaceTiming,
    drivers: &[usize],
    sinks: &[Vec<usize>],
    delay_cost: f64,
) -> Result<TimingDriven, String> {
    let err = |e: guide::timing::TimingError| format!("timing graph: {e}");
    let mut builder = TimingGraphBuilder::with_capacity(netlist.pins.len(), netlist.pins.len() * 2);
    let mut points: Vec<PointId> = Vec::with_capacity(netlist.pins.len());
    for _ in &netlist.pins {
        points.push(builder.add_point().map_err(err)?);
    }
    let domain_of = |pin: usize, instance: usize| -> Option<ClockId> {
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
            PathDomain::Untimed => None,
            PathDomain::Clock(c) => Some(ClockId(c as u32)),
            PathDomain::Every => Some(ClockId(0)),
        }
    };
    let skew = |instance: usize| {
        timing
            .skew
            .get(instance)
            .copied()
            .filter(|s| s.is_finite())
            .unwrap_or(0.0)
    };
    for (instance, inst) in netlist.instances.iter().enumerate() {
        let role = |pin: usize| timing.pins.get(pin).copied().unwrap_or(PinTiming::Ignored);
        let outputs: Vec<usize> = inst
            .pins
            .iter()
            .copied()
            .filter(|p| netlist.pins[*p].output && role(*p) == PinTiming::Through)
            .collect();
        for &pin in &inst.pins {
            match role(pin) {
                PinTiming::Through if !netlist.pins[pin].output => {
                    let reach = timing.reaches.get(pin).copied().unwrap_or(u8::MAX);
                    for (j, out) in outputs.iter().enumerate() {
                        if j >= 8 || reach >> j & 1 != 0 {
                            builder
                                .add_cell_arc(points[pin], points[*out], ps(timing.cell[instance]))
                                .map_err(err)?;
                        }
                    }
                }
                PinTiming::Start => {
                    if timing.untimed_from.get(instance).copied().unwrap_or(false) {
                        continue;
                    }
                    if let Some(clock) = domain_of(pin, instance) {
                        let launch =
                            timing.launch.get(instance).copied().unwrap_or(0.0) + skew(instance);
                        builder
                            .set_launch_on(points[pin], clock, ps(launch))
                            .map_err(err)?;
                    }
                }
                PinTiming::End => {
                    if timing.untimed_to.get(instance).copied().unwrap_or(false) {
                        continue;
                    }
                    if let Some(clock) = domain_of(pin, instance) {
                        // Arriving later, the clock gives the path that much more time: a
                        // margin that much less, never below nothing.
                        let margin =
                            timing.capture.get(instance).copied().unwrap_or(0.0) - skew(instance);
                        builder
                            .set_capture_on(points[pin], clock, ps(margin))
                            .map_err(err)?;
                    }
                }
                _ => {}
            }
        }
    }
    let estimate = reticle::fpga::place::estimated_wire(netlist, graph, placement, timing);
    let mut connections = Vec::new();
    for (net, (driver, pins)) in drivers.iter().zip(sinks).enumerate() {
        for (k, pin) in pins.iter().enumerate() {
            let arc = builder
                .add_net_arc(
                    points[*driver],
                    points[*pin],
                    ps(estimate.get(*pin).copied().unwrap_or(0.0)),
                )
                .map_err(err)?;
            connections.push(Connection {
                net: NetId(net as u32),
                terminal: k as u32 + 1,
                arc,
            });
        }
    }
    let periods: Vec<(ClockId, u64)> = timing
        .periods
        .iter()
        .enumerate()
        .map(|(c, ns)| (ClockId(c as u32), u64::from(ps(*ns)).max(1)))
        .collect();
    let constraints = if periods.is_empty() {
        Constraints::single_clock(1_000_000).map_err(err)?
    } else {
        Constraints::per_clock(&periods).map_err(err)?
    };
    let cost = guide::cost::Cost::new(delay_cost).ok_or("a delay cost that is not a cost")?;
    Ok(TimingDriven::new(
        builder.build(),
        constraints,
        connections,
        cost,
    ))
}
