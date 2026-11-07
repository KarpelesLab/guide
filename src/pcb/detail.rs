//! Detailed routing: one net at a time, through a space of points generated as the search
//! reaches them, where every step is checked exactly against the [`World`].
//!
//! A node is a point on a layer. From a node, a route can go a lattice pitch in any of eight
//! directions, change layers through a via, or, near a pad it has to reach, go straight to
//! the pad's centre. Lattices are not global: one starts at each pad's centre, so that a
//! track leaving a pad is centred on it whatever the pad's position, and searches end at
//! pads' centres. Steps between two nodes are tracks of the net's width, and each is
//! legal only if the world says so, which is the whole of the design rules.

use core::cell::RefCell;

use super::world::{Verdict, World};
use crate::cost::Cost;
use crate::geom::{LayerId, LayerSet, Point, Rect, Shape, SpatialIndex};
use crate::id::{NetId, NodeId};
use crate::search::{Search, SearchLimits, SearchOutcome, Source, Target};
use crate::space::{DenseSlots, FastMap, InternedSlots, RoutingSpace, Step};

/// A point on a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct DNode {
    pub(crate) x: i64,
    pub(crate) y: i64,
    pub(crate) l: u8,
}

impl DNode {
    pub(crate) fn point(self) -> Point {
        Point::new(self.x, self.y)
    }
}

/// The eight directions, as steps of one pitch.
const DIRS: [(i64, i64); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];

/// Nanometres to the millimetres that costs are in.
pub(crate) const NM: f64 = 1e-6;

/// The length of the shortest octilinear path between two points, in nanometres.
pub(crate) fn octile(a: Point, b: Point) -> f64 {
    let dx = (a.x - b.x).unsigned_abs() as f64;
    let dy = (a.y - b.y).unsigned_abs() as f64;
    let (lo, hi) = if dx < dy { (dx, dy) } else { (dy, dx) };
    hi - lo + lo * core::f64::consts::SQRT_2
}

/// A pad that a net connects to, as a search sees it.
#[derive(Clone, Debug)]
pub(crate) struct TermPad {
    /// Where routes end.
    pub(crate) center: Point,
    /// The layers it has copper on.
    pub(crate) layers: LayerSet,
    /// Its bounding box, from which routes may go straight to the centre.
    pub(crate) bbox: Rect,
    /// Which terminal it belongs to.
    pub(crate) terminal: usize,
}

/// What a net's routes are made of, and where they have to go.
#[derive(Clone, Debug)]
pub(crate) struct NetCtx {
    pub(crate) net: NetId,
    /// The track width by layer.
    pub(crate) widths: Vec<i64>,
    /// The via's copper and hole diameters, if the net may have vias.
    pub(crate) via: Option<(i64, i64)>,
    /// The pads of its terminals.
    pub(crate) pads: Vec<TermPad>,
    /// The number of terminals.
    pub(crate) terminals: usize,
    /// The pads' boxes, widened by the distance from which routes move onto their
    /// lattices, the `k`th pad in slot `k`.
    pad_index: SpatialIndex,
    /// The pads' centres, for finding through-hole pads at a node.
    centers: FastMap<(Point, u8), usize>,
    /// The centres of pads with copper on every layer, which join the layers, so that vias
    /// are not needed there.
    through: FastMap<Point, ()>,
}

impl NetCtx {
    pub(crate) fn new(
        net: NetId,
        widths: Vec<i64>,
        via: Option<(i64, i64)>,
        pads: Vec<TermPad>,
        terminals: usize,
        snap: i64,
    ) -> NetCtx {
        let pad_index = SpatialIndex::bulk_load(pads.iter().map(|p| p.bbox.expand(snap)));
        let mut centers = FastMap::default();
        let mut through = FastMap::default();
        let all = widths.len();
        for (i, p) in pads.iter().enumerate() {
            for l in p.layers.iter() {
                centers.entry((p.center, l.0)).or_insert(i);
            }
            if p.layers.iter().count() == all {
                through.insert(p.center, ());
            }
        }
        NetCtx {
            net,
            widths,
            via,
            pads,
            terminals,
            pad_index,
            centers,
            through,
        }
    }

    /// Whether a pad of the net at `p` has copper on both layers, and so joins them there.
    pub(crate) fn joins(&self, p: Point, a: u8, b: u8) -> bool {
        self.centers
            .get(&(p, a))
            .is_some_and(|&i| self.pads[i].layers.contains(LayerId(b)))
    }

    /// The nodes where a terminal is reached: each of its pads' centres, on each layer.
    pub(crate) fn terminal_nodes(&self, terminal: usize) -> Vec<DNode> {
        let mut out = Vec::new();
        for p in self.pads.iter().filter(|p| p.terminal == terminal) {
            for l in p.layers.iter() {
                out.push(DNode {
                    x: p.center.x,
                    y: p.center.y,
                    l: l.0,
                });
            }
        }
        out
    }
}

/// Where a net may go, from global routing: tiles, and on each the layers it prefers.
#[derive(Clone, Debug)]
pub(crate) struct Corridor {
    pub(crate) origin: Point,
    pub(crate) tile: i64,
    pub(crate) cols: usize,
    pub(crate) rows: usize,
    /// By tile: 0 if outside, and otherwise a mask of preferred layers, with bit 63 set.
    pub(crate) cells: Vec<u64>,
}

impl Corridor {
    pub(crate) const INSIDE: u64 = 1 << 63;

    /// The tile of a point, if it is on the grid.
    pub(crate) fn tile_of(&self, p: Point) -> Option<usize> {
        let cx = (p.x - self.origin.x).div_euclid(self.tile);
        let cy = (p.y - self.origin.y).div_euclid(self.tile);
        if cx < 0 || cy < 0 || cx as usize >= self.cols || cy as usize >= self.rows {
            return None;
        }
        Some(cy as usize * self.cols + cx as usize)
    }

    fn cell(&self, p: Point) -> u64 {
        self.tile_of(p).map_or(0, |t| self.cells[t])
    }
}

/// Costs of the history of conflicts, by tile, for rip-up.
#[derive(Clone, Debug)]
pub(crate) struct History {
    pub(crate) grid: Corridor,
    pub(crate) cost: Vec<f64>,
}

/// How a search negotiates with other nets: the history of conflicts makes tracks dearer
/// where nets have fought, and with a penalty, other nets' routes may be crossed at that
/// price, for a rip-up.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RipUp<'h> {
    /// What crossing another net's route costs, in millimetres of track, or `None` if it
    /// may not be crossed.
    pub(crate) penalty: Option<f64>,
    pub(crate) history: &'h History,
}

/// The settings of a detailed search.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Params {
    pub(crate) pitch: i64,
    pub(crate) via_cost: f64,
    /// The extra cost of a step on a layer that global routing didn't choose there, as a
    /// fraction of its length.
    pub(crate) layer_bias: f64,
    pub(crate) bounds: Rect,
    pub(crate) max_expansions: u64,
    /// Whether a route may go to the centre of a pad it has yet to reach on a layer the pad
    /// isn't on, for a via in the pad.
    pub(crate) under_pads: bool,
}

/// The space one net is searched in.
pub(crate) struct DetailSpace<'a> {
    world: &'a World<'a>,
    ctx: &'a NetCtx,
    params: Params,
    corridor: Option<&'a Corridor>,
    rip: Option<RipUp<'a>>,
    layers: usize,
    /// Vias found legal or not, by point, as each is asked about once for every layer.
    vias: RefCell<FastMap<Point, Verdict>>,
    near: RefCell<Vec<crate::geom::EntryId>>,
    /// The net's own vias so far, which others of its vias keep the hole-to-hole
    /// clearance from: the world doesn't have them until the net is committed.
    keep_off: Vec<Point>,
    /// By terminal, whether the net's tree has reached it yet.
    reached: Vec<bool>,
}

impl<'a> DetailSpace<'a> {
    pub(crate) fn new(
        world: &'a World<'a>,
        ctx: &'a NetCtx,
        params: Params,
        corridor: Option<&'a Corridor>,
        rip: Option<RipUp<'a>>,
    ) -> DetailSpace<'a> {
        DetailSpace {
            world,
            ctx,
            params,
            corridor,
            rip,
            layers: world.rules().layer_count(),
            vias: RefCell::new(FastMap::default()),
            near: RefCell::new(Vec::new()),
            keep_off: Vec::new(),
            reached: vec![false; ctx.terminals],
        }
    }

    /// Whether a via of the net at `a` is too close to one at `b`, hole to hole.
    fn vias_clash(&self, a: Point, b: Point) -> bool {
        let Some((_, drill)) = self.ctx.via else {
            return false;
        };
        let h2h = self.world.rules().spec().hole_to_hole;
        if a == b {
            return true;
        }
        h2h > 0
            && match (Shape::circle(a, drill), Shape::circle(b, drill)) {
                (Ok(x), Ok(y)) => x.violates(&y, h2h).unwrap_or(true),
                _ => true,
            }
    }

    /// Keeps the net's later vias clear of one at `p`.
    fn keep_off(&mut self, p: Point) {
        self.keep_off.push(p);
        self.vias.borrow_mut().clear();
    }

    /// What a step costs on top of its length, or `None` if it may not be taken.
    fn verdict_cost(&self, verdict: Verdict, at: Point) -> Option<f64> {
        match (verdict, self.rip.and_then(|r| r.penalty)) {
            (Verdict::Free, _) => Some(0.0),
            (Verdict::Routed, Some(penalty)) => Some(penalty * (1.0 + self.history(at))),
            _ => None,
        }
    }

    /// The history of conflicts at a point, as a cost per millimetre.
    fn history(&self, at: Point) -> f64 {
        self.rip.map_or(0.0, |r| {
            r.history
                .grid
                .tile_of(at)
                .map_or(0.0, |t| r.history.cost[t])
        })
    }

    /// The cost factor of a step on `layer` that ends at `to`, or `None` outside the
    /// corridor.
    fn layer_factor(&self, to: Point, layer: u8) -> Option<f64> {
        match self.corridor {
            None => Some(1.0),
            Some(c) => {
                let cell = c.cell(to);
                if cell & Corridor::INSIDE == 0 {
                    None
                } else if cell & (1u64 << layer) != 0 {
                    Some(1.0)
                } else {
                    Some(1.0 + self.params.layer_bias)
                }
            }
        }
    }

    fn track_step(&self, from: DNode, to: Point, out: &mut Vec<Step<DNode>>, snap: bool) {
        let b = &self.params.bounds;
        if to.x < b.min().x || to.y < b.min().y || to.x > b.max().x || to.y > b.max().y {
            return;
        }
        let factor = if snap {
            Some(1.0)
        } else {
            self.layer_factor(to, from.l)
        };
        let Some(factor) = factor else { return };
        let width = self.ctx.widths[usize::from(from.l)];
        let Ok(shape) = Shape::track(from.point(), to, width) else {
            return;
        };
        let verdict = self
            .world
            .track(self.ctx.net, LayerId(from.l), &shape, None);
        let Some(extra) = self.verdict_cost(verdict, to) else {
            return;
        };
        let len = octile(from.point(), to) * NM;
        if let Some(cost) = Cost::new(len * (factor + self.history(to)) + extra) {
            out.push(Step {
                to: DNode {
                    x: to.x,
                    y: to.y,
                    l: from.l,
                },
                cost,
            });
        }
    }
}

impl RoutingSpace for DetailSpace<'_> {
    type Node = DNode;
    type Resource = NodeId;
    type Slots = InternedSlots<DNode>;
    type ResourceSlots = DenseSlots<NodeId>;

    fn new_slots(&self) -> Self::Slots {
        InternedSlots::new()
    }

    fn new_resource_slots(&self) -> Self::ResourceSlots {
        DenseSlots::new()
    }

    fn contains(&self, node: DNode) -> bool {
        usize::from(node.l) < self.layers
    }

    fn expand(&self, _net: NetId, from: DNode, out: &mut Vec<Step<DNode>>) {
        let p = from.point();
        // A pad's centre connects its layers.
        if let Some(&i) = self.ctx.centers.get(&(p, from.l)) {
            for l in self.ctx.pads[i].layers.iter().filter(|l| l.0 != from.l) {
                out.push(Step {
                    to: DNode { l: l.0, ..from },
                    cost: Cost::ZERO,
                });
            }
        }
        let at_pad = self.ctx.through.get(&p);
        let pitch = self.params.pitch;
        for (dx, dy) in DIRS {
            let to = Point::new(p.x + dx * pitch, p.y + dy * pitch);
            self.track_step(from, to, out, false);
        }
        // Straight to the centre of a pad nearby, or onto the lattice of its centre, so as
        // to reach it straight.
        {
            let mut near = self.near.borrow_mut();
            self.ctx.pad_index.query(&Rect::new(p, p), &mut near);
            let found: Vec<usize> = near.iter().map(|id| id.slot() as usize).collect();
            drop(near);
            for i in found {
                let pad = &self.ctx.pads[i];
                // On a layer the pad isn't on, a route can still go to its centre for a via
                // in the pad.
                let under =
                    self.params.under_pads && self.ctx.via.is_some() && !self.reached[pad.terminal];
                if pad.center == p || !(pad.layers.contains(LayerId(from.l)) || under) {
                    continue;
                }
                let b = pad.bbox.expand(pitch);
                if b.intersects(&Rect::new(p, p)) {
                    self.track_step(from, pad.center, out, true);
                } else {
                    let near = |v: i64, c: i64| c + (v - c + pitch / 2).div_euclid(pitch) * pitch;
                    let m = Point::new(near(p.x, pad.center.x), near(p.y, pad.center.y));
                    if m != p {
                        self.track_step(from, m, out, false);
                    }
                }
            }
        }
        // A through via, anywhere but at a through-hole pad's centre, which joins the
        // layers already. In a pad, it is a via in the pad.
        if let Some((diameter, drill)) = self.ctx.via
            && self.layers > 1
            && at_pad.is_none()
        {
            let verdict = *self.vias.borrow_mut().entry(p).or_insert_with(|| {
                if self.keep_off.iter().any(|&q| self.vias_clash(p, q)) {
                    Verdict::Blocked
                } else {
                    self.world.via(self.ctx.net, p, diameter, drill, None)
                }
            });
            if let Some(extra) = self.verdict_cost(verdict, p)
                && let Some(cost) = Cost::new(self.params.via_cost + extra)
            {
                let joined = self
                    .ctx
                    .centers
                    .get(&(p, from.l))
                    .map(|&i| self.ctx.pads[i].layers);
                for l in (0..self.layers as u8)
                    .filter(|&l| l != from.l && !joined.is_some_and(|j| j.contains(LayerId(l))))
                {
                    out.push(Step {
                        to: DNode { l, ..from },
                        cost,
                    });
                }
            }
        }
    }

    fn lower_bound(&self, _net: NetId, from: DNode, to: DNode) -> Cost {
        // A little under, so that the sum of many steps rounded can't fall below it.
        Cost::new(octile(from.point(), to.point()) * NM * (1.0 - 1e-9)).unwrap_or(Cost::ZERO)
    }

    fn resources(&self, _net: NetId, _from: DNode, _to: DNode, _out: &mut Vec<NodeId>) {}

    fn capacity(&self, _resource: NodeId) -> u32 {
        1
    }
}

/// A net's route as a tree of steps between nodes.
#[derive(Clone, Debug, Default)]
pub(crate) struct Tree {
    pub(crate) edges: Vec<(DNode, DNode)>,
    pub(crate) expanded: u64,
}

/// Why a net could not be routed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    /// No path, within the corridor if there was one.
    NoPath,
    /// The expansion budget ran out.
    Budget,
}

/// Routes one net: a tree from its first terminal that reaches all of the others, each
/// search going from the whole tree so far to the nearest terminal not yet reached.
pub(crate) fn route_net(
    world: &World<'_>,
    ctx: &NetCtx,
    params: Params,
    corridor: Option<&Corridor>,
    rip: Option<RipUp<'_>>,
) -> Result<Tree, (Failure, u64)> {
    let mut space = DetailSpace::new(world, ctx, params, corridor, rip);
    let mut search = Search::new(&space);
    let mut retries = 0;
    let mut tree = Tree::default();
    let mut connected = vec![false; ctx.terminals];
    let mut sources: Vec<DNode> = ctx.terminal_nodes(0);
    connected[0] = true;
    space.reached[0] = true;
    let mut on_tree: FastMap<DNode, ()> = FastMap::default();
    for &s in &sources {
        on_tree.insert(s, ());
    }
    let terminal_of = |n: DNode| -> Option<usize> {
        ctx.centers
            .get(&(n.point(), n.l))
            .map(|&i| ctx.pads[i].terminal)
    };
    let mut budget = params.max_expansions;
    while connected.iter().any(|c| !c) {
        let targets: Vec<DNode> = (0..ctx.terminals)
            .filter(|&t| !connected[t])
            .flat_map(|t| ctx.terminal_nodes(t))
            .collect();
        let limits = SearchLimits {
            max_expansions: budget,
            cancel: None,
        };
        let outcome = search.find_path_with(
            &space,
            ctx.net,
            sources.iter().map(|&n| Source::new(n)),
            targets.iter().map(|&n| Target::new(n)),
            limits,
        );
        let path: Result<Vec<DNode>, Failure> = match outcome {
            SearchOutcome::Found(found) => Ok(found.path.to_vec()),
            SearchOutcome::LimitReached | SearchOutcome::Cancelled => Err(Failure::Budget),
            SearchOutcome::Unreachable => Err(Failure::NoPath),
        };
        let expanded = search.stats().expanded;
        tree.expanded += expanded;
        budget = budget.saturating_sub(expanded);
        let path = path.map_err(|f| (f, tree.expanded))?;
        // The path's vias, which must keep clear of each other and of the net's others.
        let mut new_vias: Vec<Point> = Vec::new();
        for w in path.windows(2) {
            let p = w[0].point();
            if w[0].l != w[1].l && !ctx.joins(p, w[0].l, w[1].l) && !new_vias.contains(&p) {
                new_vias.push(p);
            }
        }
        let clash = new_vias.iter().enumerate().find_map(|(i, &p)| {
            let earlier = new_vias[..i].iter().chain(&space.keep_off);
            earlier
                .clone()
                .any(|&q| space.vias_clash(p, q))
                .then_some(p)
        });
        if let Some(p) = clash {
            retries += 1;
            if retries > 16 {
                return Err((Failure::NoPath, tree.expanded));
            }
            space.keep_off(p);
            continue;
        }
        for &p in &new_vias {
            space.keep_off(p);
            // A via joins every layer.
            for l in 0..space.layers as u8 {
                let n = DNode { x: p.x, y: p.y, l };
                if on_tree.insert(n, ()).is_none() {
                    sources.push(n);
                }
            }
        }
        for w in path.windows(2) {
            tree.edges.push((w[0], w[1]));
        }
        let before = connected.iter().filter(|&&c| c).count();
        for &n in &path {
            if let Some(t) = terminal_of(n) {
                connected[t] = true;
                space.reached[t] = true;
            }
            if on_tree.insert(n, ()).is_none() {
                sources.push(n);
            }
        }
        if connected.iter().filter(|&&c| c).count() == before {
            // Can't happen, as every target is a terminal's, but a loop must not hang.
            return Err((Failure::NoPath, tree.expanded));
        }
        // A terminal reached on one layer of a pad is on the tree on all of them.
        for (t, _) in connected.iter().enumerate().filter(|(_, c)| **c) {
            {
                for n in ctx.terminal_nodes(t) {
                    if on_tree.insert(n, ()).is_none() {
                        sources.push(n);
                    }
                }
            }
        }
    }
    Ok(tree)
}

/// The nets whose routes a tree runs into, in the order met, and the points where it does.
pub(crate) fn victims(world: &World<'_>, ctx: &NetCtx, tree: &Tree) -> (Vec<NetId>, Vec<Point>) {
    let mut out = Vec::new();
    let mut at = Vec::new();
    let mut vias: Vec<Point> = Vec::new();
    for &(a, b) in &tree.edges {
        let verdict = if a.l == b.l {
            match Shape::track(a.point(), b.point(), ctx.widths[usize::from(a.l)]) {
                Ok(shape) if a.point() != b.point() => {
                    world.track(ctx.net, LayerId(a.l), &shape, Some(&mut out))
                }
                _ => Verdict::Free,
            }
        } else if !ctx.joins(a.point(), a.l, b.l)
            && let Some((d, drill)) = ctx.via
            && !vias.contains(&a.point())
        {
            vias.push(a.point());
            world.via(ctx.net, a.point(), d, drill, Some(&mut out))
        } else {
            Verdict::Free
        };
        if verdict != Verdict::Free {
            at.push(b.point());
        }
    }
    (out, at)
}
