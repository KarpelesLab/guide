//! Tests of non-configurable steps, which tie nodes together.
//!
//! Every result goes through the independent validator, which checks that each node of a route
//! has what is tied to it, by asking the space itself.

use super::tests::{check_honest, routable};
use super::*;
use crate::id::NodeId;
use crate::net::{Edge, NetOutcome, Terminal};
use crate::space::graph::{Graph, GraphBuilder};
use crate::timing::TimingGraphBuilder;
use crate::validate::{Fault, check, check_fixed};
use proptest::prelude::*;

fn net(id: u32, nodes: &[u32]) -> Net<NodeId> {
    Net {
        id: NetId(id),
        terminals: nodes.iter().map(|&n| Terminal::at(NodeId(n))).collect(),
    }
}

/// A graph of `(from, to, cost)` edges, and `(from, to)` non-configurable ones of cost 0.
fn graph(nodes: u32, edges: &[(u32, u32, u32)], tied: &[(u32, u32)]) -> Graph {
    let mut builder = GraphBuilder::new(nodes);
    for &(from, to, cost) in edges {
        builder
            .add_edge(NodeId(from), NodeId(to), Cost::from(cost))
            .unwrap();
    }
    for &(from, to) in tied {
        builder
            .add_non_configurable_edge(NodeId(from), NodeId(to), Cost::ZERO)
            .unwrap();
    }
    builder.build()
}

/// Routes, and checks the outcome against itself and the validator.
fn route_checked<S: RoutingSpace<Node = NodeId>>(
    space: &S,
    nets: &[Net<NodeId>],
) -> Outcome<NodeId, S::Resource> {
    let outcome = route(space, nets, EngineOptions::default()).unwrap();
    check_honest(space, nets, &outcome).unwrap();
    let report = check(space, nets, &outcome.routes);
    for fault in report.faults() {
        assert!(matches!(fault, Fault::Overuse { .. }), "{fault}");
    }
    assert_eq!(report.is_complete(), outcome.is_success());
    outcome
}

fn nodes_of(outcome: &Outcome<NodeId, NodeId>, id: u32) -> Vec<u32> {
    let mut nodes: Vec<u32> = outcome
        .routes
        .route(NetId(id))
        .unwrap()
        .nodes()
        .map(|node| node.0)
        .collect();
    nodes.sort_unstable();
    nodes
}

#[test]
fn entering_a_tied_node_takes_the_others_along() {
    // 1 and 2 are shorted both ways, and 2 drives 3 through a non-configurable buffer.
    let space = graph(5, &[(0, 1, 1), (1, 4, 1)], &[(1, 2), (2, 1), (2, 3)]);
    let outcome = route_checked(&space, &[net(0, &[0, 4])]);
    assert!(outcome.is_success());
    assert_eq!(nodes_of(&outcome, 0), [0, 1, 2, 3, 4]);
    let route = outcome.routes.route(NetId(0)).unwrap();
    assert!(route.edges().contains(&Edge {
        from: NodeId(2),
        to: NodeId(3),
    }));
    // The tied steps cost nothing here, so the route costs its two real steps.
    assert_eq!(outcome.total_cost, Cost::from(2));
}

#[test]
fn a_tie_only_goes_one_way() {
    // 2 drives 1, so a route on 1 does not need 2.
    let space = graph(4, &[(0, 1, 1), (1, 3, 1)], &[(2, 1)]);
    let outcome = route_checked(&space, &[net(0, &[0, 3])]);
    assert_eq!(nodes_of(&outcome, 0), [0, 1, 3]);
}

#[test]
fn a_tied_root_takes_its_ties_along() {
    let space = graph(4, &[(0, 3, 1)], &[(0, 1), (1, 2)]);
    let outcome = route_checked(&space, &[net(0, &[0, 3])]);
    assert_eq!(nodes_of(&outcome, 0), [0, 1, 2, 3]);
}

#[test]
fn a_terminal_reached_through_a_tie_needs_no_search_of_its_own() {
    // 2 is a dead end with no edge into it but the tie from 3, so only 3 is a target.
    let space = graph(4, &[(0, 1, 1), (1, 3, 1)], &[(3, 2)]);
    let mut nets = [net(0, &[0, 3, 2])];
    // A search could still end at 2 through the tie, so it is made dear to end there.
    nets[0].terminals[2] = Terminal::new(vec![crate::net::AccessPoint {
        node: NodeId(2),
        cost: Cost::from(10),
    }])
    .unwrap();
    let outcome = route_checked(&space, &nets);
    assert!(outcome.is_success());
    // The first search found 3, which brought 2 with it.
    assert_eq!(outcome.routes.stats.searches, 1);
}

#[test]
fn a_tie_into_what_another_net_holds_is_priced_from_the_start() {
    // Net 1 holds 2. Net 0 can go through 1, which is tied to 2, or around through 4 and 5 at
    // a little more. It sees from the first iteration that 1 would take 2 as well.
    let space = graph(
        7,
        &[
            (0, 1, 1),
            (1, 6, 1),
            (0, 4, 1),
            (4, 5, 1),
            (5, 6, 1),
            (3, 2, 1),
        ],
        &[(1, 2)],
    );
    let nets = [net(1, &[3, 2]), net(0, &[0, 6])];
    let outcome = route_checked(&space, &nets);
    assert!(outcome.is_success());
    assert_eq!(nodes_of(&outcome, 0), [0, 4, 5, 6]);
    // A run where 1 is not tied would go through it.
    let untied = graph(
        7,
        &[
            (0, 1, 1),
            (1, 6, 1),
            (0, 4, 1),
            (4, 5, 1),
            (5, 6, 1),
            (3, 2, 1),
        ],
        &[],
    );
    let plain = route_checked(&untied, &nets);
    assert_eq!(nodes_of(&plain, 0), [0, 1, 6]);
}

#[test]
fn two_nets_fighting_over_a_tied_pair_negotiate_it() {
    // Both nets prefer 1, which is tied to 2. Only one can have the pair.
    let space = graph(
        8,
        &[
            (0, 1, 1),
            (1, 3, 1),
            (0, 5, 2),
            (5, 3, 2),
            (4, 2, 1),
            (2, 6, 1),
            (4, 7, 2),
            (7, 6, 2),
        ],
        &[(1, 2), (2, 1)],
    );
    let nets = [net(0, &[0, 3]), net(1, &[4, 6])];
    let outcome = route_checked(&space, &nets);
    assert!(outcome.is_success());
    let first = nodes_of(&outcome, 0);
    let second = nodes_of(&outcome, 1);
    assert!(first.contains(&1) != second.contains(&2));
}

#[test]
fn the_validator_wants_every_tied_node_on_the_route() {
    let space = graph(5, &[(0, 1, 1), (1, 4, 1), (0, 2, 1), (2, 4, 1)], &[(1, 2)]);
    let nets = [net(0, &[0, 4])];
    let mut routes = RouteSet::new();
    let mut route = Route::new();
    route.add_path(&[NodeId(0), NodeId(1), NodeId(4)]).unwrap();
    routes.set_route(NetId(0), route);
    let report = check(&space, &nets, &routes);
    assert_eq!(
        report.faults(),
        [Fault::NonConfigurableNotTaken {
            net: NetId(0),
            from: NodeId(1),
            to: NodeId(2),
        }]
    );
    assert_eq!(report.cost(NetId(0)), None);
    assert_eq!(
        report.faults()[0].to_string(),
        "net 0: route uses NodeId(1) without NodeId(2), which is tied to it"
    );

    // Having 2 by another edge is enough.
    let mut route = Route::new();
    route.add_path(&[NodeId(0), NodeId(1), NodeId(4)]).unwrap();
    route.add_path(&[NodeId(0), NodeId(2)]).unwrap();
    routes.set_route(NetId(0), route);
    assert!(check(&space, &nets, &routes).is_complete());
}

#[test]
fn a_fixed_route_has_to_have_its_ties() {
    let space = graph(4, &[(0, 1, 1), (1, 3, 1)], &[(1, 2)]);
    let nets = [net(0, &[0, 3])];
    let mut engine = Engine::new(&space, &nets, EngineOptions::default()).unwrap();
    let mut bare = Route::new();
    bare.add_path(&[NodeId(0), NodeId(1), NodeId(3)]).unwrap();
    assert_eq!(
        engine.fix_routes([FixedRoute {
            net: NetId(0),
            route: bare.clone(),
        }]),
        Err(EngineError::InvalidFixedRoute {
            net: NetId(0),
            problem: FixedRouteProblem::NonConfigurableMissing,
        })
    );
    let mut whole = bare;
    whole.add_path(&[NodeId(1), NodeId(2)]).unwrap();
    let fixed = [FixedRoute {
        net: NetId(0),
        route: whole.clone(),
    }];
    engine.fix_routes(fixed.iter().cloned()).unwrap();
    let outcome = engine.run();
    assert_eq!(outcome.routes.route(NetId(0)), Some(&whole));
    assert!(check_fixed(&space, &nets, &outcome.routes, &fixed).is_complete());
}

#[test]
fn a_partial_route_grows_its_ties_as_well() {
    let space = graph(6, &[(0, 1, 1), (1, 3, 1), (3, 4, 1)], &[(4, 5)]);
    let nets = [net(0, &[0, 1, 4])];
    let mut start = Route::new();
    start.add_path(&[NodeId(0), NodeId(1)]).unwrap();
    let fixed = [FixedRoute {
        net: NetId(0),
        route: start,
    }];
    let mut engine = Engine::new(&space, &nets, EngineOptions::default()).unwrap();
    engine.fix_routes(fixed.iter().cloned()).unwrap();
    let outcome = engine.run();
    check_honest(&space, &nets, &outcome).unwrap();
    assert!(check_fixed(&space, &nets, &outcome.routes, &fixed).is_complete());
    assert_eq!(nodes_of(&outcome, 0), [0, 1, 3, 4, 5]);
}

#[test]
fn tied_steps_have_delays_too() {
    // The sink, 3, is only reached through the tie from 1 to 2 and on from 2.
    let mut builder = GraphBuilder::new(4);
    builder.add_edge(NodeId(0), NodeId(1), Cost::ONE).unwrap();
    builder
        .add_non_configurable_edge(NodeId(1), NodeId(2), Cost::ZERO)
        .unwrap();
    builder.add_edge(NodeId(2), NodeId(3), Cost::ONE).unwrap();
    let base = builder.build();
    let space = base.with_step_delays(|_, to| 100 * to.0);

    let mut timing = TimingGraphBuilder::new();
    let (a, b) = (timing.add_input(0).unwrap(), timing.add_output(0).unwrap());
    let arc = timing.add_net_arc(a, b, 0).unwrap();
    let setup = TimingDriven::new(
        timing.build(),
        Constraints::single_clock(0).unwrap(),
        vec![Connection {
            net: NetId(0),
            terminal: 1,
            arc,
        }],
        Cost::new(0.01).unwrap(),
    );
    let nets = [net(0, &[0, 3])];
    let outcome = route_timed(&space, &nets, EngineOptions::default(), setup).unwrap();
    assert!(outcome.is_success());
    assert_eq!(outcome.timing.unwrap().connection_delays, [Some(600)]);
}

#[test]
fn spaces_without_ties_say_so() {
    let space = graph(3, &[(0, 1, 1)], &[]);
    assert!(!space.has_non_configurable());
    let timed = space.with_step_delays(|_, _| 1);
    assert!(!timed.has_non_configurable());
    let tied = graph(3, &[(0, 1, 1)], &[(1, 2), (1, 2), (2, 2)]);
    assert!(tied.with_step_delays(|_, _| 1).has_non_configurable());
    let mut out = Vec::new();
    tied.non_configurable(NetId(0), NodeId(1), &mut out);
    // Repeats and self-loops are dropped, as they are among edges.
    assert_eq!(out, [NodeId(2)]);
    tied.non_configurable(NetId(0), NodeId(9), &mut out);
    assert_eq!(out, [NodeId(2)]);
}

#[test]
fn ties_give_the_same_result_on_any_number_of_threads() {
    let (space, nets) = tied_fabric();
    let batched = EngineOptions {
        batch_size: 4,
        ..EngineOptions::default()
    };
    let reference = route(&space, &nets, batched).unwrap();
    check_honest(&space, &nets, &reference).unwrap();
    assert!(reference.routes.routes().count() > 10);
    for threads in [1, 2, 4] {
        assert_eq!(
            route_parallel(&space, &nets, batched, threads).unwrap(),
            reference
        );
    }
}

/// A ring of 40 nodes both ways, with every fifth node tied to the next, and nets across it.
fn tied_fabric() -> (Graph, Vec<Net<NodeId>>) {
    let size = 40;
    let mut edges = Vec::new();
    let mut tied = Vec::new();
    for node in 0..size {
        let next = (node + 1) % size;
        if node % 5 == 0 {
            tied.push((node, next));
        } else {
            edges.push((node, next, 1));
        }
        edges.push((next, node, 1));
    }
    let space = graph(size, &edges, &tied);
    let nets = (0..16)
        .map(|id| net(id, &[(id * 7) % size, (id * 7 + 9) % size]))
        .collect();
    (space, nets)
}

prop_compose! {
    /// A small directed graph with some non-configurable edges, and nets on it.
    fn any_tied_problem()(nodes in 2u32..14)(
        nodes in Just(nodes),
        edges in prop::collection::vec((0..nodes, 0..nodes, 0u32..3, prop::bool::weighted(0.2)), 0..40),
        nets in prop::collection::vec(prop::collection::vec(0..nodes, 1..4), 0..5),
    ) -> (Graph, Vec<Net<NodeId>>) {
        let mut builder = GraphBuilder::new(nodes);
        for (from, to, cost, tied) in edges {
            let (from, to, cost) = (NodeId(from), NodeId(to), Cost::from(cost));
            if tied {
                builder.add_non_configurable_edge(from, to, cost).unwrap();
            } else {
                builder.add_edge(from, to, cost).unwrap();
            }
        }
        let nets = nets
            .iter()
            .enumerate()
            .map(|(id, nodes)| net(id as u32, nodes))
            .collect();
        (builder.build(), nets)
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Whatever the ties, results are honest, have every tie, and route when a path exists.
    #[test]
    fn results_on_tied_graphs_are_honest((space, nets) in any_tied_problem()) {
        let limited = EngineOptions {
            max_iterations: 12,
            ..EngineOptions::default()
        };
        let outcome = route(&space, &nets, limited).unwrap();
        let honest = check_honest(&space, &nets, &outcome);
        prop_assert!(honest.is_ok(), "{:?}", honest);
        let report = check(&space, &nets, &outcome.routes);
        for fault in report.faults() {
            prop_assert!(matches!(fault, Fault::Overuse { .. }), "{}", fault);
        }
        prop_assert_eq!(report.is_complete(), outcome.is_success());
        for net in &nets {
            let routed = matches!(outcome.routes.outcome(net.id), Some(NetOutcome::Routed(_)));
            prop_assert_eq!(routed, routable(&space, net), "{:?}", net.id);
        }
    }
}

/// A rr_graph of two pins and three wires, the last two shorted together.
#[cfg(feature = "fpga")]
#[test]
fn shorts_in_an_rr_graph_tie_their_wires() {
    use crate::fpga::read_rr_graph;
    let rr_graph = r#"<rr_graph>
  <switches>
    <switch id="0" name="delayless" type="mux"><timing Tdel="0"/><sizing mux_trans_size="0" buf_size="0"/></switch>
    <switch id="1" name="wire" type="mux"><timing Tdel="1e-10"/><sizing mux_trans_size="0" buf_size="0"/></switch>
    <switch id="2" name="tie" type="short"><timing Tdel="0"/><sizing mux_trans_size="0" buf_size="0"/></switch>
  </switches>
  <segments><segment id="0" name="L1" length="1"/></segments>
  <block_types>
    <block_type id="0" name="io" width="1" height="1">
      <pin_class type="OUTPUT"><pin ptc="0">io.inpad[0]</pin></pin_class>
      <pin_class type="INPUT"><pin ptc="1">io.outpad[0]</pin></pin_class>
    </block_type>
  </block_types>
  <grid>
    <grid_loc x="0" y="0" block_type_id="0" width_offset="0" height_offset="0"/>
    <grid_loc x="1" y="0" block_type_id="0" width_offset="0" height_offset="0"/>
  </grid>
  <rr_nodes>
    <node id="0" type="SOURCE" capacity="1"><loc xlow="0" ylow="0" xhigh="0" yhigh="0" ptc="0"/></node>
    <node id="1" type="OPIN" capacity="1"><loc xlow="0" ylow="0" xhigh="0" yhigh="0" side="RIGHT" ptc="0"/></node>
    <node id="2" type="CHANX" capacity="1"><loc xlow="0" ylow="0" xhigh="1" yhigh="0" ptc="0"/><segment segment_id="0"/></node>
    <node id="3" type="CHANX" capacity="1"><loc xlow="0" ylow="0" xhigh="1" yhigh="0" ptc="1"/><segment segment_id="0"/></node>
    <node id="4" type="CHANX" capacity="1"><loc xlow="0" ylow="0" xhigh="1" yhigh="0" ptc="2"/><segment segment_id="0"/></node>
    <node id="5" type="IPIN" capacity="1"><loc xlow="1" ylow="0" xhigh="1" yhigh="0" side="LEFT" ptc="1"/></node>
    <node id="6" type="SINK" capacity="1"><loc xlow="1" ylow="0" xhigh="1" yhigh="0" ptc="1"/></node>
  </rr_nodes>
  <rr_edges>
    <edge src_node="0" sink_node="1" switch_id="0"/>
    <edge src_node="1" sink_node="2" switch_id="1"/>
    <edge src_node="2" sink_node="5" switch_id="1"/>
    <edge src_node="2" sink_node="3" switch_id="2"/>
    <edge src_node="3" sink_node="2" switch_id="2"/>
    <edge src_node="4" sink_node="5" switch_id="1"/>
    <edge src_node="5" sink_node="6" switch_id="0"/>
  </rr_edges>
</rr_graph>"#;
    let device = read_rr_graph(rr_graph.as_bytes()).unwrap();
    assert_eq!(device.stats().non_configurable_edges, 2);
    let space = device.graph();
    assert!(space.has_non_configurable());
    let mut tied = Vec::new();
    space.non_configurable(NetId(0), NodeId(2), &mut tied);
    assert_eq!(tied, [NodeId(3)]);

    let nets = [net(0, &[0, 6])];
    let outcome = route(space, &nets, EngineOptions::default()).unwrap();
    assert!(outcome.is_success());
    let route = outcome.routes.route(NetId(0)).unwrap();
    assert!(route.contains(NodeId(3)));
    assert!(check(space, &nets, &outcome.routes).is_complete());
}
