//! Checks that [`Search`] finds optimal routes on the real [`Grid`] and [`Graph`], through
//! the public API only.
//!
//! The expected answers come from brute force written here. An [`Instance`] is built from a
//! test case's own description (the stated dimensions, blocks, edges and costs) rather than
//! from `expand`, and is solved in plain `f64` and `u128` arithmetic by two oracles: every
//! simple path on very small instances, and Bellman-Ford on larger ones. Neither uses a
//! queue, a heuristic or anything else `Search` relies on, and on small instances they
//! check each other.
//!
//! Each oracle reports two different minima, which match the guarantee levels in the
//! documentation of `guide::search`:
//!
//! - The *rounded* cost: the lowest left-to-right floating-point sum over all paths.
//! - The *exact* cost: the lowest true sum over all paths, kept in fixed point, which cannot
//!   round. See [`fixed`] for why all the costs here fit.
//!
//! A case is then held to the guarantee that applies to it:
//!
//! | Case | Check | Requires |
//! | --- | --- | --- |
//! | No sum rounds | [`assert_exact`] | reported and walked cost both equal the optimum |
//! | Rounding, zero bound | [`assert_rounded_minimum`] | both equal the rounded minimum |
//! | Rounding, non-zero bound | [`assert_within_error_bound`] | rounded minimum <= walked cost <= reported cost <= documented limit |

use std::collections::BTreeMap;
use std::f64::consts::SQRT_2;
use std::ops::RangeInclusive;

use guide::cost::Cost;
use guide::id::{NetId, NodeId};
use guide::search::{Search, SearchStats, Source};
use guide::space::graph::{Graph, GraphBuilder, LowerBound};
use guide::space::grid::{Connectivity, Grid};
use guide::space::{RoutingSpace, Step};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::strategy::{BoxedStrategy, ValueTree};
use proptest::test_runner::{FileFailurePersistence, TestRunner};

const NET: NetId = NetId(0);

/// A proptest configuration for `cases` cases.
///
/// Proptest's default place for regressions works only for sources next to a `lib.rs`, so
/// without this, failures found here wouldn't be saved in `proptest-regressions` like the
/// crate's others. Tests run from the package root, which the path is relative to.
fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "proptest-regressions/search_optimality.txt",
        ))),
        ..ProptestConfig::with_cases(cases)
    }
}

// ---------------------------------------------------------------------------------------
// The independent model and its oracles
// ---------------------------------------------------------------------------------------

/// A routing problem as plain data, with nothing from the crate in it.
#[derive(Clone, Debug)]
struct Instance {
    /// For each node, the `(to, step cost)` of the moves out of it.
    out: Vec<Vec<(usize, f64)>>,
    /// The `(node, starting cost)` of each source, possibly with repeats.
    starts: Vec<(usize, f64)>,
    /// The target nodes, possibly with repeats.
    goals: Vec<usize>,
}

/// The answers brute force gives for an instance with at least one reachable target.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Optimum {
    /// The lowest left-to-right `f64` sum over all paths to a target.
    rounded: f64,
    /// The lowest true sum over all paths to a target, in [`fixed`] form.
    exact: u128,
    /// The fewest steps of any path with that true sum.
    steps: u32,
}

/// The power of two that turns every cost in these tests into a whole number.
const FIXED_SCALE: f64 = 38_685_626_227_668_133_590_597_632.0; // 2^85

/// Converts a cost to fixed point, in which sums are exact.
///
/// The smallest non-zero generated cost is `2^-53` or about `1e-9`, whose lowest set bits
/// are no lower than `2^-82`. The largest is about `2^27`, so a total stays below `2^128`
/// for paths of under `2^16` steps, far more than any instance here has. Scaling by a power
/// of two is exact, and any value that broke these assumptions would fail the assertion.
fn fixed(value: f64) -> u128 {
    let scaled = value * FIXED_SCALE;
    assert!(
        scaled.fract() == 0.0 && (0.0..3e38).contains(&scaled),
        "{value} does not fit the fixed point format"
    );
    scaled as u128
}

/// Converts from fixed point, rounding down so that the result never exceeds the input.
fn from_fixed_rounding_down(value: u128) -> f64 {
    let mut scaled = value as f64;
    if scaled as u128 > value {
        scaled = scaled.next_down();
    }
    scaled / FIXED_SCALE
}

/// The oracle for very small instances: walks every simple path.
///
/// Paths that repeat a node can be ignored. Removing a cycle lowers the number of steps, and
/// it cannot raise either kind of sum, since rounded addition is monotonic.
fn enumerate_paths(instance: &Instance) -> Option<Optimum> {
    struct Walk<'a> {
        instance: &'a Instance,
        on_path: Vec<bool>,
        rounded: Option<f64>,
        exact: Option<(u128, u32)>,
    }

    impl Walk<'_> {
        fn visit(&mut self, node: usize, rounded: f64, exact: u128, steps: u32) {
            if self.instance.goals.contains(&node) {
                self.rounded = Some(self.rounded.map_or(rounded, |best| best.min(rounded)));
                let candidate = (exact, steps);
                self.exact = Some(self.exact.map_or(candidate, |best| best.min(candidate)));
            }
            self.on_path[node] = true;
            let instance = self.instance;
            for &(to, cost) in &instance.out[node] {
                if !self.on_path[to] {
                    self.visit(to, rounded + cost, exact + fixed(cost), steps + 1);
                }
            }
            self.on_path[node] = false;
        }
    }

    let mut walk = Walk {
        instance,
        on_path: vec![false; instance.out.len()],
        rounded: None,
        exact: None,
    };
    for &(start, cost) in &instance.starts {
        walk.visit(start, cost, fixed(cost), 0);
    }
    Some(Optimum {
        rounded: walk.rounded?,
        exact: walk.exact?.0,
        steps: walk.exact?.1,
    })
}

/// The oracle for larger instances: relaxes every edge until nothing improves.
fn bellman_ford(instance: &Instance) -> Option<Optimum> {
    let nodes = instance.out.len();
    let mut rounded: Vec<Option<f64>> = vec![None; nodes];
    let mut exact: Vec<Option<(u128, u32)>> = vec![None; nodes];
    for &(start, cost) in &instance.starts {
        rounded[start] = Some(rounded[start].map_or(cost, |best| best.min(cost)));
        let candidate = (fixed(cost), 0);
        exact[start] = Some(exact[start].map_or(candidate, |best| best.min(candidate)));
    }

    loop {
        let mut changed = false;
        for from in 0..nodes {
            for &(to, cost) in &instance.out[from] {
                if let Some(sum) = rounded[from] {
                    let candidate = sum + cost;
                    if rounded[to].is_none_or(|best| candidate < best) {
                        rounded[to] = Some(candidate);
                        changed = true;
                    }
                }
                if let Some((sum, steps)) = exact[from] {
                    let candidate = (sum + fixed(cost), steps + 1);
                    if exact[to].is_none_or(|best| candidate < best) {
                        exact[to] = Some(candidate);
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    let goal_rounded = instance.goals.iter().filter_map(|&goal| rounded[goal]);
    let goal_exact = instance.goals.iter().filter_map(|&goal| exact[goal]);
    let (sum, steps) = goal_exact.min()?;
    Some(Optimum {
        rounded: goal_rounded.reduce(f64::min)?,
        exact: sum,
        steps,
    })
}

/// Runs both oracles, which have to agree, on an instance small enough for that.
fn both_oracles(instance: &Instance) -> Result<Option<Optimum>, TestCaseError> {
    let enumerated = enumerate_paths(instance);
    let relaxed = bellman_ford(instance);
    prop_assert_eq!(
        enumerated,
        relaxed,
        "the oracles disagree on {:?}",
        instance
    );
    Ok(relaxed)
}

/// The true cost from every node to `goal`, in fixed point, by relaxing edges.
fn exact_distances_to(instance: &Instance, goal: usize) -> Vec<Option<u128>> {
    let mut distance = vec![None; instance.out.len()];
    distance[goal] = Some(0);
    loop {
        let mut changed = false;
        for from in 0..instance.out.len() {
            for &(to, cost) in &instance.out[from] {
                if let Some(rest) = distance[to] {
                    let candidate: u128 = rest + fixed(cost);
                    if distance[from].is_none_or(|best| candidate < best) {
                        distance[from] = Some(candidate);
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            return distance;
        }
    }
}

/// The documented limit on a reported cost, `S * (1 + 2^-53)^(k + 1)`, in fixed point.
///
/// The computation rounds up at each stage, so the limit is never tighter than documented,
/// and it is looser only by one unit of `2^-85` per step.
fn documented_limit(optimum: &Optimum) -> u128 {
    let mut limit = optimum.exact;
    for _ in 0..=optimum.steps {
        limit += limit.div_ceil(1 << 53);
    }
    limit
}

// ---------------------------------------------------------------------------------------
// Running the real search and judging its answers
// ---------------------------------------------------------------------------------------

/// A result of the real search, copied out of its buffers.
#[derive(Clone, Debug, PartialEq)]
struct Outcome {
    path: Vec<NodeId>,
    cost: f64,
}

fn node(index: usize) -> NodeId {
    NodeId(u32::try_from(index).unwrap())
}

fn cost(value: f64) -> Cost {
    Cost::new(value).unwrap()
}

/// Runs a query on an existing `Search`.
fn run_with<S: RoutingSpace<Node = NodeId>>(
    search: &mut Search<S>,
    space: &S,
    instance: &Instance,
) -> (Option<Outcome>, SearchStats) {
    let sources = instance
        .starts
        .iter()
        .map(|&(start, value)| Source::with_cost(node(start), cost(value)));
    let targets = instance.goals.iter().map(|&goal| node(goal));
    let outcome = search
        .find_path(space, NET, sources, targets)
        .map(|found| Outcome {
            path: found.path.to_vec(),
            cost: found.cost.get(),
        });
    (outcome, search.stats())
}

fn run<S: RoutingSpace<Node = NodeId>>(space: &S, instance: &Instance) -> Option<Outcome> {
    run_with(&mut Search::new(space), space, instance).0
}

/// Checks that `path` goes from a source to a target by steps that `expand` offers, and
/// returns what it costs as a left-to-right sum of the costs `expand` gives.
fn walk<S: RoutingSpace<Node = NodeId>>(
    space: &S,
    instance: &Instance,
    path: &[NodeId],
) -> Result<f64, TestCaseError> {
    let (Some(first), Some(last)) = (path.first(), path.last()) else {
        return Err(TestCaseError::fail("the path is empty"));
    };
    prop_assert!(
        instance.goals.contains(&last.index()),
        "{:?} does not end at a target",
        path
    );
    let start = instance
        .starts
        .iter()
        .filter(|&&(start, _)| start == first.index())
        .map(|&(_, value)| value)
        .reduce(f64::min);
    let Some(mut sum) = start else {
        return Err(TestCaseError::fail(format!(
            "{path:?} does not begin at a source"
        )));
    };

    let mut steps: Vec<Step<NodeId>> = Vec::new();
    for pair in path.windows(2) {
        steps.clear();
        space.expand(NET, pair[0], &mut steps);
        let mut matching = steps.iter().filter(|step| step.to == pair[1]);
        let Some(step) = matching.next() else {
            return Err(TestCaseError::fail(format!(
                "{path:?} takes a step from {:?} to {:?} that expand does not offer",
                pair[0], pair[1]
            )));
        };
        prop_assert!(matching.next().is_none(), "expand repeated a step");
        sum += step.cost.get();
    }
    Ok(sum)
}

/// Requires what applies when no sum rounds: exactly the optimal cost.
fn assert_exact<S: RoutingSpace<Node = NodeId>>(
    space: &S,
    instance: &Instance,
    expected: Option<Optimum>,
) -> Result<(), TestCaseError> {
    match (run(space, instance), expected) {
        (None, None) => {}
        (Some(found), Some(optimum)) => {
            // If this fails, the test case has broken the promise that nothing rounds.
            prop_assert_eq!(fixed(optimum.rounded), optimum.exact);
            prop_assert_eq!(fixed(found.cost), optimum.exact);
            prop_assert_eq!(walk(space, instance, &found.path)?, found.cost);
        }
        (found, optimum) => {
            return Err(TestCaseError::fail(format!(
                "search found {found:?}, brute force found {optimum:?}"
            )));
        }
    }
    Ok(())
}

/// Requires what applies with a zero lower bound: the minimum of the left-to-right sums, to
/// the last bit, even though sums round.
fn assert_rounded_minimum<S: RoutingSpace<Node = NodeId>>(
    space: &S,
    instance: &Instance,
    expected: Option<Optimum>,
) -> Result<(), TestCaseError> {
    match (run(space, instance), expected) {
        (None, None) => {}
        (Some(found), Some(optimum)) => {
            prop_assert_eq!(found.cost.to_bits(), optimum.rounded.to_bits());
            let walked = walk(space, instance, &found.path)?;
            prop_assert_eq!(walked.to_bits(), found.cost.to_bits());
        }
        (found, optimum) => {
            return Err(TestCaseError::fail(format!(
                "search found {found:?}, brute force found {optimum:?}"
            )));
        }
    }
    Ok(())
}

/// Requires what applies with rounding and a non-zero lower bound: a reported cost within the
/// documented limit, and a path that costs no more than that and no less than any path can.
fn assert_within_error_bound<S: RoutingSpace<Node = NodeId>>(
    space: &S,
    instance: &Instance,
    expected: Option<Optimum>,
) -> Result<(), TestCaseError> {
    match (run(space, instance), expected) {
        (None, None) => {}
        (Some(found), Some(optimum)) => {
            let walked = walk(space, instance, &found.path)?;
            prop_assert!(
                optimum.rounded <= walked,
                "the path costs {} but brute force found no path below {}, and this \
                 is impossible unless a path was missed",
                walked,
                optimum.rounded
            );
            prop_assert!(
                walked <= found.cost,
                "the path costs {} but only {} was reported",
                walked,
                found.cost
            );
            prop_assert!(
                fixed(found.cost) <= documented_limit(&optimum),
                "reported {} is above the documented limit for {:?}",
                found.cost,
                optimum
            );
        }
        (found, optimum) => {
            return Err(TestCaseError::fail(format!(
                "search found {found:?}, brute force found {optimum:?}"
            )));
        }
    }
    Ok(())
}

/// Checks that the model's moves are the ones `expand` offers, so that the oracles are
/// solving the problem the search is given.
fn assert_model_matches_expand<S: RoutingSpace<Node = NodeId>>(
    space: &S,
    instance: &Instance,
) -> Result<(), TestCaseError> {
    let mut steps = Vec::new();
    for (from, moves) in instance.out.iter().enumerate() {
        let mut expected: BTreeMap<usize, f64> = BTreeMap::new();
        for &(to, value) in moves {
            let entry = expected.entry(to).or_insert(value);
            *entry = entry.min(value);
        }

        steps.clear();
        space.expand(NET, node(from), &mut steps);
        let offered: BTreeMap<usize, f64> = steps
            .iter()
            .map(|step| (step.to.index(), step.cost.get()))
            .collect();
        prop_assert_eq!(offered.len(), steps.len(), "expand repeated a target");
        prop_assert!(!offered.contains_key(&from), "expand offered a self-loop");
        prop_assert_eq!(&offered, &expected, "moves out of node {}", from);
    }
    Ok(())
}

/// Checks a space's lower bound against the true distance to each target, which is what the
/// documented error limit requires of it. This is stricter than comparing with a rounded
/// sum, which can come out below the true one.
fn assert_bound_is_admissible<S: RoutingSpace<Node = NodeId>>(
    space: &S,
    instance: &Instance,
) -> Result<(), TestCaseError> {
    for &goal in &instance.goals {
        let distances = exact_distances_to(instance, goal);
        for (from, distance) in distances.iter().enumerate() {
            if let Some(distance) = *distance {
                let bound = space.lower_bound(NET, node(from), node(goal)).get();
                // A bound can have bits below the fixed point format, but comparing with a
                // whole number only takes its value rounded up to a whole number.
                let scaled = (bound * FIXED_SCALE).ceil();
                prop_assert!(
                    scaled <= distance as f64 && scaled as u128 <= distance,
                    "the bound from {} to {} is {}, above the true distance",
                    from,
                    goal,
                    bound
                );
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Test case generation
// ---------------------------------------------------------------------------------------

/// Whole-number costs, including some large enough to dominate a sum, whose totals are
/// nevertheless far below `2^53` so that no sum rounds.
fn integer_cost() -> BoxedStrategy<f64> {
    prop_oneof![
        9 => (0u32..=9).prop_map(f64::from),
        1 => Just(f64::from(1u32 << 26)),
    ]
    .boxed()
}

/// Costs chosen to round when added, including across widely different magnitudes.
///
/// The half-unit-in-the-last-place pair vanishes when added to 1 a step at a time but not
/// when summed first, which is exactly the situation the documentation warns about.
fn fractional_cost() -> BoxedStrategy<f64> {
    prop_oneof![
        Just(0.0),
        Just(0.1),
        Just(0.2),
        Just(0.3),
        Just(0.7),
        Just(1.0 / 3.0),
        Just(SQRT_2),
        Just(1.0),
        Just(1.0 + f64::EPSILON),
        Just(f64::EPSILON / 2.0),
        Just(1e-9),
        Just(1e8 + 0.1),
        0.01..2.0f64,
        0.001..1000.0f64,
    ]
    .boxed()
}

/// A list of sources with starting costs, and a list of targets, over `nodes` nodes.
#[allow(clippy::type_complexity)]
fn terminals(
    nodes: usize,
    counts: RangeInclusive<usize>,
    start_cost: BoxedStrategy<f64>,
) -> impl Strategy<Value = (Vec<(usize, f64)>, Vec<usize>)> {
    (
        vec((0..nodes, start_cost), counts.clone()),
        vec(0..nodes, counts),
    )
}

/// A blocked multi-layer grid with its terminals, described independently of [`Grid`].
#[derive(Clone, Debug)]
struct GridCase {
    width: u32,
    height: u32,
    layers: u32,
    connectivity: Connectivity,
    orthogonal: f64,
    diagonal: f64,
    via: f64,
    blocked: Vec<bool>,
    starts: Vec<(usize, f64)>,
    goals: Vec<usize>,
}

impl GridCase {
    /// Numbers a cell as [`Grid`] documents.
    fn cell(&self, x: u32, y: u32, layer: u32) -> usize {
        (x + self.width * y + self.width * self.height * layer) as usize
    }

    fn space(&self) -> Grid {
        let mut grid = Grid::builder(self.width, self.height, self.layers)
            .connectivity(self.connectivity)
            .orthogonal_cost(self.orthogonal)
            .diagonal_cost(self.diagonal)
            .via_cost(self.via)
            .build()
            .unwrap();
        for layer in 0..self.layers {
            for y in 0..self.height {
                for x in 0..self.width {
                    if self.blocked[self.cell(x, y, layer)] {
                        grid.set_blocked(x, y, layer, true).unwrap();
                    }
                }
            }
        }
        grid
    }

    /// States the moves as the documentation of [`Grid`] describes them: one step along an
    /// axis, one diagonal if allowed, or one layer, and never into or out of a blocked cell.
    fn instance(&self) -> Instance {
        let mut out = vec![Vec::new(); self.blocked.len()];
        let delta = [-1i64, 0, 1];
        for layer in 0..self.layers {
            for y in 0..self.height {
                for x in 0..self.width {
                    let from = self.cell(x, y, layer);
                    if self.blocked[from] {
                        continue;
                    }
                    for dl in delta {
                        for dy in delta {
                            for dx in delta {
                                let moved = (dx.abs(), dy.abs(), dl.abs());
                                let value = match moved {
                                    (1, 0, 0) | (0, 1, 0) => self.orthogonal,
                                    (1, 1, 0) if self.connectivity == Connectivity::Octilinear => {
                                        self.diagonal
                                    }
                                    (0, 0, 1) => self.via,
                                    _ => continue,
                                };
                                let (tx, ty, tl) =
                                    (i64::from(x) + dx, i64::from(y) + dy, i64::from(layer) + dl);
                                if !(0..i64::from(self.width)).contains(&tx)
                                    || !(0..i64::from(self.height)).contains(&ty)
                                    || !(0..i64::from(self.layers)).contains(&tl)
                                {
                                    continue;
                                }
                                let to = self.cell(tx as u32, ty as u32, tl as u32);
                                if !self.blocked[to] {
                                    out[from].push((to, value));
                                }
                            }
                        }
                    }
                }
            }
        }
        Instance {
            out,
            starts: self.starts.clone(),
            goals: self.goals.clone(),
        }
    }
}

fn grid_case(
    width: RangeInclusive<u32>,
    height: RangeInclusive<u32>,
    layers: RangeInclusive<u32>,
    max_cells: u32,
    costs: BoxedStrategy<f64>,
    counts: RangeInclusive<usize>,
) -> impl Strategy<Value = GridCase> {
    // Dropping to one layer rather than filtering, as proptest limits how many cases it
    // will reject. Callers choose widths and heights that fit `max_cells` by themselves.
    let dimensions = (width, height, layers).prop_map(move |(w, h, l)| {
        let l = if w * h * l <= max_cells { l } else { 1 };
        (w, h, l)
    });
    let connectivity = prop_oneof![
        Just(Connectivity::Orthogonal),
        Just(Connectivity::Octilinear)
    ];
    let density = prop_oneof![1 => Just(0u32), 3 => 5u32..45];
    (
        dimensions,
        connectivity,
        (costs.clone(), costs.clone(), costs.clone()),
        density,
    )
        .prop_flat_map(
            move |((width, height, layers), connectivity, costs_used, density)| {
                let cells = (width * height * layers) as usize;
                (
                    vec(0u32..100, cells),
                    terminals(cells, counts.clone(), costs.clone()),
                )
                    .prop_map(move |(rolls, (starts, goals))| GridCase {
                        width,
                        height,
                        layers,
                        connectivity,
                        orthogonal: costs_used.0,
                        diagonal: costs_used.1,
                        via: costs_used.2,
                        blocked: rolls.iter().map(|&roll| roll < density).collect(),
                        starts,
                        goals,
                    })
            },
        )
}

/// A grid with few enough simple paths to walk them all.
fn tiny_grid(costs: BoxedStrategy<f64>) -> impl Strategy<Value = GridCase> {
    grid_case(1..=4, 1..=3, 1..=2, 12, costs, 1..=3)
}

fn larger_grid(costs: BoxedStrategy<f64>) -> impl Strategy<Value = GridCase> {
    grid_case(2..=16, 2..=16, 1..=3, 16 * 16 * 3, costs, 1..=3)
}

/// A graph with base costs and its terminals, described independently of [`Graph`].
#[derive(Clone, Debug)]
struct GraphCase {
    /// `(from, to, edge cost)`, which may include self-loops and parallel edges.
    edges: Vec<(usize, usize, f64)>,
    /// One base cost per node.
    bases: Vec<f64>,
    starts: Vec<(usize, f64)>,
    goals: Vec<usize>,
    /// One per node, as a fraction of 255, to scale that node's estimates. Differing from
    /// node to node, these make an estimator inconsistent, and 255 makes it perfect.
    fractions: Vec<u8>,
}

impl GraphCase {
    fn space(&self) -> Graph {
        let mut builder = GraphBuilder::new(u32::try_from(self.bases.len()).unwrap());
        for &(from, to, value) in &self.edges {
            builder.add_edge(node(from), node(to), cost(value)).unwrap();
        }
        for (index, &base) in self.bases.iter().enumerate() {
            builder.set_base_cost(node(index), cost(base)).unwrap();
        }
        builder.build()
    }

    /// States the moves as the documentation of [`Graph`] describes them: a step pays for
    /// its edge and for the node it arrives at, and self-loops are dropped.
    fn instance(&self) -> Instance {
        let mut out = vec![Vec::new(); self.bases.len()];
        for &(from, to, value) in &self.edges {
            if from != to {
                out[from].push((to, value + self.bases[to]));
            }
        }
        Instance {
            out,
            starts: self.starts.clone(),
            goals: self.goals.clone(),
        }
    }
}

fn graph_case(
    nodes: RangeInclusive<usize>,
    edges_per_node: usize,
    costs: BoxedStrategy<f64>,
    counts: RangeInclusive<usize>,
) -> impl Strategy<Value = GraphCase> {
    nodes.prop_flat_map(move |nodes| {
        let base = prop_oneof![2 => Just(0.0), 1 => costs.clone()];
        // Perfect estimates next to useless ones are what make a search expand a node at
        // too high a cost and have to re-open it later.
        let fraction = prop_oneof![Just(255u8), Just(0u8), any::<u8>()];
        (
            vec(
                (0..nodes, 0..nodes, costs.clone()),
                0..=nodes * edges_per_node,
            ),
            vec(base, nodes),
            terminals(nodes, counts.clone(), costs.clone()),
            vec(fraction, nodes),
        )
            .prop_map(|(edges, bases, (starts, goals), fractions)| GraphCase {
                edges,
                bases,
                starts,
                goals,
                fractions,
            })
    })
}

/// A graph with few enough simple paths to walk them all.
fn tiny_graph(costs: BoxedStrategy<f64>) -> impl Strategy<Value = GraphCase> {
    graph_case(1..=7, 3, costs, 1..=3)
}

fn larger_graph(costs: BoxedStrategy<f64>) -> impl Strategy<Value = GraphCase> {
    graph_case(2..=64, 4, costs, 1..=3)
}

/// Builds an estimator from the true distances, scaled by the case's fractions.
///
/// Its table is indexed by target and then by node, with entries for targets only, which
/// are the only nodes a search may ask about in that position.
///
/// `scale` receives a true distance and a fraction, and must not return more than that
/// distance. Pairs with no path at all get `unreachable`, as any bound is admissible for them.
fn distance_table<F: Fn(u128, u8) -> f64>(
    case: &GraphCase,
    unreachable: f64,
    scale: F,
) -> impl LowerBound + use<F> {
    let instance = case.instance();
    let mut table: Vec<Vec<Cost>> = vec![Vec::new(); case.bases.len()];
    for &goal in &case.goals {
        table[goal] = exact_distances_to(&instance, goal)
            .iter()
            .zip(&case.fractions)
            .map(|(distance, &fraction)| {
                cost(distance.map_or(unreachable, |distance| scale(distance, fraction)))
            })
            .collect();
    }
    move |from: NodeId, to: NodeId| table[to.index()][from.index()]
}

/// An estimator that keeps every sum whole, so that nothing rounds.
fn integer_estimator(case: &GraphCase) -> impl LowerBound + use<> {
    distance_table(case, f64::from(1u32 << 30), |distance, fraction| {
        let whole = distance >> 85;
        (whole * u128::from(fraction) / 255) as f64
    })
}

/// An estimator that may have any `f64` value at or below the true distance, including the
/// true distance itself whenever that is representable.
fn fractional_estimator(case: &GraphCase) -> impl LowerBound + use<> {
    distance_table(case, f64::MAX, |distance, fraction| {
        from_fixed_rounding_down(distance / 255 * u128::from(fraction))
    })
}

/// Nodes on a plane, where each edge costs at least the distance it covers. This is how
/// real routing graphs get their estimates, and it gives a named, stateful estimator.
#[derive(Clone, Debug)]
struct Plane {
    tiles: Vec<(u32, u32)>,
    /// The cost per unit of distance that edges pay at least.
    unit: f64,
    /// What the estimate is scaled by, to leave a margin against its own rounding.
    margin: f64,
}

impl Plane {
    fn distance(&self, from: usize, to: usize) -> u32 {
        let (a, b) = (self.tiles[from], self.tiles[to]);
        a.0.abs_diff(b.0) + a.1.abs_diff(b.1)
    }
}

impl LowerBound for Plane {
    fn estimate(&self, from: NodeId, to: NodeId) -> Cost {
        let distance = f64::from(self.distance(from.index(), to.index()));
        cost(distance * self.unit * self.margin)
    }
}

/// A random plane and a graph over it, whose edge costs are the distance they cover plus
/// whatever `extra` they were generated with.
fn plane_case(
    units: BoxedStrategy<f64>,
    margin: f64,
    costs: BoxedStrategy<f64>,
) -> impl Strategy<Value = (Plane, GraphCase)> {
    (graph_case(2..=48, 4, costs, 1..=3), units).prop_flat_map(move |(case, unit)| {
        vec((0u32..12, 0u32..12), case.bases.len()).prop_map(move |tiles| {
            let plane = Plane {
                tiles,
                unit,
                margin,
            };
            let mut case = case.clone();
            for edge in &mut case.edges {
                edge.2 += f64::from(plane.distance(edge.0, edge.1)) * unit;
            }
            (plane, case)
        })
    })
}

// ---------------------------------------------------------------------------------------
// Grids
// ---------------------------------------------------------------------------------------

proptest! {
    #![proptest_config(config(1024))]

    #[test]
    fn tiny_grids_with_integer_costs_match_every_path(case in tiny_grid(integer_cost())) {
        let instance = case.instance();
        assert_exact(&case.space(), &instance, both_oracles(&instance)?)?;
    }

    #[test]
    fn tiny_grids_with_fractional_costs_match_every_path(case in tiny_grid(fractional_cost())) {
        let instance = case.instance();
        assert_within_error_bound(&case.space(), &instance, both_oracles(&instance)?)?;
    }

    #[test]
    fn grid_moves_are_as_documented(case in tiny_grid(fractional_cost())) {
        assert_model_matches_expand(&case.space(), &case.instance())?;
    }

    #[test]
    fn grid_bounds_never_exceed_the_true_distance(case in tiny_grid(fractional_cost())) {
        assert_bound_is_admissible(&case.space(), &case.instance())?;
    }
}

proptest! {
    #![proptest_config(config(384))]

    #[test]
    fn larger_grids_with_integer_costs_match_bellman_ford(case in larger_grid(integer_cost())) {
        let instance = case.instance();
        assert_exact(&case.space(), &instance, bellman_ford(&instance))?;
    }

    #[test]
    fn larger_grids_with_fractional_costs_match_bellman_ford(
        case in larger_grid(fractional_cost()),
    ) {
        let instance = case.instance();
        assert_within_error_bound(&case.space(), &instance, bellman_ford(&instance))?;
    }
}

// ---------------------------------------------------------------------------------------
// Graphs
// ---------------------------------------------------------------------------------------

proptest! {
    #![proptest_config(config(1024))]

    #[test]
    fn tiny_graphs_with_integer_costs_match_every_path(case in tiny_graph(integer_cost())) {
        let instance = case.instance();
        let expected = both_oracles(&instance)?;
        assert_exact(&case.space(), &instance, expected)?;
        let guided = case.space().with_lower_bound(integer_estimator(&case));
        assert_exact(&guided, &instance, expected)?;
    }

    /// With no estimator, the search owes the rounded minimum to the last bit.
    #[test]
    fn tiny_graphs_with_fractional_costs_match_every_path(case in tiny_graph(fractional_cost())) {
        let instance = case.instance();
        let expected = both_oracles(&instance)?;
        assert_rounded_minimum(&case.space(), &instance, expected)?;
        let guided = case.space().with_lower_bound(fractional_estimator(&case));
        assert_within_error_bound(&guided, &instance, expected)?;
    }

    #[test]
    fn graph_moves_are_as_documented(case in tiny_graph(fractional_cost())) {
        assert_model_matches_expand(&case.space(), &case.instance())?;
    }
}

proptest! {
    #![proptest_config(config(384))]

    #[test]
    fn larger_graphs_with_integer_costs_match_bellman_ford(case in larger_graph(integer_cost())) {
        let instance = case.instance();
        let expected = bellman_ford(&instance);
        assert_exact(&case.space(), &instance, expected)?;
        let guided = case.space().with_lower_bound(integer_estimator(&case));
        assert_exact(&guided, &instance, expected)?;
    }

    #[test]
    fn larger_graphs_with_fractional_costs_match_bellman_ford(
        case in larger_graph(fractional_cost()),
    ) {
        let instance = case.instance();
        let expected = bellman_ford(&instance);
        assert_rounded_minimum(&case.space(), &instance, expected)?;
        let guided = case.space().with_lower_bound(fractional_estimator(&case));
        assert_within_error_bound(&guided, &instance, expected)?;
    }

    #[test]
    fn a_distance_estimator_stays_exact_with_integer_costs(
        (plane, case) in plane_case((1u32..=3).prop_map(f64::from).boxed(), 1.0, integer_cost()),
    ) {
        let instance = case.instance();
        let guided = case.space().with_lower_bound(plane);
        assert_exact(&guided, &instance, bellman_ford(&instance))?;
    }

    /// The margin makes the estimate an underestimate despite its own rounding, so that the
    /// documented limit applies without the extra error it allows for such bounds.
    #[test]
    fn a_distance_estimator_stays_in_bounds_with_fractional_costs(
        (plane, case) in plane_case(
            prop_oneof![Just(0.1), Just(0.25), Just(1.0 / 3.0), Just(SQRT_2)].boxed(),
            1.0 - 1e-12,
            fractional_cost(),
        ),
    ) {
        let instance = case.instance();
        let guided = case.space().with_lower_bound(plane);
        assert_within_error_bound(&guided, &instance, bellman_ford(&instance))?;
    }
}

// ---------------------------------------------------------------------------------------
// Several sources and targets
// ---------------------------------------------------------------------------------------

/// Checks that a search with several terminals finds the best of the searches for each pair
/// of them, which is a statement that needs no oracle at all.
fn assert_best_of_each_pair<S: RoutingSpace<Node = NodeId>>(
    space: &S,
    instance: &Instance,
) -> Result<(), TestCaseError> {
    let mut best: Option<f64> = None;
    for &start in &instance.starts {
        for &goal in &instance.goals {
            let single = Instance {
                out: Vec::new(),
                starts: vec![start],
                goals: vec![goal],
            };
            if let Some(found) = run(space, &single) {
                best = Some(best.map_or(found.cost, |known| known.min(found.cost)));
            }
        }
    }
    let found = run(space, instance).map(|found| found.cost);
    prop_assert_eq!(found, best);
    Ok(())
}

proptest! {
    #![proptest_config(config(384))]

    #[test]
    fn grids_with_several_terminals_match_bellman_ford(
        case in grid_case(2..=12, 2..=12, 1..=2, 288, integer_cost(), 2..=5),
    ) {
        let instance = case.instance();
        let space = case.space();
        assert_exact(&space, &instance, bellman_ford(&instance))?;
        assert_best_of_each_pair(&space, &instance)?;
    }

    #[test]
    fn graphs_with_several_terminals_match_bellman_ford(
        case in graph_case(2..=48, 4, integer_cost(), 2..=5),
    ) {
        let instance = case.instance();
        let guided = case.space().with_lower_bound(integer_estimator(&case));
        assert_exact(&guided, &instance, bellman_ford(&instance))?;
        assert_best_of_each_pair(&guided, &instance)?;
    }

    #[test]
    fn tiny_graphs_with_fractional_terminals_match_every_path(
        case in graph_case(2..=7, 3, fractional_cost(), 2..=5),
    ) {
        let instance = case.instance();
        assert_rounded_minimum(&case.space(), &instance, both_oracles(&instance)?)?;
    }
}

// ---------------------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------------------

/// Checks that one `Search` reused for all of `queries`, in order and then in reverse,
/// gives the paths, costs and statistics that a new `Search` gives for each.
fn assert_reuse_matches_fresh<S: RoutingSpace<Node = NodeId>>(
    space: &S,
    queries: &[Instance],
) -> Result<(), TestCaseError> {
    let mut reused = Search::new(space);
    for query in queries.iter().chain(queries.iter().rev()) {
        let from_reused = run_with(&mut reused, space, query);
        let from_fresh = run_with(&mut Search::new(space), space, query);
        prop_assert_eq!(from_reused, from_fresh);
    }
    Ok(())
}

/// Extra terminal lists for a space of `nodes` nodes, given as instances with no moves,
/// which is all the search needs to know of them.
fn queries(nodes: usize, costs: BoxedStrategy<f64>) -> impl Strategy<Value = Vec<Instance>> {
    vec(terminals(nodes, 1..=3, costs), 2..=6).prop_map(|lists| {
        lists
            .into_iter()
            .map(|(starts, goals)| Instance {
                out: Vec::new(),
                starts,
                goals,
            })
            .collect()
    })
}

proptest! {
    #![proptest_config(config(192))]

    #[test]
    fn reused_searches_match_fresh_ones_on_grids(
        (case, queries) in larger_grid(fractional_cost()).prop_flat_map(|case| {
            let cells = case.blocked.len();
            (Just(case), queries(cells, fractional_cost()))
        }),
    ) {
        assert_reuse_matches_fresh(&case.space(), &queries)?;
    }

    #[test]
    fn reused_searches_match_fresh_ones_on_graphs(
        (case, queries) in larger_graph(fractional_cost()).prop_flat_map(|case| {
            let nodes = case.bases.len();
            (Just(case), queries(nodes, fractional_cost()))
        }),
    ) {
        assert_reuse_matches_fresh(&case.space(), &queries)?;
    }

    /// An estimator table only covers one set of targets, so a plane stands in for it here.
    #[test]
    fn reused_searches_match_fresh_ones_with_an_estimator(
        ((plane, case), queries) in plane_case(
            Just(0.25).boxed(),
            1.0,
            fractional_cost(),
        ).prop_flat_map(|pair| {
            let nodes = pair.1.bases.len();
            (Just(pair), queries(nodes, fractional_cost()))
        }),
    ) {
        assert_reuse_matches_fresh(&case.space().with_lower_bound(plane), &queries)?;
    }
}

// ---------------------------------------------------------------------------------------
// Hand-checked cases, and checks on the tests themselves
// ---------------------------------------------------------------------------------------

/// The oracles are what everything else here is judged by, so they get a hand-checked case.
#[test]
fn oracles_agree_with_a_hand_checked_instance() {
    let half_ulp = f64::EPSILON / 2.0;
    let instance = Instance {
        //  0 --1--> 1 --half--> 2 --half--> 3, and 0 --(1 + ulp)--> 3
        out: vec![
            vec![(1, 1.0), (3, 1.0 + f64::EPSILON)],
            vec![(2, half_ulp)],
            vec![(3, half_ulp)],
            Vec::new(),
        ],
        starts: vec![(0, 0.0)],
        goals: vec![3],
    };
    let expected = Optimum {
        // Each half vanishes when added to 1.
        rounded: 1.0,
        exact: fixed(1.0 + f64::EPSILON),
        // The direct edge ties with the three steps on the true sum, and has fewer.
        steps: 1,
    };
    assert_eq!(enumerate_paths(&instance), Some(expected));
    assert_eq!(bellman_ford(&instance), Some(expected));

    let limit = documented_limit(&expected);
    assert!(limit > fixed(1.0 + f64::EPSILON));
    assert!(limit < fixed(1.0 + 3.0 * f64::EPSILON));
}

/// The documentation's own example of the limits: A* may take the direct edge, Dijkstra may
/// not, and both have to be within what is promised of them.
#[test]
fn rounding_of_half_units_in_the_last_place_is_handled_as_documented() {
    let half_ulp = f64::EPSILON / 2.0;
    let case = GraphCase {
        edges: vec![
            (0, 1, 1.0),
            (1, 2, half_ulp),
            (2, 3, half_ulp),
            (0, 3, 1.0 + f64::EPSILON),
        ],
        bases: vec![0.0; 4],
        starts: vec![(0, 0.0)],
        goals: vec![3],
        fractions: vec![255; 4],
    };
    let instance = case.instance();
    let expected = bellman_ford(&instance);

    assert_rounded_minimum(&case.space(), &instance, expected).unwrap();
    let dijkstra = run(&case.space(), &instance).unwrap();
    assert_eq!(dijkstra.cost, 1.0);
    assert_eq!(dijkstra.path.len(), 4);

    let guided = case.space().with_lower_bound(fractional_estimator(&case));
    assert_within_error_bound(&guided, &instance, expected).unwrap();
    let a_star = run(&guided, &instance).unwrap();
    assert_eq!(a_star.cost, 1.0 + f64::EPSILON);
}

/// An admissible but inconsistent bound: from 1 it says 4, and from 2, one step further on,
/// it says 0. So 2 is expanded at 3 by the direct edge, before the route through 1 finds it
/// at 2, and has to be expanded again for the answer to be 5 and not 6.
#[test]
fn a_node_expanded_too_expensively_is_re_opened() {
    let case = GraphCase {
        edges: vec![(0, 2, 3.0), (0, 1, 1.0), (1, 2, 1.0), (2, 3, 3.0)],
        bases: vec![0.0; 4],
        starts: vec![(0, 0.0)],
        goals: vec![3],
        //          0    1    2  3
        fractions: vec![0, 255, 0, 0],
    };
    let instance = case.instance();
    let guided = case.space().with_lower_bound(integer_estimator(&case));
    let found = run(&guided, &instance).unwrap();
    assert_eq!(found.path, [NodeId(0), NodeId(1), NodeId(2), NodeId(3)]);
    assert_eq!(found.cost, 5.0);
}

#[test]
fn empty_terminal_lists_find_nothing() {
    let space = Grid::new(2, 2, 1).unwrap();
    let mut search = Search::new(&space);
    let none: [Source<NodeId>; 0] = [];
    let some = [Source::new(NodeId(0))];
    assert_eq!(search.find_path(&space, NET, none, [NodeId(1)]), None);
    assert_eq!(search.find_path(&space, NET, some, []), None);
}

#[test]
fn blocked_cells_can_only_be_a_path_of_one_node() {
    let case = GridCase {
        width: 3,
        height: 1,
        layers: 1,
        connectivity: Connectivity::Orthogonal,
        orthogonal: 1.0,
        diagonal: SQRT_2,
        via: 1.0,
        blocked: vec![false, true, false],
        starts: vec![(1, 2.0)],
        goals: vec![1],
    };
    let space = case.space();

    let found = run(&space, &case.instance()).unwrap();
    assert_eq!(found.path, [NodeId(1)]);
    assert_eq!(found.cost, 2.0);

    // There is no way out of a blocked cell, or through or into one.
    for (start, goal) in [(1, 0), (0, 1), (0, 2)] {
        let instance = Instance {
            out: Vec::new(),
            starts: vec![(start, 0.0)],
            goals: vec![goal],
        };
        assert_eq!(run(&space, &instance), None, "{start} to {goal}");
    }
}

#[test]
fn a_graph_path_does_not_pay_the_base_cost_of_its_first_node() {
    let case = GraphCase {
        edges: vec![(0, 1, 1.0), (1, 2, 1.0)],
        bases: vec![100.0, 10.0, 20.0],
        starts: vec![(0, 3.0)],
        goals: vec![2],
        fractions: vec![255; 3],
    };
    let found = run(&case.space(), &case.instance()).unwrap();
    assert_eq!(found.path, [NodeId(0), NodeId(1), NodeId(2)]);
    assert_eq!(found.cost, 3.0 + 1.0 + 10.0 + 1.0 + 20.0);
}

/// Generates instances from `strategy` with a fixed seed, so that this can't flake.
fn sample<T: core::fmt::Debug>(strategy: impl Strategy<Value = T>, count: usize) -> Vec<T> {
    let mut runner = TestRunner::deterministic();
    (0..count)
        .map(|_| strategy.new_tree(&mut runner).unwrap().current())
        .collect()
}

/// What share of `instances` are worth testing, as fractions of their number: ones with a
/// reachable target, ones whose best path takes several steps, and ones that round.
fn richness(instances: &[Instance]) -> (f64, f64, f64) {
    let total = instances.len() as f64;
    let (mut reachable, mut long, mut rounding) = (0, 0, 0);
    for instance in instances {
        if let Some(optimum) = bellman_ford(instance) {
            reachable += 1;
            long += usize::from(optimum.steps >= 3);
            rounding += usize::from(fixed(optimum.rounded) != optimum.exact);
        }
    }
    (
        f64::from(reachable) / total,
        long as f64 / total,
        rounding as f64 / total,
    )
}

/// Guards against the generators drifting into instances that pass without testing anything.
#[test]
fn generated_instances_are_worth_testing() {
    let grids: Vec<_> = sample(larger_grid(fractional_cost()), 200)
        .iter()
        .map(GridCase::instance)
        .collect();
    let (reachable, long, rounding) = richness(&grids);
    assert!(reachable > 0.5, "reachable grids: {reachable}");
    assert!(long > 0.4, "long grid paths: {long}");
    assert!(rounding > 0.1, "rounded grid paths: {rounding}");

    let graphs: Vec<_> = sample(larger_graph(fractional_cost()), 200)
        .iter()
        .map(GraphCase::instance)
        .collect();
    let (reachable, long, rounding) = richness(&graphs);
    assert!(reachable > 0.5, "reachable graphs: {reachable}");
    assert!(long > 0.2, "long graph paths: {long}");
    assert!(rounding > 0.05, "rounded graph paths: {rounding}");

    let integers: Vec<_> = sample(larger_grid(integer_cost()), 200)
        .iter()
        .map(GridCase::instance)
        .collect();
    let (_, _, rounding) = richness(&integers);
    assert_eq!(rounding, 0.0, "integer sums must not round");
}
