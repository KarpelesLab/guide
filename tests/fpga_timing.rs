//! From VTR's files to a timing-driven route and back, with no glue in between.
//!
//! `mult_4x4`'s netlist and placement are VPR's own, and so are the delays inside its blocks,
//! read from `k6_N10_40nm.xml`, which `mult_4x4` was packed for. The device graph isn't in
//! VTR's repository, since VPR builds it when it runs, so this generates one in VPR's format:
//! the same grid, tile types and pin names as that architecture, with channels, switch boxes
//! and routing delays of its own. None of the routing's numbers come from the architecture.

#![cfg(feature = "fpga")]

use core::fmt::Write as _;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use guide::engine::{EngineOptions, Outcome, route_timed, route_timed_parallel};
use guide::fpga::{
    Design, DesignTiming, LogicDelays, LookaheadOptions, NetOptions, NodeKind, RouteHeader,
    RrGraph, SequentialDelays, TimingOptions, build_timing, read_architecture, read_netlist,
    read_placement, read_rr_graph, resolve_nets, write_route,
};
use guide::id::NodeId;
use guide::net::Net;
use guide::space::DelaySpace;
use guide::timing::{Constraints, CriticalityOptions, TimingAnalysis};
use guide::validate::check;

const NET: &str = include_str!("../testdata/vtr/mult_4x4.net");
const PLACE: &str = include_str!("../testdata/vtr/mult_4x4.place");
const ARCH: &str = include_str!("../testdata/vtr/k6_N10_40nm.xml");

/// The tile types of `k6_N10_40nm.xml` as VPR writes them in a graph: an `io` is 8
/// positions of `outpad`, `inpad` and `clock`, each its own class, and a `clb` has 40
/// equivalent inputs, 10 equivalent outputs and a clock, in classes 0, 1 and 2.
fn k6_block_types() -> String {
    let mut xml = String::from(
        "<block_types>\n<block_type id=\"0\" name=\"EMPTY\" width=\"1\" height=\"1\"/>\n\
         <block_type id=\"1\" name=\"io\" width=\"1\" height=\"1\">\n",
    );
    let mut ptc = 0;
    for instance in 0..8 {
        for (kind, port) in [("INPUT", "outpad"), ("OUTPUT", "inpad"), ("INPUT", "clock")] {
            writeln!(
                xml,
                "<pin_class type=\"{kind}\"><pin ptc=\"{ptc}\">io[{instance}].{port}[0]</pin></pin_class>"
            )
            .unwrap();
            ptc += 1;
        }
    }
    xml.push_str(
        "</block_type>\n<block_type id=\"2\" name=\"clb\" width=\"1\" height=\"1\">\n\
         <pin_class type=\"INPUT\">",
    );
    for bit in 0..40 {
        write!(xml, "<pin ptc=\"{bit}\">clb.I[{bit}]</pin>").unwrap();
    }
    xml.push_str("</pin_class>\n<pin_class type=\"OUTPUT\">");
    for bit in 0..10 {
        write!(xml, "<pin ptc=\"{}\">clb.O[{bit}]</pin>", bit + 40).unwrap();
    }
    xml.push_str(
        "</pin_class>\n<pin_class type=\"INPUT\"><pin ptc=\"50\">clb.clk[0]</pin></pin_class>\n\
         </block_type>\n</block_types>\n",
    );
    xml
}

/// Which way a pin faces, by the channel it reaches.
#[derive(Clone, Copy)]
enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

/// Writes an island-style device of `size` by `size` tiles in VPR's `rr_graph` format, with
/// `tracks` wires of length 1 in each channel.
///
/// Wires meet in disjoint switch boxes, in both directions. A CLB's inputs each reach half
/// the tracks on one side, and its outputs half the tracks on every side. A pad reaches every
/// track of the channel beside it. Clock pins reach nothing, as with VPR's default ideal
/// clock. Delays: 60 ps through a switch box or out of a pin, 40 ps into a pin, and 5 ps for
/// each wire's own resistance and capacitance.
fn island_device(size: u32, tracks: u32) -> String {
    let mut nodes = String::new();
    let mut edges = String::new();
    let mut next = 0u32;
    // Every node, wires included, covers a single tile.
    let mut node = |nodes: &mut String,
                    kind: &str,
                    (x, y): (u32, u32),
                    ptc: u32,
                    capacity: u32,
                    wire: bool| {
        let id = next;
        next += 1;
        let extra = if wire {
            "<timing R=\"100\" C=\"1e-13\"/><segment segment_id=\"0\"/>"
        } else {
            ""
        };
        writeln!(
            nodes,
            "<node id=\"{id}\" type=\"{kind}\" capacity=\"{capacity}\"><loc xlow=\"{x}\" ylow=\"{y}\" xhigh=\"{x}\" yhigh=\"{y}\" ptc=\"{ptc}\"/>{extra}</node>"
        )
        .unwrap();
        id
    };
    let edge = |edges: &mut String, from: u32, to: u32, switch: u32| {
        writeln!(
            edges,
            "<edge src_node=\"{from}\" sink_node=\"{to}\" switch_id=\"{switch}\"/>"
        )
        .unwrap();
    };

    // Channels: CHANX at (x, y) runs above tile (x, y), and CHANY at (x, y) to its right.
    let last = size - 1;
    let mut chanx: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    let mut chany: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for y in 0..last {
        for x in 1..last {
            let wires = (0..tracks)
                .map(|t| node(&mut nodes, "CHANX", (x, y), t, 1, true))
                .collect();
            chanx.insert((x, y), wires);
        }
    }
    for x in 0..last {
        for y in 1..last {
            let wires = (0..tracks)
                .map(|t| node(&mut nodes, "CHANY", (x, y), t, 1, true))
                .collect();
            chany.insert((x, y), wires);
        }
    }

    // Switch boxes, at the corner above and to the right of each tile.
    for x in 0..last {
        for y in 0..last {
            let around: Vec<&Vec<u32>> = [
                chanx.get(&(x, y)),
                chanx.get(&(x + 1, y)),
                chany.get(&(x, y)),
                chany.get(&(x, y + 1)),
            ]
            .into_iter()
            .flatten()
            .collect();
            for a in &around {
                for b in &around {
                    if a[0] != b[0] {
                        for t in 0..tracks as usize {
                            edge(&mut edges, a[t], b[t], 1);
                        }
                    }
                }
            }
        }
    }
    let channel = |x: u32, y: u32, side: Side| -> Option<&Vec<u32>> {
        match side {
            Side::Top => chanx.get(&(x, y)),
            Side::Bottom => chanx.get(&(x, y.checked_sub(1)?)),
            Side::Right => chany.get(&(x, y)),
            Side::Left => chany.get(&(x.checked_sub(1)?, y)),
        }
    };
    const SIDES: [Side; 4] = [Side::Top, Side::Right, Side::Bottom, Side::Left];

    let mut grid = String::from("<grid>\n");
    for y in 0..size {
        for x in 0..size {
            let on_edge_x = x == 0 || x == last;
            let on_edge_y = y == 0 || y == last;
            let tile = match (on_edge_x, on_edge_y) {
                (true, true) => 0,
                (false, false) => 2,
                _ => 1,
            };
            writeln!(
                grid,
                "<grid_loc x=\"{x}\" y=\"{y}\" block_type_id=\"{tile}\" width_offset=\"0\" height_offset=\"0\"/>"
            )
            .unwrap();
            if tile == 1 {
                let side = if x == 0 {
                    Side::Right
                } else if x == last {
                    Side::Left
                } else if y == 0 {
                    Side::Top
                } else {
                    Side::Bottom
                };
                let wires = channel(x, y, side).unwrap().clone();
                // Per position: outpad (in), inpad (out), clock (in), each a class.
                for class in 0..24 {
                    let input = class % 3 != 1;
                    let at = (x, y);
                    if input {
                        let sink = node(&mut nodes, "SINK", at, class, 1, false);
                        let ipin = node(&mut nodes, "IPIN", at, class, 1, false);
                        edge(&mut edges, ipin, sink, 0);
                        if class % 3 == 0 {
                            for &wire in &wires {
                                edge(&mut edges, wire, ipin, 2);
                            }
                        }
                    } else {
                        let source = node(&mut nodes, "SOURCE", at, class, 1, false);
                        let opin = node(&mut nodes, "OPIN", at, class, 1, false);
                        edge(&mut edges, source, opin, 0);
                        for &wire in &wires {
                            edge(&mut edges, opin, wire, 1);
                        }
                    }
                }
            } else if tile == 2 {
                let at = (x, y);
                let sink = node(&mut nodes, "SINK", at, 0, 40, false);
                let source = node(&mut nodes, "SOURCE", at, 1, 10, false);
                let clock = node(&mut nodes, "SINK", at, 2, 1, false);
                for pin in 0..40 {
                    let ipin = node(&mut nodes, "IPIN", at, pin, 1, false);
                    edge(&mut edges, ipin, sink, 0);
                    let side = SIDES[pin as usize % 4];
                    if let Some(wires) = channel(x, y, side) {
                        for (t, &wire) in wires.iter().enumerate() {
                            if t as u32 % 2 == (pin / 4) % 2 {
                                edge(&mut edges, wire, ipin, 2);
                            }
                        }
                    }
                }
                for pin in 40..50 {
                    let opin = node(&mut nodes, "OPIN", at, pin, 1, false);
                    edge(&mut edges, source, opin, 0);
                    for side in SIDES {
                        if let Some(wires) = channel(x, y, side) {
                            for (t, &wire) in wires.iter().enumerate() {
                                if t as u32 % 2 == pin % 2 {
                                    edge(&mut edges, opin, wire, 1);
                                }
                            }
                        }
                    }
                }
                let clock_pin = node(&mut nodes, "IPIN", at, 50, 1, false);
                edge(&mut edges, clock_pin, clock, 0);
            }
        }
    }
    grid.push_str("</grid>\n");
    let block_types = k6_block_types();

    format!(
        "<rr_graph tool_name=\"guide test\">\n<switches>\n\
         <switch id=\"0\" name=\"__vpr_delayless_switch__\" type=\"mux\"><timing Tdel=\"0\"/></switch>\n\
         <switch id=\"1\" name=\"sb\" type=\"mux\"><timing R=\"0\" Cin=\"0\" Cout=\"0\" Tdel=\"6e-11\"/></switch>\n\
         <switch id=\"2\" name=\"ipin_cblock\" type=\"mux\"><timing R=\"0\" Cin=\"0\" Cout=\"0\" Tdel=\"4e-11\"/></switch>\n\
         </switches>\n<segments><segment id=\"0\" name=\"L1\" length=\"1\"/></segments>\n\
         {block_types}{grid}<rr_nodes>\n{nodes}</rr_nodes>\n<rr_edges>\n{edges}</rr_edges>\n</rr_graph>\n"
    )
}

/// The delays inside the blocks, from `k6_N10_40nm.xml` itself: the very file that VPR packed
/// `mult_4x4` against, as the netlist's `architecture_id` says.
fn test_delays() -> LogicDelays {
    let arch = read_architecture(ARCH.as_bytes()).unwrap();
    let delays = arch.into_delays();
    assert_eq!(
        delays.sequential("ff"),
        Some(SequentialDelays {
            clock_to_q: 124,
            setup: 66,
        })
    );
    delays
}

/// Everything read from the files, and the timing built from them.
struct Flow {
    device: RrGraph,
    design: Design,
    timing: DesignTiming,
}

fn flow(tracks: u32) -> Flow {
    let device = read_rr_graph(island_device(4, tracks).as_bytes()).unwrap();
    let netlist = read_netlist(NET.as_bytes()).unwrap();
    let placement = read_placement(PLACE.as_bytes()).unwrap();
    let design = resolve_nets(&device, &netlist, &placement, &NetOptions::default()).unwrap();
    assert!(design.shared_sink_nets().is_empty());
    let mut timing = build_timing(&netlist, &test_delays(), &TimingOptions::default()).unwrap();
    // Every delay that the design needed was given.
    assert_eq!(timing.report().fallbacks, []);
    timing.estimate_delays(&device.timed_graph(), &design);
    Flow {
        device,
        design,
        timing,
    }
}

/// Adds up the delay of each step from the root to each node, from the switches in the
/// device, without the engine's help.
fn tree_delays(device: &RrGraph, outcome: &Outcome<NodeId, NodeId>, flow: &Flow) -> Vec<u32> {
    let mut delays = Vec::new();
    for net in flow.design.nets() {
        let route = outcome.routes.route(net.id).unwrap();
        let mut at: HashMap<NodeId, u32> = HashMap::new();
        at.insert(route.root().unwrap(), 0);
        // Edges come after the edge into their start, as the engine grows routes.
        for edge in route.edges() {
            let switch = device.edge_switch(edge.from, edge.to).unwrap();
            let delay = at[&edge.from] + device.step_delay(switch, edge.to);
            at.insert(edge.to, delay);
        }
        for sink in &net.terminals[1..] {
            let fastest = sink
                .access_points()
                .iter()
                .filter_map(|point| at.get(&point.node))
                .min()
                .unwrap();
            delays.push(*fastest);
        }
    }
    delays
}

/// Analyses a copy of the timing graph with the delays of a run's routes, and returns its
/// critical-path delay.
fn critical_path_delay(
    timing: &DesignTiming,
    design: &Design,
    outcome: &Outcome<NodeId, NodeId>,
) -> u64 {
    let mut timing = timing.clone();
    let connections = timing.connections(design);
    let delays = &outcome.timing.as_ref().unwrap().connection_delays;
    timing.set_connection_delays(&connections, delays);
    let mut analysis = TimingAnalysis::new(timing.graph());
    analysis.run(timing.graph(), &Constraints::single_clock(0).unwrap());
    analysis.critical_path().unwrap().arrival()
}

#[test]
fn mult_4x4_routes_timing_driven_from_vtr_files() {
    let flow = flow(8);
    let timed = flow.device.timed_graph();
    // No SDC: as VPR does without one, ask for the fastest circuit.
    let constraints = Constraints::single_clock(0).unwrap();
    let driven = flow.timing.timing_driven(&flow.design, constraints.clone());
    let connections = driven.connections.clone();
    assert_eq!(
        connections.len(),
        flow.design
            .nets()
            .iter()
            .map(|net| net.terminals.len() - 1)
            .sum::<usize>()
    );
    let outcome =
        route_timed(&timed, flow.design.nets(), EngineOptions::default(), driven).unwrap();
    assert!(outcome.is_success(), "{:?}", outcome.stop);

    // The validator agrees.
    let report = check(&timed, flow.design.nets(), &outcome.routes);
    assert!(report.is_complete(), "{:?}", report.faults());
    assert!(report.faults().is_empty());

    // Each net leaves its block through a single output pin, as VPR requires of a CLB's
    // outputs, which are equivalent by instance in `k6_N10_40nm.xml`.
    let mut lifted = 0;
    for net in flow.design.nets() {
        let route = outcome.routes.route(net.id).unwrap();
        let opins = route
            .nodes()
            .filter(|&node| flow.device.node_kind(node) == Some(NodeKind::Opin))
            .count();
        assert_eq!(opins, 1, "net {}", net.id.0);
        if let Some(source) = flow.design.driver_source(net.id) {
            lifted += 1;
            let root = route.root().unwrap();
            assert_eq!(flow.device.node_kind(root), Some(NodeKind::Opin));
            assert!(flow.device.edge_switch(source, root).is_some());
        }
    }
    assert!(lifted > 0);

    // The engine's delays are the device's, step by step.
    let timing = outcome.timing.as_ref().unwrap();
    let delays: Vec<u32> = timing
        .connection_delays
        .iter()
        .map(|d| d.unwrap())
        .collect();
    assert_eq!(delays, tree_delays(&flow.device, &outcome, &flow));

    // The critical path through the blocks matches what the engine found.
    let critical = critical_path_delay(&flow.timing, &flow.design, &outcome);
    assert_eq!(Some(critical), timing.metrics.critical_path_delay);

    // And a route file comes out.
    let header = RouteHeader {
        placement_file: "mult_4x4.place",
        placement_id: "SHA256:0",
    };
    let mut out = Vec::new();
    let summary = write_route(
        &mut out,
        &flow.device,
        &flow.design,
        &outcome.routes,
        &header,
    )
    .unwrap();
    assert_eq!(summary.routed as usize, flow.design.nets().len());
    assert_eq!(summary.unrouted, 0);
    let text = String::from_utf8(out).unwrap();
    assert!(text.starts_with("Placement_File: mult_4x4.place Placement_ID: SHA256:0\n"));
    assert_eq!(
        text.matches("\nNet ").count(),
        flow.design.net_infos().len()
    );

    // The same on any number of threads.
    let threaded = route_timed_parallel(
        &timed,
        flow.design.nets(),
        EngineOptions::default(),
        flow.timing.timing_driven(&flow.design, constraints),
        4,
    )
    .unwrap();
    assert_eq!(threaded.routes, outcome.routes);
    assert_eq!(threaded.timing, outcome.timing);
}

#[test]
fn mult_4x4_routes_with_the_map_lookahead_and_the_graphs_options() {
    let flow = flow(8);
    let constraints = Constraints::single_clock(0).unwrap();
    let driven = || flow.timing.timing_driven(&flow.design, constraints.clone());
    let nets = flow.design.nets();
    let plain = route_timed(
        &flow.device.timed_graph(),
        nets,
        flow.device.engine_options(),
        driven(),
    )
    .unwrap();
    assert!(plain.is_success());

    let mut device = flow.device.clone();
    device.use_map_lookahead(&LookaheadOptions::default());
    let timed = device.timed_graph();
    let options = device.engine_options();
    let outcome = route_timed(&timed, nets, options, driven()).unwrap();
    assert!(outcome.is_success(), "{:?}", outcome.stop);
    let report = check(&timed, nets, &outcome.routes);
    assert!(report.is_complete(), "{:?}", report.faults());
    assert!(report.faults().is_empty());
    assert!(
        outcome.routes.stats.nodes_expanded < plain.routes.stats.nodes_expanded,
        "{} against {}",
        outcome.routes.stats.nodes_expanded,
        plain.routes.stats.nodes_expanded
    );
    // The engine's delays are still the device's, step by step.
    let delays: Vec<u32> = outcome
        .timing
        .as_ref()
        .unwrap()
        .connection_delays
        .iter()
        .map(|d| d.unwrap())
        .collect();
    assert_eq!(delays, tree_delays(&device, &outcome, &flow));

    // The same on any number of threads, in batches too.
    let mut batched = options;
    batched.batch_size = 4;
    let one = route_timed(&timed, nets, batched, driven()).unwrap();
    let four = route_timed_parallel(&timed, nets, batched, driven(), 4).unwrap();
    assert_eq!(one.routes, four.routes);
    assert_eq!(one.timing, four.timing);
}

/// The smallest delay from any access point of `from` to any of `to`, ignoring every other
/// net: Dijkstra's algorithm on step delays.
fn fastest(timed: &impl DelaySpace<Node = NodeId>, net: &Net<NodeId>, sink: usize) -> u64 {
    let mut best: HashMap<NodeId, u64> = HashMap::new();
    let mut heap = BinaryHeap::new();
    for point in net.terminals[0].access_points() {
        best.insert(point.node, 0);
        heap.push(Reverse((0u64, point.node.0)));
    }
    let targets: Vec<NodeId> = net.terminals[sink]
        .access_points()
        .iter()
        .map(|point| point.node)
        .collect();
    let (mut steps, mut delays) = (Vec::new(), Vec::new());
    while let Some(Reverse((delay, node))) = heap.pop() {
        let node = NodeId(node);
        if targets.contains(&node) {
            return delay;
        }
        if best.get(&node).is_some_and(|&known| known < delay) {
            continue;
        }
        steps.clear();
        delays.clear();
        timed.expand(net.id, node, &mut steps);
        timed.step_delays(net.id, node, &mut delays);
        for (step, &step_delay) in steps.iter().zip(&delays) {
            let reached = delay + u64::from(step_delay);
            if best.get(&step.to).is_none_or(|&known| reached < known) {
                best.insert(step.to, reached);
                heap.push(Reverse((reached, step.to.0)));
            }
        }
    }
    panic!("net {} can't reach sink {sink}", net.id.0);
}

/// Routes `mult_4x4` with and without timing on devices of several channel widths, and
/// prints the critical paths. Run it with `--nocapture` to see them.
///
/// Two references go with each: the critical path from the delay lower bounds, which is what
/// the router's estimates start from, and the one with every connection as fast as it could
/// be if no other net were there, which no legal routing can beat.
///
/// The third run starts timing-driven routing from those best delays in place of the lower
/// bounds, to see whether better first estimates matter.
///
/// This only asserts what must hold: every run is legal and no faster than the second
/// reference. `docs/domains.md` has what it printed.
#[test]
fn measure_timing_on_mult_4x4() {
    for tracks in [8, 10, 12, 16, 20] {
        let flow = flow(tracks);
        let timed = flow.device.timed_graph();
        let constraints = Constraints::single_clock(0).unwrap();
        let mut analysis = TimingAnalysis::new(flow.timing.graph());
        analysis.run(flow.timing.graph(), &constraints);
        let bound = analysis.critical_path().unwrap().arrival();

        let mut alone = flow.timing.clone();
        let connections = alone.connections(&flow.design);
        let delays: Vec<Option<u32>> = connections
            .iter()
            .map(|connection| {
                let net = flow
                    .design
                    .nets()
                    .iter()
                    .find(|net| net.id == connection.net)
                    .unwrap();
                Some(fastest(&timed, net, connection.terminal as usize) as u32)
            })
            .collect();
        alone.set_connection_delays(&connections, &delays);
        analysis.run(alone.graph(), &constraints);
        let best = analysis.critical_path().unwrap().arrival();

        let mut line = format!("{tracks} tracks: bound {bound} ps, best alone {best} ps");
        let runs = [
            ("without timing", &flow.timing, 0.0),
            ("timing-driven", &flow.timing, 0.99),
            ("timing-driven from the best alone", &alone, 0.99),
        ];
        for (label, start, max_criticality) in runs {
            let mut driven = start.timing_driven(&flow.design, constraints.clone());
            // A maximum of 0 makes every connection uncritical, which routes exactly as
            // without timing but still measures the result.
            driven.criticality = CriticalityOptions::new(1, max_criticality).unwrap();
            let outcome =
                route_timed(&timed, flow.design.nets(), EngineOptions::default(), driven).unwrap();
            assert!(outcome.is_success(), "{tracks} tracks: {:?}", outcome.stop);
            let report = check(&timed, flow.design.nets(), &outcome.routes);
            assert!(report.is_complete());
            let delay = critical_path_delay(&flow.timing, &flow.design, &outcome);
            assert!(delay >= best && best >= bound);
            write!(
                line,
                "; {label} {delay} ps, cost {:.4e}, {} iterations",
                report.summary().total_cost.get(),
                outcome.routes.stats.iterations
            )
            .unwrap();
        }
        eprintln!("{line}");
    }
}
