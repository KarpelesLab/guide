//! Placement and routing as one loop: what the router finds congested goes back into the
//! placer, which places again around it, and the router routes again.
//!
//! [`RouterCongestion`] records, tile by tile, the channels a routing used and those it
//! overused, as history; [`refine_placement`](super::refine_placement) re-places the blocks
//! near what it overused with that added to its estimate of congestion; and
//! [`place_route_loop`] goes round the two at one channel width until the design routes or
//! the rounds run out.

use core::fmt;

use super::ImportError;
use super::design::{Design, NetOptions, resolve_nets};
use super::netlist::PackedNetlist;
use super::placer::{PlaceDelayModel, PlaceProblem, RefineOptions, refine_placement};
use super::rr_graph::{NodeKind, RrGraph};
use super::timing::DesignTiming;
use crate::engine::{Engine, EngineError, EngineOptions, Outcome, OverusedResource};
use crate::id::NodeId;
use crate::net::RouteSet;
use crate::place::{CongestionGrid, PlaceError, Point, Rect};
use crate::space::RoutingSpace;
use crate::timing::Constraints;

/// What a router found of the use of a device's channels, by tile, horizontal and vertical,
/// against the tracks of the device the placement was built on.
///
/// Made from a [`PlaceProblem`], whose channels it measures against, and filled by
/// [`add_routes`](RouterCongestion::add_routes) with the wire a routing used, and by
/// [`add_overuse`](RouterCongestion::add_overuse) after each iteration of negotiation with
/// what was overused then, which adds up as the router's own history does.
/// [`correction`](RouterCongestion::correction) is what placement adds to its estimate.
#[derive(Clone, Debug, PartialEq)]
pub struct RouterCongestion {
    width: u32,
    height: u32,
    /// By tile, the tracks of the placement's device.
    tracks: Vec<[u32; 2]>,
    /// By tile, the wire the last routing added used, in tracks.
    used: Vec<[f64; 2]>,
    /// By tile, overuse added up over iterations, in tracks.
    history: Vec<[f64; 2]>,
    /// By tile, the use beyond the estimate and the history, weighed, as utilization.
    correction: Vec<[f64; 2]>,
}

impl RouterCongestion {
    /// An empty record for the device of a placement problem.
    pub fn new(problem: &PlaceProblem) -> RouterCongestion {
        let (width, height) = problem.grid_size();
        let tiles = (width * height) as usize;
        let tracks = (0..tiles as u32)
            .map(|t| {
                problem
                    .channel_tracks(t % width.max(1), t / width.max(1))
                    .unwrap_or_default()
            })
            .collect();
        RouterCongestion {
            width,
            height,
            tracks,
            used: vec![[0.0; 2]; tiles],
            history: vec![[0.0; 2]; tiles],
            correction: vec![[0.0; 2]; tiles],
        }
    }

    /// Calls `f` with the tile index and direction of each tile a channel node passes over.
    fn each_tile(&self, device: &RrGraph, node: NodeId, mut f: impl FnMut(usize, usize)) {
        let direction = match device.node_kind(node) {
            Some(NodeKind::ChanX) => 0,
            Some(NodeKind::ChanY) => 1,
            _ => return,
        };
        let Some(at) = device.node_location(node) else {
            return;
        };
        let (width, height) = (self.width, self.height);
        for y in u32::from(at.ylow)..=u32::from(at.yhigh).min(height.saturating_sub(1)) {
            for x in u32::from(at.xlow)..=u32::from(at.xhigh).min(width.saturating_sub(1)) {
                f((y * width + x) as usize, direction);
            }
        }
    }

    /// Replaces the wire used with that of these routes, on `device`, the graph they were
    /// routed on, which has to be of the same grid.
    pub fn add_routes(&mut self, device: &RrGraph, routes: &RouteSet<NodeId>) {
        let mut used = vec![[0.0f64; 2]; self.used.len()];
        for (_, route) in routes.routes() {
            for node in route.nodes() {
                self.each_tile(device, node, |tile, d| used[tile][d] += 1.0);
            }
        }
        self.used = used;
    }

    /// Adds what is overused after one iteration of negotiation to the history.
    pub fn add_overuse(&mut self, device: &RrGraph, overused: &[OverusedResource<NodeId>]) {
        let mut history = core::mem::take(&mut self.history);
        for entry in overused {
            let over = f64::from(entry.occupancy.saturating_sub(entry.capacity));
            self.each_tile(device, entry.resource, |tile, d| history[tile][d] += over);
        }
        self.history = history;
    }

    /// Works out the [`correction`](RouterCongestion::correction) for a placement: for each
    /// channel, `gap_weight` times how much more wire the routes used than
    /// [`PlaceProblem::channel_demand`] expects of `placement`, the placement they routed,
    /// plus `history_weight` times the history of overuse, each over the channel's tracks.
    pub fn update_correction(
        &mut self,
        problem: &PlaceProblem,
        placement: &crate::place::Placement,
        gap_weight: f64,
        history_weight: f64,
    ) {
        let demand = problem.channel_demand(placement);
        for tile in 0..self.tracks.len() {
            let (x, y) = (tile as u32 % self.width, tile as u32 / self.width);
            let expected = demand.utilization(x, y).unwrap_or_default();
            for (d, &expected) in expected.iter().enumerate() {
                let tracks = f64::from(self.tracks[tile][d]);
                self.correction[tile][d] = if tracks > 0.0 {
                    let gap = (self.used[tile][d] / tracks - expected).max(0.0);
                    gap_weight * gap + history_weight * self.history[tile][d] / tracks
                } else {
                    0.0
                };
            }
        }
    }

    /// What placement adds to its estimate of each channel's use, by tile, column by column
    /// within each row, horizontal and vertical, as a fraction of the channel's tracks.
    pub fn correction(&self) -> &[[f64; 2]] {
        &self.correction
    }

    /// The history of overuse at a tile, horizontal and vertical, in tracks.
    pub fn history(&self, x: u32, y: u32) -> Option<[f64; 2]> {
        (x < self.width && y < self.height).then(|| self.history[(y * self.width + x) as usize])
    }

    /// The wire used at a tile by the last routes added, horizontal and vertical, in tracks.
    pub fn used(&self, x: u32, y: u32) -> Option<[f64; 2]> {
        (x < self.width && y < self.height).then(|| self.used[(y * self.width + x) as usize])
    }

    /// The use and the tracks as a [`CongestionGrid`] of one bin per tile, for placers other
    /// than the FPGA's: each tile's demand is the wire used and the history of overuse, and
    /// its capacity is its tracks, both directions together.
    pub fn to_grid(&self) -> CongestionGrid {
        let area = Rect::new(
            Point::new(0, 0),
            Point::new(
                i64::from(self.width.max(1)) - 1,
                i64::from(self.height.max(1)) - 1,
            ),
        );
        let mut grid = CongestionGrid::new(area, 1, 0.0);
        for tile in 0..self.tracks.len() {
            let at = Point::new(
                i64::from(tile as u32 % self.width),
                i64::from(tile as u32 / self.width),
            );
            let [tx, ty] = self.tracks[tile];
            grid.set_capacity_at(at, f64::from(tx) + f64::from(ty));
            let demand = self.used[tile][0]
                + self.used[tile][1]
                + self.history[tile][0]
                + self.history[tile][1];
            grid.add_demand_at(at, demand);
        }
        grid
    }

    /// Windows around the tiles of the nodes in `overused`: each such tile grown by
    /// `margin` tiles each way, within the grid, and those that touch or overlap merged into
    /// their bounding box until none do. In order of their lower left corners.
    pub fn windows(
        &self,
        device: &RrGraph,
        overused: &[OverusedResource<NodeId>],
        margin: u32,
    ) -> Vec<Rect> {
        let mut hot = vec![false; self.used.len()];
        for entry in overused {
            // Any node, a pin's too, marks the tiles it is on.
            let Some(at) = device.node_location(entry.resource) else {
                continue;
            };
            for y in u32::from(at.ylow)..=u32::from(at.yhigh).min(self.height.saturating_sub(1)) {
                for x in u32::from(at.xlow)..=u32::from(at.xhigh).min(self.width.saturating_sub(1))
                {
                    hot[(y * self.width + x) as usize] = true;
                }
            }
        }
        let last_x = i64::from(self.width.max(1)) - 1;
        let last_y = i64::from(self.height.max(1)) - 1;
        let margin = i64::from(margin);
        let mut windows: Vec<Rect> = Vec::new();
        for (tile, _) in hot.iter().enumerate().filter(|&(_, &h)| h) {
            let x = i64::from(tile as u32 % self.width);
            let y = i64::from(tile as u32 / self.width);
            windows.push(Rect::new(
                Point::new((x - margin).max(0), (y - margin).max(0)),
                Point::new((x + margin).min(last_x), (y + margin).min(last_y)),
            ));
        }
        // Merge until nothing touches; few windows, so quadratic is fine.
        loop {
            let mut merged = false;
            let mut out: Vec<Rect> = Vec::with_capacity(windows.len());
            for window in windows {
                if let Some(other) = out.iter_mut().find(|o| touches(o, &window)) {
                    *other = other.union(&window);
                    merged = true;
                } else {
                    out.push(window);
                }
            }
            windows = out;
            if !merged {
                break;
            }
        }
        windows.sort_by_key(|w| (w.min.y, w.min.x, w.max.y, w.max.x));
        windows
    }
}

/// Whether two boxes of tiles overlap or are next to each other.
fn touches(a: &Rect, b: &Rect) -> bool {
    a.min.x <= b.max.x + 1
        && b.min.x <= a.max.x + 1
        && a.min.y <= b.max.y + 1
        && b.min.y <= a.max.y + 1
}

/// How [`place_route_loop`] goes round.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct LoopOptions {
    /// How many times to place again after a routing that fails. The default is 3.
    pub rounds: u32,
    /// How far around each tile with an overused resource blocks are placed again, in tiles.
    /// The default is 6: on VTR's benchmarks, windows 4 tiles wide routed a narrower channel
    /// on fewer circuits, and 8 on as many.
    pub margin: u32,
    /// Whether to place again the whole device, at a low temperature, rather than windows
    /// around what was overused. The default is false.
    pub whole: bool,
    /// The weight of how much more of each channel the router used than placement's estimate,
    /// in [`RouterCongestion::update_correction`]. The default is 1.
    pub gap_weight: f64,
    /// The weight of the router's history of overuse there. The default is 1.
    pub history_weight: f64,
    /// How to place again.
    pub refine: RefineOptions,
    /// Threads to route with. The result is the same for any number. The default is 1.
    pub threads: usize,
}

impl Default for LoopOptions {
    fn default() -> Self {
        LoopOptions {
            rounds: 3,
            margin: 6,
            whole: false,
            gap_weight: 1.0,
            history_weight: 1.0,
            refine: RefineOptions::default(),
            threads: 1,
        }
    }
}

/// What one routing of [`place_route_loop`], and the placement after it, came to.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct LoopRound {
    /// Whether the routing was legal.
    pub legal: bool,
    /// Resources overused by its best iteration.
    pub overused: usize,
    /// Iterations of negotiation it took.
    pub iterations: u64,
    /// Windows placed again after it, none if it was legal or the last.
    pub windows: usize,
    /// Tiles in those windows.
    pub window_tiles: u64,
    /// Blocks that the placement after it moved.
    pub moved: usize,
    /// The [`bb_estimate`](PlaceProblem::bb_estimate) of the placement it routed.
    pub bb_estimate: f64,
    /// The worst slack of the placement it routed, by the delay model, if timing-driven.
    pub worst_slack: Option<i64>,
}

/// What [`place_route_loop`] ended with.
#[derive(Debug)]
#[non_exhaustive]
pub struct LoopResult {
    /// The placement routed last if it routed, and otherwise the one whose routing overused
    /// fewest resources, the first of those on a tie.
    pub placement: crate::place::Placement,
    /// Its nets on the device.
    pub design: Design,
    /// Its routing.
    pub outcome: Outcome<NodeId, NodeId>,
    /// Each round in turn.
    pub rounds: Vec<LoopRound>,
    /// What the router found, over all rounds.
    pub congestion: RouterCongestion,
}

/// Why [`place_route_loop`] failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum LoopError {
    /// The placement didn't resolve to nets of the device.
    Import(ImportError),
    /// The engine refused the nets.
    Engine(EngineError),
    /// Placing again failed.
    Place(PlaceError),
}

impl fmt::Display for LoopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoopError::Import(e) => write!(f, "resolving nets: {e}"),
            LoopError::Engine(e) => write!(f, "routing: {e}"),
            LoopError::Place(e) => write!(f, "placing: {e}"),
        }
    }
}

impl std::error::Error for LoopError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LoopError::Import(e) => Some(e),
            LoopError::Engine(e) => Some(e),
            LoopError::Place(e) => Some(e),
        }
    }
}

/// Runs an engine to the end, adding what each iteration overuses to `congestion`.
fn negotiate<S>(
    mut engine: Engine<'_, S>,
    device: &RrGraph,
    congestion: &mut RouterCongestion,
    threads: usize,
) -> Outcome<NodeId, NodeId>
where
    S: RoutingSpace<Node = NodeId, Resource = NodeId> + Sync,
    S::Slots: Send,
    S::ResourceSlots: Send + Sync,
{
    while engine.step_parallel(threads).is_some() {
        congestion.add_overuse(device, &engine.overused());
    }
    engine.outcome()
}

/// Places and routes in a loop at one channel width, from a placement, until the design
/// routes or `options.rounds` re-placements have been tried.
///
/// Each round routes the placement on `device`, the graph of the width to route at, with
/// `engine_options`, timing-driven if `timing` is given, as `vtr_route` routes. If that is
/// legal, the loop ends. Otherwise the [`RouterCongestion`] of the routing, the wire it used
/// beyond placement's estimate and the history of what each iteration overused, added up over
/// rounds, goes into [`refine_placement`], which places again the blocks within
/// [`margin`](LoopOptions::margin) tiles of what the best iteration overused, or the whole
/// device with [`whole`](LoopOptions::whole), with its seed changed each round.
///
/// `problem` is the placement problem `start` is of, which may be built on another graph of
/// the same device, such as one with wider channels, and `delays` its delay model.
///
/// # Errors
///
/// [`LoopError`] if a placement doesn't resolve to the device's nets, the engine refuses them,
/// or `start` is not a legal placement of `problem`.
#[allow(clippy::too_many_arguments)]
pub fn place_route_loop(
    device: &RrGraph,
    netlist: &PackedNetlist,
    problem: &PlaceProblem,
    delays: &PlaceDelayModel,
    mut timing: Option<(&mut DesignTiming, &Constraints)>,
    net_options: &NetOptions,
    engine_options: EngineOptions<'_>,
    start: crate::place::Placement,
    options: &LoopOptions,
) -> Result<LoopResult, LoopError> {
    let mut congestion = RouterCongestion::new(problem);
    let mut placement = start;
    let mut rounds = Vec::new();
    let mut best: Option<(
        usize,
        crate::place::Placement,
        Design,
        Outcome<NodeId, NodeId>,
    )> = None;
    let mut round = 0u32;
    loop {
        let file = problem.to_placement(netlist, &placement, "", "");
        let design =
            resolve_nets(device, netlist, &file, net_options).map_err(LoopError::Import)?;
        let worst_slack = match timing.as_mut() {
            Some((t, c)) => super::estimate_slack(problem, delays, t, c, &placement),
            None => None,
        };
        let outcome = match timing.as_mut() {
            Some((t, c)) => {
                let timed = device.timed_graph();
                t.estimate_delays(&timed, &design);
                let driven = t.timing_driven(&design, (*c).clone());
                let engine = Engine::with_timing(&timed, design.nets(), engine_options, driven)
                    .map_err(LoopError::Engine)?;
                negotiate(engine, device, &mut congestion, options.threads)
            }
            None => {
                let engine = Engine::new(device.graph(), design.nets(), engine_options)
                    .map_err(LoopError::Engine)?;
                negotiate(engine, device, &mut congestion, options.threads)
            }
        };
        let legal = outcome.is_success();
        let mut record = LoopRound {
            legal,
            overused: outcome.overused.len(),
            iterations: outcome.routes.stats.iterations,
            windows: 0,
            window_tiles: 0,
            moved: 0,
            bb_estimate: problem.bb_estimate(&placement),
            worst_slack,
        };
        let last = legal || round >= options.rounds;
        let windows = if last {
            Vec::new()
        } else if options.whole {
            let (w, h) = problem.grid_size();
            vec![Rect::new(
                Point::new(0, 0),
                Point::new(i64::from(w) - 1, i64::from(h) - 1),
            )]
        } else {
            congestion.windows(device, &outcome.overused, options.margin)
        };
        let next = if last {
            None
        } else {
            congestion.add_routes(device, &outcome.routes);
            congestion.update_correction(
                problem,
                &placement,
                options.gap_weight,
                options.history_weight,
            );
            let refine = RefineOptions {
                seed: options.refine.seed.wrapping_add(u64::from(round)),
                ..options.refine
            };
            let reborrowed = timing.as_mut().map(|(t, c)| (&mut **t, *c));
            let refined = refine_placement(
                problem,
                delays,
                reborrowed,
                &placement,
                &windows,
                Some(&congestion),
                &refine,
            )
            .map_err(LoopError::Place)?;
            record.windows = windows.len();
            record.window_tiles = windows
                .iter()
                .map(|w| (w.width() + 1) * (w.height() + 1))
                .sum();
            record.moved = (0..problem.problem().items().len())
                .filter(|&i| {
                    let item = crate::place::ItemId(i as u32);
                    refined.placement.site(item) != placement.site(item)
                })
                .count();
            Some(refined.placement)
        };
        rounds.push(record);
        let overused = if legal {
            0
        } else {
            outcome.overused.len().max(1)
        };
        if best.as_ref().is_none_or(|(o, ..)| overused < *o) {
            best = Some((overused, placement.clone(), design, outcome));
        }
        match next {
            Some(next) => placement = next,
            None => break,
        }
        round += 1;
    }
    let (_, placement, design, outcome) = best.expect("the loop routes at least once");
    Ok(LoopResult {
        placement,
        design,
        outcome,
        rounds,
        congestion,
    })
}
