//! Tests of seeded runs, for engineering changes.
//!
//! Every result is checked with the independent validator as well as the engine's own checks.

use super::parallel_tests::{THREAD_COUNTS, fabric_graph, small_fabric};
use super::tests::{any_grid_problem, check_honest};
use super::*;
use crate::eco::{reroute, reroute_parallel, route_change};
use crate::id::NodeId;
use crate::space::grid::Grid;
use crate::validate::check;
use proptest::prelude::*;

fn net(id: u32, nodes: &[NodeId]) -> Net<NodeId> {
    Net {
        id: NetId(id),
        terminals: nodes.iter().map(|&node| Terminal::at(node)).collect(),
    }
}

fn seeds(routes: &RouteSet<NodeId>) -> impl Iterator<Item = (NetId, Route<NodeId>)> + '_ {
    routes.routes().map(|(id, route)| (id, route.clone()))
}

/// Runs from seeds, and checks the outcome against itself and the validator.
fn run_seeded<S: RoutingSpace<Node = NodeId>>(
    space: &S,
    nets: &[Net<NodeId>],
    options: EngineOptions<'_>,
    previous: &RouteSet<NodeId>,
) -> (Outcome<NodeId, S::Resource>, SeedReport) {
    let mut engine = Engine::new(space, nets, options).unwrap();
    let report = engine.seed_routes(seeds(previous)).unwrap();
    let outcome = engine.run();
    check_honest(space, nets, &outcome).unwrap();
    let validated = check(space, nets, &outcome.routes);
    assert_eq!(validated.is_complete(), outcome.is_success());
    (outcome, report)
}

/// A grid with nets along rows, every other row, that route straight.
fn rows(size: u32) -> (Grid, Vec<Net<NodeId>>) {
    let grid = Grid::new(size, size, 2).unwrap();
    let nets = (0..size / 2)
        .map(|i| {
            let y = 2 * i;
            let from = grid.node_at(0, y, 0).unwrap();
            net(i, &[from, grid.node_at(size - 1, y, 0).unwrap()])
        })
        .collect();
    (grid, nets)
}

#[test]
fn an_unchanged_problem_keeps_every_route_and_routes_nothing() {
    let fabric = small_fabric(12, 1);
    let (graph, nets) = (fabric_graph(fabric), fabric.nets(50, 6, 3));
    let before = route(&graph, &nets, EngineOptions::default()).unwrap();
    assert!(before.is_success());
    let (after, report) = run_seeded(&graph, &nets, EngineOptions::default(), &before.routes);
    assert_eq!(report.count(Seeded::Kept), nets.len());
    assert!(after.is_success());
    assert_eq!(after.routes.stats.iterations, 1);
    assert_eq!(after.routes.stats.searches, 0);
    for (id, route) in before.routes.routes() {
        assert_eq!(after.routes.route(id), Some(route));
    }
    assert_eq!(after.total_cost, before.total_cost);
}

#[test]
fn a_net_added_routes_around_the_others() {
    let (grid, mut nets) = rows(8);
    let before = route(&grid, &nets, EngineOptions::default()).unwrap();
    // Across three rows, which has to go through the second layer.
    let top = grid.node_at(3, 1, 0).unwrap();
    nets.push(net(100, &[top, grid.node_at(3, 7, 0).unwrap()]));
    let (after, report) = run_seeded(&grid, &nets, EngineOptions::default(), &before.routes);
    assert!(after.is_success());
    assert_eq!(report.count(Seeded::Kept), 4);
    let change = route_change(&before.routes, &after.routes);
    assert_eq!(change.nets_added, 1);
    assert_eq!(change.nets_unchanged, 4);
}

#[test]
fn removed_nets_are_reported_and_the_rest_kept() {
    let (grid, nets) = rows(8);
    let before = route(&grid, &nets, EngineOptions::default()).unwrap();
    let fewer = &nets[1..];
    let (after, report) = run_seeded(&grid, fewer, EngineOptions::default(), &before.routes);
    assert_eq!(report.nets[0], (NetId(0), Seeded::UnknownNet));
    assert_eq!(report.count(Seeded::Kept), 3);
    let change = route_change(&before.routes, &after.routes);
    assert_eq!((change.nets_removed, change.nets_unchanged), (1, 3));
}

#[test]
fn a_terminal_removed_prunes_its_branch() {
    let grid = Grid::new(7, 7, 1).unwrap();
    let at = |x, y| grid.node_at(x, y, 0).unwrap();
    let nets = [net(0, &[at(0, 3), at(6, 3), at(3, 6)])];
    let before = route(&grid, &nets, EngineOptions::default()).unwrap();
    let fewer = [net(0, &[at(0, 3), at(6, 3)])];
    let (after, report) = run_seeded(&grid, &fewer, EngineOptions::default(), &before.routes);
    assert_eq!(report.nets, [(NetId(0), Seeded::Pruned)]);
    assert_eq!(after.routes.stats.searches, 0);
    let old = before.routes.route(NetId(0)).unwrap();
    let new = after.routes.route(NetId(0)).unwrap();
    assert!(new.edges().iter().all(|edge| old.edges().contains(edge)));
    assert!(!new.contains(at(3, 6)));
    assert_eq!(new.nodes().count(), 7);
}

#[test]
fn a_terminal_added_grows_from_what_was_there() {
    let grid = Grid::new(7, 7, 1).unwrap();
    let at = |x, y| grid.node_at(x, y, 0).unwrap();
    let nets = [net(0, &[at(0, 3), at(6, 3)])];
    let before = route(&grid, &nets, EngineOptions::default()).unwrap();
    let more = [net(0, &[at(0, 3), at(6, 3), at(3, 6)])];
    let (after, report) = run_seeded(&grid, &more, EngineOptions::default(), &before.routes);
    assert_eq!(report.nets, [(NetId(0), Seeded::Partial)]);
    assert!(after.is_success());
    // One search, from the tree that was there.
    assert_eq!(after.routes.stats.searches, 1);
    let old = before.routes.route(NetId(0)).unwrap();
    let new = after.routes.route(NetId(0)).unwrap();
    assert!(old.edges().iter().all(|edge| new.edges().contains(edge)));
    assert_eq!(new.nodes().count(), 10);
}

#[test]
fn a_moved_driver_drops_the_seed() {
    let grid = Grid::new(7, 7, 1).unwrap();
    let at = |x, y| grid.node_at(x, y, 0).unwrap();
    let nets = [net(0, &[at(0, 3), at(6, 3)])];
    let before = route(&grid, &nets, EngineOptions::default()).unwrap();
    let moved = [net(0, &[at(0, 4), at(6, 3)])];
    let (after, report) = run_seeded(&grid, &moved, EngineOptions::default(), &before.routes);
    assert_eq!(report.nets, [(NetId(0), Seeded::Dropped)]);
    assert!(after.is_success());
}

#[test]
fn a_blocked_node_moves_only_the_nets_through_it() {
    let (mut grid, nets) = rows(8);
    let before = route(&grid, &nets, EngineOptions::default()).unwrap();
    grid.set_blocked(4, 2, 0, true).unwrap();
    let (after, report) = run_seeded(&grid, &nets, EngineOptions::default(), &before.routes);
    assert!(after.is_success());
    // What leads to the driver leads to no terminal, so nothing is kept of the net through the
    // blocked node.
    assert_eq!(report.nets[1], (NetId(1), Seeded::Dropped));
    let change = route_change(&before.routes, &after.routes);
    assert_eq!(change.nets_unchanged, 3);
    for id in [0, 2, 3] {
        assert_eq!(
            after.routes.route(NetId(id)),
            before.routes.route(NetId(id))
        );
    }
}

#[test]
fn a_capacity_taken_away_is_negotiated_like_overuse() {
    let (mut grid, nets) = rows(8);
    let before = route(&grid, &nets, EngineOptions::default()).unwrap();
    // Still a step of the space, so the seed is kept whole, and found to be overused.
    grid.set_capacity(4, 2, 0, 0).unwrap();
    let (after, report) = run_seeded(&grid, &nets, EngineOptions::default(), &before.routes);
    assert_eq!(report.count(Seeded::Kept), 4);
    assert!(after.is_success());
    assert!(
        !after
            .routes
            .route(NetId(1))
            .unwrap()
            .contains(grid.node_at(4, 2, 0).unwrap())
    );
    assert_eq!(
        route_change(&before.routes, &after.routes).nets_unchanged,
        3
    );
}

#[test]
fn seeds_and_history_have_to_come_before_the_first_iteration() {
    let (grid, nets) = rows(6);
    let before = route(&grid, &nets, EngineOptions::default()).unwrap();
    let mut engine = Engine::new(&grid, &nets, EngineOptions::default()).unwrap();
    engine.step();
    assert_eq!(
        engine.seed_routes(seeds(&before.routes)),
        Err(EngineError::AlreadyStarted)
    );
    assert_eq!(engine.seed_history([]), Err(EngineError::AlreadyStarted));
}

#[test]
fn a_second_seed_and_a_seed_for_a_fixed_net_are_ignored() {
    let (grid, nets) = rows(6);
    let before = route(&grid, &nets, EngineOptions::default()).unwrap();
    let route0 = before.routes.route(NetId(0)).unwrap().clone();
    let mut engine = Engine::new(&grid, &nets, EngineOptions::default()).unwrap();
    engine
        .fix_routes([FixedRoute {
            net: NetId(1),
            route: before.routes.route(NetId(1)).unwrap().clone(),
        }])
        .unwrap();
    let report = engine
        .seed_routes(seeds(&before.routes).chain([(NetId(0), route0)]))
        .unwrap();
    assert_eq!(
        report.nets,
        [
            (NetId(0), Seeded::Kept),
            (NetId(1), Seeded::Ignored),
            (NetId(2), Seeded::Kept),
            (NetId(0), Seeded::Ignored),
        ]
    );
    assert!(engine.run().is_success());
}

#[test]
fn a_net_fixed_after_it_was_seeded_gives_up_its_seed() {
    let (grid, nets) = rows(6);
    let before = route(&grid, &nets, EngineOptions::default()).unwrap();
    let mut engine = Engine::new(&grid, &nets, EngineOptions::default()).unwrap();
    engine.seed_routes(seeds(&before.routes)).unwrap();
    let fixed = FixedRoute {
        net: NetId(1),
        route: before.routes.route(NetId(1)).unwrap().clone(),
    };
    engine.fix_routes([fixed]).unwrap();
    // Held once, by the fixed route, and not twice.
    assert!(engine.overused().is_empty());
    let outcome = engine.run();
    check_honest(&grid, &nets, &outcome).unwrap();
    assert!(outcome.is_success());
}

#[test]
fn history_goes_from_one_run_to_the_next() {
    let fabric = small_fabric(12, 1);
    let (graph, nets) = (fabric_graph(fabric), fabric.nets(50, 6, 3));
    let mut engine = Engine::new(&graph, &nets, EngineOptions::default()).unwrap();
    while engine.step().is_some() {}
    let history = engine.history();
    assert!(!history.is_empty());
    assert!(history.windows(2).all(|pair| pair[0].0 < pair[1].0));

    let mut next = Engine::new(&graph, &nets, EngineOptions::default()).unwrap();
    next.seed_history(history.iter().copied()).unwrap();
    assert_eq!(next.history(), history);
    let outcome = next.run();
    check_honest(&graph, &nets, &outcome).unwrap();
    assert!(outcome.is_success());
}

/// A change to a fabric: a fifth of the nets of `nets` left out, some others with a sink less,
/// and a few removed, as `(before, after)`.
fn changed_fabric_nets(nets: &[Net<NodeId>]) -> (Vec<Net<NodeId>>, Vec<Net<NodeId>>) {
    let mut before = Vec::new();
    let mut after = Vec::new();
    for (index, net) in nets.iter().enumerate() {
        match index % 10 {
            0 | 5 => after.push(net.clone()),
            3 => before.push(net.clone()),
            7 if net.terminals.len() > 2 => {
                let mut fewer = net.clone();
                fewer.terminals.pop();
                before.push(fewer);
                after.push(net.clone());
            }
            _ => {
                before.push(net.clone());
                after.push(net.clone());
            }
        }
    }
    (before, after)
}

#[test]
fn seeded_runs_are_the_same_on_any_number_of_threads() {
    let fabric = small_fabric(16, 1);
    let (graph, nets) = (fabric_graph(fabric), fabric.nets(90, 6, 5));
    let (old, new) = changed_fabric_nets(&nets);
    let before = route(&graph, &old, EngineOptions::default()).unwrap();
    assert!(before.is_success());
    for incremental_reroute in [false, true] {
        let options = EngineOptions {
            batch_size: 8,
            incremental_reroute,
            ..EngineOptions::default()
        };
        let reference = reroute(&graph, &new, &before.routes, &[], options).unwrap();
        check_honest(&graph, &new, &reference.outcome).unwrap();
        assert!(reference.outcome.is_success());
        assert!(reference.change.nets_unchanged > 0);
        for threads in THREAD_COUNTS {
            let threaded =
                reroute_parallel(&graph, &new, &before.routes, &[], options, threads).unwrap();
            assert_eq!(threaded, reference, "on {threads} threads");
        }
    }
}

#[test]
fn a_seeded_fabric_keeps_most_routes_and_routes_far_fewer_nets() {
    let fabric = small_fabric(16, 1);
    let (graph, nets) = (fabric_graph(fabric), fabric.nets(90, 6, 5));
    let (old, new) = changed_fabric_nets(&nets);
    let before = route(&graph, &old, EngineOptions::default()).unwrap();
    let scratch = route(&graph, &new, EngineOptions::default()).unwrap();
    let eco = reroute(&graph, &new, &before.routes, &[], EngineOptions::default()).unwrap();
    assert!(scratch.is_success() && eco.outcome.is_success());
    let from_scratch = route_change(&before.routes, &scratch.routes);
    assert!(eco.change.nets_unchanged > from_scratch.nets_unchanged);
    assert!(eco.change.unchanged_fraction() > 0.7, "{:?}", eco.change);
    assert!(
        eco.change.nodes_changed_fraction() < from_scratch.nodes_changed_fraction() / 2.0,
        "{:?} against {from_scratch:?}",
        eco.change
    );
    assert!(
        eco.outcome.routes.stats.nodes_expanded < scratch.routes.stats.nodes_expanded / 2,
        "{} against {}",
        eco.outcome.routes.stats.nodes_expanded,
        scratch.routes.stats.nodes_expanded
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever changed, a seeded run is honest, agrees with the validator, and holds every
    /// seed that it reports as kept until something conflicts with it.
    #[test]
    fn seeded_runs_are_honest_after_any_change(
        (grid, nets) in any_grid_problem(),
        drop in 0usize..4,
        trim in prop::bool::ANY,
        incremental_reroute in prop::bool::ANY,
    ) {
        let options = EngineOptions { max_iterations: 30, incremental_reroute, ..EngineOptions::default() };
        let before = route(&grid, &nets, options).unwrap();
        let mut changed: Vec<Net<NodeId>> = nets.iter().skip(drop.min(nets.len())).cloned().collect();
        if trim {
            for net in &mut changed {
                if net.terminals.len() > 2 {
                    net.terminals.pop();
                }
            }
        }
        let (after, report) = run_seeded(&grid, &changed, options, &before.routes);
        prop_assert_eq!(report.nets.len(), before.routes.routes().count());
        let again = run_seeded(&grid, &changed, options, &before.routes).0;
        prop_assert_eq!(after, again);
    }
}

// The measurement.

/// A change to `nets`: of every hundred nets, `percent` are changed, a third of them left out
/// before, a third left out after, and a third with their last sink left out before, as
/// `(before, after)`.
fn changed_nets(
    nets: &[Net<NodeId>],
    percent: u32,
    seed: u64,
) -> (Vec<Net<NodeId>>, Vec<Net<NodeId>>) {
    let mut order: Vec<usize> = (0..nets.len()).collect();
    crate::rng::Rng::new(seed).shuffle(&mut order);
    let count = (nets.len() * percent as usize).div_ceil(100).max(3);
    let mut change = vec![0u8; nets.len()];
    for (k, &index) in order.iter().take(count).enumerate() {
        change[index] = 1 + (k % 3) as u8;
    }
    let mut before = Vec::new();
    let mut after = Vec::new();
    for (net, &kind) in nets.iter().zip(&change) {
        match kind {
            1 => after.push(net.clone()),
            2 => before.push(net.clone()),
            3 if net.terminals.len() > 2 => {
                let mut fewer = net.clone();
                fewer.terminals.pop();
                before.push(fewer);
                after.push(net.clone());
            }
            _ => {
                before.push(net.clone());
                after.push(net.clone());
            }
        }
    }
    (before, after)
}

/// One way of routing the changed problem, for the measurement.
#[derive(Clone, Copy)]
struct Way {
    name: &'static str,
    seeded: bool,
    history: bool,
    incremental_reroute: bool,
    /// The first present factor, if not the default.
    first_present: Option<f64>,
}

const WAYS: [Way; 6] = [
    Way {
        name: "From scratch",
        seeded: false,
        history: false,
        incremental_reroute: false,
        first_present: None,
    },
    Way {
        name: "Seeds, defaults",
        seeded: true,
        history: false,
        incremental_reroute: false,
        first_present: None,
    },
    Way {
        name: "Seeds, incremental reroute",
        seeded: true,
        history: false,
        incremental_reroute: true,
        first_present: None,
    },
    Way {
        name: "Seeds and history",
        seeded: true,
        history: true,
        incremental_reroute: false,
        first_present: None,
    },
    Way {
        name: "`stable_options`",
        seeded: true,
        history: false,
        incremental_reroute: true,
        first_present: Some(10.0),
    },
    Way {
        name: "Seeds, first present 10",
        seeded: true,
        history: false,
        incremental_reroute: false,
        first_present: Some(10.0),
    },
];

/// Sums of what each way gave, over instances.
#[derive(Clone, Copy, Default)]
struct Sums {
    solved: u32,
    unchanged: f64,
    nodes_changed: f64,
    cost: f64,
    expansions: f64,
    seconds: f64,
}

fn measure_ways<S: RoutingSpace<Node = NodeId>>(
    before_space: &S,
    after_space: &S,
    before: &[Net<NodeId>],
    after: &[Net<NodeId>],
    sums: &mut [Sums; WAYS.len()],
) {
    let mut engine = Engine::new(before_space, before, EngineOptions::default()).unwrap();
    while engine.step().is_some() {}
    let base = engine.outcome();
    let history = engine.history();
    assert!(base.is_success());
    let mut reference = None;
    for (slot, way) in WAYS.iter().enumerate() {
        let mut options = EngineOptions {
            incremental_reroute: way.incremental_reroute,
            ..EngineOptions::default()
        };
        if let Some(present) = way.first_present {
            options.first_present = Cost::new(present).unwrap();
        }
        let started = std::time::Instant::now();
        let mut engine = Engine::new(after_space, after, options).unwrap();
        if way.history {
            engine.seed_history(history.iter().copied()).unwrap();
        }
        if way.seeded {
            engine.seed_routes(seeds(&base.routes)).unwrap();
        }
        while engine.step().is_some() {}
        let outcome = engine.outcome();
        let seconds = started.elapsed().as_secs_f64();
        check_honest(after_space, after, &outcome).unwrap();
        let (cost, expansions) = (
            outcome.total_cost.get(),
            outcome.routes.stats.nodes_expanded as f64,
        );
        let (scratch_cost, scratch_expansions) = *reference.get_or_insert((cost, expansions));
        let change = route_change(&base.routes, &outcome.routes);
        let sum = &mut sums[slot];
        sum.solved += u32::from(outcome.is_success());
        sum.unchanged += change.unchanged_fraction();
        sum.nodes_changed += change.nodes_changed_fraction();
        sum.cost += cost / scratch_cost;
        sum.expansions += expansions / scratch_expansions.max(1.0);
        sum.seconds += seconds;
    }
}

fn print_ways(problem: &str, change: &str, sums: &[Sums; WAYS.len()], instances: u32) {
    let n = f64::from(instances);
    for (way, sum) in WAYS.iter().zip(sums) {
        println!(
            "| {problem} | {change} | {} | {} of {instances} | {:.1}% | {:.3} | {:.4} | {:.3} | {:.4} |",
            way.name,
            sum.solved,
            100.0 * sum.unchanged / n,
            sum.nodes_changed / n,
            sum.cost / n,
            sum.expansions / n,
            sum.seconds / n,
        );
    }
}

/// Routes generated problems after small changes, seeded and from scratch, and prints a table
/// of what changed and what it cost. Run with
///
/// ```text
/// cargo test --release --lib engine::seed_tests::measure -- --ignored --nocapture
/// ```
#[test]
#[ignore = "a measurement, which prints a table"]
fn measure() {
    println!(
        "| Problem | Change | Way | Legal | Routes unchanged | Nodes changed | Cost vs scratch | Expansions vs scratch | Time (s) |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    let seeds_of_problems = [1u64, 2, 3];
    for percent in [1u32, 5] {
        for (name, tiles, tracks, count) in [
            ("Fabric 48x48, 1 track, 700 nets", 48, 1, 700),
            ("Fabric 64x64, 2 tracks, 1200 nets", 64, 2, 1200),
        ] {
            let mut sums = [Sums::default(); WAYS.len()];
            for &seed in &seeds_of_problems {
                let fabric = small_fabric(tiles, tracks);
                let (graph, nets) = (fabric_graph(fabric), fabric.nets(count, 8, seed));
                let (before, after) = changed_nets(&nets, percent, seed);
                measure_ways(&graph, &graph, &before, &after, &mut sums);
            }
            print_ways(name, &format!("{percent}% of nets"), &sums, 3);
        }
        let mut sums = [Sums::default(); WAYS.len()];
        for &seed in &seeds_of_problems {
            let (grid, nets) = super::parallel_tests::local_grid(256, 2, 800, 16, seed);
            let (before, after) = changed_nets(&nets, percent, seed);
            measure_ways(&grid, &grid, &before, &after, &mut sums);
        }
        print_ways(
            "Grid 256x256x2, 800 nets",
            &format!("{percent}% of nets"),
            &sums,
            3,
        );
    }
    // Resources taken away: cells of the grid that routes use, blocked.
    for blocked in [10u32, 50] {
        let mut sums = [Sums::default(); WAYS.len()];
        for &seed in &seeds_of_problems {
            let (grid, nets) = super::parallel_tests::local_grid(256, 2, 800, 16, seed);
            let base = route(&grid, &nets, EngineOptions::default()).unwrap();
            let mut used: Vec<NodeId> = base
                .routes
                .routes()
                .flat_map(|(_, route)| route.nodes().collect::<Vec<_>>())
                .collect();
            used.sort_unstable();
            used.dedup();
            let terminals: FastSet<NodeId> = nets
                .iter()
                .flat_map(|net| net.terminals.iter())
                .flat_map(|terminal| terminal.access_points().iter().map(|point| point.node))
                .collect();
            used.retain(|node| !terminals.contains(node));
            crate::rng::Rng::new(seed).shuffle(&mut used);
            let mut changed = grid.clone();
            for &node in used.iter().take(blocked as usize) {
                let (x, y, layer) = grid.position(node).unwrap();
                changed.set_blocked(x, y, layer, true).unwrap();
            }
            measure_ways(&grid, &changed, &nets, &nets, &mut sums);
        }
        print_ways(
            "Grid 256x256x2, 800 nets",
            &format!("{blocked} cells blocked"),
            &sums,
            3,
        );
    }
}
