//! Global routing: the board cut into square tiles on each layer, routed with the
//! negotiated-congestion [engine](crate::engine), whose result is a corridor for each net.
//!
//! A resource is the side between two tiles on a layer, which as many tracks can cross as
//! fit along its free length, and a tile's vias, as many as fit in its free area. Both are
//! measured against the fixed copper with the board's most common track width and
//! clearance, by sampling: a sample point is free if a track's end there would be legal.

use super::detail::{Corridor, NM};
use super::world::{Verdict, World};
use crate::cost::Cost;
use crate::engine::{self, EngineOptions};
use crate::geom::{LayerId, Point, Rect, Shape};
use crate::id::{NetId, NodeId};
use crate::net::{AccessPoint, Net, Terminal};
use crate::space::{DenseSlots, RoutingSpace, Step};

/// A probe that never matches a net, so that all copper counts as an obstacle.
const NO_NET: NetId = NetId(u32::MAX);

/// The pads of a terminal, with their layers.
pub(crate) type TerminalPads = Vec<(Point, Vec<u8>)>;

/// What capacities are measured with: the most common track width and via of the nets,
/// and the board's clearance, with what a via costs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Probe {
    pub(crate) width: i64,
    pub(crate) clearance: i64,
    pub(crate) via: Option<(i64, i64)>,
    pub(crate) via_cost: f64,
}

/// The tiles, and what can cross between them.
pub(crate) struct GlobalSpace {
    origin: Point,
    tile: i64,
    cols: usize,
    rows: usize,
    layers: usize,
    /// Whether each tile is on the board.
    inside: Vec<bool>,
    /// By layer and tile, the capacity of the side to the tile on the right, then of the
    /// side to the tile above.
    side: Vec<[u32; 2]>,
    /// By tile, how many vias fit.
    via_capacity: Vec<u32>,
    via_cost: f64,
}

impl GlobalSpace {
    fn plane(&self) -> usize {
        self.cols * self.rows
    }

    fn node(&self, l: usize, t: usize) -> NodeId {
        NodeId((l * self.plane() + t) as u32)
    }

    fn unpack(&self, n: NodeId) -> (usize, usize) {
        (n.index() / self.plane(), n.index() % self.plane())
    }

    /// The tile of a point, clamped to the grid.
    pub(crate) fn tile_of(&self, p: Point) -> usize {
        let cx = ((p.x - self.origin.x).div_euclid(self.tile)).clamp(0, self.cols as i64 - 1);
        let cy = ((p.y - self.origin.y).div_euclid(self.tile)).clamp(0, self.rows as i64 - 1);
        cy as usize * self.cols + cx as usize
    }

    fn center(&self, t: usize) -> Point {
        let (cx, cy) = ((t % self.cols) as i64, (t / self.cols) as i64);
        Point::new(
            self.origin.x + cx * self.tile + self.tile / 2,
            self.origin.y + cy * self.tile + self.tile / 2,
        )
    }

    /// Builds the tiles over `bounds`, finds which are on the board from `seeds`, and
    /// measures capacities against the world's fixed copper with `probe`.
    pub(crate) fn new(
        world: &World<'_>,
        bounds: Rect,
        tile: i64,
        probe: Probe,
        seeds: &[Point],
    ) -> GlobalSpace {
        let Probe {
            width,
            clearance,
            via,
            via_cost,
        } = probe;
        let w = bounds.max().x - bounds.min().x;
        let h = bounds.max().y - bounds.min().y;
        let cols = (w / tile + 1).max(1) as usize;
        let rows = (h / tile + 1).max(1) as usize;
        let layers = world.rules().layer_count();
        let mut space = GlobalSpace {
            origin: bounds.min(),
            tile,
            cols,
            rows,
            layers,
            inside: vec![false; cols * rows],
            side: vec![[0, 0]; layers * cols * rows],
            via_capacity: vec![0; cols * rows],
            via_cost,
        };
        space.flood(world, seeds);
        space.measure(world, width, clearance, via);
        space
    }

    /// Marks the tiles reachable from the seeds' without crossing the board's edge.
    fn flood(&mut self, world: &World<'_>, seeds: &[Point]) {
        let mut stack: Vec<usize> = Vec::new();
        for &s in seeds {
            let t = self.tile_of(s);
            if !self.inside[t] {
                self.inside[t] = true;
                stack.push(t);
            }
        }
        while let Some(t) = stack.pop() {
            let (cx, cy) = (t % self.cols, t / self.cols);
            let mut next = Vec::with_capacity(4);
            if cx + 1 < self.cols {
                next.push(t + 1);
            }
            if cx > 0 {
                next.push(t - 1);
            }
            if cy + 1 < self.rows {
                next.push(t + self.cols);
            }
            if cy > 0 {
                next.push(t - self.cols);
            }
            for u in next {
                if !self.inside[u] && !world.crosses_edge(self.center(t), self.center(u)) {
                    self.inside[u] = true;
                    stack.push(u);
                }
            }
        }
    }

    fn measure(&mut self, world: &World<'_>, width: i64, clearance: i64, via: Option<(i64, i64)>) {
        let pitch = width + clearance;
        let step = (pitch / 2).max(1);
        let samples = (self.tile / step).max(1);
        let free = |l: usize, p: Point| -> bool {
            Shape::circle(p, width)
                .map(|s| world.track(NO_NET, LayerId(l as u8), &s, None) != Verdict::Blocked)
                .unwrap_or(false)
        };
        for l in 0..self.layers {
            for t in 0..self.plane() {
                if !self.inside[t] {
                    continue;
                }
                let (cx, cy) = (t % self.cols, t / self.cols);
                let corner = Point::new(
                    self.origin.x + (cx as i64 + 1) * self.tile,
                    self.origin.y + (cy as i64 + 1) * self.tile,
                );
                for (k, right) in [true, false].into_iter().enumerate() {
                    let neighbour = if right {
                        (cx + 1 < self.cols).then(|| t + 1)
                    } else {
                        (cy + 1 < self.rows).then(|| t + self.cols)
                    };
                    if !neighbour.is_some_and(|u| self.inside[u]) {
                        continue;
                    }
                    let mut count = 0i64;
                    for i in 0..samples {
                        let d = i * step + step / 2;
                        let p = if right {
                            Point::new(corner.x, corner.y - d)
                        } else {
                            Point::new(corner.x - d, corner.y)
                        };
                        count += i64::from(free(l, p));
                    }
                    let cap = (count * step) / pitch;
                    let cap = if count > 0 { cap.max(1) } else { 0 };
                    let at = l * self.plane() + t;
                    self.side[at][k] = cap as u32;
                }
            }
        }
        if let Some((diameter, drill)) = via {
            let via_pitch = diameter + clearance;
            let vstep = (via_pitch / 2).max(1);
            let n = (self.tile / vstep).max(1);
            for t in 0..self.plane() {
                if !self.inside[t] {
                    continue;
                }
                let (cx, cy) = ((t % self.cols) as i64, (t / self.cols) as i64);
                let mut count = 0i64;
                for i in 0..n {
                    for j in 0..n {
                        let p = Point::new(
                            self.origin.x + cx * self.tile + i * vstep + vstep / 2,
                            self.origin.y + cy * self.tile + j * vstep + vstep / 2,
                        );
                        count += i64::from(
                            world.via(NO_NET, p, diameter, drill, None) != Verdict::Blocked,
                        );
                    }
                }
                let cap = (count * vstep * vstep) / (via_pitch * via_pitch);
                self.via_capacity[t] = if count > 0 { cap.max(1) as u32 } else { 0 };
            }
        }
    }

    fn tile_mm(&self) -> f64 {
        self.tile as f64 * NM
    }

    /// The corridor of a route: its tiles, grown by `margin`, each preferring the layers
    /// the route used there.
    pub(crate) fn corridor(&self, nodes: &[NodeId], margin: usize) -> Corridor {
        let mut cells = vec![0u64; self.plane()];
        for &n in nodes {
            let (l, t) = self.unpack(n);
            let (cx, cy) = (t % self.cols, t / self.cols);
            for y in cy.saturating_sub(margin)..=(cy + margin).min(self.rows - 1) {
                for x in cx.saturating_sub(margin)..=(cx + margin).min(self.cols - 1) {
                    cells[y * self.cols + x] |= Corridor::INSIDE;
                }
            }
            cells[t] |= Corridor::INSIDE | (1u64 << l);
        }
        // Next to the route, the layers it used nearby.
        let mut out = cells.clone();
        for (t, cell) in cells.iter().enumerate() {
            let layers = cell & !Corridor::INSIDE;
            if layers == 0 {
                continue;
            }
            let (cx, cy) = (t % self.cols, t / self.cols);
            for y in cy.saturating_sub(margin)..=(cy + margin).min(self.rows - 1) {
                for x in cx.saturating_sub(margin)..=(cx + margin).min(self.cols - 1) {
                    let u = y * self.cols + x;
                    if cells[u] & !Corridor::INSIDE == 0 {
                        out[u] |= layers;
                    }
                }
            }
        }
        Corridor {
            origin: self.origin,
            tile: self.tile,
            cols: self.cols,
            rows: self.rows,
            cells: out,
        }
    }

    /// An empty grid of the same tiles, for history.
    pub(crate) fn grid(&self) -> Corridor {
        Corridor {
            origin: self.origin,
            tile: self.tile,
            cols: self.cols,
            rows: self.rows,
            cells: vec![0; self.plane()],
        }
    }

    /// The access points of a terminal with pads at these points, on these layers.
    pub(crate) fn access(&self, pads: &[(Point, Vec<u8>)]) -> Vec<AccessPoint<NodeId>> {
        let mut out: Vec<AccessPoint<NodeId>> = Vec::new();
        for (p, layers) in pads {
            let t = self.tile_of(*p);
            for &l in layers {
                let node = self.node(usize::from(l), t);
                if !out.iter().any(|a| a.node == node) {
                    out.push(AccessPoint {
                        node,
                        cost: Cost::ZERO,
                    });
                }
            }
        }
        out
    }
}

impl RoutingSpace for GlobalSpace {
    type Node = NodeId;
    type Resource = NodeId;
    type Slots = DenseSlots<NodeId>;
    type ResourceSlots = DenseSlots<NodeId>;

    fn new_slots(&self) -> Self::Slots {
        DenseSlots::new()
    }

    fn new_resource_slots(&self) -> Self::ResourceSlots {
        DenseSlots::new()
    }

    fn contains(&self, node: NodeId) -> bool {
        node.index() < self.layers * self.plane() && self.inside[self.unpack(node).1]
    }

    fn expand(&self, _net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
        let (l, t) = self.unpack(from);
        let (cx, cy) = (t % self.cols, t / self.cols);
        let step = Cost::new(self.tile_mm()).unwrap_or(Cost::ONE);
        let mut push = |u: usize| {
            if self.inside[u] {
                out.push(Step {
                    to: self.node(l, u),
                    cost: step,
                });
            }
        };
        if cx + 1 < self.cols {
            push(t + 1);
        }
        if cx > 0 {
            push(t - 1);
        }
        if cy + 1 < self.rows {
            push(t + self.cols);
        }
        if cy > 0 {
            push(t - self.cols);
        }
        if self.via_capacity[t] > 0 {
            let cost = Cost::new(self.via_cost).unwrap_or(Cost::ONE);
            for m in (0..self.layers).filter(|&m| m != l) {
                out.push(Step {
                    to: self.node(m, t),
                    cost,
                });
            }
        }
    }

    fn lower_bound(&self, _net: NetId, from: NodeId, to: NodeId) -> Cost {
        let (_, a) = self.unpack(from);
        let (_, b) = self.unpack(to);
        let dx = (a % self.cols).abs_diff(b % self.cols);
        let dy = (a / self.cols).abs_diff(b / self.cols);
        Cost::new((dx + dy) as f64 * self.tile_mm() * (1.0 - 1e-9)).unwrap_or(Cost::ZERO)
    }

    fn resources(&self, _net: NetId, from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
        let (la, a) = self.unpack(from);
        let (lb, b) = self.unpack(to);
        let sides = self.layers * self.plane() * 2;
        if la != lb {
            out.push(NodeId((sides + a) as u32));
        } else if a != b {
            let (lo, hi) = (a.min(b), a.max(b));
            let k = usize::from(hi != lo + 1);
            out.push(NodeId(((la * self.plane() + lo) * 2 + k) as u32));
        }
    }

    fn capacity(&self, resource: NodeId) -> u32 {
        let sides = self.layers * self.plane() * 2;
        let r = resource.index();
        if r >= sides {
            self.via_capacity.get(r - sides).copied().unwrap_or(0)
        } else {
            self.side[r / 2][r % 2]
        }
    }
}

/// What global routing found.
pub(crate) struct GlobalResult {
    /// By position in the nets given, the tiles of the route, or `None` if it has none.
    pub(crate) routes: Vec<Option<Vec<NodeId>>>,
    pub(crate) overused: usize,
}

/// Routes the nets over the tiles. Each net is a list of terminals, each a list of pads'
/// positions and layers. A `cancel` flag set ends it with what it has.
pub(crate) fn route(
    space: &GlobalSpace,
    nets: &[(NetId, Vec<TerminalPads>)],
    max_iterations: u32,
    cancel: Option<&core::sync::atomic::AtomicBool>,
) -> GlobalResult {
    let engine_nets: Vec<Net<NodeId>> = nets
        .iter()
        .map(|(id, terminals)| Net {
            id: *id,
            terminals: terminals
                .iter()
                .filter_map(|pads| Terminal::new(space.access(pads)).ok())
                .collect(),
        })
        .collect();
    let options = EngineOptions {
        max_iterations,
        history_factor: Cost::new(space.tile_mm()).unwrap_or(Cost::ONE),
        cancel,
        ..EngineOptions::default()
    };
    let Ok(outcome) = engine::route(space, &engine_nets, options) else {
        return GlobalResult {
            routes: vec![None; nets.len()],
            overused: 0,
        };
    };
    let routes = nets
        .iter()
        .map(|(id, _)| outcome.routes.route(*id).map(|r| r.nodes().collect()))
        .collect();
    GlobalResult {
        routes,
        overused: outcome.overused.len(),
    }
}
