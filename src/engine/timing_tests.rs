//! Tests of timing-driven routing.
//!
//! Hand-checked cases come first, then the lower bound against true distances, then whole
//! runs on generated fabrics: that timing is free when nothing is critical, that it shortens
//! the critical path, that threads change nothing, and pinned results. Every run's timing is
//! checked against an analysis made afresh from its routes, by code here that shares nothing
//! with the engine's.

use super::parallel_tests::{
    BenchFabric, SITE_NODES, THREAD_COUNTS, TileBound, fabric_graph, small_fabric,
};
use super::tests::{check_honest, hash_outcome};
use super::*;
use crate::id::NodeId;
use crate::net::Terminal;
use crate::search::Search;
use crate::space::graph::{DelayBound, Graph, GraphBuilder, LowerBound, TimedGraph};
use crate::timing::{PointId, TimingGraphBuilder};
use proptest::prelude::*;

fn net(id: u32, nodes: &[u32]) -> Net<NodeId> {
    Net {
        id: NetId(id),
        terminals: nodes.iter().map(|&n| Terminal::at(NodeId(n))).collect(),
    }
}

fn cost(value: f64) -> Cost {
    Cost::new(value).unwrap()
}

/// A graph from `(from, to, cost)` edges.
fn graph(nodes: u32, edges: &[(u32, u32, u32)]) -> Graph {
    let mut builder = GraphBuilder::new(nodes);
    for &(from, to, cost) in edges {
        builder
            .add_edge(NodeId(from), NodeId(to), Cost::from(cost))
            .unwrap();
    }
    builder.build()
}

// An analysis of routes made afresh.

/// The delay from the root of `route` to each terminal of `net`, walked here over the space's
/// steps: the smallest arrival at any of the terminal's access points.
fn walk_delays<S: DelaySpace<Node = NodeId>>(
    space: &S,
    net: &Net<NodeId>,
    route: &Route<NodeId>,
) -> Vec<Option<u32>> {
    let mut arrival = std::collections::HashMap::new();
    if let Some(root) = route.root() {
        arrival.insert(root, 0u32);
    }
    for edge in route.edges() {
        let (mut steps, mut delays) = (Vec::new(), Vec::new());
        space.expand(net.id, edge.from, &mut steps);
        space.step_delays(net.id, edge.from, &mut delays);
        let position = steps.iter().position(|step| step.to == edge.to).unwrap();
        let delay = arrival[&edge.from] + delays[position];
        arrival.insert(edge.to, delay);
    }
    net.terminals
        .iter()
        .map(|terminal| {
            terminal
                .access_points()
                .iter()
                .filter_map(|point| arrival.get(&point.node).copied())
                .min()
        })
        .collect()
}

/// Analyses the routes of `outcome` from nothing, and returns what it found and the delay of
/// each connection.
fn analyse_afresh<S: DelaySpace<Node = NodeId>>(
    space: &S,
    nets: &[Net<NodeId>],
    outcome: &Outcome<NodeId, S::Resource>,
    timing: &TimingDriven,
) -> (TimingMetrics, Vec<Option<u32>>) {
    let mut graph = timing.graph.clone();
    let mut delays = Vec::new();
    for connection in &timing.connections {
        let net = nets.iter().find(|net| net.id == connection.net).unwrap();
        let delay = outcome
            .routes
            .route(net.id)
            .and_then(|route| walk_delays(space, net, route)[connection.terminal as usize]);
        if let Some(delay) = delay {
            graph.set_delay(connection.arc, delay).unwrap();
        }
        delays.push(delay);
    }
    let mut analysis = TimingAnalysis::new(&graph);
    analysis.run(&graph, &timing.constraints);
    let metrics = TimingMetrics {
        critical_path_delay: analysis.critical_path().map(|path| path.arrival()),
        worst_slack: analysis.worst_slack(),
        total_negative_slack: analysis.total_negative_slack(),
    };
    (metrics, delays)
}

/// Routes, checks the outcome against itself and against a fresh analysis, and returns it.
fn route_checked<S: DelaySpace<Node = NodeId>>(
    space: &S,
    nets: &[Net<NodeId>],
    options: EngineOptions<'_>,
    timing: &TimingDriven,
) -> Outcome<NodeId, S::Resource> {
    let outcome = route_timed(space, nets, options, timing.clone()).unwrap();
    check_honest(space, nets, &outcome).unwrap();
    let reported = outcome.timing.as_ref().expect("the run is timing-driven");
    let (metrics, delays) = analyse_afresh(space, nets, &outcome, timing);
    assert_eq!(reported.metrics, metrics);
    assert_eq!(reported.connection_delays, delays);
    outcome
}

// Hand-checked cases.

/// Two nets, each with a cheap and slow way and a dear and fast one, as two diamonds:
/// `0 -> 1 -> 3` costs 2 and takes 1000 ps, and `0 -> 2 -> 3` costs 6 and takes 200 ps, and
/// the same from 4 to 7 through 5 and 6.
///
/// Net 0's connection is on the only constrained path, and net 1's is on none.
fn two_diamonds(max_criticality: f64) -> (Graph, Vec<Net<NodeId>>, TimingDriven) {
    let graph = graph(
        8,
        &[
            (0, 1, 1),
            (1, 3, 1),
            (0, 2, 3),
            (2, 3, 3),
            (4, 5, 1),
            (5, 7, 1),
            (4, 6, 3),
            (6, 7, 3),
        ],
    );
    let nets = vec![net(0, &[0, 3]), net(1, &[4, 7])];
    let mut builder = TimingGraphBuilder::new();
    let (input, output) = (
        builder.add_input(0).unwrap(),
        builder.add_output(0).unwrap(),
    );
    let critical = builder.add_net_arc(input, output, 0).unwrap();
    let (from, to) = (builder.add_point().unwrap(), builder.add_point().unwrap());
    let free = builder.add_net_arc(from, to, 0).unwrap();
    let connections = vec![
        Connection {
            net: NetId(0),
            terminal: 1,
            arc: critical,
        },
        Connection {
            net: NetId(1),
            terminal: 1,
            arc: free,
        },
    ];
    let mut timing = TimingDriven::new(
        builder.build(),
        Constraints::single_clock(0).unwrap(),
        connections,
        cost(0.01),
    );
    timing.criticality = CriticalityOptions::new(1, max_criticality).unwrap();
    (graph, nets, timing)
}

/// The delays of [`two_diamonds`]: 500 ps a step into or out of a slow node, 100 otherwise.
fn diamond_delay(from: NodeId, to: NodeId) -> u32 {
    let slow = |node: NodeId| node.0 == 1 || node.0 == 5;
    if slow(from) || slow(to) { 500 } else { 100 }
}

#[test]
fn a_critical_net_takes_the_fast_dear_path_and_an_uncritical_one_the_cheap_slow_path() {
    let (graph, nets, timing) = two_diamonds(0.99);
    let space = graph.with_step_delays(diamond_delay);
    let engine = Engine::with_timing(&space, &nets, EngineOptions::default(), timing.clone());
    let engine = engine.unwrap();
    // Every path through net 0's arc has the worst slack, and net 1's arc is on none.
    assert_eq!(engine.connection_criticality(0), Some(0.99));
    assert_eq!(engine.connection_criticality(1), Some(0.0));
    assert_eq!(engine.connection_criticality(2), None);

    // For net 0, by hand: through 1, 0.01 * 2 + 0.99 * 0.01 * 1000 = 9.92, and through 2,
    // 0.01 * 6 + 0.99 * 0.01 * 200 = 2.04. Net 1 sees only cost: 2 against 6.
    let outcome = route_checked(&space, &nets, EngineOptions::default(), &timing);
    assert!(outcome.is_success());
    let nodes = |id| -> Vec<u32> {
        let route = outcome.routes.route(NetId(id)).unwrap();
        route.nodes().map(|node| node.0).collect()
    };
    assert_eq!(nodes(0), [0, 2, 3]);
    assert_eq!(nodes(1), [4, 5, 7]);
    // Costs are reported before any weighing, as without timing.
    assert_eq!(outcome.total_cost, Cost::from(8));
    let timing = outcome.timing.unwrap();
    assert_eq!(timing.connection_delays, [Some(200), Some(1000)]);
    assert_eq!(timing.metrics.critical_path_delay, Some(200));
    assert_eq!(timing.metrics.worst_slack, Some(-200));
    assert_eq!(timing.metrics.total_negative_slack, -200);
}

#[test]
fn a_connection_with_little_criticality_still_goes_for_cost() {
    // At 0.1, through 1 costs 0.9 * 2 + 0.1 * 0.01 * 1000 = 2.8, and through 2 costs
    // 0.9 * 6 + 0.1 * 0.01 * 200 = 5.6.
    let (graph, nets, timing) = two_diamonds(0.1);
    let space = graph.with_step_delays(diamond_delay);
    let outcome = route_checked(&space, &nets, EngineOptions::default(), &timing);
    let tree = outcome.routes.route(NetId(0)).unwrap();
    assert!(tree.contains(NodeId(1)));
    assert_eq!(
        outcome.timing.unwrap().metrics.critical_path_delay,
        Some(1000)
    );
}

/// Two nets that both want node 2, which is fast and dear and fits one. Each also has a way
/// of its own that is cheap and slow:
///
/// ```text
/// net 0:  0 -> 2 -> 3, 200 ps and a cost of 6, or 0 -> 1 -> 3, 1000 ps and a cost of 2
/// net 1:  4 -> 2 -> 6, 200 ps and a cost of 6, or 4 -> 5 -> 6, 1000 ps and a cost of 2
/// ```
///
/// Net 0 feeds a cell of 300 ps and net 1 one of 200, so net 0 is the more critical. With it
/// on node 2 the critical path is net 1's, 1000 + 200, and the other way round it would be
/// 1000 + 300. Without timing, both take their cheap ways, and the critical path is net 0's.
fn contested_fast_node() -> (Graph, Vec<Net<NodeId>>, TimingDriven) {
    let graph = graph(
        7,
        &[
            (0, 1, 1),
            (1, 3, 1),
            (0, 2, 3),
            (2, 3, 3),
            (4, 5, 1),
            (5, 6, 1),
            (4, 2, 3),
            (2, 6, 3),
        ],
    );
    let nets = vec![net(0, &[0, 3]), net(1, &[4, 6])];

    let mut builder = TimingGraphBuilder::new();
    let mut connections = Vec::new();
    for (id, cell) in [(0, 300), (1, 200)] {
        let input = builder.add_input(0).unwrap();
        let pin = builder.add_point().unwrap();
        let output = builder.add_output(0).unwrap();
        let arc = builder.add_net_arc(input, pin, 0).unwrap();
        builder.add_cell_arc(pin, output, cell).unwrap();
        connections.push(Connection {
            net: NetId(id),
            terminal: 1,
            arc,
        });
    }
    let timing = TimingDriven::new(
        builder.build(),
        Constraints::single_clock(0).unwrap(),
        connections,
        cost(0.01),
    );
    (graph, nets, timing)
}

#[test]
fn when_timing_and_congestion_conflict_the_more_critical_net_keeps_the_fast_node() {
    let (graph, nets, timing) = contested_fast_node();
    // The slow nodes are the same as in the diamonds.
    let space = graph.with_step_delays(diamond_delay);
    let engine = Engine::with_timing(&space, &nets, EngineOptions::default(), timing.clone());
    let engine = engine.unwrap();
    // Before routing the worst path is net 0's at -300, and net 1's at -200 has a raw
    // criticality of 1 - 100 / 300 = 2/3, less VPR's 0.01.
    assert_eq!(engine.connection_criticality(0), Some(0.99));
    let second = engine.connection_criticality(1).unwrap();
    assert!((second - (2.0 / 3.0 - 0.01)).abs() < 1e-12, "{second}");

    let mut engine = engine;
    let first = engine.step().unwrap();
    // Both took the fast node at first, as both are critical enough to.
    assert_eq!(first.total_overuse, 1);
    assert_eq!(first.timing.unwrap().critical_path_delay, Some(500));
    while engine.step().is_some() {}
    let outcome = engine.outcome();
    check_honest(&space, &nets, &outcome).unwrap();
    assert!(outcome.is_success());
    // Net 1 has more weight on congestion, so it gives way first.
    assert!(outcome.routes.route(NetId(0)).unwrap().contains(NodeId(2)));
    assert!(outcome.routes.route(NetId(1)).unwrap().contains(NodeId(5)));
    let metrics = outcome.timing.as_ref().unwrap().metrics;
    assert_eq!(metrics.critical_path_delay, Some(1200));
    assert_eq!(
        analyse_afresh(&space, &nets, &outcome, &timing).0,
        metrics,
        "the reported timing is the timing of the routes"
    );

    // Without timing, both are cheap and slow, and the critical path is net 0's.
    let plain = route(&space, &nets, EngineOptions::default()).unwrap();
    assert!(plain.is_success());
    assert!(plain.routes.route(NetId(0)).unwrap().contains(NodeId(1)));
    let plain_timing = analyse_afresh(&space, &nets, &plain, &timing).0;
    assert_eq!(plain_timing.critical_path_delay, Some(1300));
    assert!(plain.total_cost < outcome.total_cost);
}

#[test]
fn a_critical_sink_pays_for_the_delay_of_joining_the_tree_far_from_the_root() {
    // The driver 0 reaches sink 4 slowly, through 1 and 2, which costs little: 3 in all. Sink
    // 6 can join that tree at 2 for a cost of 1, or come from the driver straight through 5,
    // for 4. Joining at 2 is the cheap way and the slow one, at 1000 + 100 ps against 200.
    let graph = graph(
        7,
        &[
            (0, 1, 1),
            (1, 2, 1),
            (2, 4, 1),
            (2, 6, 1),
            (0, 5, 2),
            (5, 6, 2),
        ],
    );
    let space = graph.with_step_delays(|from, to| match (from.0, to.0) {
        (0, 1) | (1, 2) => 500,
        _ => 100,
    });
    let nets = vec![net(0, &[0, 4, 6])];

    let mut builder = TimingGraphBuilder::new();
    let input = builder.add_input(0).unwrap();
    let slow_sink = builder.add_output(0).unwrap();
    let fast_sink = builder.add_output(0).unwrap();
    let to_slow = builder.add_net_arc(input, slow_sink, 0).unwrap();
    let to_fast = builder.add_net_arc(input, fast_sink, 0).unwrap();
    let connections = vec![
        Connection {
            net: NetId(0),
            terminal: 1,
            arc: to_slow,
        },
        Connection {
            net: NetId(0),
            terminal: 2,
            arc: to_fast,
        },
    ];
    let timing = TimingDriven::new(
        builder.build(),
        Constraints::single_clock(0).unwrap(),
        connections,
        cost(0.01),
    );
    let outcome = route_checked(&space, &nets, EngineOptions::default(), &timing);
    let tree = outcome.routes.route(NetId(0)).unwrap();
    // Both sinks are fully critical, and the first, sink 4, is routed first. Joining its tree
    // at 2 would then start 1000 ps late, which costs 0.99 * 0.01 * 1000 = 9.9 on its own.
    assert!(tree.contains(NodeId(5)), "{tree:?}");
    let delays = &outcome.timing.as_ref().unwrap().connection_delays;
    assert_eq!(delays, &[Some(1100), Some(200)]);

    // Without timing, sink 6 joins at 2.
    let plain = route(&space, &nets, EngineOptions::default()).unwrap();
    assert!(!plain.routes.route(NetId(0)).unwrap().contains(NodeId(5)));
    assert_eq!(
        analyse_afresh(&space, &nets, &plain, &timing).1,
        [Some(1100), Some(1100)]
    );
}

#[test]
fn sinks_are_routed_most_critical_first() {
    assert_eq!(
        most_critical(&[1, 2, 3], &[0.0, 0.2, 0.7, 0.7]),
        Some((2, 0.7))
    );
    assert_eq!(
        most_critical(&[3, 1], &[0.0, 0.2, 0.7, 0.7]),
        Some((3, 0.7))
    );
    assert_eq!(most_critical(&[1, 2], &[0.0, 0.0, 0.0]), None);
    assert_eq!(most_critical(&[1, 5], &[0.0, 0.0]), None);
}

#[test]
fn bad_connections_are_refused() {
    let (graph, nets, timing) = two_diamonds(0.99);
    let space = graph.with_step_delays(diamond_delay);
    let attempt = |change: &dyn Fn(&mut TimingDriven)| {
        let mut timing = timing.clone();
        change(&mut timing);
        Engine::with_timing(&space, &nets, EngineOptions::default(), timing).err()
    };
    let arc = timing.connections[0].arc;
    assert_eq!(attempt(&|_| {}), None);
    assert_eq!(
        attempt(&|t| t.connections[0].net = NetId(9)),
        Some(EngineError::UnknownNet { net: NetId(9) })
    );
    assert_eq!(
        attempt(&|t| t.connections[0].terminal = 0),
        Some(EngineError::NotASink {
            net: NetId(0),
            terminal: 0
        })
    );
    assert_eq!(
        attempt(&|t| t.connections[0].terminal = 2),
        Some(EngineError::NotASink {
            net: NetId(0),
            terminal: 2
        })
    );
    assert_eq!(
        attempt(&|t| t.connections[0].arc = ArcId(77)),
        Some(EngineError::NotANetArc { arc: ArcId(77) })
    );
    assert_eq!(
        attempt(&|t| t.connections[1].arc = arc),
        Some(EngineError::DuplicateConnection {
            net: NetId(1),
            terminal: 1
        })
    );
    assert_eq!(
        attempt(&|t| t.connections.push(t.connections[0])),
        Some(EngineError::DuplicateConnection {
            net: NetId(0),
            terminal: 1
        })
    );
    let message = EngineError::NotANetArc { arc: ArcId(3) }.to_string();
    assert!(message.contains("arc 3"));
}

#[test]
fn a_cell_arc_is_not_a_connection() {
    let (graph, nets, _) = two_diamonds(0.99);
    let space = graph.with_step_delays(diamond_delay);
    let mut builder = TimingGraphBuilder::new();
    let (a, b) = (
        builder.add_input(0).unwrap(),
        builder.add_output(0).unwrap(),
    );
    let cell = builder.add_cell_arc(a, b, 10).unwrap();
    let timing = TimingDriven::new(
        builder.build(),
        Constraints::single_clock(0).unwrap(),
        vec![Connection {
            net: NetId(0),
            terminal: 1,
            arc: cell,
        }],
        cost(0.01),
    );
    let error = Engine::with_timing(&space, &nets, EngineOptions::default(), timing).err();
    assert_eq!(error, Some(EngineError::NotANetArc { arc: cell }));
}

// The lower bound.

/// The cheapest sums of costs, or of delays, between all pairs, or `None` where there is no
/// path, by Floyd and Warshall in whole numbers.
fn all_pairs(nodes: usize, edges: &[(usize, usize, u64)]) -> Vec<Vec<Option<u64>>> {
    let mut distance = vec![vec![None; nodes]; nodes];
    for (node, row) in distance.iter_mut().enumerate() {
        row[node] = Some(0);
    }
    for &(from, to, weight) in edges {
        let entry = &mut distance[from][to];
        *entry = Some(entry.map_or(weight, |known: u64| known.min(weight)));
    }
    for via in 0..nodes {
        for from in 0..nodes {
            for to in 0..nodes {
                if let (Some(a), Some(b)) = (distance[from][via], distance[via][to]) {
                    let entry = &mut distance[from][to];
                    *entry = Some(entry.map_or(a + b, |known| known.min(a + b)));
                }
            }
        }
    }
    distance
}

/// A bound that is a fraction of the true distance.
struct Scaled {
    distance: Vec<Vec<Option<u64>>>,
    fraction: f64,
}

impl LowerBound for Scaled {
    fn estimate(&self, from: NodeId, to: NodeId) -> Cost {
        let distance = self.distance[from.index()][to.index()].unwrap_or(0);
        cost(distance as f64 * self.fraction)
    }
}

impl DelayBound for Scaled {
    fn estimate(&self, from: NodeId, to: NodeId) -> u32 {
        let distance = self.distance[from.index()][to.index()].unwrap_or(0);
        (distance as f64 * self.fraction).floor() as u32
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// The blended bound against the true blended distance, from every node to every other,
    /// with a price on some resources from history and overuse, at any criticality. The
    /// space's own bounds are as tight as they can be, and looser, since the blend has to
    /// hold for any admissible pair.
    #[test]
    fn the_blended_lower_bound_never_exceeds_the_true_distance(
        nodes in 2usize..10,
        edges in proptest::collection::vec((0usize..10, 0usize..10, 0u32..9, 0u32..2000), 1..40),
        priced in proptest::collection::vec((0usize..10, 0u32..5, 0u32..3), 0..6),
        criticality in 0.0f64..=1.0,
        delay_cost in prop_oneof![Just(0.01), Just(1e-12), 0.0f64..10.0],
        present in 0.0f64..100.0,
        tightness in prop_oneof![Just(1.0), 0.0f64..1.0],
    ) {
        let edges: Vec<_> = edges
            .iter()
            .map(|&(a, b, c, d)| (a % nodes, b % nodes, c, d))
            .collect();
        let mut builder = GraphBuilder::new(nodes as u32);
        for &(from, to, step_cost, _) in &edges {
            builder.add_edge(NodeId(from as u32), NodeId(to as u32), Cost::from(step_cost)).unwrap();
        }
        let plain = builder.build();
        // One delay for each pair of nodes, as the graph merges parallel edges.
        let delay_of = |from: NodeId, to: NodeId| -> u32 {
            let mut cheapest: Option<(u32, u32)> = None;
            for &(a, b, c, d) in &edges {
                if (a, b) == (from.index(), to.index()) && cheapest.is_none_or(|(best, _)| c < best) {
                    cheapest = Some((c, d));
                }
            }
            cheapest.map_or(0, |(_, d)| d)
        };
        let mut cost_edges = Vec::new();
        let mut delay_edges = Vec::new();
        for from in 0..nodes {
            let mut steps = Vec::new();
            plain.expand(NetId(0), NodeId(from as u32), &mut steps);
            for step in steps {
                cost_edges.push((from, step.to.index(), step.cost.get() as u64));
                delay_edges.push((from, step.to.index(), u64::from(delay_of(NodeId(from as u32), step.to))));
            }
        }
        let bounded = plain.clone().with_lower_bound(Scaled {
            distance: all_pairs(nodes, &cost_edges),
            fraction: tightness,
        });
        let exact_space = plain.with_step_delays(delay_of);
        let bound_space = bounded.with_step_delays(delay_of).with_delay_bound(Scaled {
            distance: all_pairs(nodes, &delay_edges),
            fraction: tightness,
        });

        // History and overuse, the same for both.
        let mut table = ResourceTable::new(&exact_space);
        for &(node, history, occupancy) in &priced {
            let slot = table.intern(&exact_space, NodeId((node % nodes) as u32));
            table.states[slot].history = Cost::from(history);
            table.states[slot].occupancy = occupancy;
        }
        let held = FastSet::default();
        let replacing = FastSet::default();
        let buffer = RefCell::new(Vec::new());
        let delays = RefCell::new(Vec::new());
        let blend = |priced| Blended {
            priced,
            step_delays: <TimedGraph<'_> as DelaySpace>::step_delays,
            delay_bound: <TimedGraph<'_> as DelaySpace>::delay_lower_bound,
            congestion: cost(1.0 - criticality),
            delay: cost(criticality) * cost(delay_cost),
            delays: &delays,
        };
        let exact_priced = Priced {
            inner: &exact_space,
            table: &table,
            present: cost(present),
            held: &held,
            buffer: &buffer,
            replacing: &replacing,
        };
        let exact = blend(&exact_priced);
        // The same table is read through a space of another type, whose resources and slots
        // are the same.
        let bound_table = ResourceTable::<TimedGraph<'_, Scaled, Scaled>> {
            slots: bound_space.new_resource_slots(),
            states: table.states.clone(),
            listed: table.listed.clone(),
            occupied_slots: table.occupied_slots.clone(),
            occupied_resources: table.occupied_resources.clone(),
        };
        let bound_priced = Priced {
            inner: &bound_space,
            table: &bound_table,
            present: cost(present),
            held: &held,
            buffer: &buffer,
            replacing: &replacing,
        };
        let bounding = Blended {
            priced: &bound_priced,
            step_delays: <TimedGraph<'_, Scaled, Scaled> as DelaySpace>::step_delays,
            delay_bound: <TimedGraph<'_, Scaled, Scaled> as DelaySpace>::delay_lower_bound,
            congestion: exact.congestion,
            delay: exact.delay,
            delays: &delays,
        };

        let mut search = Search::new(&exact_space);
        for from in 0..nodes as u32 {
            for to in 0..nodes as u32 {
                let sources = [Source::new(NodeId(from))];
                let targets = [Target::new(NodeId(to))];
                let outcome = search.find_path_with(&exact, NetId(0), sources, targets, SearchLimits::NONE);
                if let SearchOutcome::Found(found) = outcome {
                    let bound = bounding.lower_bound(NetId(0), NodeId(from), NodeId(to));
                    prop_assert!(
                        bound <= found.cost,
                        "{from} to {to}: bound {bound:?} above {:?}", found.cost
                    );
                }
            }
        }
    }
}

// Generated fabrics.

/// The delay of every step of a [`BenchFabric`], by what it enters: a pin takes 40 ps, and a
/// wire a switch of 70 ps plus 60 ps per tile for single wires and 30 for the long ones that
/// span 4. Long wires are faster per tile, and dearer per wire, as in real devices.
fn fabric_delay(fabric: BenchFabric) -> impl Fn(NodeId, NodeId) -> u32 + Copy {
    move |_from: NodeId, to: NodeId| {
        let per_tile = fabric.nodes_per_tile();
        let (tile, offset) = (to.0 / per_tile, to.0 % per_tile);
        let (x, y) = (tile % fabric.tiles, tile / fabric.tiles);
        if offset < SITE_NODES {
            0
        } else if offset < SITE_NODES + 2 * fabric.pins {
            PIN_DELAY
        } else {
            let index = offset - SITE_NODES - 2 * fabric.pins;
            let covered = fabric.wire_end(x, y, index).2;
            SWITCH_DELAY + WIRE_DELAY_PER_TILE[BenchFabric::wire_kind(index)] * covered
        }
    }
}

const PIN_DELAY: u32 = 40;
const SWITCH_DELAY: u32 = 70;
const WIRE_DELAY_PER_TILE: [u32; 2] = [60, 30];
/// The fastest a tile can be covered: by a long wire, `(70 + 4 * 30) / 4`.
const FASTEST_PER_TILE: f64 = 47.5;

/// The tile distance times the fastest delay of a tile.
fn fabric_delay_bound(fabric: BenchFabric) -> impl Fn(NodeId, NodeId) -> u32 {
    let positions = positions_of(fabric);
    move |from: NodeId, to: NodeId| {
        let (a, b) = (positions[from.index()], positions[to.index()]);
        let distance = u32::from(a.0.abs_diff(b.0)) + u32::from(a.1.abs_diff(b.1));
        (f64::from(distance) * FASTEST_PER_TILE).floor() as u32
    }
}

/// The positions [`fabric_graph`]'s bound uses: the tile of a site's nodes, and the far end of
/// a wire.
fn positions_of(fabric: BenchFabric) -> Vec<(u16, u16)> {
    let per_tile = fabric.nodes_per_tile();
    let nodes = fabric.tiles * fabric.tiles * per_tile;
    (0..nodes)
        .map(|node| {
            let (tile, offset) = (node / per_tile, node % per_tile);
            let (x, y) = (tile % fabric.tiles, tile / fabric.tiles);
            if offset < SITE_NODES + 2 * fabric.pins {
                (x as u16, y as u16)
            } else {
                let (end_x, end_y, _) =
                    fabric.wire_end(x, y, offset - SITE_NODES - 2 * fabric.pins);
                (end_x as u16, end_y as u16)
            }
        })
        .collect()
}

/// A netlist for the nets of a fabric: each tile is a register one time in four, and otherwise
/// a cell of 200 ps from the sink it is driven through to the source it drives. A pin that
/// nothing reaches launches, and one that reaches nothing captures. The clock period is 0, so
/// that the fastest circuit is wanted. Net arcs start at an estimate of 50 ps per tile.
fn fabric_timing(fabric: BenchFabric, nets: &[Net<NodeId>]) -> TimingDriven {
    let tiles = (fabric.tiles * fabric.tiles) as usize;
    let tile_of = |node: NodeId| (node.0 / fabric.nodes_per_tile()) as usize;
    let mut drives = vec![false; tiles];
    let mut driven = vec![false; tiles];
    for net in nets {
        drives[tile_of(net.terminals[0].access_points()[0].node)] = true;
        for terminal in &net.terminals[1..] {
            driven[tile_of(terminal.access_points()[0].node)] = true;
        }
    }

    let mut builder = TimingGraphBuilder::new();
    let mut pins: Vec<(PointId, PointId)> = Vec::with_capacity(tiles);
    for tile in 0..tiles {
        if tile % 4 == 0 {
            let register = builder.add_register(60, 40).unwrap();
            pins.push((register.q, register.d));
        } else {
            let (input, output) = (builder.add_point().unwrap(), builder.add_point().unwrap());
            builder.add_cell_arc(input, output, 200).unwrap();
            if !driven[tile] {
                builder.set_launch(input, 0).unwrap();
            }
            if !drives[tile] {
                builder.set_capture(output, 0).unwrap();
            }
            pins.push((output, input));
        }
    }
    let position = |node: NodeId| {
        let tile = tile_of(node) as u32;
        (tile % fabric.tiles, tile / fabric.tiles)
    };
    let mut connections = Vec::new();
    for net in nets {
        let driver = net.terminals[0].access_points()[0].node;
        for (terminal, sink) in net.terminals.iter().enumerate().skip(1) {
            let sink = sink.access_points()[0].node;
            let ((a, b), (c, d)) = (position(driver), position(sink));
            let estimate = 50 * (a.abs_diff(c) + b.abs_diff(d));
            let arc = builder
                .add_net_arc(pins[tile_of(driver)].0, pins[tile_of(sink)].1, estimate)
                .unwrap();
            connections.push(Connection {
                net: net.id,
                terminal: terminal as u32,
                arc,
            });
        }
    }
    TimingDriven::new(
        builder.build(),
        Constraints::single_clock(0).unwrap(),
        connections,
        cost(1.0 / 50.0),
    )
}

/// A fabric problem with delays: its graph, nets and timing.
fn fabric_problem(
    tiles: u32,
    tracks: u32,
    count: u32,
) -> (Graph<TileBound>, Vec<Net<NodeId>>, BenchFabric) {
    let fabric = small_fabric(tiles, tracks);
    let graph = fabric_graph(fabric);
    let nets = fabric.nets(count, 4, u64::from(tiles) * 7 + u64::from(tracks));
    (graph, nets, fabric)
}

#[test]
fn the_fabric_delay_bound_is_admissible() {
    let (graph, nets, fabric) = fabric_problem(8, 2, 30);
    let space = graph
        .with_step_delays(fabric_delay(fabric))
        .with_delay_bound(fabric_delay_bound(fabric));
    // The true delays from each net's driver, by a search on delays alone.
    let delays_only = DelaysAsCosts(&space);
    let mut search = Search::new(&space);
    for net in &nets {
        let source = net.terminals[0].access_points()[0].node;
        for terminal in &net.terminals[1..] {
            let target = terminal.access_points()[0].node;
            let sources = [Source::new(source)];
            let outcome = search.find_path_with(
                &delays_only,
                net.id,
                sources,
                [Target::new(target)],
                SearchLimits::NONE,
            );
            if let SearchOutcome::Found(found) = outcome {
                let bound = space.delay_lower_bound(net.id, source, target);
                assert!(
                    f64::from(bound) <= found.cost.get(),
                    "{bound} > {:?}",
                    found.cost
                );
            }
        }
    }
}

/// A space whose steps cost their delays, and have no bound, for true distances in delay.
struct DelaysAsCosts<'a, S>(&'a S);

impl<S: DelaySpace> RoutingSpace for DelaysAsCosts<'_, S> {
    type Node = S::Node;
    type Resource = S::Resource;
    type Slots = S::Slots;
    type ResourceSlots = S::ResourceSlots;

    fn new_slots(&self) -> S::Slots {
        self.0.new_slots()
    }
    fn new_resource_slots(&self) -> S::ResourceSlots {
        self.0.new_resource_slots()
    }
    fn contains(&self, node: S::Node) -> bool {
        self.0.contains(node)
    }
    fn expand(&self, net: NetId, from: S::Node, out: &mut Vec<Step<S::Node>>) {
        let start = out.len();
        self.0.expand(net, from, out);
        let mut delays = Vec::new();
        self.0.step_delays(net, from, &mut delays);
        for (step, delay) in out[start..].iter_mut().zip(delays) {
            step.cost = Cost::from(delay);
        }
    }
    fn lower_bound(&self, _net: NetId, _from: S::Node, _to: S::Node) -> Cost {
        Cost::ZERO
    }
    fn resources(&self, net: NetId, from: S::Node, to: S::Node, out: &mut Vec<S::Resource>) {
        self.0.resources(net, from, to, out);
    }
    fn capacity(&self, resource: S::Resource) -> u32 {
        self.0.capacity(resource)
    }
}

#[test]
fn with_nothing_critical_timing_changes_no_route() {
    for (tiles, tracks, count) in [(8, 2, 30), (12, 1, 60), (16, 2, 100)] {
        let (graph, nets, fabric) = fabric_problem(tiles, tracks, count);
        let space = graph
            .with_step_delays(fabric_delay(fabric))
            .with_delay_bound(fabric_delay_bound(fabric));
        // The same connections between points that no path is timed through.
        let mut timing = fabric_timing(fabric, &nets);
        let mut builder = TimingGraphBuilder::new();
        for connection in &mut timing.connections {
            let (a, b) = (builder.add_point().unwrap(), builder.add_point().unwrap());
            connection.arc = builder.add_net_arc(a, b, 0).unwrap();
        }
        timing.graph = builder.build();
        for batch_size in [1, 8] {
            let options = EngineOptions {
                batch_size,
                ..EngineOptions::default()
            };
            let timed = route_checked(&space, &nets, options, &timing);
            let plain = route(&graph, &nets, options).unwrap();
            assert_eq!(timed.routes, plain.routes);
            assert_eq!(timed.overused, plain.overused);
            assert_eq!(timed.best_iteration, plain.best_iteration);
            assert_eq!(timed.total_cost, plain.total_cost);
            assert_eq!(timed.timing.unwrap().metrics.critical_path_delay, None);
        }
    }
}

/// The critical-path delay of routing a fabric problem without and with timing, with the
/// wirelength of each in steps and the expansions they took.
fn compare_on_fabric(
    tiles: u32,
    tracks: u32,
    count: u32,
    options: EngineOptions<'_>,
) -> [(u64, usize, u64); 2] {
    let (graph, nets, fabric) = fabric_problem(tiles, tracks, count);
    let space = graph
        .with_step_delays(fabric_delay(fabric))
        .with_delay_bound(fabric_delay_bound(fabric));
    let timing = fabric_timing(fabric, &nets);
    let plain = route(&space, &nets, options).unwrap();
    assert!(plain.is_success());
    let timed = route_checked(&space, &nets, options, &timing);
    assert!(timed.is_success());
    let summary = |outcome: &Outcome<NodeId, NodeId>| {
        let delay = analyse_afresh(&space, &nets, outcome, &timing)
            .0
            .critical_path_delay
            .unwrap();
        let length = outcome
            .routes
            .routes()
            .map(|(_, route)| route.edges().len())
            .sum();
        (delay, length, outcome.routes.stats.nodes_expanded)
    };
    [summary(&plain), summary(&timed)]
}

#[test]
fn timing_shortens_the_critical_path_on_generated_fabrics() {
    for (tiles, tracks, count) in [(12, 1, 60), (16, 2, 100), (16, 1, 80)] {
        let [plain, timed] = compare_on_fabric(tiles, tracks, count, EngineOptions::default());
        assert!(
            timed.0 < plain.0,
            "{tiles} tiles, {tracks} tracks: {} ps with timing, {} without",
            timed.0,
            plain.0
        );
    }
}

#[test]
fn timing_driven_results_are_the_same_on_any_number_of_threads() {
    for (tiles, tracks, count, batch_size) in [
        (12, 1, 60, 8),
        (16, 2, 100, 16),
        (12, 1, 60, 1),
        (16, 1, 120, 8),
    ] {
        let (graph, nets, fabric) = fabric_problem(tiles, tracks, count);
        let space = graph
            .with_step_delays(fabric_delay(fabric))
            .with_delay_bound(fabric_delay_bound(fabric));
        let timing = fabric_timing(fabric, &nets);
        let options = EngineOptions {
            batch_size,
            ..EngineOptions::default()
        };
        let mut engine = Engine::with_timing(&space, &nets, options, timing.clone()).unwrap();
        while engine.step().is_some() {}
        let reference = (engine.outcome(), engine.reports().to_vec());
        check_honest(&space, &nets, &reference.0).unwrap();
        for threads in THREAD_COUNTS {
            let mut engine = Engine::with_timing(&space, &nets, options, timing.clone()).unwrap();
            while engine.step_parallel(threads).is_some() {}
            assert_eq!(
                engine.outcome(),
                reference.0,
                "outcome on {threads} threads"
            );
            assert_eq!(
                engine.reports(),
                reference.1,
                "reports on {threads} threads"
            );
        }
        let threaded = route_timed_parallel(&space, &nets, options, timing.clone(), 4).unwrap();
        assert_eq!(threaded, reference.0);
        // Every iteration is reported with its timing.
        assert!(reference.1.iter().all(|report| report.timing.is_some()));
    }
}

/// Pinned results of timing-driven runs, to catch any change in what they do. Like the
/// golden values without timing, a change here needs a reason.
#[test]
fn timing_driven_results_are_pinned() {
    let mut found = Vec::new();
    for batch_size in [1, 8] {
        let (graph, nets, fabric) = fabric_problem(16, 1, 120);
        let space = graph
            .with_step_delays(fabric_delay(fabric))
            .with_delay_bound(fabric_delay_bound(fabric));
        let timing = fabric_timing(fabric, &nets);
        let options = EngineOptions {
            batch_size,
            ..EngineOptions::default()
        };
        let outcome = route_checked(&space, &nets, options, &timing);
        let metrics = outcome.timing.as_ref().unwrap().metrics;
        found.push((
            hash_outcome(&outcome),
            outcome.routes.stats.iterations,
            outcome.routes.stats.nodes_expanded,
            outcome.total_cost.get().to_bits(),
            metrics.critical_path_delay,
        ));
    }
    let expected = [
        (
            9153181624322829570,
            8,
            36948,
            4652424023748116480,
            Some(6130),
        ),
        (
            14700777350216238781,
            8,
            42436,
            4652437492765556736,
            Some(6130),
        ),
    ];
    assert_eq!(found, expected);
}

#[test]
fn every_iteration_reports_its_critical_path() {
    let (graph, nets, fabric) = fabric_problem(12, 1, 60);
    let space = graph
        .with_step_delays(fabric_delay(fabric))
        .with_delay_bound(fabric_delay_bound(fabric));
    let timing = fabric_timing(fabric, &nets);
    let mut engine = Engine::with_timing(&space, &nets, EngineOptions::default(), timing).unwrap();
    while let Some(report) = engine.step() {
        let analysis = engine.timing_analysis().unwrap();
        let metrics = report.timing.unwrap();
        assert_eq!(
            metrics.critical_path_delay,
            analysis.critical_path().map(|p| p.arrival())
        );
        assert_eq!(metrics.worst_slack, analysis.worst_slack());
    }
    let outcome = engine.outcome();
    let best = engine.reports()[outcome.best_iteration as usize - 1];
    assert_eq!(best.timing, Some(outcome.timing.unwrap().metrics));
    assert!(engine.timing_graph().is_some());
}

#[test]
fn a_better_critical_path_beats_a_cheaper_route_among_legal_results() {
    let fast = TimingMetrics {
        critical_path_delay: Some(100),
        worst_slack: Some(-100),
        total_negative_slack: -100,
    };
    let slow = TimingMetrics {
        critical_path_delay: Some(200),
        worst_slack: Some(-200),
        total_negative_slack: -150,
    };
    let met = TimingMetrics {
        critical_path_delay: Some(200),
        worst_slack: Some(50),
        total_negative_slack: 0,
    };
    let more_slack = TimingMetrics {
        worst_slack: Some(80),
        ..met
    };
    assert!(rank(0, 0, Some(&fast), Cost::from(9)) < rank(0, 0, Some(&slow), Cost::from(1)));
    // Legality first.
    assert!(rank(0, 1, Some(&fast), Cost::from(1)) > rank(0, 0, Some(&slow), Cost::from(9)));
    // Once timing is met, as in VPR, more slack is not better, and cost decides.
    assert!(rank(0, 0, Some(&more_slack), Cost::from(2)) > rank(0, 0, Some(&met), Cost::from(1)));
    assert_eq!(rank(0, 0, None, Cost::from(3)), (0, 0, 0, 0, Cost::from(3)));
}
