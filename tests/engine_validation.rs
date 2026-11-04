//! Runs the engine's results through the independent validator.
//!
//! The validator shares no search code with the engine, so agreement here is the evidence
//! that the engine's claims about its own results hold.

use guide::cost::Cost;
use guide::engine::{EngineOptions, Outcome, route, route_parallel};
use guide::id::{NetId, NodeId};
use guide::net::{AccessPoint, Net, Terminal};
use guide::space::RoutingSpace;
use guide::space::graph::{Graph, GraphBuilder};
use guide::space::grid::{Connectivity, Grid};
use guide::validate::{Fault, check};
use proptest::prelude::*;

/// Checks everything the engine claims about `outcome` against the validator.
fn assert_agrees<S: RoutingSpace<Node = NodeId, Resource = NodeId>>(
    space: &S,
    nets: &[Net<NodeId>],
    outcome: &Outcome<NodeId, NodeId>,
) {
    let report = check(space, nets, &outcome.routes);

    // The engine may stop with conflicts, which it reports, but anything else is a bug.
    let mut overuse = Vec::new();
    for fault in report.faults() {
        match fault {
            Fault::Overuse {
                resource,
                capacity,
                nets,
            } => overuse.push((*resource, *capacity, nets.clone())),
            other => panic!("the engine produced a faulty route: {other}"),
        }
    }

    let claimed: Vec<_> = outcome
        .overused
        .iter()
        .map(|entry| (entry.resource, entry.capacity, entry.nets.clone()))
        .collect();
    assert_eq!(
        claimed, overuse,
        "the two disagree on which resources clash"
    );

    assert_eq!(outcome.is_success(), report.is_complete());
    assert_eq!(outcome.legal_routes().is_some(), report.is_complete());
    assert_eq!(
        outcome.routes.unrouted().collect::<Vec<_>>(),
        report.unrouted().collect::<Vec<_>>(),
    );
}

/// Asserts the validator's total cost matches the engine's, for whole-number step costs
/// and no access costs, where the order of the additions cannot change the sum.
fn assert_costs_agree<S: RoutingSpace<Node = NodeId, Resource = NodeId>>(
    space: &S,
    nets: &[Net<NodeId>],
    outcome: &Outcome<NodeId, NodeId>,
) {
    let report = check(space, nets, &outcome.routes);
    assert_eq!(report.summary().total_cost, outcome.total_cost);
}

fn grid_problem(
    width: u32,
    height: u32,
    layers: u32,
    connectivity: Connectivity,
    blocked: &[(u32, u32, u32)],
    capacity: u32,
    endpoints: &[Vec<(u32, u32, u32)>],
) -> (Grid, Vec<Net<NodeId>>) {
    // Whole-number costs, in both connectivities, keep every sum exact.
    let mut grid = Grid::builder(width, height, layers)
        .connectivity(connectivity)
        .orthogonal_cost(2.0)
        .diagonal_cost(3.0)
        .via_cost(5.0)
        .build()
        .unwrap();
    for x in 0..width {
        for y in 0..height {
            for layer in 0..layers {
                grid.set_capacity(x, y, layer, capacity).unwrap();
            }
        }
    }
    for &(x, y, layer) in blocked {
        grid.set_blocked(x % width, y % height, layer % layers, true)
            .unwrap();
    }
    let nets = endpoints
        .iter()
        .enumerate()
        .map(|(id, terminals)| Net {
            id: NetId(id as u32),
            terminals: terminals
                .iter()
                .map(|&(x, y, layer)| {
                    Terminal::at(grid.node_at(x % width, y % height, layer % layers).unwrap())
                })
                .collect(),
        })
        .collect();
    (grid, nets)
}

fn graph_problem(
    nodes: u32,
    edges: &[(u32, u32, u32)],
    capacities: &[u32],
    endpoints: &[Vec<u32>],
) -> (Graph, Vec<Net<NodeId>>) {
    let mut builder = GraphBuilder::new(nodes);
    for &(from, to, cost) in edges {
        builder
            .add_edge(NodeId(from % nodes), NodeId(to % nodes), Cost::from(cost))
            .unwrap();
    }
    for (node, &capacity) in capacities.iter().enumerate() {
        builder
            .set_capacity(NodeId(node as u32 % nodes), capacity)
            .unwrap();
    }
    let nets = endpoints
        .iter()
        .enumerate()
        .map(|(id, terminals)| Net {
            id: NetId(id as u32),
            terminals: terminals
                .iter()
                .map(|&node| Terminal::at(NodeId(node % nodes)))
                .collect(),
        })
        .collect();
    (builder.build(), nets)
}

fn options() -> EngineOptions<'static> {
    let mut options = EngineOptions::default();
    // Enough to resolve most small problems, and few enough to keep the tests quick. Those
    // that do not resolve still have to report their conflicts correctly.
    options.max_iterations = 30;
    options
}

fn connectivity() -> impl Strategy<Value = Connectivity> {
    prop_oneof![
        Just(Connectivity::Orthogonal),
        Just(Connectivity::Octilinear)
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn grid_results_pass_validation(
        (width, height, layers) in (2u32..10, 2u32..10, 1u32..4),
        connectivity in connectivity(),
        blocked in proptest::collection::vec((0u32..10, 0u32..10, 0u32..4), 0..12),
        capacity in 1u32..3,
        endpoints in proptest::collection::vec(
            proptest::collection::vec((0u32..10, 0u32..10, 0u32..4), 1..5),
            0..10,
        ),
    ) {
        let (grid, nets) =
            grid_problem(width, height, layers, connectivity, &blocked, capacity, &endpoints);
        let outcome = route(&grid, &nets, options()).unwrap();
        assert_agrees(&grid, &nets, &outcome);
        assert_costs_agree(&grid, &nets, &outcome);
    }

    #[test]
    fn graph_results_pass_validation(
        nodes in 2u32..24,
        edges in proptest::collection::vec((0u32..24, 0u32..24, 0u32..9), 0..80),
        capacities in proptest::collection::vec(0u32..3, 0..24),
        endpoints in proptest::collection::vec(
            proptest::collection::vec(0u32..24, 1..5),
            0..8,
        ),
    ) {
        let (graph, nets) = graph_problem(nodes, &edges, &capacities, &endpoints);
        let outcome = route(&graph, &nets, options()).unwrap();
        assert_agrees(&graph, &nets, &outcome);
        assert_costs_agree(&graph, &nets, &outcome);
    }

    #[test]
    fn equivalent_access_points_pass_validation(
        nodes in 2u32..16,
        edges in proptest::collection::vec((0u32..16, 0u32..16, 0u32..9), 0..48),
        endpoints in proptest::collection::vec(
            proptest::collection::vec(
                proptest::collection::vec((0u32..16, 0u32..5), 1..4),
                1..4,
            ),
            0..6,
        ),
    ) {
        let (graph, _) = graph_problem(nodes, &edges, &[], &[]);
        let nets: Vec<_> = endpoints
            .iter()
            .enumerate()
            .map(|(id, terminals)| Net {
                id: NetId(id as u32),
                terminals: terminals
                    .iter()
                    .map(|points| {
                        Terminal::new(
                            points
                                .iter()
                                .map(|&(node, cost)| AccessPoint {
                                    node: NodeId(node % nodes),
                                    cost: Cost::from(cost),
                                })
                                .collect(),
                        )
                        .unwrap()
                    })
                    .collect(),
            })
            .collect();
        let outcome = route(&graph, &nets, options()).unwrap();
        // Costs may differ here: the validator charges the cheapest access point on the
        // route, and the engine the one each path connected through.
        assert_agrees(&graph, &nets, &outcome);
    }
}

/// A batch size for a random problem, with thread counts that test more than a batch can use.
fn batched(batch_size: u32) -> EngineOptions<'static> {
    let mut options = options();
    options.batch_size = batch_size;
    options
}

// Routes from threads get the same scrutiny as any others, and have to be the routes that one
// thread finds. The validator has no idea how they were found.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn parallel_grid_results_pass_validation(
        (width, height, layers) in (2u32..10, 2u32..10, 1u32..4),
        connectivity in connectivity(),
        blocked in proptest::collection::vec((0u32..10, 0u32..10, 0u32..4), 0..12),
        capacity in 1u32..3,
        endpoints in proptest::collection::vec(
            proptest::collection::vec((0u32..10, 0u32..10, 0u32..4), 1..5),
            0..10,
        ),
        batch_size in 1u32..12,
        threads in 2usize..9,
    ) {
        let (grid, nets) =
            grid_problem(width, height, layers, connectivity, &blocked, capacity, &endpoints);
        let outcome = route_parallel(&grid, &nets, batched(batch_size), threads).unwrap();
        assert_agrees(&grid, &nets, &outcome);
        assert_costs_agree(&grid, &nets, &outcome);
        prop_assert_eq!(outcome, route(&grid, &nets, batched(batch_size)).unwrap());
    }

    #[test]
    fn parallel_graph_results_pass_validation(
        nodes in 2u32..24,
        edges in proptest::collection::vec((0u32..24, 0u32..24, 0u32..9), 0..80),
        capacities in proptest::collection::vec(0u32..3, 0..24),
        endpoints in proptest::collection::vec(
            proptest::collection::vec(0u32..24, 1..5),
            0..8,
        ),
        batch_size in 1u32..10,
        threads in 2usize..9,
    ) {
        let (graph, nets) = graph_problem(nodes, &edges, &capacities, &endpoints);
        let outcome = route_parallel(&graph, &nets, batched(batch_size), threads).unwrap();
        assert_agrees(&graph, &nets, &outcome);
        assert_costs_agree(&graph, &nets, &outcome);
        prop_assert_eq!(outcome, route(&graph, &nets, batched(batch_size)).unwrap());
    }
}

#[test]
fn a_dense_board_routes_to_a_legal_result_in_batches_on_any_number_of_threads() {
    let (width, height, layers) = (16, 16, 2);
    let endpoints: Vec<_> = (0..8)
        .map(|i| vec![(2 * i, 0, 0), (2 * (7 - i), height - 1, 0)])
        .collect();
    let (grid, nets) = grid_problem(
        width,
        height,
        layers,
        Connectivity::Orthogonal,
        &[],
        1,
        &endpoints,
    );
    let mut options = EngineOptions::default();
    options.batch_size = 4;
    for threads in [1, 2, 3, 4, 8] {
        let outcome = route_parallel(&grid, &nets, options, threads).unwrap();
        assert_agrees(&grid, &nets, &outcome);
        assert_costs_agree(&grid, &nets, &outcome);
        assert!(outcome.is_success(), "on {threads} threads");
    }
}

#[test]
fn a_dense_board_routes_to_a_legal_result() {
    // Nets on every other column connect to the reversed column on the far side, so every
    // pair must cross. The free columns leave room for the doglegs that needs: with a net on
    // every column, there may be no legal answer for the engine to find.
    let (width, height, layers) = (16, 16, 2);
    let endpoints: Vec<_> = (0..8)
        .map(|i| vec![(2 * i, 0, 0), (2 * (7 - i), height - 1, 0)])
        .collect();
    let (grid, nets) = grid_problem(
        width,
        height,
        layers,
        Connectivity::Orthogonal,
        &[],
        1,
        &endpoints,
    );
    let outcome = route(&grid, &nets, EngineOptions::default()).unwrap();
    assert_agrees(&grid, &nets, &outcome);
    assert_costs_agree(&grid, &nets, &outcome);
    assert!(
        outcome.is_success(),
        "stopped with {:?}: {} resources overused",
        outcome.stop,
        outcome.overused.len(),
    );
}
