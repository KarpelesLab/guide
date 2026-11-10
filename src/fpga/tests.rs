//! Tests that cross the formats, and those against the genuine samples in `testdata/vtr`.

use core::fmt::Write as _;
use std::collections::HashMap;
use std::io::{BufReader, Read};

use proptest::prelude::*;

use super::*;
use crate::cost::Cost;
use crate::id::{NetId, NodeId};
use crate::net::{Edge, Route, RouteSet, Terminal};
use crate::space::graph::Graph;
use crate::space::{DelaySpace, RoutingSpace};

const NET: &str = include_str!("../../testdata/vtr/mult_4x4.net");
const PLACE: &str = include_str!("../../testdata/vtr/mult_4x4.place");
const ROUTE: &str = include_str!("../../testdata/vtr/mult_4x4.route");
const Z1000: &str = include_str!("../../testdata/vtr/z1000_corner.rr_graph.xml");

/// A node as a `.route` file describes it.
#[derive(Clone, Debug)]
struct RouteNode {
    kind: String,
    low: (u32, u32),
    high: (u32, u32),
    ptc: u32,
    switch: i32,
    net_pin_index: Option<u32>,
}

/// A net as a `.route` file describes it.
#[derive(Clone, Debug, Default)]
struct RouteNet {
    id: u32,
    name: String,
    lines: Vec<(u32, RouteNode)>,
}

/// Reads the parts of a `.route` that the tests compare against. This is deliberately not
/// sharing anything with the code it checks, and is only meant for files that VPR wrote.
fn parse_route(text: &str) -> Vec<RouteNet> {
    let coordinate = |token: &str| -> (u32, u32) {
        let inner = token.trim_start_matches('(').trim_end_matches(')');
        let mut parts = inner.split(',').map(|part| part.parse().unwrap());
        (parts.next().unwrap(), parts.next().unwrap())
    };
    let mut nets: Vec<RouteNet> = Vec::new();
    for line in text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        match tokens.first() {
            Some(&"Net") => nets.push(RouteNet {
                id: tokens[1].parse().unwrap(),
                name: tokens[2][1..tokens[2].len() - 1].to_owned(),
                lines: Vec::new(),
            }),
            Some(&"Node:") => {
                let kind = tokens[2].to_owned();
                let low = coordinate(tokens[3]);
                let (high, rest) = if tokens[4] == "to" {
                    (coordinate(tokens[5]), &tokens[6..])
                } else {
                    (low, &tokens[4..])
                };
                let at = |label: &str| rest.iter().position(|&token| token == label);
                let switch = rest[at("Switch:").unwrap() + 1].parse().unwrap();
                let net_pin_index = at("Net_pin_index:").map(|at| rest[at + 1].parse().unwrap());
                nets.last_mut().unwrap().lines.push((
                    tokens[1].parse().unwrap(),
                    RouteNode {
                        kind,
                        low,
                        high,
                        ptc: rest[1].parse().unwrap(),
                        switch,
                        net_pin_index,
                    },
                ));
            }
            _ => {}
        }
    }
    nets
}

/// Turns a net's lines back into a tree: a line after a `SINK` repeats a node of the tree.
fn route_from_lines(net: &RouteNet) -> Route<NodeId> {
    let root = NodeId(net.lines[0].0);
    let mut edges = Vec::new();
    for pair in net.lines.windows(2) {
        let (from, node) = &pair[0];
        if node.kind != "SINK" {
            edges.push(Edge {
                from: NodeId(*from),
                to: NodeId(pair[1].0),
            });
        }
    }
    Route::from_edges(root, edges)
}

/// The route of a net's lines as the engine would have it: rooted on the `OPIN` if the
/// design's driver is on its `OPIN`s, with the `SOURCE` that VPR writes first left out.
fn engine_route(design: &Design, net: &RouteNet) -> Route<NodeId> {
    let route = route_from_lines(net);
    let Some(source) = design.driver_source(NetId(net.id)) else {
        return route;
    };
    assert_eq!(route.root(), Some(source));
    let edges: Vec<Edge<NodeId>> = route
        .edges()
        .iter()
        .copied()
        .filter(|edge| edge.from != source)
        .collect();
    Route::from_edges(NodeId(net.lines[1].0), edges)
}

/// Builds the architecture's `block_types` and the grid for `mult_4x4`, which used
/// `vtr_flow/arch/timing/k6_N10_40nm.xml`, as VPR would write them.
///
/// The pin order is from that file: a `clb` has 40 equivalent inputs, then 10 equivalent
/// outputs, then a clock, and an `io` is 8 positions of `outpad`, `inpad` and `clock`, each
/// in its own class.
fn k6_n10_block_types_and_grid(width: u32, height: u32) -> String {
    let mut xml = String::from(
        "<block_types>\n<block_type id=\"0\" name=\"EMPTY\" width=\"1\" height=\"1\"/>\n",
    );
    xml.push_str("<block_type id=\"1\" name=\"io\" width=\"1\" height=\"1\">\n");
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
    xml.push_str("</block_type>\n<block_type id=\"2\" name=\"clb\" width=\"1\" height=\"1\">\n");
    xml.push_str("<pin_class type=\"INPUT\">");
    for bit in 0..40 {
        write!(xml, "<pin ptc=\"{bit}\">clb.I[{bit}]</pin>").unwrap();
    }
    xml.push_str("</pin_class>\n<pin_class type=\"OUTPUT\">");
    for bit in 0..10 {
        write!(xml, "<pin ptc=\"{}\">clb.O[{bit}]</pin>", bit + 40).unwrap();
    }
    xml.push_str(
        "</pin_class>\n<pin_class type=\"INPUT\"><pin ptc=\"50\">clb.clk[0]</pin></pin_class>\n",
    );
    xml.push_str("</block_type>\n</block_types>\n<grid>\n");
    for y in 0..height {
        for x in 0..width {
            let on_edge_x = x == 0 || x == width - 1;
            let on_edge_y = y == 0 || y == height - 1;
            let block_type = match (on_edge_x, on_edge_y) {
                (true, true) => 0,
                (false, false) => 2,
                _ => 1,
            };
            writeln!(
                xml,
                "<grid_loc x=\"{x}\" y=\"{y}\" block_type_id=\"{block_type}\" width_offset=\"0\" height_offset=\"0\"/>"
            )
            .unwrap();
        }
    }
    xml.push_str("</grid>\n");
    xml
}

/// Builds a graph with just the nodes and edges that a route file mentions, with the
/// node numbers and switches it uses, so that its route is a route on the result.
///
/// The real graph isn't in VTR's repository, as VPR generates it when it runs. What this
/// keeps from it is only what the route shows, which is enough to test that route and the
/// netlist and placement it came from against each other.
fn graph_from_route(nets: &[RouteNet], block_types_and_grid: &str) -> String {
    let mut nodes: HashMap<u32, RouteNode> = HashMap::new();
    let mut edges: Vec<(u32, u32, i32)> = Vec::new();
    let mut highest = 0;
    for net in nets {
        for pair in net.lines.windows(2) {
            let (from, node) = &pair[0];
            if node.kind != "SINK" {
                edges.push((*from, pair[1].0, node.switch));
            }
        }
        for (id, node) in &net.lines {
            nodes.insert(*id, node.clone());
            highest = highest.max(*id);
        }
    }

    let mut xml = String::from("<rr_graph>\n<switches>\n");
    for id in 0..3 {
        writeln!(
            xml,
            "<switch id=\"{id}\" name=\"s{id}\" type=\"mux\"><timing Tdel=\"{}e-11\"/></switch>",
            id + 1
        )
        .unwrap();
    }
    xml.push_str(
        "</switches>\n<segments><segment id=\"0\" name=\"L4\" length=\"4\"/></segments>\n",
    );
    xml.push_str(block_types_and_grid);
    xml.push_str("<rr_nodes>\n");
    for id in 0..=highest {
        match nodes.get(&id) {
            Some(node) => {
                let segment = if node.kind.starts_with("CHAN") {
                    "<segment segment_id=\"0\"/>"
                } else {
                    ""
                };
                writeln!(
                    xml,
                    "<node id=\"{id}\" type=\"{}\" capacity=\"1\"><loc xlow=\"{}\" ylow=\"{}\" xhigh=\"{}\" yhigh=\"{}\" ptc=\"{}\"/>{segment}</node>",
                    node.kind, node.low.0, node.low.1, node.high.0, node.high.1, node.ptc
                )
                .unwrap();
            }
            // A filler, as IDs have to be gapless. Its class doesn't exist, so nothing finds it.
            None => writeln!(
                xml,
                "<node id=\"{id}\" type=\"SINK\" capacity=\"1\"><loc xlow=\"0\" ylow=\"0\" xhigh=\"0\" yhigh=\"0\" ptc=\"{}\"/></node>",
                1000 + id
            )
            .unwrap(),
        }
    }
    xml.push_str("</rr_nodes>\n<rr_edges>\n");
    for (from, to, switch) in edges {
        writeln!(
            xml,
            "<edge src_node=\"{from}\" sink_node=\"{to}\" switch_id=\"{switch}\"/>"
        )
        .unwrap();
    }
    xml.push_str("</rr_edges>\n</rr_graph>\n");
    xml
}

/// The parts of the `mult_4x4` flow, with the graph reconstructed from its route.
fn mult_4x4() -> (RrGraph, Design, Vec<RouteNet>) {
    let nets = parse_route(ROUTE);
    let xml = graph_from_route(&nets, &k6_n10_block_types_and_grid(4, 4));
    let device = read_rr_graph(xml.as_bytes()).unwrap();
    let netlist = read_netlist(NET.as_bytes()).unwrap();
    let placement = read_placement(PLACE.as_bytes()).unwrap();
    let design = resolve_nets(&device, &netlist, &placement, &NetOptions::default()).unwrap();
    (device, design, nets)
}

fn placement_id() -> &'static str {
    "SHA256:b98f0e6368789460e510c392c3dd2c9bf4568045801bf6624f890886a41976c5"
}

// VPR's route against what we derive from its netlist and placement.

#[test]
fn nets_match_those_vpr_routed() {
    let (_, design, nets) = mult_4x4();
    assert_eq!(design.net_infos().len(), nets.len());
    for (info, net) in design.net_infos().iter().zip(&nets) {
        assert_eq!(info.name, net.name);
        assert_eq!(info.status, NetStatus::Routable);
    }
    for (net, vpr) in design.nets().iter().zip(&nets) {
        assert_eq!(net.id, NetId(vpr.id));
    }
}

#[test]
fn drivers_and_sinks_match_vprs_nodes_and_pin_indices() {
    let (device, design, nets) = mult_4x4();
    assert_eq!(design.nets().len(), 25);
    let mut sinks_checked = 0;
    let mut lifted = 0;
    for (net, vpr) in design.nets().iter().zip(&nets) {
        // Its route starts at the driver, the first terminal: the SOURCE, or one of the OPINs
        // that it leads to if there are several.
        let driver: Vec<NodeId> = net.terminals[0]
            .access_points()
            .iter()
            .map(|point| point.node)
            .collect();
        match design.driver_source(net.id) {
            None => assert_eq!(driver, [NodeId(vpr.lines[0].0)], "driver of {}", vpr.name),
            Some(source) => {
                lifted += 1;
                assert_eq!(source, NodeId(vpr.lines[0].0), "driver of {}", vpr.name);
                assert!(driver.len() > 1);
                assert!(driver.contains(&NodeId(vpr.lines[1].0)), "{}", vpr.name);
                for &opin in &driver {
                    assert_eq!(device.node_kind(opin), Some(NodeKind::Opin));
                    assert!(device.edge_switch(source, opin).is_some());
                }
            }
        }
        // Every terminal is where VPR says that pin went.
        let mut seen = vec![false; net.terminals.len()];
        for (id, node) in &vpr.lines {
            if let Some(index) = node.net_pin_index {
                let terminal = &net.terminals[index as usize];
                assert_eq!(
                    terminal.access_points().len(),
                    1,
                    "a class is one SINK here"
                );
                assert_eq!(
                    terminal.access_points()[0].node,
                    NodeId(*id),
                    "pin {index} of {}",
                    vpr.name
                );
                seen[index as usize] = true;
                sinks_checked += 1;
            }
        }
        assert!(seen[1..].iter().all(|&reached| reached), "{}", vpr.name);
    }
    assert!(sinks_checked >= 25);
    // The logic blocks drive several nets each, through OPINs of the same class.
    assert!(lifted > 0);
}

#[test]
fn a_driver_on_its_opins_costs_what_the_step_from_its_source_did() {
    let (device, design, _) = mult_4x4();
    let mut steps = Vec::new();
    for net in design.nets() {
        let Some(source) = design.driver_source(net.id) else {
            continue;
        };
        steps.clear();
        device.graph().expand(net.id, source, &mut steps);
        let points: Vec<(NodeId, Cost)> = net.terminals[0]
            .access_points()
            .iter()
            .map(|point| (point.node, point.cost))
            .collect();
        let expected: Vec<(NodeId, Cost)> = steps.iter().map(|step| (step.to, step.cost)).collect();
        assert_eq!(points, expected);
        assert!(points.iter().all(|&(_, cost)| cost > Cost::ZERO));
    }
}

#[test]
fn a_route_from_the_source_of_a_driver_on_its_opins_is_refused() {
    // VPR rejects a net that leaves its block through two OPINs, and a route rooted on the
    // SOURCE is how that would come about.
    let (device, design, nets) = mult_4x4();
    let vpr = nets
        .iter()
        .find(|net| design.driver_source(NetId(net.id)).is_some())
        .unwrap();
    let mut routes = RouteSet::new();
    routes.set_route(NetId(vpr.id), route_from_lines(vpr));
    let header = RouteHeader {
        placement_file: "mult_4x4.place",
        placement_id: placement_id(),
    };
    let error = write_route(Vec::new(), &device, &design, &routes, &header).unwrap_err();
    assert!(
        matches!(error, RouteFileError::WrongRoot { net, node } if net == NetId(vpr.id) && node == NodeId(vpr.lines[0].0)),
        "{error}"
    );
}

#[test]
fn writes_vprs_route_file_byte_for_byte() {
    let (device, design, nets) = mult_4x4();
    let mut routes = RouteSet::new();
    for net in &nets {
        // Rooted where the engine would root them, so that the SOURCE has to be put back.
        routes.set_route(NetId(net.id), engine_route(&design, net));
    }
    let header = RouteHeader {
        placement_file: "mult_4x4.place",
        placement_id: placement_id(),
    };
    let mut out = Vec::new();
    let summary = write_route(&mut out, &device, &design, &routes, &header).unwrap();
    assert_eq!(summary.routed, 25);
    assert_eq!(summary.unrouted, 0);
    assert_eq!(String::from_utf8(out).unwrap(), ROUTE);
}

#[test]
fn every_route_in_the_vpr_file_validates_as_a_route_on_the_graph() {
    let (device, design, nets) = mult_4x4();
    for (net, vpr) in design.nets().iter().zip(&nets) {
        let route = engine_route(&design, vpr);
        assert_eq!(route.validate(net), Ok(()), "{}", vpr.name);
        for edge in route.edges() {
            assert!(device.edge_switch(edge.from, edge.to).is_some());
        }
    }
}

// Costs and terminals, on the same graph.

#[test]
fn step_costs_are_the_base_costs_of_vprs_scheme() {
    let (device, _, nets) = mult_4x4();
    let graph = device.graph();
    let factor = device.delay_normalization();
    assert!(factor > 0.0);

    // All wires are length 4 and fed with a mix of the switches above.
    for (id, node) in &nets[0].lines {
        let node_id = NodeId(*id);
        let base = graph.base_cost(node_id).unwrap().get();
        let expected = match node.kind.as_str() {
            "SOURCE" | "OPIN" => factor,
            "IPIN" => 0.875 * factor,
            "SINK" => 0.0,
            "CHANX" | "CHANY" => factor * 4.0,
            other => panic!("unexpected {other}"),
        };
        let difference = (base - expected).abs();
        assert!(
            difference <= expected * 1e-6,
            "{}: {base} vs {expected}",
            node.kind
        );
    }

    let mut steps = Vec::new();
    let source = NodeId(nets[0].lines[0].0);
    graph.expand(NetId(0), source, &mut steps);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].cost, graph.base_cost(steps[0].to).unwrap());
}

#[test]
fn engine_options_put_history_in_the_units_of_the_graph() {
    let (device, _, _) = mult_4x4();
    let options = device.engine_options();
    let defaults = crate::engine::EngineOptions::default();
    // Two tiles of wire, half a length-4 wire's base cost, and far from the default of 1.
    assert_eq!(
        options.history_factor.get(),
        2.0 * device.delay_normalization()
    );
    assert!(options.history_factor.get() < 1e-9);
    assert_eq!(defaults.history_factor, Cost::ONE);
    // The present factor grows more slowly, and hopeless runs give up early.
    assert_eq!(options.present_growth.get(), 1.5);
    assert_eq!(defaults.present_growth.get(), 1.8);
    assert_eq!(options.give_up_factor, 1.0);
    assert_eq!(defaults.give_up_factor, 0.0);
    assert_eq!(options.give_up_overused, defaults.give_up_overused);
    // Nothing else changes.
    assert_eq!(options.max_iterations, defaults.max_iterations);
    assert_eq!(options.first_present, defaults.first_present);
    assert_eq!(options.initial_present, defaults.initial_present);
    assert_eq!(options.max_present, defaults.max_present);
    assert_eq!(options.batch_size, defaults.batch_size);
    assert_eq!(options.timing_iterations, defaults.timing_iterations);
    // Nets keep what is not overused, and the bound is exact until there's a map.
    assert!(options.incremental_reroute && !defaults.incremental_reroute);
    assert_eq!(options.bound_weight, 1.0);
    let mut device = device;
    device.use_map_lookahead(&LookaheadOptions::default());
    assert_eq!(device.engine_options().bound_weight, 1.1);
}

#[test]
fn capacity_comes_from_the_file() {
    let (device, _, _) = mult_4x4();
    assert_eq!(device.graph().capacity(NodeId(0)), 1);
}

// Graph details, on handwritten files.

/// A graph file with one tile type, and sections that the case can fill in.
fn small_graph(switches: &str, nodes: &str, edges: &str) -> String {
    format!(
        "<rr_graph>\n<switches>\n{switches}\n</switches>\n\
         <segments><segment id=\"0\" name=\"L2\" length=\"2\"/></segments>\n\
         <block_types><block_type id=\"0\" name=\"t\" width=\"1\" height=\"1\"/></block_types>\n\
         <grid><grid_loc x=\"0\" y=\"0\" block_type_id=\"0\" width_offset=\"0\" height_offset=\"0\"/>\
         <grid_loc x=\"1\" y=\"0\" block_type_id=\"0\" width_offset=\"0\" height_offset=\"0\"/>\
         <grid_loc x=\"2\" y=\"0\" block_type_id=\"0\" width_offset=\"0\" height_offset=\"0\"/>\
         </grid>\n<rr_nodes>\n{nodes}\n</rr_nodes>\n<rr_edges>\n{edges}\n</rr_edges>\n</rr_graph>\n"
    )
}

fn wire(id: u32, x: u32) -> String {
    format!(
        "<node id=\"{id}\" type=\"CHANX\" capacity=\"1\" direction=\"INC_DIR\">\
         <loc xlow=\"{x}\" ylow=\"0\" xhigh=\"{x}\" yhigh=\"0\" ptc=\"0\"/>\
         <timing R=\"0\" C=\"0\"/><segment segment_id=\"0\"/></node>"
    )
}

fn timed_switch(id: u32, kind: &str, delay: &str) -> String {
    format!(
        "<switch id=\"{id}\" name=\"s{id}\" type=\"{kind}\"><timing Tdel=\"{delay}\"/></switch>"
    )
}

fn edge(from: u32, to: u32, switch: u32) -> String {
    format!("<edge src_node=\"{from}\" sink_node=\"{to}\" switch_id=\"{switch}\"/>")
}

#[test]
fn parallel_edges_keep_the_switch_vpr_would_find_first() {
    // VPR's order is by configurable, then switch ID, so 0 is out as a short, and 1 is next,
    // no matter what the file's order is or that switch 2 is faster.
    let switches = [
        timed_switch(0, "short", "0"),
        timed_switch(1, "mux", "2e-10"),
        timed_switch(2, "mux", "1e-10"),
    ]
    .concat();
    let nodes = [wire(0, 0), wire(1, 1)].concat();
    let edges = [edge(0, 1, 2), edge(0, 1, 1), edge(0, 1, 0), edge(0, 1, 2)].concat();
    let device = read_rr_graph(small_graph(&switches, &nodes, &edges).as_bytes()).unwrap();

    assert_eq!(device.graph().num_edges(), 1);
    assert_eq!(device.edge_switch(NodeId(0), NodeId(1)), Some(1));
    assert_eq!(device.out_switches(NodeId(0)), [1]);
    let stats = device.stats();
    assert_eq!(stats.file_edges, 4);
    assert_eq!(stats.parallel_edges, 3);
    assert_eq!(stats.parallel_pairs, 1);
    assert_eq!(stats.parallel_pairs_with_different_switches, 1);
    assert_eq!(stats.non_configurable_edges, 1);
}

#[test]
fn parallel_edges_with_the_same_switch_are_counted_apart() {
    let switches = timed_switch(0, "mux", "1e-10");
    let nodes = [wire(0, 0), wire(1, 1)].concat();
    let edges = [edge(0, 1, 0), edge(0, 1, 0)].concat();
    let device = read_rr_graph(small_graph(&switches, &nodes, &edges).as_bytes()).unwrap();
    assert_eq!(device.stats().parallel_pairs, 1);
    assert_eq!(device.stats().parallel_pairs_with_different_switches, 0);
}

#[test]
fn switches_of_edges_line_up_with_expansion_after_merging() {
    let switches = [
        timed_switch(0, "mux", "1e-10"),
        timed_switch(1, "mux", "1e-10"),
        timed_switch(2, "mux", "1e-10"),
    ]
    .concat();
    let nodes = [wire(0, 0), wire(1, 1), wire(2, 2)].concat();
    let edges = [
        edge(2, 1, 2),
        edge(0, 2, 1),
        edge(0, 0, 2),
        edge(0, 1, 2),
        edge(0, 2, 0),
        edge(1, 2, 0),
    ]
    .concat();
    let device = read_rr_graph(small_graph(&switches, &nodes, &edges).as_bytes()).unwrap();
    assert_eq!(device.stats().self_loops, 1);

    let mut steps = Vec::new();
    device.graph().expand(NetId(0), NodeId(0), &mut steps);
    let targets: Vec<u32> = steps.iter().map(|step| step.to.0).collect();
    assert_eq!(targets, [2, 1]);
    assert_eq!(device.out_switches(NodeId(0)), [0, 2]);
    assert_eq!(device.out_switches(NodeId(1)), [0]);
    assert_eq!(device.out_switches(NodeId(2)), [2]);
    assert_eq!(device.out_switches(NodeId(9)), [0u16; 0]);
    assert_eq!(device.edge_switch(NodeId(0), NodeId(0)), None);
    assert_eq!(device.edge_switch(NodeId(1), NodeId(0)), None);
}

#[test]
fn delay_normalization_follows_vprs_formulas() {
    // Two buffered wires of a type with length 2: T_linear = Tsw + Rsw (Cint + C) + R (C + Cint) / 2.
    let switches = "<switch id=\"0\" name=\"m\" type=\"mux\"><timing R=\"100\" Cinternal=\"1e-15\" Tdel=\"4e-11\"/></switch>";
    let timed = |id: u32, x: u32| {
        format!(
            "<node id=\"{id}\" type=\"CHANX\" capacity=\"1\"><loc xlow=\"{x}\" ylow=\"0\" xhigh=\"{x}\" yhigh=\"0\" ptc=\"0\"/>\
             <timing R=\"50\" C=\"2e-14\"/><segment segment_id=\"0\"/></node>"
        )
    };
    let nodes = [timed(0, 0), timed(1, 1), timed(2, 2)].concat();
    let edges = [edge(0, 1, 0), edge(1, 2, 0)].concat();
    let device = read_rr_graph(small_graph(switches, &nodes, &edges).as_bytes()).unwrap();
    let (r_sw, c_int, t_sw, r, c) = (100.0, 1e-15, 4e-11, 50.0, 2e-14);
    let expected: f64 = (t_sw + r_sw * (c_int + c) + 0.5 * r * (c + c_int)) / 2.0;
    let found = device.delay_normalization();
    assert!(
        (found - expected).abs() < expected * 1e-5,
        "{found} vs {expected}"
    );
    assert_eq!(
        device.graph().base_cost(NodeId(1)),
        Cost::new(f64::from(found as f32 / 0.5))
    );
}

#[test]
fn unbuffered_wires_use_vprs_other_formula() {
    let switches = "<switch id=\"0\" name=\"p\" type=\"pass_gate\"><timing R=\"100\" Tdel=\"4e-11\"/></switch>";
    let timed = |id: u32, x: u32| {
        format!(
            "<node id=\"{id}\" type=\"CHANX\" capacity=\"1\"><loc xlow=\"{x}\" ylow=\"0\" xhigh=\"{x}\" yhigh=\"0\" ptc=\"0\"/>\
             <timing R=\"50\" C=\"2e-14\"/><segment segment_id=\"0\"/></node>"
        )
    };
    let nodes = [timed(0, 0), timed(1, 1)].concat();
    let device = read_rr_graph(small_graph(switches, &nodes, &edge(0, 1, 0)).as_bytes()).unwrap();
    let (r_sw, t_sw, r, c) = (100.0, 4e-11, 50.0, 2e-14);
    let t_linear: f64 = t_sw + 0.5 * r_sw * c;
    let t_quadratic: f64 = (r_sw + r) * 0.5 * c;
    let expected = t_linear / 2.0 + t_quadratic / 4.0;
    let found = device.delay_normalization();
    assert!(
        (found - expected).abs() < expected * 1e-5,
        "{found} vs {expected}"
    );
}

#[test]
fn with_no_delay_anywhere_the_factor_is_vprs_nanosecond() {
    let switches = timed_switch(0, "mux", "0");
    let nodes = [wire(0, 0), wire(1, 1)].concat();
    let device = read_rr_graph(small_graph(&switches, &nodes, &edge(0, 1, 0)).as_bytes()).unwrap();
    assert!((device.delay_normalization() - 1e-9).abs() < 1e-15);
}

#[test]
fn shorts_do_not_count_towards_delay() {
    let switches = [
        timed_switch(0, "short", "9e-9"),
        timed_switch(1, "mux", "1e-10"),
    ]
    .concat();
    let nodes = [wire(0, 0), wire(1, 1)].concat();
    let edges = [edge(0, 1, 0), edge(0, 1, 1)].concat();
    let device = read_rr_graph(small_graph(&switches, &nodes, &edges).as_bytes()).unwrap();
    let expected = 1e-10 / 2.0;
    assert!((device.delay_normalization() - expected).abs() < expected * 1e-5);
}

#[test]
fn lookups_find_wires_along_their_length_and_pins_on_each_side() {
    let nodes = "\
        <node id=\"0\" type=\"CHANX\" capacity=\"1\"><loc xlow=\"0\" ylow=\"0\" xhigh=\"2\" yhigh=\"0\" ptc=\"4\"/></node>\
        <node id=\"1\" type=\"CHANX\" capacity=\"1\"><loc xlow=\"0\" ylow=\"0\" xhigh=\"1\" yhigh=\"0\" ptc=\"5\"/></node>\
        <node id=\"2\" type=\"CHANX\" capacity=\"1\"><loc xlow=\"2\" ylow=\"0\" xhigh=\"2\" yhigh=\"0\" ptc=\"5\"/></node>\
        <node id=\"3\" type=\"IPIN\" capacity=\"1\"><loc xlow=\"1\" ylow=\"0\" xhigh=\"1\" yhigh=\"0\" side=\"LEFT\" ptc=\"7\"/></node>\
        <node id=\"4\" type=\"IPIN\" capacity=\"1\"><loc xlow=\"1\" ylow=\"0\" xhigh=\"1\" yhigh=\"0\" side=\"TOP_RIGHT\" ptc=\"7\"/></node>\
        <node id=\"5\" type=\"SINK\" capacity=\"3\"><loc xlow=\"0\" ylow=\"0\" xhigh=\"2\" yhigh=\"0\" ptc=\"0\"/></node>";
    let device = read_rr_graph(small_graph("", nodes, "").as_bytes()).unwrap();

    use NodeKind::{ChanX, ChanY, Ipin, Sink};
    for x in 0..=2 {
        assert_eq!(device.find_nodes(ChanX, x, 0, 4), [NodeId(0)]);
        assert_eq!(device.find_nodes(Sink, x, 0, 0), [NodeId(5)]);
    }
    assert_eq!(device.find_nodes(ChanX, 3, 0, 4), []);
    assert_eq!(device.find_nodes(ChanX, 1, 0, 5), [NodeId(1)]);
    assert_eq!(device.find_nodes(ChanX, 2, 0, 5), [NodeId(2)]);
    assert_eq!(device.find_nodes(ChanY, 1, 0, 5), []);
    assert_eq!(device.find_nodes(Ipin, 1, 0, 7), [NodeId(3), NodeId(4)]);
    assert_eq!(device.find_nodes(Ipin, 1, 0, 8), []);
    assert_eq!(device.find_nodes(Ipin, 1 << 20, 0, 7), []);

    assert_eq!(
        device.node_sides(NodeId(3)),
        Some([true, false, false, false])
    );
    assert_eq!(
        device.node_sides(NodeId(4)),
        Some([false, true, true, false])
    );
    assert_eq!(device.graph().capacity(NodeId(5)), 3);
    assert_eq!(device.node_kind(NodeId(5)), Some(Sink));
    assert_eq!(device.node_kind(NodeId(6)), None);
    assert_eq!(device.node_rc(NodeId(0)), Some((0.0, 0.0)));
}

#[test]
fn nodes_can_come_in_any_order_and_sections_too() {
    let text = "<rr_graph>\
        <rr_edges><edge src_node=\"1\" sink_node=\"0\" switch_id=\"7\"/></rr_edges>\
        <rr_nodes>\
        <node id=\"1\" type=\"CHANY\" capacity=\"2\"><loc xlow=\"1\" ylow=\"0\" xhigh=\"1\" yhigh=\"0\" ptc=\"11\"/></node>\
        <node id=\"0\" type=\"CHANX\" capacity=\"1\"><loc xlow=\"0\" ylow=\"0\" xhigh=\"0\" yhigh=\"0\" ptc=\"10\"/><timing R=\"1\" C=\"2\"/></node>\
        </rr_nodes>\
        <grid><grid_loc x=\"0\" y=\"0\" block_type_id=\"5\" width_offset=\"0\" height_offset=\"0\"/></grid>\
        <block_types><block_type id=\"5\" name=\"t\" width=\"1\" height=\"1\"/></block_types>\
        <switches><switch id=\"7\" name=\"s\"/></switches>\
        </rr_graph>";
    let device = read_rr_graph(text.as_bytes()).unwrap();
    assert_eq!(device.node_ptc(NodeId(0)), Some(10));
    assert_eq!(device.node_rc(NodeId(0)), Some((1.0, 2.0)));
    assert_eq!(device.node_kind(NodeId(1)), Some(NodeKind::ChanY));
    assert_eq!(device.graph().capacity(NodeId(1)), 2);
    assert_eq!(device.edge_switch(NodeId(1), NodeId(0)), Some(7));
    assert_eq!(
        device.switch(7).map(|switch| switch.kind),
        Some(SwitchKind::Mux)
    );
}

#[test]
fn reads_layers_and_ptc_lists_of_newer_graphs() {
    let nodes = "<node id=\"0\" type=\"CHANX\" capacity=\"1\"><loc layer_low=\"1\" layer_high=\"2\" xlow=\"0\" ylow=\"0\" xhigh=\"0\" yhigh=\"0\" ptc=\"3,4,5\"/></node>";
    let device = read_rr_graph(small_graph("", nodes, "").as_bytes()).unwrap();
    assert_eq!(device.node_ptc(NodeId(0)), Some(3));
    let location = device.node_location(NodeId(0)).unwrap();
    assert_eq!((location.layer_low, location.layer_high), (1, 2));
}

#[test]
fn switch_timing_follows_the_schemas_rules() {
    let with = |timing: &str| {
        let switches = format!("<switch id=\"0\" name=\"s\"><timing {timing}/></switch>");
        read_rr_graph(small_graph(&switches, "", "").as_bytes()).map(|device| {
            let switch = device.switch(0).unwrap();
            (switch.t_del_min, switch.t_del_max)
        })
    };
    assert_eq!(with("Tdel=\"2\"").unwrap(), (2.0, 2.0));
    assert_eq!(with("Tdel_min=\"1\" Tdel_max=\"2\"").unwrap(), (1.0, 2.0));
    assert_eq!(with("Tdel_max=\"2\"").unwrap(), (0.0, 2.0));
    assert_eq!(with("R=\"1\"").unwrap(), (0.0, 0.0));
    assert!(with("Tdel=\"2\" Tdel_max=\"2\"").is_err());
    assert!(with("Tdel_min=\"3\" Tdel_max=\"2\"").is_err());
    assert!(with("Tdel_min=\"1\"").is_err());
    assert!(with("Tdel=\"nan\"").is_err());
    assert!(with("Tdel=\"inf\"").is_err());
    assert!(with("Tdel=\"-1\"").is_err());
    assert!(with("Tdel=\"1e99\"").is_err());
    assert!(with("R=\"x\"").is_err());
}

#[test]
fn rejects_graphs_that_do_not_hold_together() {
    let switch = timed_switch(0, "mux", "1e-10");
    let valid = small_graph(&switch, &wire(0, 0), "");
    assert!(read_rr_graph(valid.as_bytes()).is_ok());

    // In these files, a switch is on line 3, a node is on line 9 and an edge on line 12. What
    // can only be known at the end of the file is reported on the last line, which is 15.
    let cases: Vec<(String, u64)> = vec![
        ("<nope/>".to_owned(), 1),
        // Edges to nodes that don't exist, and through switches that don't.
        (small_graph(&switch, &wire(0, 0), &edge(0, 1, 0)), 12),
        (small_graph(&switch, &wire(0, 0), &edge(0, 0, 1)), 12),
        // A gap in the IDs, and a repeated one.
        (small_graph(&switch, &wire(1, 0), ""), 15),
        (
            small_graph(&switch, &[wire(0, 0), wire(0, 1)].concat(), ""),
            15,
        ),
        // A wire with a segment that doesn't exist.
        (
            small_graph(
                &switch,
                &wire(0, 0).replace("segment_id=\"0\"", "segment_id=\"3\""),
                "",
            ),
            9,
        ),
        // Coordinates out of range, backwards or negative.
        (
            small_graph(
                &switch,
                &wire(0, 0).replace("xhigh=\"0\"", "xhigh=\"70000\""),
                "",
            ),
            9,
        ),
        (
            small_graph(
                &switch,
                &wire(0, 1).replace("xhigh=\"1\"", "xhigh=\"0\""),
                "",
            ),
            9,
        ),
        (
            small_graph(
                &switch,
                &wire(0, 0).replace("xlow=\"0\"", "xlow=\"-1\""),
                "",
            ),
            9,
        ),
        // A node with no location.
        (
            small_graph(&switch, "<node id=\"0\" type=\"SINK\" capacity=\"1\"/>", ""),
            9,
        ),
        // Unknown names.
        (
            small_graph(&switch, &wire(0, 0).replace("CHANX", "WIRE"), ""),
            9,
        ),
        (
            small_graph(&switch, &wire(0, 0).replace("INC_DIR", "UP"), ""),
            9,
        ),
        (small_graph(&timed_switch(0, "laser", "0"), "", ""), 3),
        // IDs of switches too large for VPR.
        (small_graph(&timed_switch(99999, "mux", "0"), "", ""), 3),
        (small_graph(&switch, "", &edge(0, 0, 70000)), 12),
        // A switch given twice.
        (
            small_graph(&[switch.clone(), switch.clone()].concat(), "", ""),
            3,
        ),
    ];
    for (text, line) in cases {
        let error = read_rr_graph(text.as_bytes()).err();
        let Some(error) = error else {
            panic!("should reject {text}");
        };
        assert_eq!(error.line(), Some(line), "{error} for {text}");
    }
}

#[test]
fn rejects_missing_sections_and_bad_grids() {
    let whole = small_graph("", "", "");
    for section in ["switches", "block_types", "grid", "rr_nodes", "rr_edges"] {
        let without = whole
            .replace(&format!("<{section}>"), "<x>")
            .replace(&format!("</{section}>"), "</x>");
        assert!(
            matches!(read_rr_graph(without.as_bytes()), Err(ImportError::MissingSection { section: name }) if name == section),
            "{section}"
        );
    }

    let grid = |entries: &str| {
        format!(
            "<rr_graph><switches/><block_types><block_type id=\"0\" name=\"t\" width=\"1\" height=\"1\"/></block_types>\
             <grid>{entries}</grid><rr_nodes/><rr_edges/></rr_graph>"
        )
    };
    let tile = |x: u32, y: u32, block: u32| {
        format!(
            "<grid_loc x=\"{x}\" y=\"{y}\" block_type_id=\"{block}\" width_offset=\"0\" height_offset=\"0\"/>"
        )
    };
    // A tiny file claiming a vast device has to be turned down without allocating for it.
    assert!(matches!(
        read_rr_graph(grid(&tile(60000, 60000, 0)).as_bytes()),
        Err(ImportError::IncompleteGrid { .. })
    ));
    assert!(matches!(
        read_rr_graph(grid(&[tile(0, 0, 0), tile(0, 0, 0)].concat()).as_bytes()),
        Err(ImportError::IncompleteGrid { .. })
    ));
    assert!(matches!(
        read_rr_graph(grid(&tile(0, 0, 9)).as_bytes()),
        Err(ImportError::BadReference {
            what: "block type",
            ..
        })
    ));
}

#[test]
fn node_ids_cannot_ask_for_memory() {
    let nodes = "<node id=\"4294967295\" type=\"SINK\" capacity=\"1\"><loc xlow=\"0\" ylow=\"0\" xhigh=\"0\" yhigh=\"0\" ptc=\"0\"/></node>";
    let error = read_rr_graph(small_graph("", nodes, "").as_bytes()).unwrap_err();
    assert!(
        matches!(error, ImportError::BadNodeId { id: u32::MAX, .. }),
        "{error}"
    );
}

#[test]
fn node_table_entries_stay_compact() {
    assert_eq!(size_of::<rr_graph::NodeInfo>(), 20);
    assert_eq!(size_of::<NodeLocation>(), 10);
}

// Nets, terminals and pin classes, on handwritten files.

/// A device of two tall `dsp` tiles, each holding two blocks, and a pad.
fn dsp_device() -> RrGraph {
    let text = "<rr_graph>\
        <switches><switch id=\"0\" name=\"s\"><timing Tdel=\"1e-10\"/></switch></switches>\
        <segments><segment id=\"0\" name=\"L1\"/></segments>\
        <block_types>\
        <block_type id=\"0\" name=\"pad\" width=\"1\" height=\"1\">\
        <pin_class type=\"OUTPUT\"><pin ptc=\"0\">pad.inpad[0]</pin></pin_class>\
        <pin_class type=\"INPUT\"><pin ptc=\"1\">pad.outpad[0]</pin></pin_class>\
        <pin_class type=\"INPUT\"><pin ptc=\"2\">pad.clk[0]</pin></pin_class>\
        </block_type>\
        <block_type id=\"1\" name=\"dsp_tile\" width=\"1\" height=\"2\">\
        <pin_class type=\"INPUT\"><pin ptc=\"0\">dsp_tile[0].a[0]</pin><pin ptc=\"1\">dsp_tile[0].a[1]</pin></pin_class>\
        <pin_class type=\"OUTPUT\"><pin ptc=\"2\">dsp_tile[0].o[0]</pin></pin_class>\
        <pin_class type=\"INPUT\"><pin ptc=\"3\">dsp_tile[1].a[0]</pin><pin ptc=\"4\">dsp_tile[1].a[1]</pin></pin_class>\
        <pin_class type=\"OUTPUT\"><pin ptc=\"5\">dsp_tile[1].o[0]</pin></pin_class>\
        </block_type>\
        </block_types>\
        <grid>\
        <grid_loc x=\"0\" y=\"0\" block_type_id=\"0\" width_offset=\"0\" height_offset=\"0\"/>\
        <grid_loc x=\"1\" y=\"0\" block_type_id=\"1\" width_offset=\"0\" height_offset=\"0\"/>\
        <grid_loc x=\"0\" y=\"1\" block_type_id=\"0\" width_offset=\"0\" height_offset=\"0\"/>\
        <grid_loc x=\"1\" y=\"1\" block_type_id=\"1\" width_offset=\"0\" height_offset=\"1\"/>\
        </grid>\
        <rr_nodes>\
        <node id=\"0\" type=\"SOURCE\" capacity=\"1\"><loc xlow=\"0\" ylow=\"0\" xhigh=\"0\" yhigh=\"0\" ptc=\"0\"/></node>\
        <node id=\"1\" type=\"SINK\" capacity=\"1\"><loc xlow=\"0\" ylow=\"0\" xhigh=\"0\" yhigh=\"0\" ptc=\"1\"/></node>\
        <node id=\"2\" type=\"SINK\" capacity=\"1\"><loc xlow=\"0\" ylow=\"0\" xhigh=\"0\" yhigh=\"0\" ptc=\"2\"/></node>\
        <node id=\"3\" type=\"SINK\" capacity=\"2\"><loc xlow=\"1\" ylow=\"0\" xhigh=\"1\" yhigh=\"1\" ptc=\"0\"/></node>\
        <node id=\"4\" type=\"SOURCE\" capacity=\"1\"><loc xlow=\"1\" ylow=\"0\" xhigh=\"1\" yhigh=\"1\" ptc=\"1\"/></node>\
        <node id=\"5\" type=\"SINK\" capacity=\"2\"><loc xlow=\"1\" ylow=\"0\" xhigh=\"1\" yhigh=\"1\" ptc=\"2\"/></node>\
        <node id=\"6\" type=\"SOURCE\" capacity=\"1\"><loc xlow=\"1\" ylow=\"0\" xhigh=\"1\" yhigh=\"1\" ptc=\"3\"/></node>\
        <node id=\"7\" type=\"SINK\" capacity=\"2\"><loc xlow=\"1\" ylow=\"0\" xhigh=\"1\" yhigh=\"0\" ptc=\"0\"/></node>\
        </rr_nodes>\
        <rr_edges/>\
        </rr_graph>";
    read_rr_graph(text.as_bytes()).unwrap()
}

/// A netlist where `in` goes from a pad to two blocks, and `out` from one of them to a pad.
const DSP_NET: &str = "<block name=\"t\" instance=\"FPGA_packed_netlist[0]\">\
    <block name=\"in\" instance=\"pad[0]\" mode=\"m\"><outputs><port name=\"inpad\">inpad[0].inpad[0]-&gt;x</port></outputs>\
    <block name=\"in\" instance=\"inpad[0]\"><outputs><port name=\"inpad\">in</port></outputs></block></block>\
    <block name=\"d0\" instance=\"dsp[0]\" mode=\"m\"><inputs><port name=\"a\">open in</port></inputs>\
    <outputs><port name=\"o\">out[0].o[0]-&gt;x</port></outputs>\
    <block name=\"out\" instance=\"out[0]\"><outputs><port name=\"o\">out</port></outputs></block></block>\
    <block name=\"d1\" instance=\"dsp[1]\" mode=\"m\"><inputs><port name=\"a\">in out</port></inputs></block>\
    <block name=\"out:out\" instance=\"pad[1]\" mode=\"m\"><inputs><port name=\"outpad\">out</port></inputs></block>\
    </block>";

const DSP_PLACE: &str = "in 0 0 0\nd0 1 0 0\nd1 1 0 1\nout:out 0 0 0\n";

#[test]
fn pins_find_their_class_nodes_on_tiles_with_several_positions_and_rows() {
    let device = dsp_device();
    let netlist = read_netlist(DSP_NET.as_bytes()).unwrap();
    let placement = read_placement(DSP_PLACE.as_bytes()).unwrap();
    let design = resolve_nets(&device, &netlist, &placement, &NetOptions::default()).unwrap();

    let net = design
        .nets()
        .iter()
        .find(|net| design.net_info(net.id).unwrap().name == "in")
        .unwrap();
    let nodes = |terminal: &Terminal<NodeId>| {
        terminal
            .access_points()
            .iter()
            .map(|point| point.node.0)
            .collect::<Vec<_>>()
    };
    // The driver, and then an access point per node that a class has here, of which one
    // block has two, in the tile's bottom row.
    assert_eq!(nodes(&net.terminals[0]), [0]);
    assert_eq!(nodes(&net.terminals[1]), [3, 7]);
    assert_eq!(nodes(&net.terminals[2]), [5]);
    assert!(net.terminals.iter().all(|terminal| {
        terminal
            .access_points()
            .iter()
            .all(|point| point.cost == Cost::ZERO)
    }));
    assert_eq!(design.net_info(net.id).unwrap().pins[2].class, 2);
    assert_eq!(design.shared_sink_nets(), []);
}

#[test]
fn nets_keep_their_vpr_numbers_with_some_skipped() {
    let device = dsp_device();
    let netlist = "<block name=\"t\" instance=\"FPGA_packed_netlist[0]\">\
        <block name=\"in\" instance=\"pad[0]\"><outputs><port name=\"inpad\">clock</port></outputs></block>\
        <block name=\"d0\" instance=\"dsp[0]\" mode=\"m\"><inputs><port name=\"a\">floating</port></inputs></block>\
        <block name=\"out:x\" instance=\"pad[1]\"><clocks><port name=\"clk\">clock</port></clocks>\
        <outputs><port name=\"inpad\">unused</port></outputs></block>\
        <block name=\"d1\" instance=\"dsp[1]\" mode=\"m\"><inputs><port name=\"a\">data</port></inputs></block>\
        <block name=\"q\" instance=\"pad[2]\"><outputs><port name=\"inpad\">data</port></outputs></block>\
        </block>";
    let placement = "in 0 0 0\nd0 1 0 0\nout:x 0 0 0\nd1 1 0 1\nq 0 0 0\n";
    let netlist = read_netlist(netlist.as_bytes()).unwrap();
    let placement = read_placement(placement.as_bytes()).unwrap();
    let design = resolve_nets(&device, &netlist, &placement, &NetOptions::default()).unwrap();

    let statuses: Vec<(&str, NetStatus)> = design
        .net_infos()
        .iter()
        .map(|info| (info.name.as_str(), info.status))
        .collect();
    assert_eq!(
        statuses,
        [
            ("clock", NetStatus::Global),
            ("floating", NetStatus::Undriven),
            ("unused", NetStatus::NoSinks),
            ("data", NetStatus::Routable),
        ]
    );
    // The ones in the list are numbered as the rest of the file is.
    let ids: Vec<u32> = design.nets().iter().map(|net| net.id.0).collect();
    assert_eq!(ids, [2, 3]);
    assert!(!design.nets()[0].needs_routing());
    assert!(design.nets()[1].needs_routing());
    let global: Vec<NetId> = design.global_nets().map(|(net, _)| net).collect();
    assert_eq!(global, [NetId(0)]);
    let undriven: Vec<NetId> = design.undriven_nets().map(|(net, _)| net).collect();
    assert_eq!(undriven, [NetId(1)]);

    // Told to, it routes the clock, and the sink is what it is for any other net.
    let options = NetOptions {
        route_global_nets: true,
        ..NetOptions::default()
    };
    let design = resolve_nets(&device, &netlist, &placement, &options).unwrap();
    assert_eq!(design.net_infos()[0].status, NetStatus::Routable);
    let clock = design.nets().iter().find(|net| net.id == NetId(0)).unwrap();
    assert_eq!(clock.terminals[1].access_points()[0].node, NodeId(2));
}

#[test]
fn non_clock_globals_are_as_the_caller_names_them() {
    let device = dsp_device();
    let netlist = read_netlist(DSP_NET.as_bytes()).unwrap();
    let placement = read_placement(DSP_PLACE.as_bytes()).unwrap();
    let mut options = NetOptions::default();
    options.global_ports.push(GlobalPort {
        pb_type: "dsp".to_owned(),
        port: "a".to_owned(),
    });
    let design = resolve_nets(&device, &netlist, &placement, &options).unwrap();
    let names: Vec<&str> = design
        .global_nets()
        .map(|(_, info)| info.name.as_str())
        .collect();
    // Both nets reach a port of that name, on some block.
    assert_eq!(names, ["in", "out"]);
    assert_eq!(design.nets().len(), 0);
}

/// A pad and a `ram` two tiles tall, whose classes' nodes are each on the tile of their pin,
/// as VPR 9 builds them, and not on the block's root tile.
fn tall_ram_device() -> RrGraph {
    let text = "<rr_graph>\
        <switches><switch id=\"0\" name=\"s\"><timing Tdel=\"1e-10\"/></switch></switches>\
        <segments><segment id=\"0\" name=\"L1\"/></segments>\
        <block_types>\
        <block_type id=\"0\" name=\"pad\" width=\"1\" height=\"1\">\
        <pin_class type=\"OUTPUT\"><pin ptc=\"0\">pad.inpad[0]</pin></pin_class>\
        <pin_class type=\"INPUT\"><pin ptc=\"1\">pad.outpad[0]</pin></pin_class>\
        </block_type>\
        <block_type id=\"1\" name=\"ram\" width=\"1\" height=\"2\">\
        <pin_class type=\"INPUT\"><pin ptc=\"0\">ram.a[0]</pin></pin_class>\
        <pin_class type=\"INPUT\"><pin ptc=\"1\">ram.a[1]</pin></pin_class>\
        </block_type>\
        </block_types>\
        <grid>\
        <grid_loc x=\"0\" y=\"0\" block_type_id=\"0\" width_offset=\"0\" height_offset=\"0\"/>\
        <grid_loc x=\"1\" y=\"0\" block_type_id=\"1\" width_offset=\"0\" height_offset=\"0\"/>\
        <grid_loc x=\"0\" y=\"1\" block_type_id=\"0\" width_offset=\"0\" height_offset=\"0\"/>\
        <grid_loc x=\"1\" y=\"1\" block_type_id=\"1\" width_offset=\"0\" height_offset=\"1\"/>\
        </grid>\
        <rr_nodes>\
        <node id=\"0\" type=\"SOURCE\" capacity=\"1\"><loc xlow=\"0\" ylow=\"0\" xhigh=\"0\" yhigh=\"0\" ptc=\"0\"/></node>\
        <node id=\"1\" type=\"SINK\" capacity=\"1\"><loc xlow=\"1\" ylow=\"0\" xhigh=\"1\" yhigh=\"0\" ptc=\"0\"/></node>\
        <node id=\"2\" type=\"SINK\" capacity=\"1\"><loc xlow=\"1\" ylow=\"1\" xhigh=\"1\" yhigh=\"1\" ptc=\"1\"/></node>\
        </rr_nodes>\
        <rr_edges/>\
        </rr_graph>";
    read_rr_graph(text.as_bytes()).unwrap()
}

#[test]
fn pins_of_blocks_over_several_tiles_find_class_nodes_on_any_of_them() {
    let device = tall_ram_device();
    let netlist = "<block name=\"t\" instance=\"FPGA_packed_netlist[0]\">\
        <block name=\"in\" instance=\"pad[0]\"><outputs><port name=\"inpad\">in</port></outputs></block>\
        <block name=\"r\" instance=\"ram[0]\" mode=\"m\"><inputs><port name=\"a\">in in</port></inputs></block>\
        </block>";
    let netlist = read_netlist(netlist.as_bytes()).unwrap();
    // The ram is placed at its root, the tile without the second class's node.
    let placement = read_placement("in 0 0 0\nr 1 0 0\n".as_bytes()).unwrap();
    let design = resolve_nets(&device, &netlist, &placement, &NetOptions::default()).unwrap();
    let sinks: Vec<Vec<u32>> = design.nets()[0].terminals[1..]
        .iter()
        .map(|terminal| {
            terminal
                .access_points()
                .iter()
                .map(|point| point.node.0)
                .collect()
        })
        .collect();
    assert_eq!(sinks, [vec![1], vec![2]]);
}

#[test]
fn constant_nets_are_global_and_found_through_logic_and_registers() {
    // `vcc` comes from a LUT without inputs, `k` from a LUT fed only by `vcc`, and `q` from a
    // register that takes `k`, so all three are constant. `x` mixes `k` with an input.
    let text = "<block name=\"t\" instance=\"FPGA_packed_netlist[0]\">\
        <inputs>in clk</inputs><outputs>out:x out:q</outputs><clocks>clk</clocks>\
        <block name=\"in\" instance=\"pad[0]\" mode=\"m\"><outputs><port name=\"inpad\">inpad[0].inpad[0]-&gt;o</port></outputs>\
        <block name=\"in\" instance=\"inpad[0]\"><outputs><port name=\"inpad\">in</port></outputs></block></block>\
        <block name=\"vcc\" instance=\"clb[0]\" mode=\"m\">\
        <inputs><port name=\"I\">open</port></inputs>\
        <outputs><port name=\"O\">lut[0].out[0]-&gt;o</port></outputs>\
        <block name=\"vcc\" instance=\"lut[0]\"><inputs><port name=\"in\">open</port></inputs>\
        <outputs><port name=\"out\">vcc</port></outputs></block></block>\
        <block name=\"k\" instance=\"clb[1]\" mode=\"m\">\
        <inputs><port name=\"I\">vcc in</port></inputs>\
        <outputs><port name=\"O\">lut[0].out[0]-&gt;o lut[1].out[0]-&gt;o ff[0].Q[0]-&gt;o</port></outputs>\
        <clocks><port name=\"clk\">clk</port></clocks>\
        <block name=\"k\" instance=\"lut[0]\"><inputs><port name=\"in\">clb.I[0]-&gt;x</port></inputs>\
        <outputs><port name=\"out\">k</port></outputs></block>\
        <block name=\"x\" instance=\"lut[1]\"><inputs><port name=\"in\">lut[0].out[0]-&gt;x clb.I[1]-&gt;x</port></inputs>\
        <outputs><port name=\"out\">x</port></outputs></block>\
        <block name=\"q\" instance=\"ff[0]\"><inputs><port name=\"D\">lut[0].out[0]-&gt;x</port></inputs>\
        <outputs><port name=\"Q\">q</port></outputs><clocks><port name=\"clk\">clb.clk[0]-&gt;c</port></clocks></block>\
        </block>\
        <block name=\"out:x\" instance=\"pad[1]\" mode=\"m\"><inputs><port name=\"outpad\">x</port></inputs></block>\
        <block name=\"out:q\" instance=\"pad[2]\" mode=\"m\"><inputs><port name=\"outpad\">q k</port></inputs></block>\
        <block name=\"clk\" instance=\"pad[3]\" mode=\"m\"><outputs><port name=\"inpad\">inpad[0].inpad[0]-&gt;o</port></outputs>\
        <block name=\"clk\" instance=\"inpad[0]\"><outputs><port name=\"inpad\">clk</port></outputs></block></block>\
        </block>";
    let netlist = read_netlist(text.as_bytes()).unwrap();
    let names = |nets: &[NetId]| -> Vec<&str> {
        nets.iter()
            .map(|net| netlist.nets()[net.0 as usize].name.as_str())
            .collect()
    };
    let constants = constant_nets(&netlist);
    let mut found = names(&constants);
    found.sort_unstable();
    assert_eq!(found, ["k", "q", "vcc"]);
    assert!(constants.windows(2).all(|pair| pair[0] < pair[1]));

    // What holds each primitive, which tells apart those that VPR makes up for a class.
    let parents: Vec<(&str, Option<&str>)> = netlist
        .primitives()
        .iter()
        .enumerate()
        .map(|(index, primitive)| {
            (
                primitive.name.as_str(),
                netlist.primitive_parent(index as u32),
            )
        })
        .collect();
    assert_eq!(
        parents,
        [
            ("in", Some("pad")),
            ("vcc", Some("clb")),
            ("k", Some("clb")),
            ("x", Some("clb")),
            ("q", Some("clb")),
            ("clk", Some("pad")),
        ]
    );
    assert_eq!(netlist.primitive_parent(6), None);

    // As global nets, they aren't routed.
    let mut options = NetOptions::default();
    let device = dsp_device();
    let netlist = read_netlist(DSP_NET.as_bytes()).unwrap();
    let placement = read_placement(DSP_PLACE.as_bytes()).unwrap();
    options.constant_nets = vec![NetId(1)];
    let design = resolve_nets(&device, &netlist, &placement, &options).unwrap();
    let names: Vec<&str> = design
        .global_nets()
        .map(|(_, info)| info.name.as_str())
        .collect();
    assert_eq!(names, ["out"]);
    assert_eq!(design.nets().len(), 1);
}

#[test]
fn import_failures_say_what_failed() {
    let device = dsp_device();
    let netlist = read_netlist(DSP_NET.as_bytes()).unwrap();
    let placement = read_placement(DSP_PLACE.as_bytes()).unwrap();
    let options = NetOptions::default();

    let without_d1 = read_placement("in 0 0 0\nd0 1 0 0\nout:out 0 0 0\n".as_bytes()).unwrap();
    assert!(matches!(
        resolve_nets(&device, &netlist, &without_d1, &options),
        Err(ImportError::UnplacedBlock { block }) if block == "d1"
    ));

    let outside =
        read_placement("in 9 9 0\nd0 1 0 0\nd1 1 0 1\nout:out 0 0 0\n".as_bytes()).unwrap();
    assert!(matches!(
        resolve_nets(&device, &netlist, &outside, &options),
        Err(ImportError::BadLocation { x: 9, y: 9, .. })
    ));

    // A position that the tile doesn't have, and a block on the wrong type of tile.
    for place in [
        "in 0 0 0\nd0 1 0 0\nd1 1 0 2\nout:out 0 0 0\n",
        "in 1 0 0\nd0 1 0 0\nd1 1 0 1\nout:out 0 0 0\n",
    ] {
        let bad = read_placement(place.as_bytes()).unwrap();
        assert!(matches!(
            resolve_nets(&device, &netlist, &bad, &options),
            Err(ImportError::PinNotFound { .. })
        ));
    }

    // Pin classes that disagree with how the netlist uses the pin.
    let backwards = "<block name=\"t\" instance=\"FPGA_packed_netlist[0]\">\
        <block name=\"in\" instance=\"pad[0]\"><inputs><port name=\"inpad\">n</port></inputs></block>\
        </block>";
    let backwards = read_netlist(backwards.as_bytes()).unwrap();
    assert!(matches!(
        resolve_nets(&device, &backwards, &placement, &options),
        Err(ImportError::PinDirection { .. })
    ));

    // A class that the graph has no node for.
    let no_nodes = read_rr_graph(
        "<rr_graph><switches/><block_types>\
         <block_type id=\"0\" name=\"pad\" width=\"1\" height=\"1\">\
         <pin_class type=\"OUTPUT\"><pin ptc=\"0\">pad.inpad[0]</pin></pin_class>\
         <pin_class type=\"INPUT\"><pin ptc=\"1\">pad.outpad[0]</pin></pin_class>\
         </block_type></block_types>\
         <grid><grid_loc x=\"0\" y=\"0\" block_type_id=\"0\" width_offset=\"0\" height_offset=\"0\"/></grid>\
         <rr_nodes/><rr_edges/></rr_graph>"
            .as_bytes(),
    )
    .unwrap();
    let simple = "<block name=\"t\" instance=\"FPGA_packed_netlist[0]\">\
        <block name=\"in\" instance=\"pad[0]\"><outputs><port name=\"inpad\">n</port></outputs></block>\
        </block>";
    let simple = read_netlist(simple.as_bytes()).unwrap();
    let here = read_placement("in 0 0 0\n".as_bytes()).unwrap();
    assert!(matches!(
        resolve_nets(&no_nodes, &simple, &here, &options),
        Err(ImportError::NodeNotFound {
            kind: NodeKind::Source,
            class: 0,
            ..
        })
    ));
}

#[test]
fn two_pins_in_one_class_are_reported_and_cannot_be_written() {
    let device = dsp_device();
    let netlist = "<block name=\"t\" instance=\"FPGA_packed_netlist[0]\">\
        <block name=\"in\" instance=\"pad[0]\"><outputs><port name=\"inpad\">n</port></outputs></block>\
        <block name=\"d1\" instance=\"dsp[1]\" mode=\"m\"><inputs><port name=\"a\">n n</port></inputs></block>\
        </block>";
    let netlist = read_netlist(netlist.as_bytes()).unwrap();
    let placement = read_placement("in 0 0 0\nd1 1 0 1\n".as_bytes()).unwrap();
    let design = resolve_nets(&device, &netlist, &placement, &NetOptions::default()).unwrap();
    assert_eq!(design.shared_sink_nets(), [NetId(0)]);
    assert_eq!(design.nets()[0].terminals.len(), 3);
}

// Route output.

/// A device with one wire, whose ends are a pad and a block, and a net across it.
fn wire_design() -> (RrGraph, Design) {
    let text = "<rr_graph>\
        <switches>\
        <switch id=\"0\" name=\"none\"/><switch id=\"1\" name=\"mux\"><timing Tdel=\"1e-10\"/></switch>\
        <switch id=\"2\" name=\"tied\" type=\"short\"/></switches>\
        <segments><segment id=\"0\" name=\"L2\"/></segments>\
        <block_types>\
        <block_type id=\"0\" name=\"io\" width=\"1\" height=\"1\">\
        <pin_class type=\"OUTPUT\"><pin ptc=\"0\">io.inpad[0]</pin></pin_class>\
        <pin_class type=\"INPUT\"><pin ptc=\"1\">io.outpad[0]</pin></pin_class>\
        </block_type>\
        <block_type id=\"1\" name=\"tile\" width=\"1\" height=\"1\">\
        <pin_class type=\"INPUT\"><pin ptc=\"0\">tile.I[0]</pin><pin ptc=\"1\">tile.I[1]</pin></pin_class>\
        <pin_class type=\"INPUT\"><pin ptc=\"2\">tile.clk[0]</pin></pin_class>\
        <pin_class type=\"OUTPUT\"><pin ptc=\"3\">tile.O[0]</pin></pin_class>\
        </block_type>\
        </block_types>\
        <grid>\
        <grid_loc x=\"0\" y=\"0\" block_type_id=\"0\" width_offset=\"0\" height_offset=\"0\"/>\
        <grid_loc x=\"1\" y=\"0\" block_type_id=\"1\" width_offset=\"0\" height_offset=\"0\"/>\
        </grid>\
        <rr_nodes>\
        <node id=\"0\" type=\"SOURCE\" capacity=\"1\"><loc xlow=\"0\" ylow=\"0\" xhigh=\"0\" yhigh=\"0\" ptc=\"0\"/></node>\
        <node id=\"1\" type=\"OPIN\" capacity=\"1\"><loc xlow=\"0\" ylow=\"0\" xhigh=\"0\" yhigh=\"0\" ptc=\"0\"/></node>\
        <node id=\"2\" type=\"CHANX\" capacity=\"1\"><loc xlow=\"0\" ylow=\"0\" xhigh=\"1\" yhigh=\"0\" ptc=\"3\"/><segment segment_id=\"0\"/></node>\
        <node id=\"3\" type=\"IPIN\" capacity=\"1\"><loc xlow=\"1\" ylow=\"0\" xhigh=\"1\" yhigh=\"0\" ptc=\"1\"/></node>\
        <node id=\"4\" type=\"SINK\" capacity=\"2\"><loc xlow=\"1\" ylow=\"0\" xhigh=\"1\" yhigh=\"0\" ptc=\"0\"/></node>\
        <node id=\"5\" type=\"CHANY\" capacity=\"1\"><loc xlow=\"1\" ylow=\"0\" xhigh=\"1\" yhigh=\"0\" ptc=\"7\"/><segment segment_id=\"0\"/></node>\
        <node id=\"6\" type=\"CHANY\" capacity=\"1\"><loc xlow=\"1\" ylow=\"0\" xhigh=\"1\" yhigh=\"0\" ptc=\"8\"/><segment segment_id=\"0\"/></node>\
        <node id=\"7\" type=\"SOURCE\" capacity=\"1\"><loc xlow=\"1\" ylow=\"0\" xhigh=\"1\" yhigh=\"0\" ptc=\"2\"/></node>\
        </rr_nodes>\
        <rr_edges>\
        <edge src_node=\"0\" sink_node=\"1\" switch_id=\"0\"/>\
        <edge src_node=\"1\" sink_node=\"2\" switch_id=\"1\"/>\
        <edge src_node=\"2\" sink_node=\"3\" switch_id=\"1\"/>\
        <edge src_node=\"3\" sink_node=\"4\" switch_id=\"0\"/>\
        <edge src_node=\"2\" sink_node=\"5\" switch_id=\"1\"/>\
        <edge src_node=\"2\" sink_node=\"6\" switch_id=\"2\"/>\
        </rr_edges></rr_graph>";
    let device = read_rr_graph(text.as_bytes()).unwrap();
    let netlist = "<block name=\"top\" instance=\"FPGA_packed_netlist[0]\">\
        <block name=\"pad\" instance=\"io[0]\"><outputs><port name=\"inpad\">n</port></outputs></block>\
        <block name=\"logic\" instance=\"clb[0]\" mode=\"m\"><inputs><port name=\"I\">n open</port></inputs>\
        <clocks><port name=\"clk\">clock</port></clocks>\
        <outputs><port name=\"O\">lut[0].out[0]-&gt;x</port></outputs>\
        <block name=\"lut\" instance=\"lut[0]\"><outputs><port name=\"out\">local</port></outputs></block></block>\
        </block>";
    let netlist = read_netlist(netlist.as_bytes()).unwrap();
    let placement = read_placement("pad 0 0 0\nlogic 1 0 0\n".as_bytes()).unwrap();
    let design = resolve_nets(&device, &netlist, &placement, &NetOptions::default()).unwrap();
    (device, design)
}

const HEADER: RouteHeader<'static> = RouteHeader {
    placement_file: "a.place",
    placement_id: "SHA256:00",
};

fn path(nodes: &[u32]) -> Route<NodeId> {
    let mut route = Route::new();
    let nodes: Vec<NodeId> = nodes.iter().map(|&node| NodeId(node)).collect();
    route.add_path(&nodes).unwrap();
    route
}

fn written(
    device: &RrGraph,
    design: &Design,
    routes: &RouteSet<NodeId>,
) -> Result<String, RouteFileError> {
    let mut out = Vec::new();
    write_route(&mut out, device, design, routes, &HEADER)?;
    Ok(String::from_utf8(out).unwrap())
}

#[test]
fn route_files_have_vprs_layout_for_every_kind_of_net() {
    let (device, design) = wire_design();
    let mut routes = RouteSet::new();
    routes.set_route(NetId(0), path(&[0, 1, 2, 3, 4]));
    let mut out = Vec::new();
    let summary = write_route(&mut out, &device, &design, &routes, &HEADER).unwrap();
    assert_eq!(
        (summary.routed, summary.global, summary.no_sinks),
        (1, 1, 1)
    );
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "Placement_File: a.place Placement_ID: SHA256:00\n\
         Array size: 2 x 1 logic blocks.\n\
         \n\
         Routing:\n\
         \n\
         Net 0 (n)\n\
         \n\
         Node:\t0\tSOURCE (0,0,0)  Pad: 0  Switch: 0\n\
         Node:\t1\t  OPIN (0,0,0)  Pad: 0  Switch: 1\n\
         Node:\t2\t CHANX (0,0,0) to (1,0,0)  Track: 3  Switch: 1\n\
         Node:\t3\t  IPIN (1,0,0)  Pin: 1   clb.I[1] Switch: 0\n\
         Node:\t4\t  SINK (1,0,0)  Class: 0  Switch: -1 Net_pin_index: 1\n\
         \n\
         \n\
         Net 1 (local)\n\
         \n\
         \n\
         \n\
         Used in local cluster only, reserved one CLB pin\n\
         \n\
         \n\
         \n\
         Net 2 (clock): global net connecting:\n\
         \n\
         Block logic (#1) at (1,0,0), Pin class 1.\n"
    );
}

#[test]
fn nets_without_routes_are_counted_and_keep_their_heading() {
    let (device, design) = wire_design();
    let mut routes = RouteSet::new();
    routes.set_unrouted(NetId(0), crate::net::UnroutedReason::NoPath);
    let mut out = Vec::new();
    let summary = write_route(&mut out, &device, &design, &routes, &HEADER).unwrap();
    assert_eq!(summary.unrouted, 1);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("Net 0 (n)\n\n\n\nNet 1 (local)"), "{text}");
}

#[test]
fn branches_repeat_their_branch_point_with_their_own_switch() {
    // A wire to a second route onward, through a short whose switch has to show up on the
    // repeated line for the wire and not the first.
    let (device, design) = wire_design();
    let mut route = path(&[0, 1, 2, 3, 4]);
    route.add_path(&[NodeId(2), NodeId(6)]).unwrap();
    let mut routes = RouteSet::new();
    routes.set_route(NetId(0), route);
    let text = written(&device, &design, &routes).unwrap();
    assert!(
        text.contains(
            "Node:\t2\t CHANX (0,0,0) to (1,0,0)  Track: 3  Switch: 1\n\
             Node:\t3\t  IPIN (1,0,0)  Pin: 1   clb.I[1] Switch: 0\n\
             Node:\t4\t  SINK (1,0,0)  Class: 0  Switch: -1 Net_pin_index: 1\n\
             Node:\t2\t CHANX (0,0,0) to (1,0,0)  Track: 3  Switch: 2\n\
             Node:\t6\t CHANY (1,0,0)  Track: 8  Switch: -1\n"
        ),
        "{text}"
    );
}

#[test]
fn routes_that_vpr_would_reject_are_refused() {
    let (device, design) = wire_design();
    let write = |route: Route<NodeId>| {
        let mut routes = RouteSet::new();
        routes.set_route(NetId(0), route);
        written(&device, &design, &routes).unwrap_err()
    };

    // Doesn't start at the net's driver.
    assert!(matches!(
        write(path(&[1, 2, 3, 4, 0])),
        RouteFileError::InvalidRoute { .. } | RouteFileError::WrongRoot { .. }
    ));
    let mut wrong_start = path(&[1, 2, 3, 4]);
    wrong_start.add_path(&[NodeId(1), NodeId(0)]).unwrap();
    assert!(matches!(
        write(wrong_start),
        RouteFileError::WrongRoot { .. }
    ));
    // An edge that isn't in the graph, even if the tree is fine.
    assert!(matches!(
        write(path(&[0, 3, 4])),
        RouteFileError::MissingEdge { .. }
    ));
    // Missing a terminal.
    assert!(matches!(
        write(path(&[0, 1, 2])),
        RouteFileError::InvalidRoute { .. }
    ));
    // A stub through a configurable switch, which VPR calls a dangling branch.
    let mut stub = path(&[0, 1, 2, 3, 4]);
    stub.add_path(&[NodeId(2), NodeId(5)]).unwrap();
    assert!(matches!(write(stub), RouteFileError::DanglingBranch { .. }));
    // Not a tree.
    let edges = vec![
        Edge {
            from: NodeId(0),
            to: NodeId(1),
        },
        Edge {
            from: NodeId(0),
            to: NodeId(1),
        },
    ];
    assert!(matches!(
        write(Route::from_edges(NodeId(0), edges)),
        RouteFileError::InvalidRoute { .. }
    ));
}

#[test]
fn bad_headers_are_refused() {
    let (device, design) = wire_design();
    let routes = RouteSet::new();
    for header in [
        RouteHeader {
            placement_file: "",
            placement_id: "x",
        },
        RouteHeader {
            placement_file: "a b",
            placement_id: "x",
        },
        RouteHeader {
            placement_file: "a",
            placement_id: "x\ny",
        },
    ] {
        let error = write_route(Vec::new(), &device, &design, &routes, &header).unwrap_err();
        assert!(matches!(error, RouteFileError::BadHeader { .. }));
    }
}

#[test]
fn write_failures_are_reported() {
    struct Broken;
    impl std::io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("full"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let (device, design) = wire_design();
    let error = write_route(Broken, &device, &design, &RouteSet::new(), &HEADER).unwrap_err();
    assert!(matches!(error, RouteFileError::Io(_)));
    assert!(error.to_string().contains("full"));
}

// The genuine device graph.

#[test]
fn reads_the_genuine_device_graph() {
    let device = read_rr_graph(Z1000.as_bytes()).unwrap();
    let stats = device.stats();
    assert_eq!(stats.nodes, 1002);
    assert_eq!(stats.file_edges, 3277);
    assert_eq!(stats.self_loops, 0);
    assert_eq!(stats.parallel_edges, 0);
    assert_eq!(stats.non_configurable_edges, 0);
    assert_eq!(device.graph().num_edges(), 3277);
    assert_eq!(device.grid_size(), (3, 3));
    assert_eq!(device.channel_width(), Some(100));

    // The first node, as the file has it, at the same place in the graph.
    assert_eq!(device.node_kind(NodeId(0)), Some(NodeKind::ChanX));
    assert_eq!(
        device.node_direction(NodeId(0)),
        Some(ChannelDirection::Decreasing)
    );
    assert_eq!(device.node_ptc(NodeId(0)), Some(1));
    assert_eq!(device.node_segment(NodeId(0)), Some(1));
    assert_eq!(device.find_nodes(NodeKind::ChanX, 1, 1, 1), [NodeId(0)]);
    // Named, with times from the file.
    let switch = device.switch(3).unwrap();
    assert_eq!(
        switch.name,
        "z1000/subarray_0/tile_clock_iob_sb_5_cb_5_sb_5_sb_out_west_0"
    );
    assert_eq!(switch.t_del_max, 1.188_721_9e-10);
    assert_eq!(switch.t_del_min, switch.t_del_max);
    assert_eq!(switch.kind, SwitchKind::Mux);
    assert!(device.switch(1).is_none());
    assert_eq!(
        device.segment(0).map(|segment| segment.name.as_str()),
        Some("sbox_mux")
    );
    assert!(device.delay_normalization() > 0.0);
    assert!(device.heap_bytes() > 0);
}

/// Checks a device's class nodes against its `block_types`: a `SOURCE` or `SINK` is an index
/// in the list of classes of its tile, of a matching direction, and is connected only to the
/// pins of that class.
///
/// This is why pins can go through a class's index with no architecture file, so this has to
/// be true of real graphs.
fn check_class_nodes(device: &RrGraph) -> u64 {
    let mut steps = Vec::new();
    let mut checked = 0;
    // The clock spine of the z1000 reaches across the device, and that has to stay contained.
    let wire = (0..device.graph().num_nodes())
        .map(NodeId)
        .find(|&node| device.node_kind(node).is_some_and(NodeKind::is_wire));
    if let Some(wire) = wire {
        assert_eq!(device.bound().tail_reach(wire), 0);
    }
    for node in 0..device.graph().num_nodes() {
        let node = NodeId(node);
        let kind = device.node_kind(node).unwrap();
        let want = match kind {
            NodeKind::Source => PinClassKind::Output,
            NodeKind::Sink => PinClassKind::Input,
            _ => continue,
        };
        let location = device.node_location(node).unwrap();
        let tile = device
            .tile(u32::from(location.xlow), u32::from(location.ylow), 0)
            .unwrap();
        let block = &device.block_types()[usize::from(tile.block_type)];
        let class = &block.classes[device.node_ptc(node).unwrap() as usize];
        assert_eq!(class.kind, want, "{kind} of {node:?} on {}", block.name);
        if kind == NodeKind::Source {
            steps.clear();
            device.graph().expand(NetId(0), node, &mut steps);
            for step in &steps {
                assert_eq!(device.node_kind(step.to), Some(NodeKind::Opin));
                let pin = device.node_ptc(step.to).unwrap();
                assert!(class.pins.iter().any(|candidate| candidate.ptc == pin));
                checked += 1;
            }
        }
    }
    checked
}

#[test]
fn class_nodes_agree_with_the_pin_classes_of_the_genuine_graph() {
    let device = read_rr_graph(Z1000.as_bytes()).unwrap();
    assert!(check_class_nodes(&device) > 0);
}

#[test]
fn pin_names_on_the_genuine_graph_find_pins_at_each_position() {
    // z1000's I/O tiles have 4 positions with the same names, in one class each.
    let device = read_rr_graph(Z1000.as_bytes()).unwrap();
    let block = device
        .block_types()
        .iter()
        .find(|block| block.name == "clock_iob")
        .unwrap();
    assert!(block.is_io());
    let ptcs: Vec<u32> = (0..4)
        .map(|sub_block| {
            let slot = block.find_pin(sub_block, "inpad", 0).unwrap();
            block.classes[slot.class as usize].pins[0].ptc
        })
        .collect();
    assert_eq!(ptcs, [1, 3, 5, 7]);
    assert!(block.find_pin(4, "inpad", 0).is_none());
    let logic = device
        .block_types()
        .iter()
        .find(|block| block.name.starts_with("tile_clb"))
        .unwrap();
    assert!(!logic.is_io());
}

// Admissibility of the bound, against true distances.

/// Returns the cheapest cost from `source` to every node, by Dijkstra's algorithm, summing as
/// a search does, in the graph as the router sees it.
fn distances<B: crate::space::graph::LowerBound>(
    graph: &Graph<B>,
    source: NodeId,
) -> Vec<Option<Cost>> {
    use core::cmp::Reverse;
    use std::collections::BinaryHeap;

    let mut best: Vec<Option<Cost>> = vec![None; graph.num_nodes() as usize];
    let mut heap = BinaryHeap::new();
    best[source.index()] = Some(Cost::ZERO);
    heap.push((Reverse(Cost::ZERO), source.0));
    let mut steps = Vec::new();
    while let Some((Reverse(cost), node)) = heap.pop() {
        let node = NodeId(node);
        if best[node.index()].is_some_and(|known| cost > known) {
            continue;
        }
        steps.clear();
        graph.expand(NetId(0), node, &mut steps);
        for step in &steps {
            let reached = cost + step.cost;
            if best[step.to.index()].is_none_or(|known| reached < known) {
                best[step.to.index()] = Some(reached);
                heap.push((Reverse(reached), step.to.0));
            }
        }
    }
    best
}

/// Tries the bound on all pairs of `sources` and every node. Returns how many pairs had a
/// path, and how many of those it estimated above zero.
fn check_admissible(device: &RrGraph, sources: impl Iterator<Item = u32>) -> (u64, u64) {
    let graph = device.graph();
    let (mut pairs, mut useful) = (0, 0);
    for source in sources {
        let source = NodeId(source);
        let found = distances(graph, source);
        for (to, distance) in found.iter().enumerate() {
            let Some(distance) = *distance else {
                continue;
            };
            let to = NodeId(to as u32);
            let bound = graph.lower_bound(NetId(0), source, to);
            assert!(
                bound <= distance,
                "bound {} above distance {} from {source:?} to {to:?}",
                bound.get(),
                distance.get()
            );
            pairs += 1;
            useful += u64::from(bound > Cost::ZERO);
        }
    }
    (pairs, useful)
}

#[test]
fn bound_is_admissible_on_a_genuine_graph() {
    let device = read_rr_graph(Z1000.as_bytes()).unwrap();
    let (pairs, useful) = check_admissible(&device, 0..device.graph().num_nodes());
    assert!(pairs > 100_000, "only {pairs} pairs");
    assert!(useful > 1000, "only {useful} useful of {pairs}");
    assert_eq!(
        device.graph().lower_bound(NetId(0), NodeId(0), NodeId(0)),
        Cost::ZERO
    );
}

/// Writes a small FPGA in the style of VPR: an island of CLBs surrounded by wires of length
/// `length`, staggered along each track, with buffered or pass-gate switches, and with pins
/// that connect to the channels on all sides.
fn island_graph(
    size: u32,
    tracks: u32,
    length: u32,
    kind: &str,
    wire_r: f64,
    wire_c: f64,
) -> String {
    let mut nodes = String::new();
    let mut edges = String::new();
    let mut next = 0u32;
    let mut add = |kind: &str, x0: u32, y0: u32, x1: u32, y1: u32, ptc: u32, segment: bool| {
        let id = next;
        next += 1;
        let extra = if segment {
            format!("<timing R=\"{wire_r}\" C=\"{wire_c}\"/><segment segment_id=\"0\"/>")
        } else {
            String::new()
        };
        writeln!(
            nodes,
            "<node id=\"{id}\" type=\"{kind}\" capacity=\"1\"><loc xlow=\"{x0}\" ylow=\"{y0}\" xhigh=\"{x1}\" yhigh=\"{y1}\" ptc=\"{ptc}\"/>{extra}</node>"
        )
        .unwrap();
        id
    };

    // Wires along each row and column, cut up into pieces of `length`, with the cuts moved
    // along by the track number. CHANX at (x, y) runs above tile (x, y).
    let mut wires = Vec::new();
    for track in 0..tracks {
        for y in 0..=size {
            let mut x = 1;
            while x <= size {
                let stagger = if x == 1 { track % length } else { 0 };
                let end = (x + length - 1 - stagger).min(size);
                wires.push(("X", add("CHANX", x, y, end, y, track, true), x, y, end, y));
                x = end + 1;
            }
        }
        for x in 0..=size {
            let mut y = 1;
            while y <= size {
                let stagger = if y == 1 { track % length } else { 0 };
                let end = (y + length - 1 - stagger).min(size);
                wires.push(("Y", add("CHANY", x, y, x, end, track, true), x, y, x, end));
                y = end + 1;
            }
        }
    }

    // Any two wires that come together at a switch box connect, in the direction away from
    // the end of the first, as a subset pattern of tracks.
    let touching = |a: &(&str, u32, u32, u32, u32, u32), b: &(&str, u32, u32, u32, u32, u32)| {
        let gap = |low_a: u32, high_a: u32, low_b: u32, high_b: u32| {
            low_a
                .saturating_sub(high_b)
                .max(low_b.saturating_sub(high_a))
        };
        gap(a.2, a.4, b.2, b.4) + gap(a.3, a.5, b.3, b.5) <= 1
    };
    for a in &wires {
        for b in &wires {
            if a.1 != b.1 && touching(a, b) && (a.1 + b.1) % tracks == 0 {
                writeln!(
                    edges,
                    "<edge src_node=\"{}\" sink_node=\"{}\" switch_id=\"1\"/>",
                    a.1, b.1
                )
                .unwrap();
            }
        }
    }

    // Each CLB: a source and an output pin that reaches the wires around it, and an input
    // pin and a sink that those wires reach.
    let mut blocks = String::new();
    for y in 1..=size {
        for x in 1..=size {
            let source = add("SOURCE", x, y, x, y, 1, false);
            let opin = add("OPIN", x, y, x, y, 1, false);
            let ipin = add("IPIN", x, y, x, y, 0, false);
            let sink = add("SINK", x, y, x, y, 0, false);
            writeln!(
                edges,
                "<edge src_node=\"{source}\" sink_node=\"{opin}\" switch_id=\"0\"/>"
            )
            .unwrap();
            writeln!(
                edges,
                "<edge src_node=\"{ipin}\" sink_node=\"{sink}\" switch_id=\"0\"/>"
            )
            .unwrap();
            for wire in &wires {
                let beside = wire.2 <= x && x <= wire.4 && wire.3 <= y && y <= wire.5;
                if beside {
                    writeln!(
                        edges,
                        "<edge src_node=\"{opin}\" sink_node=\"{}\" switch_id=\"1\"/>",
                        wire.1
                    )
                    .unwrap();
                    writeln!(
                        edges,
                        "<edge src_node=\"{}\" sink_node=\"{ipin}\" switch_id=\"1\"/>",
                        wire.1
                    )
                    .unwrap();
                }
            }
        }
    }
    for y in 0..=size + 1 {
        for x in 0..=size + 1 {
            let block = if x == 0 || y == 0 || x > size || y > size {
                0
            } else {
                1
            };
            writeln!(blocks, "<grid_loc x=\"{x}\" y=\"{y}\" block_type_id=\"{block}\" width_offset=\"0\" height_offset=\"0\"/>").unwrap();
        }
    }

    format!(
        "<rr_graph>\n<switches>\n\
         <switch id=\"0\" name=\"none\" type=\"mux\"><timing Tdel=\"0\"/></switch>\n\
         <switch id=\"1\" name=\"sw\" type=\"{kind}\"><timing R=\"500\" Cout=\"1e-15\" Cin=\"1e-15\" Tdel=\"5e-11\"/></switch>\n\
         </switches>\n<segments><segment id=\"0\" name=\"wire\" length=\"{length}\"/></segments>\n\
         <block_types><block_type id=\"0\" name=\"EMPTY\" width=\"1\" height=\"1\"/>\
         <block_type id=\"1\" name=\"clb\" width=\"1\" height=\"1\">\
         <pin_class type=\"INPUT\"><pin ptc=\"0\">clb.I[0]</pin></pin_class>\
         <pin_class type=\"OUTPUT\"><pin ptc=\"1\">clb.O[0]</pin></pin_class></block_type></block_types>\n\
         <grid>\n{blocks}</grid>\n<rr_nodes>\n{nodes}</rr_nodes>\n<rr_edges>\n{edges}</rr_edges>\n</rr_graph>\n"
    )
}

#[test]
fn bound_is_admissible_on_realistic_small_graphs() {
    for (size, tracks, length, kind) in [
        (4, 4, 1, "mux"),
        (4, 4, 4, "mux"),
        (5, 6, 2, "pass_gate"),
        (3, 3, 3, "tristate"),
    ] {
        let text = island_graph(size, tracks, length, kind, 200.0, 5e-14);
        let device = read_rr_graph(text.as_bytes()).unwrap();
        let (pairs, useful) = check_admissible(&device, 0..device.graph().num_nodes());
        assert!(
            useful > 0,
            "{pairs} pairs, none useful for {size} {tracks} {length} {kind}"
        );
        assert!(check_class_nodes(&device) > 0);
    }
}

#[test]
fn bound_handles_wide_sinks_and_long_jumps() {
    // A SINK that's free to enter and two tiles wide, and an edge that jumps across a tile,
    // which is a shortcut that the cost per tile has to allow for.
    let switches = timed_switch(0, "mux", "1e-10");
    let nodes = "\
        <node id=\"0\" type=\"CHANX\" capacity=\"1\"><loc xlow=\"0\" ylow=\"0\" xhigh=\"0\" yhigh=\"0\" ptc=\"0\"/><segment segment_id=\"0\"/></node>\
        <node id=\"1\" type=\"CHANX\" capacity=\"1\"><loc xlow=\"1\" ylow=\"0\" xhigh=\"1\" yhigh=\"0\" ptc=\"0\"/><segment segment_id=\"0\"/></node>\
        <node id=\"2\" type=\"CHANX\" capacity=\"1\"><loc xlow=\"2\" ylow=\"0\" xhigh=\"2\" yhigh=\"0\" ptc=\"0\"/><segment segment_id=\"0\"/></node>\
        <node id=\"3\" type=\"SINK\" capacity=\"1\"><loc xlow=\"1\" ylow=\"0\" xhigh=\"2\" yhigh=\"0\" ptc=\"0\"/></node>";
    let edges = [edge(0, 1, 0), edge(1, 2, 0), edge(2, 3, 0), edge(0, 2, 0)].concat();
    let device = read_rr_graph(small_graph(&switches, nodes, &edges).as_bytes()).unwrap();
    // Node 1 leads on to more, so the whole of its distance counts.
    let bound = device.graph().lower_bound(NetId(0), NodeId(0), NodeId(1));
    assert!(bound > Cost::ZERO);
    assert!(bound <= distances(device.graph(), NodeId(0))[1].unwrap());
    // Node 2 leads only to the SINK, and is reached from two tiles away by the jump.
    assert_eq!(device.bound().tail_reach(NodeId(1)), 0);
    assert_eq!(device.bound().tail_reach(NodeId(2)), 2);
    assert_eq!(device.bound().tail_reach(NodeId(3)), 3);
    assert_eq!(
        device.graph().lower_bound(NetId(0), NodeId(0), NodeId(3)),
        Cost::ZERO
    );
    check_admissible(&device, 0..4);
}

/// Checks the map lookahead against true distances on `device`, from every node. Returns how
/// many pairs of a wire and a `SINK` it can reach there are, and how many of those the map
/// put above their distance.
fn check_map_lookahead(device: &RrGraph) -> (u64, u64) {
    let mut mapped = device.clone();
    mapped.use_map_lookahead(&LookaheadOptions::default());
    assert!(mapped.map_lookahead().is_some());
    assert!(mapped.heap_bytes() > device.heap_bytes());
    let (mut pairs, mut above) = (0, 0);
    for from in 0..device.graph().num_nodes() {
        let from = NodeId(from);
        let found = distances(device.graph(), from);
        for to in 0..device.graph().num_nodes() {
            let to = NodeId(to);
            let location = device.graph().lower_bound(NetId(0), from, to);
            let map = mapped.graph().lower_bound(NetId(0), from, to);
            match found[to.index()] {
                // What the map says can't be reached can't.
                None => {}
                Some(distance) => {
                    assert!(
                        map < Cost::MAX,
                        "{from:?} reaches {to:?} at {}",
                        distance.get()
                    );
                    // It never knows less than the location bound.
                    assert!(map >= location, "{from:?} to {to:?}");
                    let wire = device.node_kind(from).is_some_and(NodeKind::is_wire);
                    if wire && device.node_kind(to) == Some(NodeKind::Sink) {
                        pairs += 1;
                        above += u64::from(map > distance);
                    }
                }
            }
            if found[to.index()].is_none()
                && matches!(
                    device.node_kind(from),
                    Some(NodeKind::Ipin | NodeKind::Sink)
                )
            {
                // A pin, or a sink, that has no way there is known to have none.
                assert_eq!(map, Cost::MAX, "{from:?} to {to:?}");
            }
        }
    }
    (pairs, above)
}

#[test]
fn map_lookahead_knows_more_than_location_and_rarely_too_much() {
    for (size, tracks, length, kind) in [(5, 4, 1, "mux"), (6, 6, 2, "mux"), (5, 4, 4, "pass_gate")]
    {
        let text = island_graph(size, tracks, length, kind, 200.0, 5e-14);
        let device = read_rr_graph(text.as_bytes()).unwrap();
        let (pairs, above) = check_map_lookahead(&device);
        assert!(pairs > 100, "{pairs} pairs");
        // Sampled tables are not a bound everywhere, but nearly. Edges look different.
        assert!(
            above * 5 <= pairs,
            "{above} of {pairs} above for {size} {tracks} {length}"
        );
    }
    let device = read_rr_graph(Z1000.as_bytes()).unwrap();
    check_map_lookahead(&device);
}

#[test]
fn map_lookahead_delays_bound_like_its_costs() {
    let text = island_graph(5, 4, 2, "mux", 200.0, 5e-14);
    let mut device = read_rr_graph(text.as_bytes()).unwrap();
    let plain: Vec<u32> = {
        let timed = device.timed_graph();
        (0..device.graph().num_nodes())
            .flat_map(|from| (0..device.graph().num_nodes()).map(move |to| (from, to)))
            .map(|(from, to)| timed.delay_lower_bound(NetId(0), NodeId(from), NodeId(to)))
            .collect()
    };
    device.use_map_lookahead(&LookaheadOptions {
        samples_per_axis: 3,
        ..LookaheadOptions::default()
    });
    let timed = device.timed_graph();
    let count = device.graph().num_nodes();
    let mut tighter = 0;
    for from in 0..count {
        for to in 0..count {
            let map = timed.delay_lower_bound(NetId(0), NodeId(from), NodeId(to));
            let before = plain[(from * count + to) as usize];
            assert!(map >= before);
            tighter += u32::from(map > before);
        }
    }
    assert!(tighter > 0);
    // From a node to itself, nothing.
    assert_eq!(timed.delay_lower_bound(NetId(0), NodeId(7), NodeId(7)), 0);
    assert_eq!(
        device.graph().lower_bound(NetId(0), NodeId(7), NodeId(7)),
        Cost::ZERO
    );
}

proptest! {
    /// Random graphs that follow the format, without any structure to make them kind: edges
    /// can jump anywhere, so the bound has to hold on what the file says, whatever that is.
    #[test]
    fn bound_is_admissible_on_any_graph(
        spots in proptest::collection::vec((0u32..6, 0u32..6, 0u32..3, 0u32..3, 0u8..6), 2..16),
        wiring in proptest::collection::vec((any::<prop::sample::Index>(), any::<prop::sample::Index>()), 0..48),
    ) {
        let mut nodes = String::new();
        for (id, (x, y, wide, tall, kind)) in spots.iter().enumerate() {
            let kind = ["SOURCE", "SINK", "OPIN", "IPIN", "CHANX", "CHANY"][usize::from(*kind)];
            let segment = if kind.starts_with("CHAN") { "<segment segment_id=\"0\"/>" } else { "" };
            writeln!(
                nodes,
                "<node id=\"{id}\" type=\"{kind}\" capacity=\"1\"><loc xlow=\"{x}\" ylow=\"{y}\" xhigh=\"{}\" yhigh=\"{}\" ptc=\"0\"/>{segment}</node>",
                x + wide,
                y + tall,
            )
            .unwrap();
        }
        let mut edges = String::new();
        for (from, to) in &wiring {
            let (from, to) = (from.index(spots.len()), to.index(spots.len()));
            writeln!(edges, "{}", edge(from as u32, to as u32, 0)).unwrap();
        }
        let text = small_graph(&timed_switch(0, "mux", "1e-10"), &nodes, &edges);
        let device = read_rr_graph(text.as_bytes()).unwrap();
        check_admissible(&device, 0..spots.len() as u32);
    }

    #[test]
    fn random_input_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = read_rr_graph(bytes.as_slice());
    }

    /// Cutting a graph short at any point fails with a line that's within the file.
    #[test]
    fn truncated_graphs_are_errors_with_a_sensible_line(cut: prop::sample::Index) {
        let end = Z1000.trim_end().len();
        let at = cut.index(end);
        let error = read_rr_graph(&Z1000.as_bytes()[..at]);
        match error {
            Err(error) => {
                let lines = Z1000.as_bytes()[..at].iter().filter(|&&b| b == b'\n').count() as u64;
                let line = error.line().unwrap();
                prop_assert!(line >= 1 && line <= lines + 1);
            }
            Ok(_) => prop_assert!(false, "a graph cut at {} was accepted", at),
        }
    }

    /// Damage a graph at random, keeping to characters that matter. Whatever comes of it, it
    /// isn't a panic, and if it's still accepted, the result is safe to query and to route on.
    #[test]
    fn damaged_graphs_never_panic(
        edits in proptest::collection::vec(
            (any::<prop::sample::Index>(), proptest::sample::select(b"0123456789<>/\"=- \n".to_vec())),
            1..8,
        ),
    ) {
        let mut bytes = wire_design_text().into_bytes();
        for (at, byte) in edits {
            let at = at.index(bytes.len());
            bytes[at] = byte;
        }
        if let Ok(device) = read_rr_graph(bytes.as_slice()) {
            let graph = device.graph();
            let mut steps = Vec::new();
            for node in 0..graph.num_nodes() {
                let node = NodeId(node);
                steps.clear();
                graph.expand(NetId(0), node, &mut steps);
                prop_assert_eq!(steps.len(), device.out_switches(node).len());
                let kind = device.node_kind(node).unwrap();
                let location = device.node_location(node).unwrap();
                let ptc = device.node_ptc(node).unwrap();
                prop_assert!(device
                    .find_nodes(kind, u32::from(location.xlow), u32::from(location.ylow), ptc)
                    .contains(&node));
                let _ = graph.lower_bound(NetId(0), node, NodeId(0));
            }
        }
    }

    #[test]
    fn buffer_size_does_not_matter_to_a_graph(capacity in 1usize..64) {
        let text = wire_design_text();
        let whole = read_rr_graph(text.as_bytes()).unwrap();
        let chunked = read_rr_graph(BufReader::with_capacity(capacity, text.as_bytes())).unwrap();
        prop_assert_eq!(whole.stats(), chunked.stats());
        prop_assert_eq!(whole.delay_normalization(), chunked.delay_normalization());
        prop_assert_eq!(whole.heap_bytes(), chunked.heap_bytes());
    }

    /// Any placement of a real netlist, right or wrong, gives nets or an error.
    #[test]
    fn scrambled_placements_never_panic(
        placed in proptest::collection::vec((0u32..5, 0u32..5, 0u32..9), 19),
    ) {
        let netlist = read_netlist(NET.as_bytes()).unwrap();
        let nets = parse_route(ROUTE);
        let xml = graph_from_route(&nets, &k6_n10_block_types_and_grid(4, 4));
        let device = read_rr_graph(xml.as_bytes()).unwrap();
        let mut text = String::new();
        for (block, (x, y, sub)) in netlist.blocks().iter().zip(&placed) {
            writeln!(text, "{} {x} {y} {sub}", block.name).unwrap();
        }
        let placement = read_placement(text.as_bytes()).unwrap();
        let _ = resolve_nets(&device, &netlist, &placement, &NetOptions::default());
    }
}

fn wire_design_text() -> String {
    small_graph(
        &[
            timed_switch(0, "mux", "1e-10"),
            timed_switch(1, "short", "0"),
        ]
        .concat(),
        &[wire(0, 0), wire(1, 1), wire(2, 2)].concat(),
        &[edge(0, 1, 0), edge(1, 2, 1), edge(2, 0, 0)].concat(),
    )
}

// Measurements on full-size files, which aren't in the repository. Run these with a file
// named in `VTR_RR_GRAPH`, in release mode, as in:
//
//     VTR_RR_GRAPH=z1000_rr_graph.xml cargo test --release --features fpga -- \
//         --ignored --nocapture measure_
//
// One of each per process, as peak memory never goes back down.

/// Returns the peak memory of this process so far, in bytes, on Linux.
fn peak_memory() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
    let kilobytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kilobytes * 1024)
}

/// A reader that counts its bytes, to report what was read when it wasn't all in a file.
struct Counting<R> {
    inner: R,
    bytes: u64,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(out)?;
        self.bytes += count as u64;
        Ok(count)
    }
}

fn open_measured_file() -> Option<BufReader<Counting<std::fs::File>>> {
    let path = std::env::var_os("VTR_RR_GRAPH")?;
    let inner = std::fs::File::open(path).unwrap();
    Some(BufReader::with_capacity(
        1 << 20,
        Counting { inner, bytes: 0 },
    ))
}

#[test]
#[ignore = "needs a full device graph, named in VTR_RR_GRAPH"]
fn measure_parse_only() {
    let Some(mut file) = open_measured_file() else {
        return;
    };
    let baseline = peak_memory().unwrap_or(0);
    let start = std::time::Instant::now();
    let mut parser = xml::XmlReader::new(&mut file);
    let mut events = 0u64;
    while parser.next_event().unwrap().is_some() {
        events += 1;
    }
    let lines = parser.line();
    let elapsed = start.elapsed().as_secs_f64();
    let bytes = file.get_ref().bytes;
    println!(
        "XML only: {} MB, {events} events, {lines} lines, {elapsed:.2} s, {:.0} MB/s, peak {} MB over a baseline of {} MB",
        bytes / 1_000_000,
        bytes as f64 / 1e6 / elapsed,
        peak_memory().unwrap_or(0) / 1_000_000,
        baseline / 1_000_000,
    );
}

#[test]
#[ignore = "needs a full device graph, named in VTR_RR_GRAPH"]
fn measure_import() {
    let Some(mut file) = open_measured_file() else {
        return;
    };
    let start = std::time::Instant::now();
    let device = read_rr_graph(&mut file).unwrap();
    let elapsed = start.elapsed().as_secs_f64();
    let bytes = file.get_ref().bytes;
    let stats = *device.stats();
    println!(
        "import: {} MB, {} nodes, {} edges, {elapsed:.2} s, {:.0} MB/s, peak {} MB, kept {} MB ({} in the Graph)",
        bytes / 1_000_000,
        stats.nodes,
        stats.file_edges,
        bytes as f64 / 1e6 / elapsed,
        peak_memory().unwrap_or(0) / 1_000_000,
        device.heap_bytes() / 1_000_000,
        device.graph().heap_bytes() / 1_000_000,
    );
    println!("{stats:?}");
    println!(
        "delay normalization {:e}, cost per tile {:e}",
        device.delay_normalization(),
        device.bound().cost_per_tile(),
    );
    println!(
        "{} class-to-pin edges agree with block_types",
        check_class_nodes(&device)
    );

    // How much of the true distance the bound sees, from a sample of sources.
    let graph = device.graph();
    let (mut bound_total, mut distance_total, mut pairs) = (0.0, 0.0, 0u64);
    for source in (0..graph.num_nodes()).step_by((graph.num_nodes() as usize / 16).max(1)) {
        let source = NodeId(source);
        for (to, distance) in distances(graph, source).iter().enumerate() {
            if let Some(distance) = distance {
                let bound = graph.lower_bound(NetId(0), source, NodeId(to as u32));
                assert!(bound <= *distance, "not admissible from {source:?} to {to}");
                bound_total += bound.get();
                distance_total += distance.get();
                pairs += 1;
            }
        }
    }
    println!(
        "bound over distance {:.3} on {pairs} sampled pairs",
        bound_total / distance_total
    );
}

// Delays for timing-driven routing.

#[test]
fn step_delays_are_the_elmore_delay_of_a_buffered_step() {
    let switches = "<switch id=\"0\" name=\"m\" type=\"mux\"><timing R=\"100\" Cinternal=\"1e-15\" Tdel=\"4e-11\"/></switch>\
                    <switch id=\"1\" name=\"slow\" type=\"mux\"><timing Tdel=\"1e-10\"/></switch>";
    let timed = |id: u32, x: u32| {
        format!(
            "<node id=\"{id}\" type=\"CHANX\" capacity=\"1\"><loc xlow=\"{x}\" ylow=\"0\" xhigh=\"{x}\" yhigh=\"0\" ptc=\"0\"/>\
             <timing R=\"100\" C=\"2e-14\"/><segment segment_id=\"0\"/></node>"
        )
    };
    let nodes = [timed(0, 0), timed(1, 1), timed(2, 2)].concat();
    let edges = [edge(0, 1, 0), edge(1, 2, 0), edge(0, 2, 1), edge(2, 1, 0)].concat();
    let device = read_rr_graph(small_graph(switches, &nodes, &edges).as_bytes()).unwrap();
    // Tdel + R(switch) C(node) + R(node) C(node) / 2 is 40 + 2 + 1 ps, leaving out the
    // internal capacitance. The slow switch has no R, so it is 100 + 1.
    assert_eq!(device.step_delay(0, NodeId(1)), 43);
    assert_eq!(device.step_delay(1, NodeId(2)), 101);
    assert_eq!(device.step_delay(9, NodeId(2)), 1);
    assert_eq!(device.step_delay(0, NodeId(99)), 40);

    let timed_graph = device.timed_graph();
    let mut delays = Vec::new();
    timed_graph.step_delays(NetId(0), NodeId(0), &mut delays);
    let mut steps = Vec::new();
    device.graph().expand(NetId(0), NodeId(0), &mut steps);
    let expected: Vec<u32> = steps
        .iter()
        .zip(device.out_switches(NodeId(0)))
        .map(|(step, &switch)| device.step_delay(switch, step.to))
        .collect();
    assert_eq!(delays, expected);
    assert_eq!(delays, [43, 101]);
    let mut timed_steps = Vec::new();
    timed_graph.expand(NetId(0), NodeId(0), &mut timed_steps);
    assert_eq!(timed_steps, steps);
    // The fastest rate is 43 ps for the one tile between neighbours, against 101 for two.
    assert_eq!(timed_graph.delay_bound().picoseconds_per_tile(), 43.0);
    // Two tiles, rounded down after a margin: the true delay is 86, through node 1.
    assert_eq!(
        timed_graph.delay_lower_bound(NetId(0), NodeId(0), NodeId(2)),
        85
    );
    assert_eq!(assert_delay_bound_holds(&device, (0..3).map(NodeId)), 7);
}

/// Checks every bound from each of `sources` against the true smallest sum of delays, found by
/// Dijkstra's algorithm on delays alone, and returns how many pairs were checked.
fn assert_delay_bound_holds(device: &RrGraph, sources: impl Iterator<Item = NodeId>) -> usize {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    let timed = device.timed_graph();
    let nodes = device.graph().num_nodes() as usize;
    let mut checked = 0;
    let (mut steps, mut delays) = (Vec::new(), Vec::new());
    for source in sources {
        let mut distance = vec![u64::MAX; nodes];
        let mut queue = BinaryHeap::from([Reverse((0u64, source.0))]);
        distance[source.index()] = 0;
        while let Some(Reverse((here, node))) = queue.pop() {
            if here > distance[node as usize] {
                continue;
            }
            steps.clear();
            delays.clear();
            timed.expand(NetId(0), NodeId(node), &mut steps);
            timed.step_delays(NetId(0), NodeId(node), &mut delays);
            for (step, &delay) in steps.iter().zip(&delays) {
                let there = here + u64::from(delay);
                if there < distance[step.to.index()] {
                    distance[step.to.index()] = there;
                    queue.push(Reverse((there, step.to.0)));
                }
            }
        }
        for (target, &true_delay) in distance.iter().enumerate() {
            if true_delay != u64::MAX {
                let bound = timed.delay_lower_bound(NetId(0), source, NodeId(target as u32));
                assert!(
                    u64::from(bound) <= true_delay,
                    "{source:?} to {target}: bound {bound} above {true_delay}"
                );
                checked += 1;
            }
        }
    }
    checked
}

#[test]
fn the_delay_bound_holds_on_a_genuine_device_graph() {
    let device = read_rr_graph(Z1000.as_bytes()).unwrap();
    // The z1000's wires have no R or C, so its delays are its switches' Tdel.
    assert!(device.timed_graph().delay_bound().picoseconds_per_tile() > 0.0);
    let nodes = device.graph().num_nodes();
    let checked = assert_delay_bound_holds(&device, (0..nodes).step_by(7).map(NodeId));
    assert!(checked > 1000, "{checked}");
}

#[test]
fn the_delay_bound_holds_on_a_graph_from_vprs_routes() {
    let (device, _, _) = mult_4x4();
    let nodes = device.graph().num_nodes();
    let checked = assert_delay_bound_holds(&device, (0..nodes).map(NodeId));
    assert!(checked > 100, "{checked}");
}

// Placement.

/// A device of three columns: pads two to a tile, logic blocks with a carry chain's pins, and
/// rams two tiles tall, whose second input is on their upper tile.
const PLACE_DEVICE: &str = "<rr_graph>\
    <switches><switch id=\"0\" name=\"s\"><timing Tdel=\"1e-10\"/></switch></switches>\
    <segments><segment id=\"0\" name=\"L1\"/></segments>\
    <block_types>\
    <block_type id=\"0\" name=\"EMPTY\" width=\"1\" height=\"1\"/>\
    <block_type id=\"1\" name=\"io\" width=\"1\" height=\"1\">\
    <pin_class type=\"OUTPUT\"><pin ptc=\"0\">io[0].inpad[0]</pin></pin_class>\
    <pin_class type=\"INPUT\"><pin ptc=\"1\">io[0].outpad[0]</pin></pin_class>\
    <pin_class type=\"OUTPUT\"><pin ptc=\"2\">io[1].inpad[0]</pin></pin_class>\
    <pin_class type=\"INPUT\"><pin ptc=\"3\">io[1].outpad[0]</pin></pin_class>\
    </block_type>\
    <block_type id=\"2\" name=\"clb\" width=\"1\" height=\"1\">\
    <pin_class type=\"INPUT\"><pin ptc=\"0\">clb.I[0]</pin></pin_class>\
    <pin_class type=\"INPUT\"><pin ptc=\"1\">clb.cin[0]</pin></pin_class>\
    <pin_class type=\"OUTPUT\"><pin ptc=\"2\">clb.O[0]</pin></pin_class>\
    <pin_class type=\"OUTPUT\"><pin ptc=\"3\">clb.cout[0]</pin></pin_class>\
    </block_type>\
    <block_type id=\"3\" name=\"ram\" width=\"1\" height=\"2\">\
    <pin_class type=\"INPUT\"><pin ptc=\"0\">ram.a[0]</pin></pin_class>\
    <pin_class type=\"INPUT\"><pin ptc=\"1\">ram.a[1]</pin></pin_class>\
    <pin_class type=\"OUTPUT\"><pin ptc=\"2\">ram.o[0]</pin></pin_class>\
    </block_type>\
    </block_types>\
    <grid>\
    <grid_loc x=\"0\" y=\"0\" block_type_id=\"1\" width_offset=\"0\" height_offset=\"0\"/>\
    <grid_loc x=\"0\" y=\"1\" block_type_id=\"1\" width_offset=\"0\" height_offset=\"0\"/>\
    <grid_loc x=\"0\" y=\"2\" block_type_id=\"1\" width_offset=\"0\" height_offset=\"0\"/>\
    <grid_loc x=\"0\" y=\"3\" block_type_id=\"1\" width_offset=\"0\" height_offset=\"0\"/>\
    <grid_loc x=\"1\" y=\"0\" block_type_id=\"2\" width_offset=\"0\" height_offset=\"0\"/>\
    <grid_loc x=\"1\" y=\"1\" block_type_id=\"2\" width_offset=\"0\" height_offset=\"0\"/>\
    <grid_loc x=\"1\" y=\"2\" block_type_id=\"2\" width_offset=\"0\" height_offset=\"0\"/>\
    <grid_loc x=\"1\" y=\"3\" block_type_id=\"2\" width_offset=\"0\" height_offset=\"0\"/>\
    <grid_loc x=\"2\" y=\"0\" block_type_id=\"3\" width_offset=\"0\" height_offset=\"0\"/>\
    <grid_loc x=\"2\" y=\"1\" block_type_id=\"3\" width_offset=\"0\" height_offset=\"1\"/>\
    <grid_loc x=\"2\" y=\"2\" block_type_id=\"3\" width_offset=\"0\" height_offset=\"0\"/>\
    <grid_loc x=\"2\" y=\"3\" block_type_id=\"3\" width_offset=\"0\" height_offset=\"1\"/>\
    </grid>\
    <rr_nodes>\
    <node id=\"0\" type=\"IPIN\" capacity=\"1\"><loc xlow=\"2\" ylow=\"0\" xhigh=\"2\" yhigh=\"0\" side=\"RIGHT\" ptc=\"0\"/></node>\
    <node id=\"1\" type=\"IPIN\" capacity=\"1\"><loc xlow=\"2\" ylow=\"1\" xhigh=\"2\" yhigh=\"1\" side=\"RIGHT\" ptc=\"1\"/></node>\
    <node id=\"2\" type=\"IPIN\" capacity=\"1\"><loc xlow=\"2\" ylow=\"1\" xhigh=\"2\" yhigh=\"1\" side=\"LEFT\" ptc=\"1\"/></node>\
    <node id=\"3\" type=\"OPIN\" capacity=\"1\"><loc xlow=\"2\" ylow=\"0\" xhigh=\"2\" yhigh=\"0\" side=\"LEFT\" ptc=\"2\"/></node>\
    <node id=\"4\" type=\"OPIN\" capacity=\"1\"><loc xlow=\"2\" ylow=\"1\" xhigh=\"2\" yhigh=\"1\" side=\"LEFT\" ptc=\"2\"/></node>\
    <node id=\"5\" type=\"OPIN\" capacity=\"1\"><loc xlow=\"2\" ylow=\"1\" xhigh=\"2\" yhigh=\"1\" side=\"RIGHT\" ptc=\"2\"/></node>\
    </rr_nodes>\
    <rr_edges/>\
    </rr_graph>";

/// A logic block of [`PLACE_DEVICE`], with its inputs, and the nets on its two outputs.
fn place_clb(
    name: &str,
    index: u32,
    input: &str,
    carry_in: &str,
    carry_out: Option<&str>,
) -> String {
    let (cout, cout_block) = match carry_out {
        Some(net) => (
            "q[0].o[0]-&gt;x".to_owned(),
            format!(
                "<block name=\"{net}\" instance=\"q[0]\"><outputs><port name=\"o\">{net}</port></outputs></block>"
            ),
        ),
        None => ("open".to_owned(), String::new()),
    };
    format!(
        "<block name=\"{name}\" instance=\"clb[{index}]\" mode=\"m\">\
         <inputs><port name=\"I\">{input}</port><port name=\"cin\">{carry_in}</port></inputs>\
         <outputs><port name=\"O\">p[0].o[0]-&gt;x</port><port name=\"cout\">{cout}</port></outputs>\
         <block name=\"n{index}\" instance=\"p[0]\"><outputs><port name=\"o\">n{index}</port></outputs></block>\
         {cout_block}</block>"
    )
}

/// A pad, three logic blocks in a carry chain, a ram, and a pad out.
fn place_netlist() -> PackedNetlist {
    let text = format!(
        "<block name=\"t\" instance=\"FPGA_packed_netlist[0]\">\
         <block name=\"in\" instance=\"io[0]\" mode=\"m\"><outputs><port name=\"inpad\">inpad[0].inpad[0]-&gt;x</port></outputs>\
         <block name=\"in\" instance=\"inpad[0]\"><outputs><port name=\"inpad\">in</port></outputs></block></block>\
         {}{}{}\
         <block name=\"r0\" instance=\"ram[3]\" mode=\"m\"><inputs><port name=\"a\">n2 n0</port></inputs>\
         <outputs><port name=\"o\">m[0].o[0]-&gt;x</port></outputs>\
         <block name=\"r\" instance=\"m[0]\"><outputs><port name=\"o\">r</port></outputs></block></block>\
         <block name=\"out:r\" instance=\"io[1]\" mode=\"m\"><inputs><port name=\"outpad\">r</port></inputs></block>\
         </block>",
        place_clb("c0", 0, "in", "open", Some("k0")),
        place_clb("c1", 1, "n0", "k0", Some("k1")),
        place_clb("c2", 2, "n1", "k1", None),
    );
    read_netlist(text.as_bytes()).unwrap()
}

const CARRY: &str = "<architecture><directlist>\
    <direct name=\"carry\" from_pin=\"clb.cout\" to_pin=\"clb.cin\" x_offset=\"0\" y_offset=\"-1\" z_offset=\"0\"/>\
    </directlist></architecture>";

#[test]
fn directs_are_read_from_the_architecture() {
    let arch = read_architecture(CARRY.as_bytes()).unwrap();
    let direct = &arch.directs()[0];
    assert_eq!(direct.name, "carry");
    assert_eq!(
        (direct.from_type.as_str(), direct.from_port.as_str()),
        ("clb", "cout")
    );
    assert_eq!(
        (direct.to_type.as_str(), direct.to_port.as_str()),
        ("clb", "cin")
    );
    assert_eq!(
        (direct.x_offset, direct.y_offset, direct.z_offset),
        (0, -1, 0)
    );
    let ranged = "<architecture><directlist><direct name=\"d\" from_pin=\"a.o[0:0]\" to_pin=\"b.i\"/></directlist></architecture>";
    let ranged = read_architecture(ranged.as_bytes()).unwrap();
    let direct = &ranged.directs()[0];
    assert_eq!((direct.from_port.as_str(), direct.y_offset), ("o", 0));
    for bad in [
        "<architecture><directlist><direct name=\"d\" from_pin=\"a\" to_pin=\"b.i\"/></directlist></architecture>",
        "<architecture><directlist><direct name=\"d\" from_pin=\"a.o\" to_pin=\"b.i\" y_offset=\"up\"/></directlist></architecture>",
        "<architecture><directlist><direct from_pin=\"a.o\" to_pin=\"b.i\"/></directlist></architecture>",
    ] {
        assert!(read_architecture(bad.as_bytes()).is_err(), "{bad}");
    }
}

#[test]
fn a_place_problem_has_the_devices_sites_and_the_netlists_chains() {
    let device = read_rr_graph(PLACE_DEVICE.as_bytes()).unwrap();
    assert_eq!(
        device
            .block_types()
            .iter()
            .map(BlockType::capacity)
            .collect::<Vec<_>>(),
        [0, 2, 1, 1]
    );
    let netlist = place_netlist();
    let arch = read_architecture(CARRY.as_bytes()).unwrap();
    let problem =
        build_place_problem(&device, &netlist, &arch, &ProblemOptions::default()).unwrap();
    let places = problem.problem();
    assert_eq!(problem.grid_size(), (3, 4));

    // A site per position: two per pad tile, and one at the foot of each ram.
    let kinds: Vec<u32> = places.sites().iter().map(|site| site.kind).collect();
    assert_eq!(kinds.iter().filter(|&&kind| kind == 1).count(), 8);
    assert_eq!(kinds.iter().filter(|&&kind| kind == 2).count(), 4);
    assert_eq!(kinds.iter().filter(|&&kind| kind == 3).count(), 2);
    let rams: Vec<(u32, u32, u32)> = (0..places.sites().len() as u32)
        .filter(|&site| kinds[site as usize] == 3)
        .map(|site| problem.site_location(crate::place::SiteId(site)).unwrap())
        .collect();
    assert_eq!(rams, [(2, 0, 0), (2, 2, 0)]);

    // The chain runs downwards from c0, as the direct connection goes.
    assert_eq!(problem.chains(), (1, 3));
    let p = crate::place::Point::new;
    assert_eq!(
        places.macros(),
        [vec![
            (problem.item(1), p(0, 0)),
            (problem.item(2), p(0, -1)),
            (problem.item(3), p(0, -2)),
        ]]
    );

    // Every net with a driver and a sink, carry nets too, and n0 reaches the ram's upper tile.
    assert_eq!(places.net_count(), 7);
    let n0 = (0..places.net_count() as u32)
        .map(NetId)
        .find(|&net| netlist.nets()[problem.packed_net(net).unwrap().index()].name == "n0")
        .unwrap();
    assert_eq!(
        places.net_pins(n0),
        [
            crate::place::Pin::new(problem.item(1)),
            crate::place::Pin::new(problem.item(2)),
            crate::place::Pin::at(problem.item(4), 0, 1),
        ]
    );

    // Ignored nets are left out.
    let options = ProblemOptions {
        ignored_nets: vec![problem.packed_net(n0).unwrap()],
        ..ProblemOptions::default()
    };
    let fewer = build_place_problem(&device, &netlist, &arch, &options).unwrap();
    assert_eq!(fewer.problem().net_count(), 6);
}

#[test]
fn designs_are_placed_legally_and_written_for_vpr() {
    let device = read_rr_graph(PLACE_DEVICE.as_bytes()).unwrap();
    let netlist = place_netlist();
    let arch = read_architecture(CARRY.as_bytes()).unwrap();
    let fixed = read_placement("in 0 3 1\n".as_bytes()).unwrap();
    let options = ProblemOptions {
        fixed: Some(&fixed),
        ..ProblemOptions::default()
    };
    let problem = build_place_problem(&device, &netlist, &arch, &options).unwrap();
    assert!(problem.problem().item(problem.item(0)).fixed().is_some());
    let delays = PlaceDelayModel::from_rr_graph(&device);

    for method in [PlaceMethod::Analytic, PlaceMethod::Anneal] {
        let placer = PlacerOptions {
            method,
            ..PlacerOptions::default()
        };
        let result = place_design(&problem, &delays, None, &placer).unwrap();
        assert!(crate::place::check(problem.problem(), &result.placement).is_empty());
        assert_eq!(result.worst_slack, None);
        assert_eq!(result.bb_estimate, problem.bb_estimate(&result.placement));
        assert_eq!(
            place_design(&problem, &delays, None, &placer).unwrap(),
            result,
            "the same seed gives the same placement"
        );

        let placement = problem.to_placement(&netlist, &result.placement, "t.net", "SHA256:1");
        assert_eq!(placement.array_size(), Some((3, 4)));
        assert_eq!(placement.netlist_id(), Some("SHA256:1"));
        let block = |name: &str| placement.block(name).unwrap().clone();
        assert_eq!(
            (block("in").x, block("in").y, block("in").sub_block),
            (0, 3, 1)
        );
        let chain = [block("c0"), block("c1"), block("c2")];
        assert!(chain.iter().all(|b| b.x == 1));
        assert_eq!((chain[0].y - 1, chain[0].y - 2), (chain[1].y, chain[2].y));
        assert_eq!(block("r0").x, 2);

        let mut text = Vec::new();
        write_placement(&mut text, &placement).unwrap();
        let read = read_placement(text.as_slice()).unwrap();
        assert_eq!(read, placement);
        assert_eq!(
            problem.from_placement(&netlist, &read).unwrap(),
            result.placement
        );
    }
}

#[test]
fn placement_problems_refuse_what_cannot_be_placed() {
    let device = read_rr_graph(PLACE_DEVICE.as_bytes()).unwrap();
    let netlist = place_netlist();
    let arch = Architecture::default();
    for fixed in ["in 1 0 0\n", "in 0 0 2\n", "r0 2 1 0\n"] {
        let fixed = read_placement(fixed.as_bytes()).unwrap();
        let options = ProblemOptions {
            fixed: Some(&fixed),
            ..ProblemOptions::default()
        };
        assert!(matches!(
            build_place_problem(&device, &netlist, &arch, &options),
            Err(ImportError::BadLocation { .. })
        ));
    }
    let strange = read_netlist(
        "<block name=\"t\" instance=\"FPGA_packed_netlist[0]\">\
         <block name=\"d\" instance=\"dsp[0]\" mode=\"m\"/></block>"
            .as_bytes(),
    )
    .unwrap();
    let error =
        build_place_problem(&device, &strange, &arch, &ProblemOptions::default()).unwrap_err();
    assert!(matches!(error, ImportError::NoTileType { .. }));
    assert!(error.to_string().contains("dsp"));

    // Without the chain, there is no macro, and a placement that leaves a block out is
    // refused.
    let problem =
        build_place_problem(&device, &netlist, &arch, &ProblemOptions::default()).unwrap();
    assert_eq!(problem.chains(), (0, 0));
    let partial = read_placement("in 0 0 0\n".as_bytes()).unwrap();
    assert!(matches!(
        problem.from_placement(&netlist, &partial),
        Err(ImportError::UnplacedBlock { .. })
    ));
}

/// Logic blocks in a line, each driving the next through its one input, from a pad in the
/// first block to a pad in the last, as the root names them.
pub(super) fn line_netlist(blocks: u32) -> PackedNetlist {
    let mut text = "<block name=\"t\" instance=\"FPGA_packed_netlist[0]\">\
         <inputs>n0</inputs><outputs>out:z</outputs><clocks></clocks>"
        .to_owned();
    for k in 0..blocks {
        let input = if k == 0 {
            "open".to_owned()
        } else {
            format!("n{}", k - 1)
        };
        let inner_input = if k == 0 {
            ""
        } else {
            "<inputs><port name=\"i\">clb.I[0]-&gt;x</port></inputs>"
        };
        if k + 1 == blocks {
            write!(
                text,
                "<block name=\"b{k}\" instance=\"clb[{k}]\" mode=\"m\">\
                 <inputs><port name=\"I\">{input}</port></inputs>\
                 <block name=\"out:z\" instance=\"p[0]\">{inner_input}</block></block>"
            )
            .unwrap();
        } else {
            write!(
                text,
                "<block name=\"b{k}\" instance=\"clb[{k}]\" mode=\"m\">\
                 <inputs><port name=\"I\">{input}</port></inputs>\
                 <outputs><port name=\"O\">p[0].o[0]-&gt;x</port></outputs>\
                 <block name=\"n{k}\" instance=\"p[0]\">{inner_input}\
                 <outputs><port name=\"o\">n{k}</port></outputs></block></block>"
            )
            .unwrap();
        }
    }
    text.push_str("</block>");
    read_netlist(text.as_bytes()).unwrap()
}

/// A square of logic blocks, each with `tracks` wires of one tile that join the same track on
/// the tiles beside it, both ways, and that its output pin drives and its input pin listens to.
pub(super) fn mesh_graph(size: u32, tracks: u32) -> String {
    let mut nodes = String::new();
    let mut edges = String::new();
    let mut next = 0u32;
    let mut add = |kind: &str, x: u32, y: u32, ptc: u32| {
        let id = next;
        next += 1;
        let extra = if kind == "CHANX" {
            "<timing R=\"100\" C=\"1e-14\"/><segment segment_id=\"0\"/>"
        } else {
            ""
        };
        writeln!(
            nodes,
            "<node id=\"{id}\" type=\"{kind}\" capacity=\"1\"><loc xlow=\"{x}\" ylow=\"{y}\" xhigh=\"{x}\" yhigh=\"{y}\" ptc=\"{ptc}\"/>{extra}</node>"
        )
        .unwrap();
        id
    };
    let mut edge = |from: u32, to: u32, switch: u32| {
        writeln!(
            edges,
            "<edge src_node=\"{from}\" sink_node=\"{to}\" switch_id=\"{switch}\"/>"
        )
        .unwrap();
    };
    let mut wire = HashMap::new();
    for y in 1..=size {
        for x in 1..=size {
            let source = add("SOURCE", x, y, 1);
            let opin = add("OPIN", x, y, 1);
            let ipin = add("IPIN", x, y, 0);
            let sink = add("SINK", x, y, 0);
            edge(source, opin, 0);
            edge(ipin, sink, 0);
            for track in 0..tracks {
                let w = add("CHANX", x, y, track);
                edge(opin, w, 1);
                edge(w, ipin, 1);
                wire.insert((x, y, track), w);
            }
        }
    }
    for (&(x, y, track), &w) in &wire {
        for (nx, ny) in [(x + 1, y), (x, y + 1)] {
            if let Some(&other) = wire.get(&(nx, ny, track)) {
                edge(w, other, 1);
                edge(other, w, 1);
            }
        }
    }
    let mut grid = String::new();
    for y in 0..=size + 1 {
        for x in 0..=size + 1 {
            let block = u32::from((1..=size).contains(&x) && (1..=size).contains(&y));
            writeln!(grid, "<grid_loc x=\"{x}\" y=\"{y}\" block_type_id=\"{block}\" width_offset=\"0\" height_offset=\"0\"/>").unwrap();
        }
    }
    // Edges in a fixed order, whatever the map's.
    let mut lines: Vec<&str> = edges.lines().collect();
    lines.sort_unstable();
    format!(
        "<rr_graph>\n<switches>\n\
         <switch id=\"0\" name=\"none\" type=\"mux\"><timing Tdel=\"0\"/></switch>\n\
         <switch id=\"1\" name=\"sw\" type=\"mux\"><timing R=\"500\" Cout=\"1e-15\" Cin=\"1e-15\" Tdel=\"5e-11\"/></switch>\n\
         </switches>\n<segments><segment id=\"0\" name=\"wire\" length=\"1\"/></segments>\n\
         <block_types><block_type id=\"0\" name=\"EMPTY\" width=\"1\" height=\"1\"/>\
         <block_type id=\"1\" name=\"clb\" width=\"1\" height=\"1\">\
         <pin_class type=\"INPUT\"><pin ptc=\"0\">clb.I[0]</pin></pin_class>\
         <pin_class type=\"OUTPUT\"><pin ptc=\"1\">clb.O[0]</pin></pin_class></block_type></block_types>\n\
         <grid>\n{grid}</grid>\n<rr_nodes>\n{nodes}</rr_nodes>\n<rr_edges>\n{}\n</rr_edges>\n</rr_graph>\n",
        lines.join("\n")
    )
}

#[test]
fn delays_for_placement_grow_with_distance() {
    let device = read_rr_graph(mesh_graph(5, 2).as_bytes()).unwrap();
    let delays = PlaceDelayModel::from_rr_graph(&device);
    // Out of a block and back in takes a wire.
    assert!(delays.delay(0, 0) > 0);
    for d in 0..4 {
        assert!(delays.delay(d + 1, 0) > delays.delay(d, 0), "{d}");
        assert!(delays.delay(0, d + 1) > delays.delay(0, d), "{d}");
        assert_eq!(delays.delay(d, 1), delays.delay(1, d), "{d}");
    }
    assert!(delays.delay(4, 4) > delays.delay(3, 4));
    // Offsets that no block is apart from take the nearest.
    assert_eq!(delays.delay(100, 100), delays.delay(6, 6));
    assert_eq!(delays.delay(6, 6), delays.delay(4, 4));
}

#[test]
fn timing_driven_placements_route() {
    let device = read_rr_graph(mesh_graph(4, 2).as_bytes()).unwrap();
    let netlist = line_netlist(10);
    let problem = build_place_problem(
        &device,
        &netlist,
        &Architecture::default(),
        &ProblemOptions::default(),
    )
    .unwrap();
    let delays = PlaceDelayModel::from_rr_graph(&device);
    let mut timing =
        build_timing(&netlist, &LogicDelays::new(), &TimingOptions::default()).unwrap();
    let constraints = timing.vpr_constraints(0).unwrap();
    let result = place_design(
        &problem,
        &delays,
        Some((&mut timing, &constraints)),
        &PlacerOptions::default(),
    )
    .unwrap();
    assert!(crate::place::check(problem.problem(), &result.placement).is_empty());
    // Nine nets between neighbours, each over two tiles at best.
    assert_eq!(problem.problem().net_count(), 9);
    assert!(
        result.bb_estimate <= 9.0 * 3.0 * 1.2,
        "{}",
        result.bb_estimate
    );
    let slack = result.worst_slack.unwrap();
    assert!(slack < 0);
    assert_eq!(
        estimate_slack(
            &problem,
            &delays,
            &mut timing,
            &constraints,
            &result.placement
        ),
        Some(slack)
    );
    // The same seed gives the same placement.
    let mut again = build_timing(&netlist, &LogicDelays::new(), &TimingOptions::default()).unwrap();
    let repeated = place_design(
        &problem,
        &delays,
        Some((&mut again, &constraints)),
        &PlacerOptions::default(),
    )
    .unwrap();
    assert_eq!(repeated, result);

    let placement = problem.to_placement(&netlist, &result.placement, "t.net", "SHA256:0");
    let design = resolve_nets(&device, &netlist, &placement, &NetOptions::default()).unwrap();
    let outcome =
        crate::engine::route(device.graph(), design.nets(), device.engine_options()).unwrap();
    let routes = outcome
        .legal_routes()
        .expect("a placement this short routes");
    let report = crate::validate::check(device.graph(), design.nets(), routes);
    assert!(report.is_complete() && report.faults().is_empty());
}

#[test]
fn channel_demand_spreads_each_nets_wire_over_its_box() {
    let device = read_rr_graph(mesh_graph(4, 2).as_bytes()).unwrap();
    let netlist = line_netlist(10);
    let problem = build_place_problem(
        &device,
        &netlist,
        &Architecture::default(),
        &ProblemOptions::default(),
    )
    .unwrap();
    // The mesh has two horizontal tracks on each logic block's tile, and nothing else.
    assert_eq!(problem.channel_tracks(1, 1), Some([2, 0]));
    assert_eq!(problem.channel_tracks(4, 4), Some([2, 0]));
    assert_eq!(problem.channel_tracks(0, 3), Some([0, 0]));
    assert_eq!(problem.channel_tracks(6, 0), None);

    let delays = PlaceDelayModel::from_rr_graph(&device);
    let placed = place_design(&problem, &delays, None, &PlacerOptions::default()).unwrap();
    let demand = problem.channel_demand(&placed.placement);
    // Each net of two pins needs a track over each column of its box, all on logic blocks'
    // tiles, which have horizontal tracks; vertical channels have none, so count nothing.
    let places = problem.problem();
    let wanted: u64 = (0..places.net_count() as u32)
        .map(|net| {
            places
                .net_box(&placed.placement, NetId(net))
                .unwrap()
                .width()
                + 1
        })
        .sum();
    let mut horizontal = 0.0;
    for y in 0..6 {
        for x in 0..6 {
            let [ux, uy] = demand.utilization(x, y).unwrap();
            assert_eq!(uy, 0.0);
            horizontal += ux * f64::from(problem.channel_tracks(x, y).unwrap()[0]);
        }
    }
    assert!(
        (horizontal - wanted as f64).abs() < 1e-9,
        "{horizontal} against {wanted}"
    );
    assert_eq!(demand.utilization(6, 0), None);
    assert!(demand.peak() > 0.0);
    assert!(demand.peak() >= demand.quantile(0.9));
    assert!(demand.quantile(0.9) >= demand.quantile(0.1));
    assert_eq!(demand.quantile(1.0), demand.peak());
    let total = demand.overflow(0.0);
    assert!((total * 2.0 - horizontal).abs() < 1e-9, "{total}");
    assert_eq!(demand.overflow(demand.peak()), 0.0);
}

#[test]
fn congestion_aware_placements_are_legal_and_repeatable() {
    let device = read_rr_graph(mesh_graph(5, 1).as_bytes()).unwrap();
    let netlist = line_netlist(20);
    let problem = build_place_problem(
        &device,
        &netlist,
        &Architecture::default(),
        &ProblemOptions::default(),
    )
    .unwrap();
    let delays = PlaceDelayModel::from_rr_graph(&device);
    let options = PlacerOptions {
        congestion_factor: 2.0,
        congestion_threshold: 0.2,
        max_inflation: 2.0,
        ..PlacerOptions::default()
    };
    let placed = place_design(&problem, &delays, None, &options).unwrap();
    assert!(crate::place::check(problem.problem(), &placed.placement).is_empty());
    assert_eq!(
        place_design(&problem, &delays, None, &options).unwrap(),
        placed
    );
    for method in [PlaceMethod::Anneal, PlaceMethod::Analytic] {
        let options = PlacerOptions { method, ..options };
        let placed = place_design(&problem, &delays, None, &options).unwrap();
        assert!(crate::place::check(problem.problem(), &placed.placement).is_empty());
    }
}
