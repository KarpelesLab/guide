//! Tests of fixed and partially pre-routed nets.
//!
//! Every result is checked with the independent validator's [`check_fixed`], which shares
//! nothing with the engine, as well as with the engine's own test checks.

use super::tests::{any_grid_problem, check_honest, hash_outcome};
use super::*;
use crate::id::NodeId;
use crate::net::{AccessPoint, Edge, NetOutcome, Terminal};
use crate::space::graph::{Graph, GraphBuilder};
use crate::space::grid::Grid;
use crate::timing::TimingGraphBuilder;
use crate::validate::{Fault, check_fixed};
use proptest::prelude::*;

fn net(id: u32, nodes: &[NodeId]) -> Net<NodeId> {
    Net {
        id: NetId(id),
        terminals: nodes.iter().map(|&node| Terminal::at(node)).collect(),
    }
}

fn path(nodes: &[NodeId]) -> Route<NodeId> {
    let mut route = Route::new();
    route.add_path(nodes).unwrap();
    route
}

fn fixed(id: u32, route: Route<NodeId>) -> FixedRoute<NodeId> {
    FixedRoute {
        net: NetId(id),
        route,
    }
}

/// A directed graph with edges of cost 1.
fn graph(nodes: u32, edges: &[(u32, u32)]) -> Graph {
    let mut builder = GraphBuilder::new(nodes);
    for &(from, to) in edges {
        builder
            .add_edge(NodeId(from), NodeId(to), Cost::ONE)
            .unwrap();
    }
    builder.build()
}

/// Runs with fixed routes, and checks the outcome against itself and the validator.
fn run_fixed<S: RoutingSpace<Node = NodeId>>(
    space: &S,
    nets: &[Net<NodeId>],
    options: EngineOptions<'_>,
    routes: &[FixedRoute<NodeId>],
) -> Outcome<NodeId, S::Resource> {
    let mut engine = Engine::new(space, nets, options).unwrap();
    engine.fix_routes(routes.iter().cloned()).unwrap();
    let outcome = engine.run();
    check_honest(space, nets, &outcome).unwrap();
    let report = check_fixed(space, nets, &outcome.routes, routes);
    for fault in report.faults() {
        assert!(
            matches!(fault, Fault::Overuse { .. }),
            "the validator found {fault}"
        );
    }
    assert_eq!(report.is_complete(), outcome.is_success());
    outcome
}

/// A row of grid nodes on layer 0.
fn row(grid: &Grid, y: u32, xs: core::ops::RangeInclusive<u32>) -> Vec<NodeId> {
    xs.map(|x| grid.node_at(x, y, 0).unwrap()).collect()
}

#[test]
fn a_fixed_net_keeps_its_route_and_others_pay_to_cross_it() {
    let grid = Grid::new(5, 5, 2).unwrap();
    let at = |x, y| grid.node_at(x, y, 0).unwrap();
    let nets = [net(0, &[at(0, 2), at(4, 2)]), net(1, &[at(2, 0), at(2, 4)])];
    let clock = path(&row(&grid, 2, 0..=4));
    let outcome = run_fixed(&grid, &nets, options(), &[fixed(0, clock.clone())]);

    assert!(outcome.is_success());
    assert_eq!(outcome.routes.route(NetId(0)), Some(&clock));
    let crossing = outcome.routes.route(NetId(1)).unwrap();
    assert!(!crossing.contains(at(2, 2)));
    // The crossing net was searched for once in each iteration, and the clock never was.
    assert_eq!(
        outcome.routes.stats.searches,
        outcome.routes.stats.iterations
    );
}

#[test]
fn a_fixed_net_is_never_rerouted_even_when_it_is_in_the_way() {
    // 0 -> 1 -> 2 is the only way for net 1, and the fixed net 0 sits on 1.
    let space = graph(4, &[(0, 1), (1, 2), (3, 1)]);
    let nets = [
        net(0, &[NodeId(3), NodeId(1)]),
        net(1, &[NodeId(0), NodeId(2)]),
    ];
    let routes = [fixed(0, path(&[NodeId(3), NodeId(1)]))];
    let mut limited = options();
    limited.max_iterations = 5;
    let outcome = run_fixed(&space, &nets, limited, &routes);

    assert!(!outcome.is_success());
    assert_eq!(outcome.stop, StopReason::IterationLimit);
    assert_eq!(outcome.routes.route(NetId(0)), Some(&routes[0].route));
    let [entry] = &outcome.overused[..] else {
        panic!("one resource is overused: {:?}", outcome.overused);
    };
    assert_eq!(entry.resource, NodeId(1));
    assert_eq!(entry.nets, [NetId(0), NetId(1)]);
    // Only one of them holds it through a fixed route, so the conflict is the other's to settle.
    assert_eq!(entry.fixed, 1);
}

#[test]
fn fixed_routes_that_conflict_are_reported_and_the_run_still_converges() {
    let grid = Grid::new(5, 5, 1).unwrap();
    let at = |x, y| grid.node_at(x, y, 0).unwrap();
    let nets = [
        net(0, &[at(0, 2), at(4, 2)]),
        net(1, &[at(2, 0), at(2, 4)]),
        net(2, &[at(0, 0), at(4, 4)]),
    ];
    let routes = [
        fixed(0, path(&row(&grid, 2, 0..=4))),
        fixed(1, path(&[at(2, 0), at(2, 1), at(2, 2), at(2, 3), at(2, 4)])),
    ];
    let mut engine = Engine::new(&grid, &nets, options()).unwrap();
    engine.fix_routes(routes.iter().cloned()).unwrap();
    // The conflict shows before anything is routed.
    let before = engine.overused();
    assert_eq!(before.len(), 1);
    assert_eq!((before[0].resource, before[0].fixed), (at(2, 2), 2));

    // Net 2 has to cross the cross of fixed routes on a single layer, which it can only do
    // through one of their nodes, so it overuses something whatever it does.
    let mut limited = options();
    limited.max_iterations = 8;
    let outcome = Engine::new(&grid, &nets, limited)
        .and_then(|mut engine| {
            engine.fix_routes(routes.iter().cloned())?;
            Ok(engine.run())
        })
        .unwrap();
    check_honest(&grid, &nets, &outcome).unwrap();
    assert_eq!(outcome.stop, StopReason::IterationLimit);
    assert!(outcome.overused.iter().any(|entry| entry.fixed == 2));

    // Without net 2, nothing is left but the fixed conflict, and the run stops at once.
    let outcome = run_fixed(&grid, &nets[..2], options(), &routes);
    assert_eq!(outcome.stop, StopReason::Converged);
    assert_eq!(outcome.routes.stats.iterations, 1);
    assert!(!outcome.is_success());
    let [entry] = &outcome.overused[..] else {
        panic!("{:?}", outcome.overused);
    };
    assert_eq!(
        (entry.resource, entry.fixed, entry.capacity),
        (at(2, 2), 2, 1)
    );
}

#[test]
fn a_fixed_route_on_a_blocked_cell_is_reported_without_looping() {
    let mut grid = Grid::new(4, 3, 1).unwrap();
    grid.set_blocked(1, 1, 0, true).unwrap();
    let nets = [
        net(
            0,
            &[
                grid.node_at(0, 1, 0).unwrap(),
                grid.node_at(3, 1, 0).unwrap(),
            ],
        ),
        net(
            1,
            &[
                grid.node_at(0, 0, 0).unwrap(),
                grid.node_at(3, 0, 0).unwrap(),
            ],
        ),
    ];
    // Steps into a blocked cell are not steps, so the only way to sit on one is to start
    // there, as the root of a net that needs no routing.
    let stub = Net {
        id: NetId(2),
        terminals: vec![Terminal::at(grid.node_at(1, 1, 0).unwrap())],
    };
    let mut nets = nets.to_vec();
    nets.push(stub);
    let routes = [fixed(2, Route::from_root(grid.node_at(1, 1, 0).unwrap()))];
    let mut engine = Engine::new(&grid, &nets, options()).unwrap();
    engine.fix_routes(routes.iter().cloned()).unwrap();
    let outcome = engine.run();
    // The other two nets settle their own conflict, and then nothing is left to do.
    assert_eq!(outcome.stop, StopReason::Converged);
    assert!(outcome.routes.stats.iterations < 10);
    let [entry] = &outcome.overused[..] else {
        panic!("{:?}", outcome.overused);
    };
    assert_eq!((entry.capacity, entry.fixed), (0, 1));
    assert_eq!(engine_report_fixed_overuse(&grid, &nets, &routes), 1);
    let report = check_fixed(&grid, &nets, &outcome.routes, &routes);
    assert_eq!(
        report.faults(),
        [Fault::Overuse {
            resource: grid.node_at(1, 1, 0).unwrap(),
            capacity: 0,
            nets: vec![NetId(2)],
        }]
    );
}

/// The fixed overuse that the first iteration reports.
fn engine_report_fixed_overuse(
    grid: &Grid,
    nets: &[Net<NodeId>],
    routes: &[FixedRoute<NodeId>],
) -> u64 {
    let mut engine = Engine::new(grid, nets, options()).unwrap();
    engine.fix_routes(routes.iter().cloned()).unwrap();
    engine.step().unwrap().fixed_overuse
}

#[test]
fn a_partial_route_is_completed_and_kept() {
    let grid = Grid::new(6, 6, 2).unwrap();
    let at = |x, y| grid.node_at(x, y, 0).unwrap();
    let nets = [net(0, &[at(0, 0), at(3, 0), at(3, 5), at(5, 3)])];
    // Fixed as far as the first sink, by a detour the engine would never take.
    let start = path(&[at(0, 0), at(0, 1), at(1, 1), at(2, 1), at(3, 1), at(3, 0)]);
    let outcome = run_fixed(&grid, &nets, options(), &[fixed(0, start.clone())]);

    assert!(outcome.is_success());
    let route = outcome.routes.route(NetId(0)).unwrap();
    assert_eq!(&route.edges()[..start.edges().len()], start.edges());
    assert!(route.edges().len() > start.edges().len());
}

#[test]
fn a_partial_route_only_loses_what_the_engine_added() {
    // Net 0's start is fixed along the middle row, and what it adds to reach the bottom edge
    // has to negotiate with net 1 along that edge, so it is routed again, always from its fixed
    // part.
    let grid = Grid::new(5, 5, 2).unwrap();
    let at = |x, y| grid.node_at(x, y, 0).unwrap();
    let nets = [
        net(0, &[at(0, 2), at(4, 2), at(2, 4)]),
        net(1, &[at(1, 4), at(3, 4)]),
    ];
    let start = path(&row(&grid, 2, 0..=4));
    let routes = [fixed(0, start.clone())];
    let mut engine = Engine::new(&grid, &nets, options()).unwrap();
    engine.fix_routes(routes.iter().cloned()).unwrap();
    while let Some(report) = engine.step() {
        let current = engine.outcome();
        if let Some(route) = current.routes.route(NetId(0)) {
            assert_eq!(route.root(), start.root());
            assert_eq!(&route.edges()[..start.edges().len()], start.edges());
        }
        assert!(report.iteration > 1 || report.total_overuse > 0);
    }
    let outcome = engine.outcome();
    check_honest(&grid, &nets, &outcome).unwrap();
    assert!(outcome.is_success());
    assert!(outcome.routes.stats.iterations > 1);
    assert!(check_fixed(&grid, &nets, &outcome.routes, &routes).is_complete());
}

#[test]
fn a_partial_route_that_cannot_be_completed_is_no_path_and_gives_its_part_up() {
    // The driver can start at 0 or 5, and only 5 leads to 3. The fixed part starts at 0, and
    // takes node 1, which net 1 needs.
    let space = graph(6, &[(0, 1), (5, 2), (2, 3), (4, 1)]);
    let driver = Terminal::new(vec![
        AccessPoint {
            node: NodeId(0),
            cost: Cost::ZERO,
        },
        AccessPoint {
            node: NodeId(5),
            cost: Cost::ZERO,
        },
    ])
    .unwrap();
    let nets = [
        Net {
            id: NetId(0),
            terminals: vec![driver, Terminal::at(NodeId(3))],
        },
        net(1, &[NodeId(4), NodeId(1)]),
    ];
    let routes = [fixed(0, path(&[NodeId(0), NodeId(1)]))];
    let outcome = run_fixed(&space, &nets, options(), &routes);

    assert_eq!(
        outcome.routes.outcome(NetId(0)),
        Some(&NetOutcome::Unrouted(UnroutedReason::NoPath))
    );
    assert!(outcome.overused.is_empty());
    assert!(outcome.routes.route(NetId(1)).unwrap().contains(NodeId(1)));
}

#[test]
fn a_net_with_a_terminal_outside_the_space_gives_its_fixed_part_up() {
    let space = graph(3, &[(0, 1), (1, 2)]);
    let nets = [
        net(0, &[NodeId(0), NodeId(9)]),
        net(1, &[NodeId(1), NodeId(2)]),
    ];
    let routes = [fixed(0, path(&[NodeId(0), NodeId(1)]))];
    let mut engine = Engine::new(&space, &nets, options()).unwrap();
    engine.fix_routes(routes.iter().cloned()).unwrap();
    let outcome = engine.run();
    check_honest(&space, &nets, &outcome).unwrap();
    assert_eq!(
        outcome.routes.outcome(NetId(0)),
        Some(&NetOutcome::Unrouted(UnroutedReason::NoPath))
    );
    assert!(outcome.overused.is_empty());
    assert!(outcome.routes.route(NetId(1)).is_some());
}

#[test]
fn a_fixed_route_on_a_net_that_needs_no_routing_is_kept_as_a_blockage() {
    let space = graph(4, &[(0, 1), (1, 3), (0, 2), (2, 3)]);
    let nets = [
        Net {
            id: NetId(0),
            terminals: vec![Terminal::at(NodeId(1))],
        },
        net(1, &[NodeId(0), NodeId(3)]),
    ];
    let routes = [fixed(0, Route::from_root(NodeId(1)))];
    let mut engine = Engine::new(&space, &nets, options()).unwrap();
    engine.fix_routes(routes.iter().cloned()).unwrap();
    let outcome = engine.run();
    assert!(outcome.is_success());
    assert_eq!(outcome.routes.route(NetId(0)), Some(&routes[0].route));
    assert!(outcome.routes.route(NetId(1)).unwrap().contains(NodeId(2)));
    assert!(check_fixed(&space, &nets, &outcome.routes, &routes).is_complete());
}

#[test]
fn bad_fixed_routes_are_refused_and_leave_the_engine_alone() {
    let space = graph(4, &[(0, 1), (1, 2), (2, 3)]);
    let nets = [
        net(0, &[NodeId(0), NodeId(3)]),
        Net {
            id: NetId(1),
            terminals: vec![Terminal::at(NodeId(3))],
        },
    ];
    let problem = |routes: Vec<FixedRoute<NodeId>>| {
        let mut engine = Engine::new(&space, &nets, options()).unwrap();
        match engine.fix_routes(routes) {
            Err(EngineError::InvalidFixedRoute { problem, .. }) => {
                // Nothing was taken, not even the routes before the bad one.
                assert!(engine.states.iter().all(|state| state.fixed.is_none()));
                Some(problem)
            }
            Err(other) => panic!("{other}"),
            Ok(()) => None,
        }
    };
    let good = || fixed(0, path(&[NodeId(0), NodeId(1)]));

    assert_eq!(
        problem(vec![fixed(5, path(&[NodeId(0)]))]),
        Some(FixedRouteProblem::UnknownNet)
    );
    assert_eq!(
        problem(vec![good(), good()]),
        Some(FixedRouteProblem::Repeated)
    );
    assert_eq!(
        problem(vec![good(), fixed(0, Route::new())]),
        Some(FixedRouteProblem::Repeated)
    );
    assert_eq!(
        problem(vec![fixed(0, Route::new())]),
        Some(FixedRouteProblem::Empty)
    );
    let backwards = Route::from_edges(
        NodeId(0),
        vec![Edge {
            from: NodeId(1),
            to: NodeId(2),
        }],
    );
    assert_eq!(
        problem(vec![good(), fixed(1, backwards)]),
        Some(FixedRouteProblem::NotATree)
    );
    assert_eq!(
        problem(vec![fixed(0, path(&[NodeId(1), NodeId(2)]))]),
        Some(FixedRouteProblem::RootNotOnDriver)
    );
    assert_eq!(
        problem(vec![fixed(0, path(&[NodeId(0), NodeId(7)]))]),
        Some(FixedRouteProblem::NodeNotInSpace)
    );
    assert_eq!(
        problem(vec![fixed(0, path(&[NodeId(0), NodeId(1), NodeId(3)]))]),
        Some(FixedRouteProblem::NotAStep { index: 1 })
    );
    assert_eq!(
        problem(vec![fixed(1, Route::from_root(NodeId(2)))]),
        Some(FixedRouteProblem::MissingTerminal)
    );
    assert_eq!(problem(vec![good()]), None);

    let mut engine = Engine::new(&space, &nets, options()).unwrap();
    engine.step();
    assert_eq!(
        engine.fix_routes([good()]),
        Err(EngineError::InvalidFixedRoute {
            net: NetId(0),
            problem: FixedRouteProblem::AlreadyStarted,
        })
    );
    // Nothing at all is not an error, even then.
    assert_eq!(engine.fix_routes([]), Ok(()));
    assert_eq!(
        EngineError::InvalidFixedRoute {
            net: NetId(0),
            problem: FixedRouteProblem::NotAStep { index: 1 },
        }
        .to_string(),
        "the fixed route of net 0: edge 1 of the route is not a step of the space"
    );
}

#[test]
fn fixed_routes_in_any_edge_order_are_taken_as_trees() {
    let space = graph(4, &[(0, 1), (1, 2), (1, 3)]);
    let nets = [net(0, &[NodeId(0), NodeId(2), NodeId(3)])];
    let edge = |from, to| Edge {
        from: NodeId(from),
        to: NodeId(to),
    };
    let shuffled = Route::from_edges(NodeId(0), vec![edge(1, 3), edge(1, 2), edge(0, 1)]);
    let routes = [fixed(0, shuffled.clone())];
    let mut engine = Engine::new(&space, &nets, options()).unwrap();
    engine.fix_routes(routes.iter().cloned()).unwrap();
    let outcome = engine.run();
    assert!(outcome.is_success());
    assert!(check_fixed(&space, &nets, &outcome.routes, &routes).is_complete());
    assert_eq!(outcome.routes.route(NetId(0)), Some(&shuffled));
    assert_eq!(outcome.total_cost, Cost::from(3));
    assert_eq!(outcome.routes.stats.searches, 0);
}

#[test]
fn fixed_delays_count_from_the_start_in_a_timing_driven_run() {
    // Net 0 is fixed on the slow way, 0 -> 1 -> 3, and net 1 completes from the fixed part
    // 4 -> 5 to its two sinks.
    let mut builder = GraphBuilder::new(8);
    for (from, to) in [(0, 1), (1, 3), (0, 2), (2, 3), (4, 5), (5, 6), (5, 7)] {
        builder
            .add_edge(NodeId(from), NodeId(to), Cost::ONE)
            .unwrap();
    }
    let graph = builder.build();
    let space = graph.with_step_delays(|from, to| if from.0 == 1 || to.0 == 1 { 500 } else { 100 });

    let mut timing = TimingGraphBuilder::new();
    let (a, b) = (timing.add_input(0).unwrap(), timing.add_output(0).unwrap());
    let (c, d, e) = (
        timing.add_input(0).unwrap(),
        timing.add_output(0).unwrap(),
        timing.add_output(0).unwrap(),
    );
    let arcs = [
        timing.add_net_arc(a, b, 0).unwrap(),
        timing.add_net_arc(c, d, 0).unwrap(),
        timing.add_net_arc(c, e, 0).unwrap(),
    ];
    let connections = vec![
        Connection {
            net: NetId(0),
            terminal: 1,
            arc: arcs[0],
        },
        Connection {
            net: NetId(1),
            terminal: 1,
            arc: arcs[1],
        },
        Connection {
            net: NetId(1),
            terminal: 2,
            arc: arcs[2],
        },
    ];
    let setup = TimingDriven::new(
        timing.build(),
        Constraints::single_clock(0).unwrap(),
        connections,
        Cost::new(0.01).unwrap(),
    );
    let nets = [
        net(0, &[NodeId(0), NodeId(3)]),
        net(1, &[NodeId(4), NodeId(6), NodeId(7)]),
    ];
    let routes = [
        fixed(0, path(&[NodeId(0), NodeId(1), NodeId(3)])),
        fixed(1, path(&[NodeId(4), NodeId(5)])),
    ];
    let mut engine = Engine::with_timing(&space, &nets, options(), setup).unwrap();
    engine.fix_routes(routes.iter().cloned()).unwrap();
    // The slow fixed net is the critical path already, before any iteration.
    assert_eq!(
        engine
            .timing_analysis()
            .unwrap()
            .critical_path()
            .map(|path| path.arrival()),
        Some(1000)
    );
    assert_eq!(engine.connection_criticality(0), Some(0.99));
    let outcome = engine.run();
    assert!(outcome.is_success());
    assert_eq!(outcome.routes.route(NetId(0)), Some(&routes[0].route));
    let timing = outcome.timing.unwrap();
    assert_eq!(timing.connection_delays, [Some(1000), Some(200), Some(200)]);
    assert_eq!(timing.metrics.critical_path_delay, Some(1000));
    assert!(check_fixed(&space, &nets, &outcome.routes, &routes).is_complete());
}

#[test]
fn fixed_routes_give_the_same_result_on_any_number_of_threads() {
    let grid = Grid::new(12, 12, 2).unwrap();
    let at = |x, y| grid.node_at(x, y, 0).unwrap();
    let mut nets: Vec<_> = (0..10)
        .map(|i| net(i, &[at(0, i + 1), at(11, 10 - i)]))
        .collect();
    nets.push(net(10, &[at(5, 0), at(5, 11), at(8, 6)]));
    let clock: Vec<NodeId> = (0..=11).map(|y| at(5, y)).collect();
    let routes = [fixed(10, path(&clock[..6]))];

    let mut batched = options();
    batched.batch_size = 4;
    let reference = run_fixed(&grid, &nets, batched, &routes);
    for threads in [1, 2, 4] {
        let mut engine = Engine::new(&grid, &nets, batched).unwrap();
        engine.fix_routes(routes.iter().cloned()).unwrap();
        let outcome = engine.run_parallel(threads);
        assert_eq!(hash_outcome(&outcome), hash_outcome(&reference));
        assert_eq!(outcome, reference);
    }
}

#[test]
fn a_run_without_fixed_routes_is_unchanged_by_an_empty_call() {
    let grid = Grid::new(8, 8, 2).unwrap();
    let at = |x, y| grid.node_at(x, y, 0).unwrap();
    let nets: Vec<_> = (0..6).map(|i| net(i, &[at(0, i), at(7, 5 - i)])).collect();
    let plain = route(&grid, &nets, options()).unwrap();
    let mut engine = Engine::new(&grid, &nets, options()).unwrap();
    engine.fix_routes([]).unwrap();
    assert_eq!(engine.run(), plain);
}

fn options() -> EngineOptions<'static> {
    EngineOptions::default()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Fixing some nets of a first result, and routing again, keeps them and is honest.
    #[test]
    fn fixing_part_of_a_result_keeps_it_and_stays_honest(
        (grid, nets) in any_grid_problem(),
        pick in any::<u64>(),
        cut in any::<u64>(),
    ) {
        let mut limited = options();
        limited.max_iterations = 12;
        let first = route(&grid, &nets, limited).unwrap();
        let mut routes = Vec::new();
        for (position, net) in nets.iter().enumerate() {
            if pick >> (position % 64) & 1 == 0 || !net.needs_routing() {
                continue;
            }
            let Some(route) = first.routes.route(net.id) else { continue };
            // Every other one keeps only a prefix of its edges, which is still a tree from the
            // same root, and makes a partial pre-route.
            let keep = if cut >> (position % 64) & 1 == 1 {
                route.edges().len() / 2
            } else {
                route.edges().len()
            };
            let Some(root) = route.root() else { continue };
            routes.push(FixedRoute {
                net: net.id,
                route: Route::from_edges(root, route.edges()[..keep].to_vec()),
            });
        }
        let mut engine = Engine::new(&grid, &nets, limited).unwrap();
        engine.fix_routes(routes.iter().cloned()).unwrap();
        let outcome = engine.run();
        let honest = check_honest(&grid, &nets, &outcome);
        prop_assert!(honest.is_ok(), "{:?}", honest);
        let report = check_fixed(&grid, &nets, &outcome.routes, &routes);
        for fault in report.faults() {
            prop_assert!(matches!(fault, Fault::Overuse { .. }), "{}", fault);
        }
        prop_assert_eq!(report.is_complete(), outcome.is_success());
        // A net that was fixed whole is exactly as given.
        for fixed in &routes {
            let net = nets.iter().find(|net| net.id == fixed.net).unwrap();
            if fixed.route.validate(net).is_ok() {
                prop_assert_eq!(outcome.routes.route(net.id), Some(&fixed.route));
            }
        }
    }
}
