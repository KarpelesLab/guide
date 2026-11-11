//! The flow: nets routed on the plane by the negotiated-congestion engine, then given layers.

use crate::cost::Cost;
use crate::engine::{Engine, EngineOptions, IterationReport, Outcome};
use crate::id::{NetId, NodeId};
use crate::net::{AccessPoint, Edge, Net, Route, Terminal};

use super::format::{Design, Segment};
use super::layers::{self, LayerGrid, LayerNet, PlanarTree};
use super::pattern;
use super::planar::PlanarSpace;
use super::rsmt;

/// The most pins a net may have and still be routed, as the contests allow.
pub const MAX_PINS: usize = 1000;

/// How [`route_design`] routes.
///
/// The defaults are what `docs/benchmarks.md` measured on the ISPD 2008 designs.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct AsicOptions {
    /// Threads for the engine's searches. The result is the same for any number. The default
    /// is 1.
    pub threads: usize,
    /// How many nets the engine routes at a time against the same usage, which does change
    /// results: see [the engine](crate::engine#batches-and-threads). The default is 32.
    pub batch_size: u32,
    /// The most iterations of negotiation on the plane, over all its
    /// [restarts](Self::restarts). The default is 500.
    pub max_iterations: u32,
    /// How many iterations negotiation may go on without taking the overflow on the plane 1%
    /// below the least it has had, before it stalls, or 0 to go on to
    /// [`max_iterations`](Self::max_iterations). The default is 15.
    ///
    /// Negotiation that stalls starts again, if it has [restarts](Self::restarts) left, and
    /// otherwise stops: a design that does not converge would spend most of its time on
    /// iterations that change nothing.
    pub stall_iterations: u32,
    /// How many times negotiation that stalls starts again, from the best routes so far and
    /// with the history it has learned, but with the present factor back at its start, so that
    /// nets may share edges again for a while, and move where history now tells them to. The
    /// default is 20. It gives up sooner if two passes in a row end without a new least
    /// overflow.
    ///
    /// Negotiation stalls with a few edges one net over, which it moves around rather than
    /// settles; starting again takes them down to a fraction each time, and is what makes the
    /// hardest designs converge.
    pub restarts: u32,
    /// Whether to start every net from a pattern route, L or Z shapes along a Steiner tree of
    /// its pins, so that the engine only routes again those that overflow. The default is
    /// true.
    pub pattern: bool,
    /// Whether pattern routes follow a rectilinear Steiner tree of each net's pins, by the
    /// iterated 1-Steiner heuristic, rather than a minimum spanning tree. The default is true.
    pub steiner: bool,
    /// Whether pattern routes may take Z shapes, with two turns, where both L shapes are
    /// dearer. The default is true.
    pub z_shapes: bool,
    /// What a turn costs on the plane, on top of a step of 1, since it takes a via. The default
    /// is 1.
    pub bend_cost: f64,
    /// How far beyond the box around its pins a net's searches may go at first, in cells, or
    /// `None` for no limit. The default is 10.
    pub box_margin: Option<u32>,
    /// How many cells the box of a net grows by on every side after each iteration in which
    /// it holds an edge that overflows. The default is 5.
    pub box_growth: u32,
    /// How many cells a box may grow by in all. The default is 40.
    ///
    /// Boxes that grow without end make the searches of late iterations cover the whole
    /// design, and without making negotiation converge any sooner.
    pub max_box_growth: u32,
    /// Whether nets take their turns longest first, by the half perimeter of the box around
    /// their pins, rather than in the order of the design, as NTHU-Route 2.0 has them, since
    /// a long net has more ways round congestion than a short one. The default is true.
    pub longest_first: bool,
    /// The engine's [`crowding`](EngineOptions::crowding): how much dearer an edge gets as it
    /// fills up. The default is 16.
    pub crowding: f64,
    /// The engine's [`crowding_exponent`](EngineOptions::crowding_exponent): how late it gets
    /// dearer. The default is 8, so that an edge 90% full costs 7.9 times as much as an empty
    /// one, and a full one 17 times.
    pub crowding_exponent: u32,
    /// The engine's [`first_present`](EngineOptions::first_present). The default is 0.5.
    pub first_present: f64,
    /// The engine's [`initial_present`](EngineOptions::initial_present). The default is 0.5.
    pub initial_present: f64,
    /// The engine's [`present_growth`](EngineOptions::present_growth). The default is 1.8.
    pub present_growth: f64,
    /// The engine's [`history_factor`](EngineOptions::history_factor), in cells of wire. The
    /// default is 1.
    pub history_factor: f64,
    /// Whether, once the plane is free of overflow, nets longer than a Steiner tree of their
    /// pins are routed again, each by its shortest way that overflows nothing, against the
    /// others as they are, to take back the detours that crowding made. The default is true.
    pub refine: bool,
    /// How many times to assign layers again to the nets on edges that overflow on their
    /// layer. The default is 4.
    pub layer_passes: u32,
}

impl Default for AsicOptions {
    fn default() -> Self {
        AsicOptions {
            threads: 1,
            batch_size: 32,
            max_iterations: 500,
            stall_iterations: 15,
            restarts: 20,
            pattern: true,
            steiner: true,
            z_shapes: true,
            bend_cost: 1.0,
            box_margin: Some(10),
            box_growth: 5,
            max_box_growth: 40,
            longest_first: true,
            crowding: 16.0,
            crowding_exponent: 8,
            first_present: 0.5,
            initial_present: 0.5,
            present_growth: 1.8,
            history_factor: 1.0,
            refine: true,
            layer_passes: 4,
        }
    }
}

/// What [`route_design`] found.
#[derive(Clone, Debug)]
pub struct AsicRouting {
    /// By net of the design, its wires and vias, empty for a net that needs none.
    pub routes: Vec<Vec<Segment>>,
    /// The nets routed, which are those with at most [`MAX_PINS`] pins in more than one cell.
    pub routed_nets: usize,
    /// The engine's report of each iteration on the plane, with those after each
    /// [restart](AsicOptions::restarts) numbered from 1 again, and last that of the
    /// [refinement](AsicOptions::refine), if there was one.
    pub iterations: Vec<IterationReport>,
    /// How many times negotiation [started again](AsicOptions::restarts).
    pub restarts: u32,
    /// How many nets the [refinement](AsicOptions::refine) routed again.
    pub refined_nets: usize,
    /// Overflow left on the plane, in tracks, summed over edges.
    pub planar_overflow: u64,
}

/// How many passes of negotiation in a row may end without a new least overflow before it
/// gives up, however many [restarts](AsicOptions::restarts) it has left.
const FRUITLESS_RESTARTS: u32 = 2;

/// What crowding is during the [refinement](AsicOptions::refine), with the same exponent:
/// little, so that nets take the shortest ways they have, but enough that of two they take
/// the emptier.
const REFINE_CROWDING: u32 = 1;

/// Routes every net of `design` that needs it: on the plane first, by the engine, then onto
/// layers. See the [module documentation](super#the-flow).
///
/// The result is the same for any number of [threads](AsicOptions::threads).
pub fn route_design(design: &Design, options: &AsicOptions) -> AsicRouting {
    let capacities = design.capacities();
    let tracks = |layer: usize, capacity: u32| -> u32 {
        let pitch = design.min_width[layer] + design.min_spacing[layer];
        capacity.checked_div(pitch).unwrap_or(capacity)
    };
    let mut plane = PlanarSpace::new(&capacities, tracks);
    plane.set_bend_cost(Cost::new(options.bend_cost).unwrap_or(Cost::ZERO));

    // Nets on the plane: distinct cells, the first pin's first, which is the driver.
    let mut engine_nets: Vec<Net<NodeId>> = Vec::new();
    let mut pins_by_net: Vec<Vec<(usize, u8)>> = Vec::new();
    let mut design_index: Vec<usize> = Vec::new();
    for (n, net) in design.nets.iter().enumerate() {
        if net.pins.len() > MAX_PINS {
            continue;
        }
        let mut cells: Vec<(usize, u8)> = net
            .pins
            .iter()
            .filter_map(|p| {
                let (x, y) = design.cell_of(p.x, p.y)?;
                Some((plane.cell_at(x as usize, y as usize), p.layer))
            })
            .collect();
        let Some(&(first, _)) = cells.first() else {
            continue;
        };
        let mut distinct: Vec<usize> = cells.iter().map(|c| c.0).collect();
        distinct.sort_unstable();
        distinct.dedup();
        if distinct.len() < 2 {
            continue;
        }
        let terminal = |cell: usize| {
            let access = plane.access(cell).map(|node| AccessPoint {
                node,
                cost: Cost::ZERO,
            });
            Terminal::new(access.to_vec()).unwrap_or_else(|_| unreachable!("two access points"))
        };
        let mut terminals = vec![terminal(first)];
        terminals.extend(
            distinct
                .iter()
                .filter(|&&c| c != first)
                .map(|&c| terminal(c)),
        );
        cells.sort_unstable();
        engine_nets.push(Net {
            id: NetId(n as u32),
            terminals,
        });
        pins_by_net.push(cells);
        design_index.push(n);
    }
    // The box around each net's pins, by its number, which is its place in the design.
    let mut boxes = vec![[0, u32::MAX, 0, u32::MAX]; design.nets.len()];
    for net in &engine_nets {
        let mut b = [u32::MAX, 0, u32::MAX, 0];
        for cell in terminal_cells(net, &plane) {
            let (x, y) = plane.cell_xy(cell);
            let (x, y) = (x as u32, y as u32);
            b = [b[0].min(x), b[1].max(x), b[2].min(y), b[3].max(y)];
        }
        boxes[net.id.index()] = b;
    }
    if options.longest_first {
        let half_perimeter = |k: usize| {
            let b = boxes[engine_nets[k].id.index()];
            (b[1] - b[0]) + (b[3] - b[2])
        };
        let mut order: Vec<usize> = (0..engine_nets.len()).collect();
        order.sort_by_key(|&k| (core::cmp::Reverse(half_perimeter(k)), k));
        engine_nets = order.iter().map(|&k| engine_nets[k].clone()).collect();
        pins_by_net = order.iter().map(|&k| pins_by_net[k].clone()).collect();
        design_index = order.iter().map(|&k| design_index[k]).collect();
    }
    if let Some(margin) = options.box_margin {
        plane.set_boxes(&boxes);
        for net in &engine_nets {
            plane.grow_box(net.id, margin);
        }
    }

    let engine_options = EngineOptions {
        max_iterations: options.max_iterations,
        batch_size: options.batch_size,
        crowding: Cost::new(options.crowding).unwrap_or(Cost::ZERO),
        crowding_exponent: options.crowding_exponent,
        history_factor: Cost::new(options.history_factor).unwrap_or(Cost::ONE),
        first_present: Cost::new(options.first_present).unwrap_or(Cost::ONE),
        initial_present: Cost::new(options.initial_present).unwrap_or(Cost::ONE),
        present_growth: Cost::new(options.present_growth).unwrap_or(Cost::ONE),
        ..EngineOptions::default()
    };
    let mut engine = Engine::new(&plane, &engine_nets, engine_options)
        .unwrap_or_else(|e| unreachable!("nets have distinct numbers: {e}"));
    if options.pattern {
        let cells: Vec<Vec<usize>> = engine_nets
            .iter()
            .map(|net| terminal_cells(net, &plane).collect())
            .collect();
        let patterns = pattern::pattern_routes(
            &plane,
            &cells,
            options.bend_cost,
            options.steiner,
            options.z_shapes,
        );
        let seeds =
            engine_nets
                .iter()
                .zip(patterns)
                .zip(&pins_by_net)
                .map(|((net, links), pins)| {
                    let tree = tree_of_links(cells_root(net, &plane), links, pins);
                    (net.id, node_route(&plane, &tree))
                });
        engine
            .seed_routes(seeds)
            .unwrap_or_else(|e| unreachable!("seeds come before the first iteration: {e}"));
    }
    let mut grown = vec![0u32; design.nets.len()];
    let mut reports: Vec<IterationReport> = Vec::new();
    let mut best: Option<(u64, Outcome<NodeId, NodeId>)> = None;
    let mut restarts = 0;
    // Passes in a row that ended without a new least overflow.
    let mut fruitless = 0;
    loop {
        let (mut least, mut least_at) = (u64::MAX, 0);
        let mut stalled = false;
        while let Some(report) = engine.step_parallel(options.threads) {
            if report.total_overuse.saturating_mul(100) < least.saturating_mul(99) {
                (least, least_at) = (report.total_overuse, report.iteration);
            }
            if options.stall_iterations > 0
                && report.iteration >= least_at + options.stall_iterations
            {
                stalled = true;
                break;
            }
            // Nets that end an iteration on an edge that overflows may look further next time.
            if options.box_margin.is_some() && options.box_growth > 0 {
                let mut grow: Vec<NetId> = engine
                    .overused()
                    .iter()
                    .flat_map(|r| r.nets.iter().copied())
                    .collect();
                grow.sort_unstable();
                grow.dedup();
                for net in grow {
                    let left = options.max_box_growth.saturating_sub(grown[net.index()]);
                    let by = options.box_growth.min(left);
                    if by > 0 {
                        grown[net.index()] += by;
                        plane.grow_box(net, by);
                    }
                }
            }
        }
        reports.extend_from_slice(engine.reports());
        let outcome = engine.outcome();
        let overuse = outcome
            .overused
            .iter()
            .map(|r| u64::from(r.occupancy.saturating_sub(r.capacity)))
            .sum();
        if best.as_ref().is_none_or(|(least, _)| overuse < *least) {
            best = Some((overuse, outcome));
            fruitless = 0;
        } else {
            fruitless += 1;
        }
        let budget = options.max_iterations.saturating_sub(reports.len() as u32);
        let Some((overuse, from)) = best.as_ref() else {
            unreachable!("set above")
        };
        if !stalled
            || *overuse == 0
            || restarts >= options.restarts
            || fruitless >= FRUITLESS_RESTARTS
            || budget == 0
        {
            break;
        }
        // Negotiation again from the best routes so far, with the history it has learned and
        // the present factor back where it starts.
        restarts += 1;
        let history = engine.history();
        engine = Engine::new(
            &plane,
            &engine_nets,
            EngineOptions {
                max_iterations: budget,
                ..engine_options
            },
        )
        .unwrap_or_else(|e| unreachable!("nets have distinct numbers: {e}"));
        engine
            .seed_routes(from.routes.routes().map(|(id, route)| (id, route.clone())))
            .unwrap_or_else(|e| unreachable!("seeds come before the first iteration: {e}"));
        engine
            .seed_history(history)
            .unwrap_or_else(|e| unreachable!("history comes before the first iteration: {e}"));
    }
    drop(engine);
    let Some((_, mut outcome)) = best else {
        unreachable!("set in the first pass")
    };
    let mut refined_nets = 0;
    if options.refine && outcome.overused.is_empty() {
        let refine_options = EngineOptions {
            max_iterations: 1,
            batch_size: options.batch_size,
            // Nothing a net could save on length is worth an overflow.
            first_present: Cost::from(1000),
            crowding: Cost::from(REFINE_CROWDING),
            crowding_exponent: options.crowding_exponent,
            ..EngineOptions::default()
        };
        let mut again = Engine::new(&plane, &engine_nets, refine_options)
            .unwrap_or_else(|e| unreachable!("nets have distinct numbers: {e}"));
        let detoured: Vec<NetId> = engine_nets
            .iter()
            .filter(|net| {
                outcome
                    .routes
                    .route(net.id)
                    .is_some_and(|route| planar_length(&plane, route) > steiner_length(net, &plane))
            })
            .map(|net| net.id)
            .collect();
        again
            .seed_routes(
                outcome
                    .routes
                    .routes()
                    .map(|(id, route)| (id, route.clone())),
            )
            .unwrap_or_else(|e| unreachable!("seeds come before the first iteration: {e}"));
        refined_nets = again.rip_up(detoured);
        while again.step_parallel(options.threads).is_some() {}
        let refined = again.outcome();
        // Only the nets routed again move, each to a way that overflows nothing, so this
        // holds; but a result with overflow is never traded for this one.
        if refined.overused.is_empty() {
            reports.extend_from_slice(again.reports());
            outcome = refined;
        }
    }
    let planar_overflow = outcome
        .overused
        .iter()
        .map(|r| u64::from(r.occupancy.saturating_sub(r.capacity)))
        .sum();

    // Trees for layer assignment.
    let mut nets: Vec<LayerNet> = Vec::with_capacity(engine_nets.len());
    for (k, net) in engine_nets.iter().enumerate() {
        let tree = outcome
            .routes
            .route(net.id)
            .map(|route| cell_tree(&plane, route, &pins_by_net[k]))
            .unwrap_or_default();
        let width = (0..design.layers())
            .map(|l| design.wire_usage(&design.nets[design_index[k]], l))
            .collect();
        nets.push(LayerNet { tree, width });
    }
    // Nets with the least wire per pin first: their vias are at their pins, and they have
    // the least length to win back what going up a layer costs there. Ties go by place in
    // the design.
    let mut order: Vec<usize> = (0..nets.len()).collect();
    order.sort_by_key(|&k| {
        let t = &nets[k].tree;
        let pins = t.pins.iter().filter(|p| p.is_some()).count().max(1);
        (t.cells.len() * 100 / pins, design_index[k])
    });
    let mut grid = LayerGrid::new(design, &capacities, &plane);
    layers::assign(&mut grid, &plane, &mut nets, &order, options.layer_passes);

    let mut routes = vec![Vec::new(); design.nets.len()];
    for (k, net) in nets.iter().enumerate() {
        routes[design_index[k]] = layers::segments(&plane, &net.tree);
    }
    AsicRouting {
        routes,
        routed_nets: engine_nets.len(),
        iterations: reports,
        restarts,
        refined_nets,
        planar_overflow,
    }
}

/// The cells of a net's terminals, the driver's first.
fn terminal_cells<'a>(
    net: &'a Net<NodeId>,
    plane: &'a PlanarSpace,
) -> impl Iterator<Item = usize> + 'a {
    net.terminals
        .iter()
        .filter_map(|t| t.access_points().first())
        .map(|a| plane.cell_of(a.node))
}

/// How many edges between cells a route on the plane crosses.
fn planar_length(plane: &PlanarSpace, route: &Route<NodeId>) -> u64 {
    route
        .edges()
        .iter()
        .filter(|e| plane.cell_of(e.from) != plane.cell_of(e.to))
        .count() as u64
}

/// The length of a rectilinear Steiner tree of a net's terminals, as pattern routes have it.
fn steiner_length(net: &Net<NodeId>, plane: &PlanarSpace) -> u64 {
    let points: Vec<(u32, u32)> = terminal_cells(net, plane)
        .map(|c| {
            let (x, y) = plane.cell_xy(c);
            (x as u32, y as u32)
        })
        .collect();
    let (points, edges) = rsmt::steiner_tree(&points);
    edges
        .iter()
        .map(|&(a, b)| {
            let (p, q) = (points[a], points[b]);
            u64::from(p.0.abs_diff(q.0)) + u64::from(p.1.abs_diff(q.1))
        })
        .sum()
}

/// The tree of cells of a route on the plane, rooted at the cell of its root, with the span of
/// layers of the pins in each cell.
///
/// A route can pass through a cell twice, arriving along a row once and along a column once,
/// and so close a loop of cells. Edges are taken from the root outwards, breadth first, and
/// those that would close a loop are left out, as are branches that then lead to no pin.
fn cell_tree(plane: &PlanarSpace, route: &Route<NodeId>, pins: &[(usize, u8)]) -> PlanarTree {
    let Some(root) = route.root() else {
        return PlanarTree::default();
    };
    let links = route
        .edges()
        .iter()
        .map(|e| (plane.cell_of(e.from), plane.cell_of(e.to)))
        .collect();
    tree_of_links(plane.cell_of(root), links, pins)
}

/// The tree of cells reached from `root` by `links`, which go either way, breadth first, with
/// branches that lead to no pin left out.
fn tree_of_links(root: usize, links: Vec<(usize, usize)>, pins: &[(usize, u8)]) -> PlanarTree {
    let mut both: Vec<(usize, usize)> = Vec::with_capacity(links.len() * 2);
    for (a, b) in links {
        if a != b {
            both.push((a, b));
            both.push((b, a));
        }
    }
    both.sort_unstable();
    both.dedup();
    let links = both;
    let mut cells = vec![root];
    let mut parent = vec![u32::MAX];
    let mut seen = vec![root];
    let mut next = 0;
    while next < cells.len() {
        let c = cells[next];
        let from = links.partition_point(|&(a, _)| a < c);
        for &(_, d) in links[from..].iter().take_while(|&&(a, _)| a == c) {
            if let Err(at) = seen.binary_search(&d) {
                seen.insert(at, d);
                cells.push(d);
                parent.push(next as u32);
            }
        }
        next += 1;
    }
    let span = |c: usize| -> Option<(u8, u8)> {
        let from = pins.partition_point(|&(n, _)| n < c);
        let mut span: Option<(u8, u8)> = None;
        for &(_, l) in pins[from..].iter().take_while(|&&(n, _)| n == c) {
            span = Some(span.map_or((l, l), |(a, b)| (a.min(l), b.max(l))));
        }
        span
    };
    let pin_span: Vec<Option<(u8, u8)>> = cells.iter().map(|&c| span(c)).collect();
    // Keep a node if it has a pin or a kept child: children come after parents.
    let mut keep: Vec<bool> = pin_span.iter().map(Option::is_some).collect();
    keep[0] = true;
    for v in (1..cells.len()).rev() {
        if keep[v] {
            keep[parent[v] as usize] = true;
        }
    }
    let mut new_index = vec![u32::MAX; cells.len()];
    let mut tree = PlanarTree::default();
    for v in 0..cells.len() {
        if !keep[v] {
            continue;
        }
        new_index[v] = tree.cells.len() as u32;
        tree.cells.push(cells[v] as u32);
        tree.parent.push(if v == 0 {
            u32::MAX
        } else {
            new_index[parent[v] as usize]
        });
        tree.pins.push(pin_span[v]);
    }
    tree
}

/// The cell of a net's driver.
fn cells_root(net: &Net<NodeId>, plane: &PlanarSpace) -> usize {
    net.terminals
        .first()
        .and_then(|t| t.access_points().first())
        .map_or(0, |a| plane.cell_of(a.node))
}

/// A tree of cells as a route on the plane's nodes: each cell is entered at the node for the
/// way its edge runs, and the root at its node for rows.
fn node_route(plane: &PlanarSpace, tree: &PlanarTree) -> Route<NodeId> {
    let mut nodes: Vec<NodeId> = Vec::with_capacity(tree.cells.len());
    let mut edges = Vec::with_capacity(tree.cells.len().saturating_sub(1));
    for v in 0..tree.cells.len() {
        let cell = tree.cells[v] as usize;
        if v == 0 {
            nodes.push(plane.access(cell)[0]);
            continue;
        }
        let p = tree.parent[v] as usize;
        let along_column = plane
            .edge(tree.cells[p] as usize, cell)
            .is_some_and(|e| e >= plane.horizontal_edges());
        let node = plane.access(cell)[usize::from(along_column)];
        edges.push(Edge {
            from: nodes[p],
            to: node,
        });
        nodes.push(node);
    }
    match nodes.first() {
        Some(&root) => Route::from_edges(root, edges),
        None => Route::new(),
    }
}
